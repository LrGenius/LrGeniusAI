# LrGeniusAI Backend Server (Rust)

This is the Rust rewrite of the LrGeniusAI backend, replacing the Python
server under [`../server`](../server). It's a single binary
(`geniusai-server`) that speaks the exact same HTTP API the Lightroom
plugin already expects — see [../CLAUDE.md](../CLAUDE.md) for the
architecture overview and [`docs/wiki/Dev-Backend-API.md`](../docs/wiki/Dev-Backend-API.md)
for the endpoint reference.

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

Model files are **not** downloaded automatically yet (`lrg-ml::model_paths`
still documents this as an interim convention) — you need to place them on
disk yourself and point the server at them via environment variables.

### SigLIP2 (semantic embeddings)

There is no signed release with prebuilt fp16 ONNX assets to download yet
(the `build-model-assets` CI job that produces and attaches them only runs
against a real tagged release). Until one exists, export the model
yourself with the script already used to build and verify that CI job:

```bash
cd server-rs
uv run --project ../server --with onnxscript \
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
uv run --project ../server python scripts/export_siglip2_fp16.py \
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
are already ONNX and are the same files the Python backend used. Point
`INSIGHTFACE_ROOT` at the directory containing `models/buffalo_l/{det_10g.onnx,w600k_r50.onnx}`
(default `~/.insightface`, same convention as before — if you previously
ran the Python backend, the files are very likely already there).

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

Not yet ported from the Python backend: the periodic face-clustering and
database-backup schedulers (`GENIUSAI_FACES_CLUSTER_*` / `GENIUSAI_BACKUP_*`
env vars documented in [`../server/README.md`](../server/README.md) are
Python-only for now).

## Status

This backend is under active development on the `rust-rewrite` branch —
see the plan and progress notes there before assuming a given endpoint or
feature is fully live. The Python backend at `../server` remains the
shipping backend until the rewrite is validated end-to-end in Lightroom.
