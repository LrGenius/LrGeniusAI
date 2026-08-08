# LrGeniusAI Backend Server (Rust)

This is the LrGeniusAI backend. It's a single binary (`geniusai-server`)
that speaks the HTTP API the Lightroom plugin expects — see
[../CLAUDE.md](../CLAUDE.md) for the architecture overview and
[`docs/wiki/Dev-Backend-API.md`](../docs/wiki/Dev-Backend-API.md) for the
endpoint reference.

Workspace layout: `crates/lrg-common`, `lrg-store` (LanceDB), `lrg-imaging`,
`lrg-ml` (ONNX Runtime via `ort`), `lrg-analysis`, `lrg-providers` (LLM
clients), `lrg-api` (axum routers), `lrg-server` (the binary).

## Build & run locally

```bash
cd server-rs
cargo build --release -p lrg-server
./target/release/geniusai-server --db-path /path/to/lrgenius.db
```

`cargo test --workspace` and `cargo clippy --workspace --all-targets` should
both be clean before sending changes.

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
has been retired). Still under active development on the `rust-rewrite`
branch — see the plan and progress notes there before assuming a given
endpoint or feature is fully live.
