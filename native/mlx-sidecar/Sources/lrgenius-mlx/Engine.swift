// Model loading and generation. Everything MLX-facing lives here; `main.swift`
// only moves JSON in and out.

import CoreImage
import Foundation
import MLX
import MLXGuidedGeneration
import MLXHuggingFace
import MLXLLM
import MLXLMCommon
import MLXVLM
import Tokenizers

enum EngineError: LocalizedError {
    case noModelLoaded
    case modelDirectoryMissing(String)
    case unsupportedModel(String)
    case imageUnreadable(String)
    case visionUnsupported

    var errorDescription: String? {
        switch self {
        case .noModelLoaded:
            return "No MLX model is loaded."
        case .modelDirectoryMissing(let path):
            return "MLX model directory not found: \(path)"
        case .unsupportedModel(let detail):
            return "This MLX model is not supported by the bundled mlx-swift-lm: \(detail)"
        case .imageUnreadable(let path):
            return "Could not read staged image pixels at \(path)"
        case .visionUnsupported:
            return
                "This MLX model is text-only, but the request included a photo. Choose a vision "
                + "model (for example a Gemma 4 or Qwen3-VL MLX repo)."
        }
    }
}

/// Render an error for the user.
///
/// `localizedDescription` alone is not good enough here. Several of the types
/// thrown through this path — `GrammarError` most importantly — are plain
/// `Error` enums with associated messages but no `LocalizedError` conformance,
/// and Foundation renders those as "(MLXGuidedGeneration.GrammarError error
/// 6.)", which tells the user nothing and tells a maintainer almost nothing.
/// `String(reflecting:)` prints the case name and its payload.
func describe(_ error: Error) -> String {
    // The one guided-generation failure a user can act on. It means the model
    // was still mid-object when the token budget ran out, even after the
    // closing bias tried to wind the answer down -- so say which setting moves,
    // rather than surfacing "GuidedGenerationError.incompleteOutput".
    if let guided = error as? GuidedGenerationError, case .incompleteOutput = guided {
        return
            "the model ran out of output tokens before it finished its JSON answer. Raise Max "
            + "Tokens in the plugin (General tab → AI Model section) — try 4096 or higher. A large "
            + "keyword taxonomy increases token usage significantly."
    }
    if error is LocalizedError {
        return error.localizedDescription
    }
    return String(reflecting: error)
}

/// Owns the resident model. One instance per process; `main.swift` keeps it
/// alive across requests so a batch does not reload multi-GB weights per photo.
final class Engine {

    private struct Loaded {
        let container: ModelContainer
        let directory: URL
        let supportsVision: Bool
    }

    private var loaded: Loaded?

    /// The vocabulary XGrammar compiles grammars against.
    ///
    /// Cached because building it walks the whole tokenizer vocabulary, and it
    /// depends only on the loaded model — so it is rebuilt on `load` and never
    /// per request.
    ///
    /// Note what is *not* cached: the `GrammarConstraint` itself.
    /// `GrammarConstraint` carries mutable matcher state, so reusing one across
    /// photos would need `clone()` — and `clone()` (xgrammar's
    /// `GrammarMatcher::Fork()`) fails with `forkFailed` in this build. So each
    /// request compiles its own constraint. That costs a grammar compile per
    /// photo, which the README warns is hundreds of milliseconds; it is still
    /// small against a VLM prefill, and correctness beats the saving. If fork
    /// starts working upstream, caching a pristine template and cloning it is
    /// the optimization to make here.
    private var grammarTokenizer: GrammarTokenizer?

    /// Logit bias favouring the tokens that *close* a JSON value (`"`, `}`,
    /// `]`, digits, EOS), cached for the same reason as `grammarTokenizer`:
    /// computing it walks the whole vocabulary.
    ///
    /// Without it, `GuidedGenerationLoop` disables its entire budget policy —
    /// every zone check sits behind `if let bias = closingBias` — so a model
    /// that is still mid-object when `maxTokens` runs out throws
    /// `incompleteOutput` and the photo yields nothing at all. A photo with
    /// many keywords hits that every time. With the bias in place the loop
    /// steers toward closing the object as the budget runs down, so the answer
    /// comes back valid and merely shorter.
    private var closingBias: MLXArray?

    /// Negative bias on tokens that decode to nothing but JSON whitespace,
    /// plus the ids themselves so the loop's run tracker can spot a run.
    ///
    /// JSON grammar permits whitespace almost anywhere, so the grammar mask
    /// never rules it out, and this model will happily spend hundreds of tokens
    /// emitting spaces between two fields. Measured on the 13-category schema:
    /// a two-category cut-down produced seven keywords and burned 1986 of 2048
    /// tokens getting there. That is what exhausts the budget -- not the actual
    /// answer, which runs about 200 tokens unconstrained.
    private var whitespacePenalty: (bias: MLXArray, tokenIDs: Set<Int>)?

    var isLoaded: Bool { loaded != nil }

    func info() -> EngineInfoPayload? {
        guard let loaded else { return nil }
        return EngineInfoPayload(
            modelPath: loaded.directory.path,
            modelName: loaded.directory.lastPathComponent,
            supportsVision: loaded.supportsVision)
    }

    // MARK: - Loading

    func load(modelDir: String) async throws -> EngineInfoPayload {
        let directory = URL(fileURLWithPath: modelDir, isDirectory: true)
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: directory.path, isDirectory: &isDir),
            isDir.boolValue
        else {
            throw EngineError.modelDirectoryMissing(directory.path)
        }

        // Reloading always drops the previous model first. Holding both
        // resident would momentarily double peak memory, which on a 16 GB
        // machine loading a second 8 GB model is the difference between a swap
        // storm and a clean swap.
        unload()

        let tokenizerLoader = #huggingFaceTokenizerLoader()

        // Try the vision factory first and fall back to the text-only one.
        // Both read `model_type` out of config.json; the VLM registry simply
        // does not know the text-only architectures. A text model is still
        // useful here — keyword clustering (`generate_text`) needs no vision —
        // so falling back is better than refusing to load.
        let container: ModelContainer
        var supportsVision = true
        do {
            container = try await VLMModelFactory.shared.loadContainer(
                from: directory, using: tokenizerLoader)
        } catch {
            let visionError = error
            do {
                container = try await LLMModelFactory.shared.loadContainer(
                    from: directory, using: tokenizerLoader)
                supportsVision = false
            } catch {
                // Report the vision failure: for a model the user picked
                // expecting photo analysis, "unsupported VLM architecture" is
                // the actionable message, not "also not a text model".
                throw EngineError.unsupportedModel(visionError.localizedDescription)
            }
        }

        loaded = Loaded(
            container: container, directory: directory, supportsVision: supportsVision)
        grammarTokenizer = nil
        closingBias = nil
        whitespacePenalty = nil

        let payload = EngineInfoPayload(
            modelPath: directory.path,
            modelName: directory.lastPathComponent,
            supportsVision: supportsVision)
        Log.info(
            "MLX model ready: \(payload.modelName) vision=\(supportsVision) at \(directory.path)")
        return payload
    }

    func unload() {
        guard loaded != nil else { return }
        loaded = nil
        grammarTokenizer = nil
        closingBias = nil
        whitespacePenalty = nil
        // MLX keeps freed blocks in its own allocator pool, so dropping the
        // container is not by itself enough to return the weights to the OS.
        MLX.GPU.clearCache()
        Log.info("Unloaded MLX model")
    }

    // MARK: - Generation

    /// Run a whole batch. Each entry gets its own result so one bad photo does
    /// not sink the rest, matching `LocalEngine::generate`'s contract.
    ///
    /// The photos are evaluated one after another rather than as concurrent
    /// sequences. Unlike llama.cpp there is no shared pinned prefix to amortise
    /// across a batch here (see `GenerationResultPayload.prefixReused`), and a
    /// single VLM prefill already saturates the GPU, so batching buys ordering
    /// convenience rather than throughput.
    func generate(specs: [GenerationSpec]) async throws -> [GenerationResultPayload] {
        guard let loaded else { throw EngineError.noModelLoaded }

        var results: [GenerationResultPayload] = []
        results.reserveCapacity(specs.count)
        for spec in specs {
            do {
                results.append(try await generateOne(spec: spec, loaded: loaded))
            } catch {
                results.append(.failure(describe(error)))
            }
        }
        return results
    }

    private func generateOne(spec: GenerationSpec, loaded: Loaded) async throws
        -> GenerationResultPayload
    {
        if spec.image != nil && !loaded.supportsVision {
            throw EngineError.visionUnsupported
        }

        let image = try spec.image.map(Self.loadImage)

        // Ordering matters: the run-constant half goes first so that whatever
        // prefix reuse the stack can manage has the longest possible common
        // head. It buys nothing today (a fresh cache per call), but putting the
        // volatile half first would make it impossible later.
        var userText = spec.stablePrompt
        if !spec.perPhotoPrompt.isEmpty {
            if !userText.isEmpty { userText += "\n\n" }
            userText += spec.perPhotoPrompt
        }

        var messages: [Chat.Message] = []
        if !spec.systemPrompt.isEmpty {
            messages.append(.system(spec.systemPrompt))
        }
        messages.append(
            .user(userText, images: image.map { [.ciImage($0)] } ?? []))

        let userInput = UserInput(chat: messages)

        // The same turns without the photo. Used only to price the image half
        // of the prompt — see `textOnlyTokens` below.
        var textOnlyMessages: [Chat.Message] = []
        if !spec.systemPrompt.isEmpty {
            textOnlyMessages.append(.system(spec.systemPrompt))
        }
        textOnlyMessages.append(.user(userText))
        let textOnlyInput = UserInput(chat: textOnlyMessages)

        return try await loaded.container.perform { context in
            let tStart = DispatchTime.now().uptimeNanoseconds
            let input = try await context.processor.prepare(input: userInput)
            let promptTokens = input.text.tokens.size
            let prepareMs = Self.msSince(tStart)

            // The processor splices the image's placeholder tokens straight
            // into `input.text.tokens`, so `promptTokens` already counts them
            // and no separate image-token tally is missing. What it hides is
            // the *split*: Gemma 4 spends a fixed budget per photo regardless
            // of pixel size (measured: 262 tokens for both a 187x125 and a
            // 2048x1365 input), so a prompt that looks large may be mostly
            // picture, or mostly keyword taxonomy, and those have very
            // different fixes. Re-rendering the turns without the photo is a
            // template render plus a tokenize — no model forward — and it is
            // the only way to tell the two apart. Best effort: a failure here
            // must never cost the photo its answer.
            var textOnlyTokens = promptTokens
            if image != nil,
                let textOnly = try? await context.processor.prepare(input: textOnlyInput)
            {
                textOnlyTokens = textOnly.text.tokens.size
            }

            guard let schema = spec.schema, !schema.isEmpty else {
                let parameters = GenerateParameters(
                    maxTokens: spec.maxTokens, temperature: spec.temperature)
                let genStart = DispatchTime.now().uptimeNanoseconds
                var firstTokenNs: UInt64?
                // The closure parameter is spelled out because `generate` is
                // overloaded on `([Int]) -> _` and `(Int) -> _`, and only the
                // former hands back the full token list this needs.
                let result = try MLXLMCommon.generate(
                    input: input, parameters: parameters, context: context
                ) { (_: [Int]) -> GenerateDisposition in
                    if firstTokenNs == nil { firstTokenNs = DispatchTime.now().uptimeNanoseconds }
                    return .more
                }
                Self.logStages(
                    prepareMs: prepareMs, grammarMs: nil, genStart: genStart,
                    firstTokenNs: firstTokenNs, textTokens: textOnlyTokens,
                    promptTokens: promptTokens, produced: result.tokenIds.count)
                return .success(
                    text: result.output,
                    promptTokens: promptTokens,
                    completionTokens: result.tokenIds.count)
            }

            let tGrammar = DispatchTime.now().uptimeNanoseconds
            let (constraint, vocabSize) = try self.constraint(for: schema, context: context)
            let whitespace = self.whitespace(for: context)
            let grammarMs = Self.msSince(tGrammar)
            var output = ""
            // Marks the end of vision-encode + prefill: everything after the
            // first token is pure decode.
            var firstTokenNs: UInt64?
            let genStart = DispatchTime.now().uptimeNanoseconds
            // `run` throws away everything it generated when it throws, which
            // makes an overrun impossible to tell apart from a stall. Keep
            // enough of a tally to say which one happened.
            let produced: Int
            do {
                produced = try GuidedGenerationLoop.run(
                    input: input,
                    context: context,
                    constraint: constraint,
                    maxTokens: spec.maxTokens,
                    vocabSize: vocabSize,
                    // Soft zone only (the library's default 64-token reserve).
                    // Deliberately no `hardReserve`: the hard zone suppresses
                    // every token that is not "closing", and `ClosingTokenBias`
                    // counts the digits 0-9 as closing (they finish a JSON
                    // *number*). Inside a string that forces digits, which
                    // yields structurally valid, semantically worthless output
                    // -- a measured `{"title": "19001", "caption": "19002"}`.
                    // Junk written into someone's catalog is worse than a photo
                    // that failed, so the budget only ever nudges here and a
                    // genuine overrun stays an error.
                    //
                    // The reserve is clamped rather than left at the library's
                    // flat 64 because the zone is defined as
                    // `tokenCount >= maxTokens - completionReserve`: at a
                    // budget of 64 or less that is true from the very first
                    // token, so the bias -- which lifts digits by +100, they
                    // being how a JSON *number* ends -- drives the whole
                    // answer. Measured at `max_tokens: 64`, the model emitted
                    // `{"keywords": ["19000000000...`. A quarter of the budget
                    // keeps the nudge to the tail where it belongs.
                    completionReserve: min(64, max(1, spec.maxTokens / 4)),
                    closingBias: self.bias(for: context),
                    whitespaceBias: whitespace.bias,
                    whitespaceTokenIDs: whitespace.tokenIDs
                ) { delta in
                    if firstTokenNs == nil { firstTokenNs = DispatchTime.now().uptimeNanoseconds }
                    output += delta
                    return true
                }
            } catch {
                Log.info(
                    "guided generation failed after \(output.count) chars "
                        + "(\(Self.whitespaceShare(of: output))% whitespace, budget "
                        + "\(spec.maxTokens) tokens); tail: \(String(output.suffix(120)))")
                throw error
            }
            Self.logStages(
                prepareMs: prepareMs, grammarMs: grammarMs, genStart: genStart,
                firstTokenNs: firstTokenNs, textTokens: textOnlyTokens,
                promptTokens: promptTokens, produced: produced)
            return .success(
                text: output, promptTokens: promptTokens, completionTokens: produced)
        }
    }

    /// Milliseconds since a `DispatchTime.now().uptimeNanoseconds` reading.
    /// Monotonic, so a clock adjustment mid-run cannot skew it.
    @inline(__always)
    static func msSince(_ startNanos: UInt64) -> Double {
        Double(DispatchTime.now().uptimeNanoseconds &- startNanos) / 1_000_000
    }

    /// One line per photo, splitting a generation into the stages that have
    /// different fixes.
    ///
    /// `ttft` (time to first token) is vision-encode plus prefill: everything
    /// the model does before it can emit anything. What follows is pure
    /// decode, which is memory-bandwidth bound and scales with the number of
    /// output tokens. Reading the two against each other is what says whether
    /// a slow run wants a shorter prompt or a shorter answer — and `grammar`
    /// prices the constraint compile this backend pays per photo, because
    /// `GrammarConstraint` cannot be cloned in this build.
    static func logStages(
        prepareMs: Double, grammarMs: Double?, genStart: UInt64, firstTokenNs: UInt64?,
        textTokens: Int, promptTokens: Int, produced: Int
    ) {
        let totalMs = msSince(genStart)
        let ttftMs = firstTokenNs.map { Double($0 &- genStart) / 1_000_000 }
        let decodeMs = ttftMs.map { totalMs - $0 }
        // The first token is charged to ttft, so the rate covers the rest;
        // with one token or none there is no rate worth printing.
        let decodeRate: Double? = decodeMs.flatMap { elapsed in
            produced > 1 && elapsed > 0 ? Double(produced - 1) / (elapsed / 1000) : nil
        }
        func fmt(_ value: Double?) -> String {
            value.map { String(format: "%.1f", $0) } ?? "n/a"
        }
        var line = "stages: prepare=\(fmt(prepareMs))ms"
        if let grammarMs { line += " grammar=\(fmt(grammarMs))ms" }
        line += " ttft=\(fmt(ttftMs))ms decode=\(fmt(decodeMs))ms"
        line += " | tokens: text=\(textTokens) image=\(promptTokens - textTokens) out=\(produced)"
        if let decodeRate { line += " | decode \(fmt(decodeRate)) tok/s" }
        Log.info(line)
    }

    /// Percentage of `text` that is whitespace, rounded to a whole number.
    ///
    /// The number that separates "this photo genuinely needs more tokens" from
    /// "the model stalled emitting spaces again": a healthy JSON answer sits
    /// around 10-20%.
    private static func whitespaceShare(of text: String) -> Int {
        guard !text.isEmpty else { return 0 }
        let spaces = text.reduce(into: 0) { count, character in
            if character.isWhitespace { count += 1 }
        }
        return Int((Double(spaces) / Double(text.count) * 100).rounded())
    }

    /// A fresh constraint for `schema` plus the vocabulary size the decode loop
    /// needs.
    ///
    /// The size comes back alongside the constraint because `GrammarConstraint`
    /// keeps its own copy private; only the tokenizer exposes one.
    private func constraint(for schema: String, context: ModelContext) throws
        -> (GrammarConstraint, Int)
    {
        let tokenizer: GrammarTokenizer
        if let existing = grammarTokenizer {
            tokenizer = existing
        } else {
            let vocab = TokenizerVocabExtractor.extractForGrammar(from: context.tokenizer)
            tokenizer = try GrammarTokenizer(
                vocab: vocab.vocab,
                vocabType: vocab.vocabType,
                eosTokenId: Int32(context.tokenizer.eosTokenId ?? 0))
            grammarTokenizer = tokenizer
        }

        let constraint = try GrammarConstraint(
            tokenizer: tokenizer,
            jsonSchema: schema,
            fastForward: true,
            hostTokenizer: context.tokenizer)
        return (constraint, tokenizer.vocabSize)
    }

    /// The cached closing-token bias for the loaded model, computed on first use.
    ///
    /// Built here rather than taken from `ClosingTokenBias.compute` because that
    /// one also lifts the digits 0-9, on the grounds that a digit is how a JSON
    /// *number* ends. No schema this app sends contains a number -- every leaf
    /// is a string -- so here a digit can never close anything, and boosting it
    /// only gives the model a way to corrupt the string it is inside. Measured
    /// with the library's array: a caption ending
    /// `...glow of a setting sun9999999999999999`. Same tiers otherwise: EOS
    /// above the structural closers, so a finishable answer prefers to stop.
    private func bias(for context: ModelContext) -> MLXArray {
        if let closingBias {
            return closingBias
        }
        let tokenizer = context.tokenizer
        var vocabSize = 0
        while tokenizer.convertIdToToken(vocabSize) != nil {
            vocabSize += 1
            if vocabSize > 500_000 { break }
        }
        var biases = [Float](repeating: 0.0, count: vocabSize)
        for id in 0 ..< vocabSize {
            if let token = tokenizer.convertIdToToken(id), ["\"", "}", "]"].contains(token) {
                biases[id] = 100.0
            }
        }
        if let eos = tokenizer.eosTokenId, eos >= 0, eos < vocabSize {
            biases[eos] = 200.0
        }
        let computed = MLXArray(biases)
        closingBias = computed
        return computed
    }

    /// The cached whitespace penalty for the loaded model, computed on first use.
    private func whitespace(for context: ModelContext) -> (bias: MLXArray, tokenIDs: Set<Int>) {
        if let whitespacePenalty {
            return whitespacePenalty
        }
        let computed = WhitespaceTokenBias.compute(tokenizer: context.tokenizer)
        whitespacePenalty = computed
        return computed
    }

    // MARK: - Images

    /// Rebuild a `CIImage` from the raw pixels the Rust side staged on disk.
    ///
    /// The pixels arrive as a file rather than inside the JSON because a
    /// decoded photo is several megabytes and base64 would inflate every batch
    /// line by a third on top of that.
    private static func loadImage(_ payload: ImagePayload) throws -> CIImage {
        guard let data = FileManager.default.contents(atPath: payload.path) else {
            throw EngineError.imageUnreadable(payload.path)
        }
        let expected = payload.width * payload.height * payload.channels
        guard data.count == expected, payload.width > 0, payload.height > 0 else {
            throw EngineError.imageUnreadable(
                "\(payload.path): expected \(expected) bytes, found \(data.count)")
        }

        // CIImage has no packed-RGB24 format, so widen to RGBA8. Done here
        // rather than on the Rust side to keep a third of the bytes off the
        // wire and `LocalImage`'s RGB contract untouched.
        let pixelCount = payload.width * payload.height
        var rgba = [UInt8](repeating: 255, count: pixelCount * 4)
        if payload.channels == 4 {
            rgba.withUnsafeMutableBytes { destination in
                data.copyBytes(to: destination)
            }
        } else {
            data.withUnsafeBytes { (source: UnsafeRawBufferPointer) in
                for pixel in 0 ..< pixelCount {
                    rgba[pixel * 4 + 0] = source[pixel * 3 + 0]
                    rgba[pixel * 4 + 1] = source[pixel * 3 + 1]
                    rgba[pixel * 4 + 2] = source[pixel * 3 + 2]
                }
            }
        }

        return CIImage(
            bitmapData: Data(rgba),
            bytesPerRow: payload.width * 4,
            size: CGSize(width: payload.width, height: payload.height),
            format: .RGBA8,
            colorSpace: CGColorSpace(name: CGColorSpace.sRGB))
    }
}
