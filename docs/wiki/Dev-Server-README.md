# Server README

> Auto-generated from `server-rs/README.md`. Do not edit this page manually.

# LrGeniusAI Backend Server (Rust)

This is the LrGeniusAI backend. It's a single binary (`geniusai-server`)
that speaks the HTTP API the Lightroom plugin expects — see
[../CLAUDE.md](../CLAUDE.md) for the architecture overview and
[`docs/wiki/Dev-Backend-API.md`](../docs/wiki/Dev-Backend-API.md) for the
endpoint reference.

Workspace layout: `crates/lrg-common`, `lrg-store` (LanceDB), `lrg-imaging`,
`lrg-ml` (ONNX Runtime via `ort`), `lrg-analysis`, `lrg-providers` (LLM
clients), `lrg-llama` (in-process llama.cpp, behind the `llamacpp` feature),
`lrg-mlx` (supervises the Apple silicon MLX sidecar), `lrg-api` (axum
routers), `lrg-server` (the binary).

There are two *local* LLM backends, selected as the `llamacpp` and `mlx`
providers. They are independent — a build can have either, both, or neither —
and both are served by the same `LocalEngine` trait in
`lrg-providers/src/local_provider.rs`. Their build requirements differ, so they
are covered separately below.

**Which one ships is a per-platform decision.** Release builds enable
`llamacpp` on **Windows only**; the **macOS build is MLX-only** and does not
compile the feature at all (see `cargo_features` in the release matrix,
`.github/workflows/release.yml`). Enabling `llamacpp` locally on macOS to
compare the two engines still works — just don't re-add it to the macOS
release job.

## Build & run locally

```bash
cd server-rs
cargo build --release -p lrg-server
./target/release/geniusai-server --db-path /path/to/lrgenius.db
```

`cargo test --workspace` and `cargo clippy --workspace --all-targets` should
both be clean before sending changes.

### Optional feature: `llamacpp`

The in-process local LLM is behind a cargo feature that is **off by default**.
Without it the `llamacpp` provider reports that the build has no local-model
support and `/llm/catalog` returns `supported: false`, so the plugin's "Local AI
Model" settings have nothing to work with. It is opt-in because it compiles
llama.cpp from source: that needs `cmake` and `libclang` (for bindgen) and adds
minutes to a cold build, which nobody working on an unrelated crate should pay.

```bash
# macOS: libclang ships with the Xcode command line tools; brew install cmake
# Linux: apt install cmake libclang-dev
# Windows: cmake + LLVM, plus the Vulkan SDK (VULKAN_SDK must be set — the build
#   panics without it). Vulkan, deliberately not CUDA: see crates/lrg-llama/Cargo.toml
cargo build --release -p lrg-server --features llamacpp
cargo clippy --workspace --all-targets --features llamacpp
```

Then point it at a model — see [Local LLM](#local-llm-in-process-llamacpp) below
for the env vars — or download one from the plugin's settings. The release
workflow builds with this feature on Windows only, so the shipped macOS binary
does **not** have it.

Tests that exercise a real model are `#[ignore]`d, since they need a multi-GB
GGUF on disk:

```bash
export LRG_TEST_MODEL_GGUF=/path/to/model-Q4_K_M.gguf
export LRG_TEST_MMPROJ_GGUF=/path/to/mmproj-BF16.gguf
cargo test -p lrg-llama --test engine_smoke -- --ignored
```

One gotcha when testing through Lightroom: the plugin auto-launches the
*installed* binary, not your dev build. `startServer` pings port 19819 first and
short-circuits when something already answers, so start your feature-enabled
build by hand and the plugin will use that instead.

### Optional native helper: the MLX sidecar (Apple silicon)

`mlx` is the second local backend, and it is **not** behind a cargo feature:
`lrg-mlx` only spawns and talks to a helper process, so it costs nothing to
compile and availability is a runtime question (Apple silicon + an installed
helper). What it does need is that helper built, and `swift build` will not do
— SwiftPM on the command line cannot compile MLX's Metal shaders, and the
binary it produces dies on the first inference with "Failed to load the default
metallib":

```bash
xcodebuild -downloadComponent MetalToolchain          # ~690 MB, once per machine
cd native/mlx-sidecar
xcodebuild build -scheme lrgenius-mlx -destination 'platform=macOS,arch=arm64' \
  -configuration Release -derivedDataPath .build/xcode \
  -skipPackagePluginValidation -skipMacroValidation
export LRG_MLX_SIDECAR=$PWD/.build/xcode/Build/Products/Release/lrgenius-mlx
```

Without it, `/llm/catalog` reports `mlx.supported: false` with a reason naming
what is missing (wrong architecture, or no helper found). The sidecar is
resolved from `LRG_MLX_SIDECAR`, then from next to the running server, which is
how the shipped `.pkg` finds it. See
[`native/mlx-sidecar/README.md`](../native/mlx-sidecar/README.md) for the
JSON-lines protocol, why it is a separate process, and the model layout.

```bash
export LRG_TEST_MLX_MODEL_DIR=/path/to/mlx-community/gemma-4-e4b-it-4bit
cargo test -p lrg-mlx --test sidecar_smoke -- --ignored
```

## Model files

### SigLIP2 (semantic embeddings)

`POST /clip/download/start` + `GET /clip/download/status` fetch the fp16
ONNX assets (`siglip2_image_fp16.onnx`, `siglip2_text_fp16.onnx`,
`tokenizer.json`) and place them at the `LRG_SIGLIP_*` paths below,
pulling CI-exported ONNX assets from a release instead of the fp32
checkpoint from Hugging Face.

They come from the fixed `model-assets-v1` tag
(`MODEL_ASSETS_RELEASE_TAG` in `routes/clip.rs`), **not** from the binary's
own version tag: the assets change far less often than the app, so
re-uploading them on every release would be wasted. Bumping them means
bumping that constant and the tag together (e.g. `model-assets-v2`).

That release exists with all three assets attached, so the endpoint works
on any build. To export the models yourself instead — for a local change,
or to avoid the download — run the same script the `build-model-assets` CI
job runs:

```bash
cd server-rs
uv run --project scripts --with onnxscript \
    python scripts/export_siglip2_fp16.py --output-dir /path/to/models/siglip2
```

`--with onnxscript` is required: torch >=2.9's `torch.onnx.export` imports
it unconditionally even with `dynamo=False`. This downloads the SigLIP2
checkpoint from Hugging Face, traces it natively in fp16, and writes
`siglip2_image_fp16.onnx` (~860 MB), `siglip2_text_fp16.onnx` (~1.4 GB),
and `tokenizer.json` into the output directory.

Verify the export against the checked-in goldens (no torch/open_clip
needed for this half, just onnxruntime/numpy/tokenizers):

```bash
uv run --project scripts python scripts/export_siglip2_fp16.py \
    --verify-only --output-dir /path/to/models/siglip2
```

Both towers should score cosine similarity >0.99999 against the fp32
goldens. Then point the server at the files:

```bash
export LRG_SIGLIP_IMAGE_ONNX=/path/to/models/siglip2/siglip2_image_fp16.onnx
export LRG_SIGLIP_TEXT_ONNX=/path/to/models/siglip2/siglip2_text_fp16.onnx
export LRG_SIGLIP_TOKENIZER=/path/to/models/siglip2/tokenizer.json
```

Without these set, `lrg-ml` falls back to `~/.cache/lrgenius/models/{siglip2_image,siglip2_text}.onnx`
and `~/.cache/lrgenius/models/tokenizer.json`.

### BioCLIP 2 (species identification)

Two artifacts, because BioCLIP is a CLIP rather than a classifier: the ViT-L/14
image tower, and a precomputed Tree-of-Life zero-shot head that the taxonomy
actually lives in. Upstream's head is 2.66 GB fp32 over 867,455 taxa — larger
than the model — so the export prunes it per
`scripts/bioclip_taxa_filter.toml`, which documents both the rules and what
pruning costs at the species rank.

```bash
uv run --project scripts --with onnxscript \
    python scripts/export_bioclip2_fp16.py --output-dir /path/to/models/bioclip2
```

The script prints the kept taxon count and every asset's size; the target for
the whole set is <= 800 MB. Verify without re-exporting, and optionally compare
the pruned head's top-1 against the *full* upstream head on real photos — the
measurement that justifies the pruning rules:

```bash
uv run --project scripts python scripts/export_bioclip2_fp16.py \
    --verify-only --output-dir /path/to/models/bioclip2 \
    --fixtures /path/to/some/wildlife/photos
```

Then point the server at the files:

```bash
export LRG_BIOCLIP_IMAGE_ONNX=/path/to/models/bioclip2/bioclip2_image_fp16.onnx
export LRG_BIOCLIP_TAXA_BIN=/path/to/models/bioclip2/bioclip2_taxa.bin
export LRG_BIOCLIP_TAXA_JSON=/path/to/models/bioclip2/bioclip2_taxa.json
```

Without these, `lrg-ml` falls back to
`~/.cache/lrgenius/models/bioclip2_{image.onnx,taxa.bin,taxa.json}`, which is
where `POST /bioclip/download/start` puts them.

### InsightFace (face detection/recognition)

No export step needed — `buffalo_l`'s `det_10g.onnx` and `w600k_r50.onnx`
are already ONNX. Point `INSIGHTFACE_ROOT` at the directory containing
`models/buffalo_l/{det_10g.onnx,w600k_r50.onnx}` (default `~/.insightface`,
the same location `insightface`'s own Python library uses, so the files
are very likely already there if you've used InsightFace before).

### Local LLM (in-process llama.cpp)

Only present in builds compiled with the `llamacpp` cargo feature; without
it the `llamacpp` provider reports that the build has no local-model
support and `/llm/catalog` returns `supported: false`.

Models are GGUF pairs — the weights plus an `mmproj` vision projector,
which is what lets the model see the photo. `GET /llm/catalog` lists what
is installed and what can be downloaded; `POST /llm/download/start` fetches
a catalog entry. Discovery looks at, in order: the explicit env overrides,
`LRG_LLAMA_MODEL_DIR` (default `~/.cache/lrgenius/models/llm/`), and any
GGUFs already under `~/.lmstudio/models`, so a model you downloaded in LM
Studio is offered without a second copy.

```bash
# Point at specific files (wins over any directory scan)
export LRG_LLAMA_MODEL_GGUF=/path/to/model-Q4_K_M.gguf
export LRG_LLAMA_MMPROJ_GGUF=/path/to/mmproj-BF16.gguf

# Tuning. The plugin's "Local AI Model" settings send these per request and
# take precedence; these are the fallback when it does not.
export LRG_LLAMA_N_CTX=8192       # context window
export LRG_LLAMA_N_PARALLEL=1     # photos decoded concurrently
export LRG_LLAMA_GPU_LAYERS=999   # 0 = CPU only; anything that does not fit stays on the CPU
```

`n_ctx` and `n_parallel` trade against each other: the whole group of
photos has to fit the context window alongside the shared prompt prefix, and
the engine reduces `n_parallel` with a warning rather than overrunning it.
Changing any of these reloads the model on the next request.

A note on chat templates: llama.cpp's C API applies only templates it
recognises, so a model shipping a modern Jinja template (Gemma 4's is 18 KB
of macros) is refused outright. The engine therefore picks a built-in
template from the GGUF's `general.architecture` and confirms it applies
before use, logging which one it chose. `cargo run -p lrg-llama --example
probe_template -- model.gguf` prints what a GGUF reports and which
templates llama.cpp will accept — start there if a new model misbehaves.

### Local LLM (MLX, Apple silicon)

Same role as the section above, different artifacts: **an MLX model is a
directory, not a file** — a Hugging Face repo snapshot with `config.json`, one
or more safetensors shards, and the tokenizer files. Discovery therefore looks
for directories that contain a `config.json` *and* at least one `.safetensors`
(the config alone would match a half-finished download).

`GET /llm/catalog` reports MLX under a nested `mlx` key alongside the GGUF half,
including `supported`/`reason`, and `POST /llm/download/start` takes an MLX
catalog id on the same route — the id alone picks the backend, so there is one
download queue rather than two competing for the network.

```bash
# Use exactly this model (wins over any directory scan)
export LRG_MLX_MODEL_DIR=/path/to/mlx-community/gemma-4-e4b-it-4bit

# Root that downloads land in and discovery scans
export LRG_MLX_MODEL_ROOT=~/.cache/lrgenius/models/mlx
```

Discovery also picks up `~/.lmstudio/models` (LM Studio has shipped an MLX
engine for a long time) and the `huggingface-cli` cache at
`~/.cache/huggingface/hub`, so a model pulled by hand needs no second copy.

There are no tuning knobs to match llama.cpp's `n_ctx`/`n_parallel`/GPU layers,
and that is not an oversight: `GuidedGenerationLoop.run` in mlx-swift-lm
allocates a fresh KV cache per call, so there is no pinned prompt prefix to
size a context window around, the preferred batch size is 1, and the grammar is
compiled per request. The prompt is still sent pre-split and stable-first so the
ordering is right if that changes upstream.

### Troubleshooting: `LRG_DISABLE_KLEIDIAI`

Setting `LRG_DISABLE_KLEIDIAI=1` turns off ONNX Runtime's arm64 KleidiAI
convolution kernels for every session. It exists only as a field escape
hatch: onnxruntime 1.24 leaked ~25-31 MB per photo in those kernels (a
14k-photo indexing run reached a 45 GB footprint), which is fixed in the
1.28 build we ship. If unbounded memory growth during indexing ever
reappears on an arm64 machine, set this to confirm the cause before
digging further.

It costs roughly **3x indexing throughput on Apple Silicon**
(measured end to end: SigLIP2 316 ms -> 952 ms per photo, face detection
37 ms -> 60 ms), so leave it unset in normal use. Embeddings are
unaffected either way — search rankings and distances are identical.

## Docker

`Dockerfile` here builds and ships the compiled binary (multi-stage:
`rust:1-bookworm` builder, `debian:bookworm-slim` runtime). Model files
still need to be supplied via a mounted `/models` volume following the
env vars above (the Dockerfile pre-sets them to `/models/siglip2/...` and
`/models/insightface`) — see the Dockerfile's own comments for the
`ort`/ONNX-Runtime dylib packaging details.

```bash
docker build -t geniusai-server -f server-rs/Dockerfile server-rs
docker run -p 19819:19819 -v /path/to/data:/data -v /path/to/models:/models \
    -e GENIUSAI_HOST=0.0.0.0 geniusai-server
```

Or via Compose: `docker compose -f ../docker-compose-dev.yml up -d --build`.

The image is built **without** the `llamacpp` feature and has no MLX sidecar, so
neither local backend is available in a container — point the containerized
server at Ollama or LM Studio, or use a cloud provider. Add
`--features llamacpp` to the Dockerfile's `cargo build` (plus `cmake` and
`libclang-dev` in the builder stage) if you want in-process inference there.

Not yet implemented: the periodic face-clustering and database-backup
schedulers — the `GENIUSAI_FACES_CLUSTER_*` / `GENIUSAI_BACKUP_*` env vars
are currently no-ops here.

## Memory tuning

LanceDB's defaults are sized for servers: a 6 GiB index cache and a 1 GiB
file-metadata cache per session. Running next to Lightroom on a desktop,
that headroom is indistinguishable from a leak, so `lrg-store` opens its
connection with an explicit, much smaller session. Both caps are pure
speed/memory tradeoffs (a miss re-reads from disk) and can be overridden,
in MiB:

| Env var | Default |
|---|---|
| `GENIUSAI_LANCE_INDEX_CACHE_MB` | 128 |
| `GENIUSAI_LANCE_METADATA_CACHE_MB` | 128 |

The other half of memory behaviour during a long indexing run is
compaction — see `Store::optimize_all` and the constants above it. To
reproduce and measure the write-path memory profile without Lightroom in
the loop:

```bash
cargo run --release -p lrg-store --example memgrow -- 3000
```

It replays the exact per-photo write pattern `index_one` performs and
prints RSS plus LanceDB cache size every 100 photos.

## Status

This is the sole backend implementation (the earlier Python/Flask server
has been retired) and it ships on `main`. Still under active development,
so check `git log` before assuming a given endpoint or feature behaves the
way an older note describes.
