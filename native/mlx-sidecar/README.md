# lrgenius-mlx — the MLX inference sidecar

A small Swift executable that runs vision-language models on Apple silicon
through [MLX](https://github.com/ml-explore/mlx-swift-lm). The Rust backend
(`server-rs/crates/lrg-mlx`) spawns it and drives it over a JSON-lines protocol
on stdio; it never listens on a socket and never downloads anything.

This is the `mlx` provider. It sits alongside `llamacpp` rather than replacing
it — see [Which backend does what](#which-backend-does-what).

## Building

**`swift build` does not work.** mlx-swift's own README is explicit that SwiftPM
on the command line cannot compile MLX's Metal shaders; a binary built that way
loads fine and then dies on the first inference with:

```
MLX error: Failed to load the default metallib. library not found
```

Use `xcodebuild`, and install the Metal toolchain first — Xcode 26 ships the
Metal compiler as a separate downloadable component:

```bash
xcodebuild -downloadComponent MetalToolchain      # ~690 MB, once per machine

cd native/mlx-sidecar
xcodebuild build \
  -scheme lrgenius-mlx \
  -destination 'platform=macOS,arch=arm64' \
  -configuration Release \
  -derivedDataPath .build/xcode \
  -skipPackagePluginValidation \
  -skipMacroValidation
```

`-skipPackagePluginValidation` is needed because mlx-swift carries a build
plugin (`CudaBuild`) that Xcode otherwise stops to ask about — which a CI runner
can never answer.

The products land in `.build/xcode/Build/Products/Release/`:

| File | Why it matters |
| --- | --- |
| `lrgenius-mlx` | the executable |
| `mlx-swift_Cmlx.bundle` | MLX's compiled Metal kernels |
| `swift-transformers_Hub.bundle`, `swift-crypto_Crypto.bundle` | tokenizer resources |

**The bundles must ship next to the executable.** They are found through
`Bundle.module`, which resolves relative to the binary; move the binary alone
and you get the metallib error above. `installers/macos/build_pkg.sh` copies all
of them into `/Applications/LrGeniusAI/Server/`.

## Running it by hand

The protocol is one JSON object per line in, one per line out, with `id` echoed
back. Handy for debugging without the Rust server in the way:

```bash
printf '%s\n' \
  '{"id":1,"op":"load","model_dir":"/path/to/gemma-4-e2b-it-4bit"}' \
  '{"id":2,"op":"generate","requests":[{"system_prompt":"","stable_prompt":"Name three primary colours.","per_photo_prompt":"","max_tokens":40,"temperature":0.0}]}' \
  '{"id":3,"op":"shutdown"}' \
  | ./.build/xcode/Build/Products/Release/lrgenius-mlx
```

Operations: `ping`, `load`, `generate`, `status`, `unload`, `shutdown`. Only
`load` and `generate` are used by the Rust side; the rest exist for exactly this
kind of manual poking.

stdout carries protocol lines *only*. Logs go to stderr, which the Rust
supervisor inherits so they land in the server log. Nothing in the target may
`print`.

Photos are not embedded in the JSON: a decoded photo is several megabytes and
base64 would inflate every batch line by a third on top of that. The Rust side
writes raw RGB to a temp file and sends `{path,width,height,channels}`.

## Where models come from

The sidecar only ever loads a local directory. Everything else — discovery,
the curated catalog, downloading — lives in the Rust backend
(`crates/lrg-api/src/mlx_models.rs`) so that there is one download queue and one
progress bar for every model the app fetches.

An MLX model is a Hugging Face repo snapshot (a directory with `config.json`,
safetensors shards and tokenizer files), not a single file like a GGUF. The
backend scans its own model root, `~/.lmstudio/models`, and the Hugging Face CLI
cache, plus `LRG_MLX_MODEL_DIR` as an explicit override.

| Variable | Meaning |
| --- | --- |
| `LRG_MLX_MODEL_ROOT` | where downloads land and discovery scans |
| `LRG_MLX_MODEL_DIR` | use exactly this model directory |
| `LRG_MLX_SIDECAR` | path to `lrgenius-mlx`, overriding the search |

Without `LRG_MLX_SIDECAR` the Rust side looks next to the running server (the
installed layout), then at `native/mlx-sidecar/.build/release/lrgenius-mlx`
relative to a cargo dev build. Note that path is the **SwiftPM** location — if
you build with `xcodebuild` as instructed above, set `LRG_MLX_SIDECAR`.

## Which backend does what

| | `llamacpp` | `mlx` |
| --- | --- | --- |
| Platforms | macOS, Windows, Linux | Apple silicon only |
| Model format | GGUF file + mmproj | model directory |
| In-process | yes | no, a child process |
| Schema-constrained decoding | llguidance | XGrammar |
| Pinned prompt prefix | **yes** | no — see below |
| Photos per batch | `n_parallel` | 1 |

The prefix difference is the one to understand. `lrg-llama` evaluates the
run-constant half of the prompt once and keeps it in the KV cache across photos.
MLX cannot: `GuidedGenerationLoop.run` allocates a fresh cache per call
(`model.newCache`) with no seam to hand it a pre-filled one, so the stable half
is re-prefilled for every photo and `prefix_reused` is always reported as
`false`. The prompt is still sent pre-split and still ordered
stable-then-volatile, so the ordering is right if that changes upstream.

Two smaller consequences of the same limitation: batches are evaluated one photo
at a time (there is no shared prefix to amortise and one VLM prefill already
saturates the GPU), and the JSON-Schema grammar is compiled per request rather
than cloned, because `GrammarConstraint.clone()` — xgrammar's
`GrammarMatcher::Fork()` — fails with `forkFailed` in this build.

## Dependency pinning

`Package.swift` pins mlx-swift-lm to an exact commit rather than a tag, because
`MLXGuidedGeneration` — the JSON-Schema-constrained decoder — landed on `main`
in July 2026 and is not in any release yet. Going without it was not an option:
every other provider in `lrg-providers` enforces the response schema
server-side, and a small local model left to free-decode JSON truncates it often
enough that the llama.cpp provider has a dedicated error message for exactly
that. Move back to `.upToNextMinor` once a release contains
`Libraries/MLXGuidedGeneration`.

## Testing

The Rust-side integration tests drive a real sidecar against a real model and
are `#[ignore]`d:

```bash
export LRG_MLX_SIDECAR=$PWD/native/mlx-sidecar/.build/xcode/Build/Products/Release/lrgenius-mlx
export LRG_TEST_MLX_MODEL_DIR=/path/to/gemma-4-e2b-it-4bit
cd server-rs && cargo test -p lrg-mlx --test sidecar_smoke -- --ignored
```

## Known model incompatibilities

Not every MLX repo loads. `mlx-community/SmolVLM-256M-Instruct-8bit` fails with
a broadcast error (`Shapes (1,576,768) and (1,1024,768)`) because mlx-swift-lm's
Idefics3 processor assumes a 384px vision tower while that model's config says
512. The failure is in the model/library pairing, not in this sidecar — which is
why the curated catalog in `mlx_models.rs` lists only combinations that have
been run.
