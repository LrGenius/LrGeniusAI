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

        return try await loaded.container.perform { context in
            let input = try await context.processor.prepare(input: userInput)
            let promptTokens = input.text.tokens.size

            guard let schema = spec.schema, !schema.isEmpty else {
                let parameters = GenerateParameters(
                    maxTokens: spec.maxTokens, temperature: spec.temperature)
                // The closure parameter is spelled out because `generate` is
                // overloaded on `([Int]) -> _` and `(Int) -> _`, and only the
                // former hands back the full token list this needs.
                let result = try MLXLMCommon.generate(
                    input: input, parameters: parameters, context: context
                ) { (_: [Int]) -> GenerateDisposition in .more }
                return .success(
                    text: result.output,
                    promptTokens: promptTokens,
                    completionTokens: result.tokenIds.count)
            }

            let (constraint, vocabSize) = try self.constraint(for: schema, context: context)
            var output = ""
            let produced = try GuidedGenerationLoop.run(
                input: input,
                context: context,
                constraint: constraint,
                maxTokens: spec.maxTokens,
                vocabSize: vocabSize,
                // Soft zone only (the library's default 64-token reserve).
                // Deliberately no `hardReserve`: the hard zone suppresses every
                // token that is not "closing", and `ClosingTokenBias` counts
                // the digits 0-9 as closing (they finish a JSON *number*).
                // Inside a string that forces digits, which yields structurally
                // valid, semantically worthless output -- a measured
                // `{"title": "19001", "caption": "19002"}`. Junk written into
                // someone's catalog is worse than a failed photo, so the budget
                // only ever nudges here, and a genuine overrun stays an error.
                closingBias: self.bias(for: context)
            ) { delta in
                output += delta
                return true
            }
            return .success(
                text: output, promptTokens: promptTokens, completionTokens: produced)
        }
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
    private func bias(for context: ModelContext) -> MLXArray {
        if let closingBias {
            return closingBias
        }
        let computed = ClosingTokenBias.compute(
            tokenizer: context.tokenizer, eosTokenId: context.tokenizer.eosTokenId)
        closingBias = computed
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
