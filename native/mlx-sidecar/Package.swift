// swift-tools-version: 6.1
//
// The MLX inference sidecar for the LrGenius backend.
//
// This builds one executable, `lrgenius-mlx`, which the Rust server spawns and
// drives over a JSON-lines protocol on stdio (see Sources/lrgenius-mlx/Protocol.swift).
// It exists as a separate process rather than as a library linked into the Rust
// binary for three reasons:
//
//  * MLX's Metal kernels ship as a SwiftPM resource bundle that `Bundle.module`
//    locates relative to the *executable*. Staging that next to a normal
//    SwiftPM binary is the supported layout; teaching cargo to reproduce it
//    around a static Swift archive is not.
//  * a wedged or OOM-killed inference run takes down the sidecar, not the
//    server that is holding the user's LanceDB tables open.
//  * `swift build` stays a plain, cacheable CI step with no bindgen or
//    cross-language linking to get wrong.
//
// MLXFoundationModels is deliberately not linked: it wraps Apple's
// FoundationModels framework and needs the macOS 27 SDK, while this ships
// against the macOS 14 floor the rest of the app supports.

import PackageDescription

let package = Package(
    name: "lrgenius-mlx",
    platforms: [
        // MLX itself requires Apple silicon at runtime; the deployment floor
        // here only has to match what the notarized .pkg claims to support.
        .macOS(.v14)
    ],
    dependencies: [
        // Pinned to an exact commit rather than to the 3.31.4 tag on purpose.
        // `MLXGuidedGeneration` — the JSON-Schema-constrained decoder this
        // sidecar needs — landed on main in July 2026 and is not in any tag yet.
        //
        // Doing without it is not an option: every other provider in
        // `lrg-providers` enforces the response schema server-side (Ollama's
        // `format`, LM Studio's `response_format`, llama.cpp's llguidance), and
        // a small local model left to free-decode JSON truncates and malforms
        // it often enough that the llamacpp provider has a dedicated error
        // message for exactly that. Matching the rest of the backend matters
        // more than sitting on a tag.
        //
        // Revisit once mlx-swift-lm cuts a release containing
        // Libraries/MLXGuidedGeneration and move back to `.upToNextMinor`.
        .package(
            url: "https://github.com/ml-explore/mlx-swift-lm",
            revision: "c97539da0e8554d2fad90cc79692381eab1c7906"
        ),
        // mlx-swift-lm deliberately does not depend on a tokenizer
        // implementation — `MLXLMCommon.Tokenizer` is a protocol and consumers
        // inject their own. `MLXHuggingFace`'s macros bridge
        // `Tokenizers.AutoTokenizer` onto it, so we supply swift-transformers.
        .package(
            url: "https://github.com/huggingface/swift-transformers",
            .upToNextMinor(from: "1.3.3")
        ),
    ],
    targets: [
        .executableTarget(
            name: "lrgenius-mlx",
            dependencies: [
                .product(name: "MLXVLM", package: "mlx-swift-lm"),
                // For the text-only fallback in `Engine.load`: a model whose
                // architecture the VLM registry does not know may still be a
                // perfectly good text model, and keyword clustering needs no
                // vision.
                .product(name: "MLXLLM", package: "mlx-swift-lm"),
                .product(name: "MLXLMCommon", package: "mlx-swift-lm"),
                .product(name: "MLXGuidedGeneration", package: "mlx-swift-lm"),
                .product(name: "MLXHuggingFace", package: "mlx-swift-lm"),
                .product(name: "Tokenizers", package: "swift-transformers"),
            ],
            // The MLX generation APIs are not annotated for strict concurrency
            // checking, and this process is single-threaded by design anyway:
            // one request is served at a time off stdin. Swift 5 mode keeps the
            // sidecar readable instead of scattering `nonisolated(unsafe)`
            // through it to satisfy checks that buy us nothing here.
            swiftSettings: [.swiftLanguageMode(.v5)]
        )
    ]
)
