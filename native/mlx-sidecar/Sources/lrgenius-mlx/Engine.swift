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
    /// Note what is *not* cached: the `GrammarConstraint` itself. It carries
    /// mutable matcher state, so one cannot be shared across photos, and the
    /// two ways to get a pristine one are both closed here. `clone()`
    /// (xgrammar's `GrammarMatcher::Fork()`) fails with `forkFailed` in this
    /// build. Rewinding a used constraint with `rollback` is worse than it
    /// looks: the loop's token count is not the matcher's acceptance count
    /// (truncated fast-forward tokens are accepted but never counted), and the
    /// obvious guard — compare the rewound mask against the pristine one — has
    /// real collisions, because in an array-of-objects schema the state after
    /// `"keywords": [` admits exactly the same tokens as the very start.
    ///
    /// So every request compiles its own constraint, at a measured ~450ms.
    /// What removes that from the clock is `pendingGrammar`: the compile is
    /// pure CPU work in xgrammar with no MLX involvement, so it runs for the
    /// *next* photo while the current one decodes on the GPU.
    private var grammarTokenizer: GrammarTokenizer?

    /// The model's own tokenizer, kept so a grammar compile can be started
    /// from outside `ModelContainer.perform`.
    ///
    /// `GrammarConstraint` only stores this (it encodes fast-forward strings
    /// during generation, long after construction), so holding the reference
    /// here adds no concurrent use of the tokenizer.
    /// (`Tokenizer` is spelled out because `Tokenizers` exports one too.)
    private var hostTokenizer: (any MLXLMCommon.Tokenizer)?

    /// A grammar compile started ahead of the photo that will use it.
    ///
    /// At most one is ever in flight: the compile for photo *i+1* is started
    /// only once photo *i* has taken its own constraint, so two threads never
    /// sit in xgrammar at the same time.
    private var pendingGrammar: (schema: String, task: Task<GrammarConstraint, Error>)?

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
        dropGrammarState()
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
        dropGrammarState()
        closingBias = nil
        whitespacePenalty = nil
        // MLX keeps freed blocks in its own allocator pool, so dropping the
        // container is not by itself enough to return the weights to the OS.
        MLX.GPU.clearCache()
        Log.info("Unloaded MLX model")
    }

    /// Forget everything tied to the outgoing model's vocabulary.
    ///
    /// A grammar in flight is cancelled and dropped rather than awaited: it was
    /// compiled against a tokenizer that is going away, so its only remaining
    /// use would be to produce masks over the wrong vocabulary.
    private func dropGrammarState() {
        pendingGrammar?.task.cancel()
        pendingGrammar = nil
        grammarTokenizer = nil
        hostTokenizer = nil
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

        // Only pay for the vocabulary walk when something in this batch is
        // actually schema-constrained; `generate_text` callers are not.
        if specs.contains(where: { !($0.schema ?? "").isEmpty }) {
            try await prepareGrammarVocabulary(loaded)
        }

        var results: [GenerationResultPayload] = []
        results.reserveCapacity(specs.count)
        startGrammarCompile(for: specs.first?.schema)
        for (index, spec) in specs.enumerated() {
            do {
                let grammar = try await takeConstraint(for: spec.schema)
                // Started before the photo runs, so the compile overlaps this
                // photo's prefill and decode instead of preceding the next one.
                startGrammarCompile(for: index + 1 < specs.count ? specs[index + 1].schema : nil)
                results.append(
                    try await generateOne(spec: spec, loaded: loaded, grammar: grammar))
            } catch {
                results.append(.failure(describe(error)))
            }
        }
        return results
    }

    /// A constraint, the vocabulary size the decode loop needs, and the
    /// milliseconds this photo actually spent waiting for the compile — near
    /// zero whenever the prefetch landed in time.
    private typealias Grammar = (constraint: GrammarConstraint, vocabSize: Int, waitMs: Double)

    private func generateOne(spec: GenerationSpec, loaded: Loaded, grammar: Grammar?) async throws
        -> GenerationResultPayload
    {
        if spec.image != nil && !loaded.supportsVision {
            throw EngineError.visionUnsupported
        }

        let image = try spec.image.map(Self.loadImage)

        // Ordering matters: the run-constant half goes first so that whatever
        // prefix reuse the stack can manage has the longest possible common
        // head. It buys nothing today, and on Gemma it would buy nothing even
        // with a warm cache: `Gemma4MessageGenerator` builds the user turn as
        // image parts *then* the text part, so the photo — the one thing that
        // changes every request — sits ahead of all of this. llama.cpp puts its
        // media marker last for exactly that reason (`worker.rs::split_render`),
        // but the ordering inside an MLX chat turn is the model's to decide.
        // Reusing the taxonomy prefix here would mean moving it into the
        // *system* message, which renders before the user turn.
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

            guard let grammar else {
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

            let (constraint, vocabSize, grammarMs) = grammar
            let whitespace = self.whitespace(for: context)
            // Splits the answer into what the model chose and what the grammar
            // forced. Both cost the same: `GuidedGenerationLoop` runs a forward
            // pass per fast-forward token just as it does per sampled one, so a
            // high `ff` share is JSON punctuation being decoded at full price.
            // That is the number that says whether a leaner response *shape*
            // would pay off, as opposed to a shorter answer.
            //
            // The sink is the library's own injection point and every recording
            // site is nil-guarded, so binding it costs two array appends per
            // token and changes nothing about the generation.
            let telemetry = GuidedGenerationDiagnosticSink()
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
                produced = try GuidedGenerationDiagnosticSink.$current.withValue(telemetry) {
                    try GuidedGenerationLoop.run(
                        input: input,
                        context: context,
                        constraint: constraint,
                        maxTokens: spec.maxTokens,
                        vocabSize: vocabSize,
                        // Soft zone only (the library's default 64-token
                        // reserve). Deliberately no `hardReserve`: the hard zone
                        // suppresses every token that is not "closing", and
                        // `ClosingTokenBias` counts the digits 0-9 as closing
                        // (they finish a JSON *number*). Inside a string that
                        // forces digits, which yields structurally valid,
                        // semantically worthless output -- a measured
                        // `{"title": "19001", "caption": "19002"}`. Junk written
                        // into someone's catalog is worse than a photo that
                        // failed, so the budget only ever nudges here and a
                        // genuine overrun stays an error.
                        //
                        // The reserve is clamped rather than left at the
                        // library's flat 64 because the zone is defined as
                        // `tokenCount >= maxTokens - completionReserve`: at a
                        // budget of 64 or less that is true from the very first
                        // token, so the bias -- which lifts digits by +100, they
                        // being how a JSON *number* ends -- drives the whole
                        // answer. Measured at `max_tokens: 64`, the model
                        // emitted `{"keywords": ["19000000000...`. A quarter of
                        // the budget keeps the nudge to the tail where it
                        // belongs.
                        completionReserve: min(64, max(1, spec.maxTokens / 4)),
                        closingBias: self.bias(for: context),
                        whitespaceBias: whitespace.bias,
                        whitespaceTokenIDs: whitespace.tokenIDs
                    ) { delta in
                        if firstTokenNs == nil {
                            firstTokenNs = DispatchTime.now().uptimeNanoseconds
                        }
                        output += delta
                        return true
                    }
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
                promptTokens: promptTokens, produced: produced,
                sampled: telemetry.sampledTokenIDs.count,
                forced: telemetry.fastForwardTokenIDs.count)
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
    /// is what this photo *waited* for its constraint, not what the constraint
    /// cost to compile. The compile still runs per photo (see
    /// `grammarTokenizer`), but off the critical path, so anything above a
    /// millisecond or two here means the prefetch did not land in time.
    ///
    /// `sampled`/`forced` split `out` into what the model chose and what the
    /// grammar forced. They cost the same wall-clock, so a large `ff` share is
    /// the signal that the *shape* of the response is expensive rather than its
    /// content — punctuation and field names decoded one forward pass at a time.
    /// Omitted on the unconstrained path, where nothing is forced.
    static func logStages(
        prepareMs: Double, grammarMs: Double?, genStart: UInt64, firstTokenNs: UInt64?,
        textTokens: Int, promptTokens: Int, produced: Int,
        sampled: Int? = nil, forced: Int? = nil
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
        if let sampled, let forced {
            let share = produced > 0 ? Int((Double(forced) / Double(produced) * 100).rounded()) : 0
            line += " (sampled=\(sampled) ff=\(forced), \(share)% forced)"
        }
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

    /// Build the grammar vocabulary once per loaded model.
    ///
    /// Taken out of the per-photo path so a compile can be started without
    /// holding `ModelContainer.perform` — the container is busy generating for
    /// the whole time the prefetch wants to run.
    private func prepareGrammarVocabulary(_ loaded: Loaded) async throws {
        if grammarTokenizer != nil, hostTokenizer != nil { return }
        try await loaded.container.perform { context in
            let tVocab = DispatchTime.now().uptimeNanoseconds
            let vocab = TokenizerVocabExtractor.extractForGrammar(from: context.tokenizer)
            self.grammarTokenizer = try GrammarTokenizer(
                vocab: vocab.vocab,
                vocabType: vocab.vocabType,
                eosTokenId: Int32(context.tokenizer.eosTokenId ?? 0))
            self.hostTokenizer = context.tokenizer
            let vocabMs = Self.msSince(tVocab)

            // Warmed here rather than on first use so the whole one-time cost
            // lands in this line instead of inflating the first photo's stage
            // timings. Each of these walks the model's full vocabulary, which
            // is where the ~450ms once attributed to "grammar compile per
            // photo" actually went — see the numbers this logs.
            let tBias = DispatchTime.now().uptimeNanoseconds
            _ = self.bias(for: context)
            _ = self.whitespace(for: context)
            Log.info(
                String(
                    format: "grammar vocabulary ready: extract=%.1fms bias=%.1fms (%d tokens)",
                    vocabMs, Self.msSince(tBias), vocab.vocab.count))
        }
    }

    /// Start compiling `schema` in the background, to be picked up by
    /// `takeConstraint` when the photo that needs it comes round.
    ///
    /// Silently does nothing when there is no schema or no vocabulary yet;
    /// `takeConstraint` then compiles inline, which is exactly the old
    /// behaviour. Nothing here can make a photo fail — a compile error is
    /// stored in the task and surfaces at the point that photo asks for it.
    private func startGrammarCompile(for schema: String?) {
        guard let schema, !schema.isEmpty,
            let tokenizer = grammarTokenizer, let host = hostTokenizer
        else { return }
        if let pending = pendingGrammar, pending.schema == schema { return }
        pendingGrammar?.task.cancel()
        pendingGrammar = (
            schema,
            Task.detached(priority: .userInitiated) {
                try GrammarConstraint(
                    tokenizer: tokenizer,
                    jsonSchema: schema,
                    fastForward: true,
                    hostTokenizer: host)
            }
        )
    }

    /// The constraint for `schema`, from the prefetch when it matches and from
    /// a fresh compile otherwise.
    ///
    /// The vocabulary size comes back alongside because `GrammarConstraint`
    /// keeps its own copy private; only the tokenizer exposes one.
    private func takeConstraint(for schema: String?) async throws -> Grammar? {
        guard let schema, !schema.isEmpty else { return nil }
        guard let tokenizer = grammarTokenizer, let host = hostTokenizer else {
            throw EngineError.noModelLoaded
        }
        let start = DispatchTime.now().uptimeNanoseconds
        if let pending = pendingGrammar, pending.schema == schema {
            pendingGrammar = nil
            return (try await pending.task.value, tokenizer.vocabSize, Self.msSince(start))
        }
        // A different schema than the one queued: that compile is now waste.
        pendingGrammar?.task.cancel()
        pendingGrammar = nil
        let constraint = try GrammarConstraint(
            tokenizer: tokenizer,
            jsonSchema: schema,
            fastForward: true,
            hostTokenizer: host)
        return (constraint, tokenizer.vocabSize, Self.msSince(start))
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
