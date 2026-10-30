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
routers, including the agent bridge to the plugin), `lrg-mcp` (the MCP endpoint
AI agents connect to at `/mcp`, and the `geniusai-server mcp` stdio proxy —
see [`docs/wiki/Dev-Agent-Bridge.md`](../docs/wiki/Dev-Agent-Bridge.md)),
`lrg-chat` (the in-app agent chat at `/v1/chat/*` and its page, a local or
cloud model working through the same tools — see
[`docs/wiki/Dev-Agent-Chat.md`](../docs/wiki/Dev-Agent-Chat.md)),
`lrg-server` (the binary).

There are two *local* LLM backends, selected as the `llamacpp` and `mlx`
providers. They are independent — a build can have either, both, or neither —
and both are served by the same `LocalEngine` trait in
`lrg-providers/src/local.rs`. Besides indexing photos in batches
(`generate`), the trait answers a whole conversation with one turn,
streaming its text as it is decoded (`chat`) — the agent chat is built on that.
The chat itself talks to a model-neutral `ChatModel` (`lrg-providers/src/chat.rs`);
`local_chat.rs` puts a local engine behind it, calling tools by decoding
against a JSON Schema with one branch per tool, since llama.cpp cannot render
the Jinja templates that carry a model's native tool-call format.
Their build requirements differ, so they are covered separately below.

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

### Host and port

The server listens on `127.0.0.1:19819`. Both halves are environment
variables, which is how the Docker image binds to all interfaces:

| Env var | Default |
|---|---|
| `GENIUSAI_HOST` | `127.0.0.1` |
| `GENIUSAI_PORT` | `19819` |

```bash
GENIUSAI_PORT=19833 ./target/release/geniusai-server --db-path /path/to/lrgenius.db
```

An unparseable `GENIUSAI_PORT` falls back to the default rather than refusing
to start, so a typo costs you the port you asked for, not the server.

Moving the port is mainly useful for running a second instance beside an
installed one — a dev build cannot share 19819 with the copy the plug-in
auto-launches. Point the plug-in at it with **Plug-in Manager → Backend server
URL** (`http://127.0.0.1:19833`); that setting is the only place the plug-in
learns the address, so nothing else needs changing. Note that
`SearchIndexAPI.startServer` still launches the *installed* binary on the
default port, so start a custom-port build yourself.

**Do not expose the server publicly.** It speaks plain HTTP, and the plug-in
puts the user's cloud-provider API key (`api_key`, `openai_apikey`,
`server_apikey`, …) in the body of every request that uses one. Bound to
`127.0.0.1` none of that leaves the machine. Bound to anything else, the keys,
the photos and the access token (below) cross the network unencrypted. The
token keeps out whoever does not have it, but not someone who can read the
traffic. Until the server has TLS, run a non-loopback server only on a network
you trust, never with its port forwarded to the internet or open on a cloud
host; reach it from elsewhere through a VPN, an SSH tunnel
(`ssh -L 19819:127.0.0.1:19819 user@host`) or a reverse proxy with HTTPS. The
server does not store the keys; it uses each one only for the request it came
with.

### Access token

Every request needs the server's access token as
`Authorization: Bearer <token>`; without it the answer is a 403 with
`"error_code": "api_token"`. Only `/ping`, `/version`, `/version/check`, the
browser pages' HTML and the routes with tokens of their own (`/mcp`,
`/v1/chat/*`) go without (`lrg_api::api_auth`; the full rules are under
*Access* in [Dev: Backend API](../docs/wiki/Dev-Backend-API.md)).

The server makes the token on its first start and keeps it in a file only the
user can read (`lrg_common::api_token`):

| OS | File |
|---|---|
| macOS | `~/Library/Application Support/LrGeniusAI/api-token` |
| Windows | `%USERPROFILE%\AppData\Local\LrGeniusAI\api-token` |
| Linux | `$XDG_CONFIG_HOME/LrGeniusAI/api-token`, or `~/.config/LrGeniusAI/api-token` |

A plug-in on the same computer reads that file itself. For one on another
computer, print the token and paste it under *Plug-in Manager → Backend
Server → Access token*:

```bash
geniusai-server token          # prints it (makes it if there is none yet)
geniusai-server token --new    # replaces it; restart the server afterwards
```

`LRG_API_TOKEN` (32–256 letters, digits, `-`, `_`) replaces the file with a
fixed token. To call the API by hand:

```bash
curl -H "Authorization: Bearer $(geniusai-server token 2>/dev/null)" \
    http://127.0.0.1:19819/v1/server/health
```

### Golden tests and `LRG_REQUIRE_GOLDENS`

The ML golden tests — SigLIP2, the aesthetic scorer, BioCLIP 2 and the face pipeline — compare real
inference against torch-computed reference values. They need their model files
on disk and **skip when those are absent**, which the Rust test harness reports
as a pass. That is fine on a working copy, and dangerous in CI: a green run is
otherwise indistinguishable from one where the numerical checks never executed.

`LRG_REQUIRE_GOLDENS` turns the skip into a failure for the families you name,
so a job that provisions assets can prove it used them:

```bash
LRG_REQUIRE_GOLDENS=face          cargo test --workspace   # one family
LRG_REQUIRE_GOLDENS=face,siglip   cargo test --workspace   # a subset
LRG_REQUIRE_GOLDENS=all           cargo test --workspace   # every family
```

Family names are `face`, `siglip`, `aesthetic` and `bioclip`. Per-family rather than a
single switch because they differ in cost by more than an order of magnitude
(89 MB, 2.2 GB, 868 MB and 834 MB respectively).

CI uses both halves: `server-rs-tests.yml` downloads the face pair on every PR
and sets `LRG_REQUIRE_GOLDENS=face`, while the nightly `golden-tests-full.yml`
fetches all three and runs with `all`. The models resolve through the same
`LRG_*_ONNX` env vars and `~/.cache/lrgenius/models/` fallback the server uses,
so a machine that has run the plugin's model download needs no extra setup.

### Optional feature: `llamacpp`

The in-process local LLM is behind a cargo feature that is **off by default**.
Without it the `llamacpp` provider reports that the build has no local-model
support and `/v1/llm/catalog` returns `supported: false`, so the plugin's "Local AI
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

Without it, `/v1/llm/catalog` reports `mlx.supported: false` with a reason naming
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

### SigLIP2 NaFlex (semantic embeddings)

`google/siglip2-so400m-patch16-naflex`: the image keeps its aspect ratio
(at most 576 patches of 16x16) instead of being center-cropped to a square.
`POST /v1/models/clip/downloads` + `GET /v1/models/clip/downloads` fetch the fp16
ONNX assets (`siglip2_naflex_image_fp16.onnx`, `siglip2_naflex_text_fp16.onnx`,
`siglip2_naflex_tokenizer.json`) and place them at the `LRG_SIGLIP_*` paths below,
pulling CI-exported ONNX assets from a release instead of the fp32
checkpoint from Hugging Face.

They come from the fixed `model-assets-v2` tag
(`MODEL_ASSETS_RELEASE_TAG` in `routes/clip.rs`), **not** from the binary's
own version tag: the assets change far less often than the app, so
re-uploading them on every release would be wasted. Bumping them means
bumping that constant and the tag together. `model-assets-v2` was that bump
away from the fixed-resolution 384 export, which stays on `model-assets-v1`
for the builds that predate NaFlex. The `.github/workflows/model-assets.yml`
workflow publishes the tag; a build is only usable once it has run.

The image tower takes four inputs (`pixel_values`, `pixel_attention_mask`,
`pos_interp_h`, `pos_interp_w`; see the script's docstring and
`lrg-ml/src/naflex_pre.rs`). The position-embedding resize is done by the
caller as two matmuls because transformers' own data-dependent
`F.interpolate` does not export to ONNX.

To export the models yourself instead — for a local change, or to avoid the
download — run the script the `model-assets.yml` CI job runs:

```bash
cd server-rs
uv run --project scripts --with onnxscript \
    python scripts/export_siglip2_fp16.py --output-dir /path/to/models/siglip2
```

`--with onnxscript` is required: torch >=2.9's `torch.onnx.export` imports
it unconditionally even with `dynamo=False`. This downloads the SigLIP2
checkpoint from Hugging Face, traces it natively in fp16, and writes the two
`.onnx` files, `siglip2_naflex_tokenizer.json` and `siglip_goldens.json`
(reference embeddings from transformers' own fp32 `Siglip2Model`) into the
output directory, then verifies the fp16 graphs against them. Copy
`siglip_goldens.json` to `testdata/` — `tests/goldens.rs` checks its
`model_id` against `lrg_ml::siglip::MODEL_ID`.

Before a real export, `--self-test` checks the hand-written wrappers against
the transformers forward on a tiny random-weight model (seconds, no download):

```bash
uv run --project scripts --with onnxscript \
    python scripts/export_siglip2_fp16.py --self-test
```

Re-verify existing output without exporting again:

```bash
uv run --project scripts python scripts/export_siglip2_fp16.py \
    --verify-only --output-dir /path/to/models/siglip2
```

Both towers should score cosine similarity >0.999 against the fp32
goldens. Then point the server at the files:

```bash
export LRG_SIGLIP_IMAGE_ONNX=/path/to/models/siglip2/siglip2_naflex_image_fp16.onnx
export LRG_SIGLIP_TEXT_ONNX=/path/to/models/siglip2/siglip2_naflex_text_fp16.onnx
export LRG_SIGLIP_TOKENIZER=/path/to/models/siglip2/siglip2_naflex_tokenizer.json
```

Without these set, `lrg-ml` falls back to
`~/.cache/lrgenius/models/{siglip2_naflex_image,siglip2_naflex_text}.onnx`
and `~/.cache/lrgenius/models/siglip2_naflex_tokenizer.json`.

### AestheticSigLIP (photo quality score)

`somepago/AestheticSigLIP` (Apache-2.0) is a SigLIP 2 So400m NaFlex encoder
fine-tuned to score a photo from 1 to 9. It is its **own backbone**, not a head
on the search model's stored embedding, so scoring a photo is a second So400m
forward pass — at the 256 patches it was trained with, with the authors' own
rule for choosing the patch grid (`lrg-ml/src/aesthetic.rs`). Indexing computes
it for the `embeddings` and `cull` tasks, and stores the score as
`aesthetic_score` next to `aesthetic_model`, so a different export is recognised
as stale and re-queued.

It is a separate download (`aesthetic_siglip_fp16.onnx`, ~870 MB) from the same
`model-assets-v2` release, reported as the `aesthetic` family by
`/v1/models/assets` and fetched by the combined download; `LRG_AESTHETIC_ONNX`
overrides the path (default `~/.cache/lrgenius/models/aesthetic_siglip.onnx`).
Without it indexing still works and says once per run that scores were skipped.

The checkpoint is a pickle and the model code is third-party Python, so
`scripts/export_aesthetic_siglip_fp16.py` pins the upstream revision, the
checkpoint's SHA-256 and the SHA-256 of every file it imports, and refuses
anything else; the checkpoint loads with `weights_only=True`.

```bash
cd server-rs
uv run --project scripts --with onnxscript \
    python scripts/export_aesthetic_siglip_fp16.py --output-dir /path/to/models/aesthetic
```

It writes `aesthetic_siglip_fp16.onnx`, `aesthetic_goldens.json` (scores from the
authors' own fp32 code and preprocessing) and `aesthetic_pre_goldens.json`
(their patch grid for 41 image shapes, and their patches for the golden images),
then verifies the fp16 graph against them. `--self-test` checks the wrapper
against the authors' model on a tiny random-weight copy first. Copy both golden
files to `testdata/`; `tests/aesthetic_goldens.rs` needs the ONNX file
(`LRG_REQUIRE_GOLDENS=aesthetic` makes its absence a failure) and
`tests/aesthetic_pre_goldens.rs` needs no model.

The genre next to it (`lrg-ml/src/genre.rs`) is zero-shot from the search
model's embedding: thirteen genres described by prompts whose text embeddings
come from the database like the other fixed prompts, so no text tower runs.

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

The script prints the kept taxon count and every asset's size; the whole set
lands at roughly 880 MB. Verify without re-exporting, and optionally compare
the pruned head's top-1 against the *full* upstream head on real photos — the
measurement that justifies the pruning rules:

```bash
uv run --project scripts python scripts/export_bioclip2_fp16.py \
    --verify-only --output-dir /path/to/models/bioclip2 \
    --fixtures /path/to/some/wildlife/photos
```

Both the export and `--verify-only` score the goldens through one
`bioclip_preprocess()`, which is what `lrg-ml::bioclip_pre` reproduces — the two
sides used to preprocess differently and reported cosines around 0.97 that read
like fp16 damage but were pure measurement error.

Changing the taxa rules does not require re-exporting the tower, which takes
minutes and only changes when the checkpoint does:

```bash
uv run --project scripts python scripts/export_bioclip2_fp16.py \
    --skip-tower --output-dir /path/to/models/bioclip2
```

Then point the server at the files:

```bash
export LRG_BIOCLIP_IMAGE_ONNX=/path/to/models/bioclip2/bioclip2_image_fp16.onnx
export LRG_BIOCLIP_TAXA_BIN=/path/to/models/bioclip2/bioclip2_taxa.bin
export LRG_BIOCLIP_TAXA_JSON=/path/to/models/bioclip2/bioclip2_taxa.json
```

Without these, `lrg-ml` falls back to
`~/.cache/lrgenius/models/bioclip2_{image.onnx,taxa.bin,taxa.json}`, which is
where `POST /v1/models/bioclip/downloads` — and `POST /v1/models/assets/downloads`, which
the plugin actually calls — put them. Note the fallback name for the tower is
`bioclip2_image.onnx`, without the export script's `_fp16` suffix.

Assets are published by `.github/workflows/model-assets-bioclip.yml` to the
fixed `bioclip-assets-v1` tag, independent of SigLIP2's `model-assets-v2`.
**That release does not exist yet**, so the download route reports a 404 until
the workflow has run once; an export placed in the cache directory by hand works
in the meantime.

### Face detection/recognition (YuNet + FaceNet)

Two files: OpenCV Zoo's YuNet detector (already ONNX, downloaded as-is) and
facenet-pytorch's Inception-ResNet-v1 with the VGGFace2 weights (exported).
`scripts/export_face_models.py` produces both and verifies them:

```bash
uv run --no-project server-rs/scripts/export_face_models.py --output-dir face-assets
```

It runs with `--no-project` on purpose — `facenet-pytorch` pins `torch<2.3`
and `numpy<2.0`, which `scripts/pyproject.toml` cannot satisfy, so the script
carries its own PEP 723 dependency header.

Then point the server at the files:

```bash
export LRG_YUNET_ONNX=/path/to/face-assets/yunet_face_detection.onnx
export LRG_FACENET_ONNX=/path/to/face-assets/facenet_vggface2.onnx
```

Without these, `lrg-ml` falls back to
`~/.cache/lrgenius/models/{yunet_face_detection.onnx,facenet_vggface2.onnx}`,
which is where `POST /v1/models/assets/downloads` puts them. Assets are published
by `.github/workflows/model-assets-face.yml` to the fixed `face-assets-v1`
tag, independent of the other two families.

**AuraFace (optional recognizer).** `fal/AuraFace-v1`'s `glintr100.onnx`
(Apache-2.0, ~260 MB) can replace FaceNet: `POST /v1/faces/recognizer/downloads`
fetches it from Hugging Face (`HF_ENDPOINT` is honoured) to
`~/.cache/lrgenius/models/auraface_glintr100.onnx` (or `LRG_AURAFACE_ONNX`),
and `POST /v1/faces/recognizer` switches. The choice is saved in
`~/.cache/lrgenius/models/face_recognizer.txt` (or `LRG_FACE_RECOGNIZER_FILE`)
and restored at start. It runs on the same YuNet landmarks and the same 112x112
ArcFace crop InsightFace makes, RGB, `(v - 127.5) / 127.5`
(`lrg-ml/src/auraface.rs`). The download is pinned to the Hugging Face commit
(`AURAFACE_REVISION` in `lrg-api/src/routes/face_recognizer.rs`) whose model
InsightFace's own embedding in `testdata/face_goldens.json` was made from; the
golden test checks the chain against it (cosine 0.9988, needs > 0.99). To move
the pin, record the reference for the new commit and run the test again:

```bash
uv run --no-project server-rs/scripts/auraface_reference.py --revision <commit> --output ~/.cache/lrgenius/models/auraface_glintr100.onnx
cargo test -p lrg-ml --test face_goldens -- auraface   # LRG_REQUIRE_GOLDENS=auraface to fail instead of skip
```

The two recognizers' rows carry different `face_model` ids
(`yunet-2023mar/facenet-vggface2`, `yunet-2023mar/auraface-v1`), so nothing
compares across them; on re-detection, person assignments from the other
recognizer's rows are carried over by box overlap (same detector).

These replaced InsightFace's `buffalo_l` (SCRFD + ArcFace), which was
resolved from `INSIGHTFACE_ROOT` and could not be redistributed — face
detection was the one family the plugin could not download for you. Note the
two produce embeddings in **different spaces at the same 512 dims**, so face
rows carry `lrg_ml::faces::MODEL_ID` and anything written by the old pair is
ignored by clustering and re-queued for detection.

### Local LLM (in-process llama.cpp)

Only present in builds compiled with the `llamacpp` cargo feature; without
it the `llamacpp` provider reports that the build has no local-model
support and `/v1/llm/catalog` returns `supported: false`.

Models are GGUF pairs — the weights plus an `mmproj` vision projector,
which is what lets the model see the photo. `GET /v1/llm/catalog` lists what
is installed and what can be downloaded; `POST /v1/llm/downloads` fetches
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
for directories that contain a `config.json`, at least one `.safetensors` (the
config alone would match a half-finished download) *and* a chat template
(`chat_template.jinja`, `chat_template.json` or `chat_template` in
`tokenizer_config.json`). The template is what separates a language model from
an image encoder with the same layout: `google/siglip2-so400m-patch16-naflex`,
which the search model's export script leaves in the Hugging Face cache, was
otherwise offered as a chat model.

`GET /v1/llm/catalog` reports MLX under a nested `mlx` key alongside the GGUF half,
including `supported`/`reason`, and `POST /v1/llm/downloads` takes an MLX
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
Models from that cache are listed under their repo's name, not the snapshot
hash.

A model outside both catalogs can be downloaded by repo — "Other model from
Hugging Face…" in the plugin, `POST /v1/llm/downloads/check` then
`POST /v1/llm/downloads` with `repo` (see `crates/lrg-api/src/hf_repo.rs`). The
check refuses, before any download, a repo whose `model_type` or
`processor_class` the pinned mlx-swift-lm does not register:
`hf_repo::MLX_VISION_MODEL_TYPES` and `MLX_VISION_PROCESSORS` copy its
registries and must be updated with every bump of the pin in
`native/mlx-sidecar/Package.swift`.

```bash
# Hugging Face server for catalog downloads and repo checks (a mirror, or a
# fake in tests). Default: https://huggingface.co
export HF_ENDPOINT=https://huggingface.co
```

There are no tuning knobs to match llama.cpp's `n_ctx`/`n_parallel`/GPU layers,
and that is not an oversight: `GuidedGenerationLoop.run` in mlx-swift-lm
allocates a fresh KV cache per call, so indexing has no pinned prompt prefix to
size a context window around, the preferred batch size is 1, and the grammar is
compiled per photo. The prompt is still sent pre-split and stable-first so the
ordering is right if that changes upstream. (The agent chat runs its own copy
of that loop, which keeps a conversation's cache between turns — see
`native/mlx-sidecar/README.md`.)

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
`rust:1-trixie` builder, `debian:trixie-slim` runtime; trixie because the
ONNX Runtime libraries need glibc 2.38+). Model files live in the `/models`
volume: the Dockerfile points every family's env var above there
(`/models/siglip2/...`, `/models/face/...`, `/models/bioclip/...`,
`/models/llm`), which is where the plug-in's *Download AI models* puts them —
see the Dockerfile's own comments for the `ort`/ONNX-Runtime dylib packaging
details.

```bash
docker build -t geniusai-server -f server-rs/Dockerfile server-rs
docker run -p 19819:19819 -v /path/to/data:/data -v /path/to/models:/models \
    -e GENIUSAI_HOST=0.0.0.0 geniusai-server
```

Or via Compose: `docker compose -f ../docker-compose-dev.yml up -d --build`.

The server opens the database `--db-path` names (`/data/db` in the image) when
it starts. It has to: the plug-in binds a backend on its own computer to the
open catalog, but leaves a remote one — a container — on the database it was
started with, and sends it no `db_path` at all.

The image keeps the access token in its `/data` volume
(`XDG_CONFIG_HOME=/data`), so it survives the container being recreated. Print
it for the plug-in with
`docker exec geniusai-server /app/geniusai-server token`.

`-p 19819:19819` publishes the port on every interface of the host, so the
warning under [Host and port](#host-and-port) applies. When Lightroom runs on
the same computer, publish it to that computer only:
`-p 127.0.0.1:19819:19819`.

### Local AI model in a container

The default image has no local AI model: it is built **without** the
`llamacpp` feature, and the MLX sidecar is macOS-only. Inside the container,
`localhost` is not the host, so Ollama and LM Studio are not found
automatically either: enter the one on the host as the plug-in's **Other AI
server** (e.g. `http://host.docker.internal:1234` for LM Studio), or use a cloud
provider.

For in-process inference, build with `--build-arg LLAMACPP=...`. The user
side — the host's GPU drivers, the NVIDIA Container Toolkit, Compose files and
troubleshooting — is the wiki page
[Docker with GPU Access](https://github.com/LrGenius/LrGeniusAI/wiki/Help-Docker-GPU).

| `LLAMACPP` | GPUs | Architectures | Published as |
|---|---|---|---|
| `none` (default) | — | amd64, arm64 | `:latest`, `:v1.2.3` |
| `vulkan` | AMD, Intel, NVIDIA | amd64, arm64 | `:latest-vulkan`, `:v1.2.3-vulkan` |
| `cuda` | NVIDIA (faster there than Vulkan) | amd64 | `:latest-cuda`, `:v1.2.3-cuda` |

```bash
docker build --build-arg LLAMACPP=vulkan -t geniusai-server:vulkan -f server-rs/Dockerfile server-rs
docker run --device /dev/dri ... geniusai-server:vulkan    # AMD/Intel
docker run --gpus all ...        geniusai-server:vulkan    # NVIDIA, either image
```

Both variants run on the CPU when the container is given no GPU, so check the
log: `llama: using <GPU> via Vulkan` (or `via CUDA`) when a model loads, or
`llama: no GPU backend registered, running on CPU`. NVIDIA needs the NVIDIA
Container Toolkit on the host; the Vulkan image asks it for the `graphics`
capability (`NVIDIA_DRIVER_CAPABILITIES`), which is what mounts NVIDIA's
Vulkan driver. Models downloaded from the plug-in's settings go to
`/models/llm` (`LRG_LLAMA_MODEL_DIR`), so they survive the container.

Details that matter when changing this:

- **CUDA** builds on NVIDIA's `devel` image and runs on their `base` image plus
  cuBLAS (`CUDA_VERSION`, default 12.6.3: a 12.x line, because CUDA 13 drops
  Pascal). The host driver has to support that CUDA version. Kernels are
  compiled for ggml's default GPU list; `--build-arg CUDA_ARCHITECTURES=86-real`
  builds for one generation, much faster. Cards that only get PTX compile it on
  first use, into `CUDA_CACHE_PATH=/data/cuda-cache`.
- **amd64** builds llama.cpp's GPU backend and CPU code as modules it loads at
  start (`llamacpp-dynamic-backends`): the CUDA module needs `libcuda.so.1`,
  which a linked-in backend would need just to start the server, and the CPU
  code comes in a variant per CPU generation instead of the no-AVX baseline.
- **arm64** links them in instead, at the armv8-a baseline, and has no CUDA
  image: llama-cpp-sys-2 0.1.156 cannot build the modules for aarch64 Linux
  (see the `dynamic-backends` feature in `crates/lrg-llama/Cargo.toml`).
- The image build fails when the server or one of its modules links a library
  the image lacks, apart from the driver's `libcuda.so.1`, so a packaging
  mistake shows in CI rather than at the first start. Pull requests that touch
  the Dockerfile build every variant (`.github/workflows/docker-images.yml`).

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

Writes have a ceiling of their own. Once the `id` index covers a table,
LanceDB's `merge_insert` loads its whole input into a fixed 150 MB pool it
cannot spill out of, so `Store::upsert` splits anything over 32 MB
(`UPSERT_CHUNK_BYTES`) into several commits. Face clustering, which writes
every face back at once, is the caller that reaches it: about 60,000 faces.

Indexing reads and normalises several photos at once, which is the other
place a batch can grow its peak: that many originals — 25-50 MB apiece for
raw — are resident before any of them has been shrunk to a normalised JPEG.

| Env var | Default |
|---|---|
| `GENIUSAI_INDEX_DECODE_CONCURRENCY` | `min(3, cores)` |

Decode is pure CPU and by far the largest sequential cost of a batch without
an LLM in it, so this trades wall time against peak memory directly. `1`
restores the one-at-a-time behaviour.

The SigLIP2 text tower (about 1 GB resident) is kept out of indexing, culling
and the edit routes altogether. What they score photos against is fixed text —
the CLIP-IQA moment prompts, the genre classifier, the organism gate in front of BioCLIP, the scene probes —
and its vectors are stored in the `text_embeddings` LanceDB table, keyed by
`model_id` and `text` (`lrg-api/src/prompt_embeddings.rs`). A request that needs
a prompt the table lacks runs the text tower for it once, stores the result, and
unloads the tower again straight after when that request is what loaded it;
afterwards only a free-text search or keyword clustering loads it.

The other half of memory behaviour during a long indexing run is
compaction — see `Store::optimize_all` and the constants above it. To
reproduce and measure the write-path memory profile without Lightroom in
the loop:

```bash
cargo run --release -p lrg-store --example memgrow -- 3000
```

It replays the exact per-photo write pattern `index_one` performs and
prints RSS plus LanceDB cache size every 100 photos.

## Disk use: search-page previews

Indexing stores a small JPEG per photo for the browser search page
(`/v1/ui/search`), served by `GET /v1/photos/thumbnail` — `lrg-api/src/thumbnails.rs`.
256 px on the long edge at JPEG quality 75 (`lrg_imaging::convert::THUMBNAIL_*`),
7–20 KB a photo depending on its detail (12.6 KB on average over thirteen
sample photos), so roughly 130 MB per 10,000 photos. They live as files under
the lance root, `lrgenius-lance/thumbnails/<aa>/<sha256 of photo_id>.jpg`, not in a
LanceDB column: every metadata scan would otherwise decode them. Removing a photo
from the index and the index cleanup's purge delete its preview; `POST /v1/db/backups`
leaves the folder out, since any indexing pass writes the previews again. Deleting
the `thumbnails` folder by hand is safe — the page then shows placeholders until
the photos are indexed again.

## Status

This is the sole backend implementation (the earlier Python/Flask server
has been retired) and it ships on `main`. Still under active development,
so check `git log` before assuming a given endpoint or feature behaves the
way an older note describes.
