// The wire protocol between the Rust server (`lrg-mlx`) and this sidecar.
//
// One JSON object per line, requests on stdin and responses on stdout, each
// response echoing the request's `id`. Requests are served strictly in order:
// MLX evaluation is not reentrant here and a photo batch saturates the GPU
// anyway, so there is nothing to gain from interleaving.
//
// stdout carries *only* protocol lines. Anything the process wants to say to a
// human goes to stderr, which the Rust side relays into the server log. Getting
// this wrong corrupts the stream, so nothing in this target may `print`.
//
// Images do not travel in the JSON. A decoded photo is several megabytes of raw
// RGB and base64 would inflate every batch line past comfort, so the Rust side
// writes the pixels to a temp file and sends the path; see `ImagePayload`.

import Foundation

// MARK: - Requests

/// The operations the sidecar understands. Decoded via `op`.
enum RequestOp: String, Codable {
    case ping
    case load
    case generate
    case status
    case unload
    case shutdown
}

struct SidecarRequest: Decodable {
    let id: Int
    let op: RequestOp

    /// `load` only: the directory holding an MLX model (config.json, weights,
    /// tokenizer). Always a local path — the sidecar never downloads anything,
    /// because the Rust side owns download progress reporting for every other
    /// model in the app and a second, invisible download path would be worse
    /// than no download path.
    let modelDir: String?

    /// `generate` only.
    let requests: [GenerationSpec]?

    enum CodingKeys: String, CodingKey {
        case id
        case op
        case modelDir = "model_dir"
        case requests
    }
}

/// Raw pixels for one photo, staged in a file the Rust side owns.
struct ImagePayload: Decodable {
    let path: String
    let width: Int
    let height: Int
    /// Bytes per pixel in `path`: 3 for RGB, 4 for RGBA. Only 3 is sent today;
    /// carried explicitly so the reader never has to infer it from the length.
    let channels: Int
}

/// One photo's worth of work. Mirrors `lrg_providers::local::LocalRequest`.
struct GenerationSpec: Decodable {
    let systemPrompt: String
    /// Run-constant across an indexing batch. Kept as its own field rather than
    /// pre-joined so the ordering that makes prefix reuse possible is decided
    /// here and not accidentally reshuffled by a caller.
    let stablePrompt: String
    /// Varies per photo.
    let perPhotoPrompt: String
    let image: ImagePayload?
    /// A JSON Schema, already serialized. Decoding is constrained to it when
    /// present and free when absent.
    let schema: String?
    let maxTokens: Int
    let temperature: Float

    enum CodingKeys: String, CodingKey {
        case systemPrompt = "system_prompt"
        case stablePrompt = "stable_prompt"
        case perPhotoPrompt = "per_photo_prompt"
        case image
        case schema
        case maxTokens = "max_tokens"
        case temperature
    }
}

// MARK: - Responses

/// What the engine settled on after loading. Mirrors `lrg_llama::EngineInfo`
/// closely enough that `/llm/status` can report either backend uniformly.
struct EngineInfoPayload: Encodable {
    let modelPath: String
    let modelName: String
    let supportsVision: Bool

    enum CodingKeys: String, CodingKey {
        case modelPath = "model_path"
        case modelName = "model_name"
        case supportsVision = "supports_vision"
    }
}

/// One photo's result. `ok == false` carries `error` and nothing else, so one
/// bad photo never sinks the rest of a batch.
struct GenerationResultPayload: Encodable {
    let ok: Bool
    let text: String?
    let promptTokens: Int?
    let completionTokens: Int?
    /// Always false on this backend, for two reasons that both have to be
    /// fixed before it could ever be true.
    ///
    /// `GuidedGenerationLoop.run` allocates a fresh KV cache per call
    /// (`model.newCache`) and neither accepts nor returns one, so there is no
    /// seam to hand it a pre-filled cache. Still true at mlx-swift-lm HEAD as
    /// of 2026-08-13; the prompt-cache work in #475 landed in MLXLMCommon, not
    /// in the guided loop.
    ///
    /// And even with that seam there would be almost nothing to reuse: Gemma's
    /// message generator orders the user turn as image-then-text, so the photo
    /// precedes the run-constant prompt and only the system turn is common
    /// across photos. See the ordering note in `Engine.generateOne`.
    ///
    /// Reported honestly rather than optimistically: the llama.cpp backend
    /// really does pin its prefix, and conflating the two would make the
    /// `--debug` logs lie about where the time goes.
    let prefixReused: Bool
    let error: String?

    enum CodingKeys: String, CodingKey {
        case ok
        case text
        case promptTokens = "prompt_tokens"
        case completionTokens = "completion_tokens"
        case prefixReused = "prefix_reused"
        case error
    }

    static func success(text: String, promptTokens: Int, completionTokens: Int) -> Self {
        .init(
            ok: true, text: text, promptTokens: promptTokens,
            completionTokens: completionTokens, prefixReused: false, error: nil)
    }

    static func failure(_ message: String) -> Self {
        .init(
            ok: false, text: nil, promptTokens: nil, completionTokens: nil,
            prefixReused: false, error: message)
    }
}

struct SidecarResponse: Encodable {
    let id: Int
    let ok: Bool
    let error: String?
    let info: EngineInfoPayload?
    let results: [GenerationResultPayload]?
    /// `status` only: whether a model is currently resident.
    let loaded: Bool?

    static func ok(id: Int) -> Self {
        .init(id: id, ok: true, error: nil, info: nil, results: nil, loaded: nil)
    }

    static func failure(id: Int, _ message: String) -> Self {
        .init(id: id, ok: false, error: message, info: nil, results: nil, loaded: nil)
    }

    static func loaded(id: Int, info: EngineInfoPayload) -> Self {
        .init(id: id, ok: true, error: nil, info: info, results: nil, loaded: true)
    }

    static func status(id: Int, info: EngineInfoPayload?) -> Self {
        .init(id: id, ok: true, error: nil, info: info, results: nil, loaded: info != nil)
    }

    static func generated(id: Int, results: [GenerationResultPayload]) -> Self {
        .init(id: id, ok: true, error: nil, info: nil, results: results, loaded: nil)
    }
}
