# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## Project Overview

**LrGeniusAI** is an Adobe Lightroom Classic plugin that brings AI-powered photo analysis (tagging, descriptions, semantic search, develop edits, face recognition) into Lightroom. It consists of two main components:

- **Plugin** (`plugin/LrGeniusAI.lrdevplugin/`) — Lua frontend using the Lightroom SDK
- **Backend** (`server-rs/`) — a local background process the plugin talks to over HTTP on port 19819. Rust (axum + LanceDB + `ort`/ONNX Runtime), single binary `geniusai-server`. See [server-rs/README.md](server-rs/README.md).

The backend was previously a Python/Flask server (`server/`); that implementation has been fully removed now that the Rust rewrite is the shipping backend. If you encounter references to a Python backend in older docs, git history, or memory files, treat them as historical.

---

## Development Environment Setup

### Backend (Rust)

Standard cargo workspace; the default build needs nothing beyond `rustup`/`cargo`.

```bash
cd server-rs
cargo build --release -p lrg-server
```

**The in-process local LLM is behind the `llamacpp` cargo feature, off by default.** Without it the `llamacpp` provider reports "this backend build has no local-model support" and `/llm/catalog` returns `supported: false` — which is the answer to "why doesn't the Local AI Model section do anything". It is off by default because it compiles llama.cpp from source, so it needs `cmake` and `libclang` (bindgen) and adds minutes to a cold build; anyone touching an unrelated crate should not pay that.

**Which local engine ships is a per-platform decision.** Release builds enable `llamacpp` on **Windows only**; the **macOS build is MLX-only** and does not compile the feature at all (see `cargo_features` in the release matrix, `.github/workflows/release.yml`). The plugin mirrors that: `sectionsForTopOfDialog` and the onboarding wizard build the MLX group box on macOS and the llama.cpp one elsewhere, and the provider dropdown needs no special-casing because `/models` reports `llamacpp: []` when the feature is absent. The feature still *builds* on macOS, so enabling it locally to compare the two engines is fine — just don't re-add it to the macOS release job.

```bash
# Needs cmake + libclang. macOS: libclang ships with the Xcode CLT, `brew install cmake`.
# Linux: `apt install cmake libclang-dev`. Windows also builds Vulkan (not CUDA — see
# lrg-llama/Cargo.toml) and additionally needs the Vulkan SDK with VULKAN_SDK set.
cargo build --release -p lrg-server --features llamacpp
cargo clippy --workspace --all-targets --features llamacpp   # must also be clean when you touch this path
```

**MLX (Apple silicon) is a second, separate local backend**, selected as the `mlx` provider. It is *not* behind a cargo feature — `lrg-mlx` only spawns and talks to a helper process, so it costs nothing to compile — but it needs that helper built, and the helper needs `xcodebuild` plus the Metal toolchain (`swift build` cannot compile MLX's Metal shaders):

```bash
xcodebuild -downloadComponent MetalToolchain          # once per machine
cd native/mlx-sidecar
xcodebuild build -scheme lrgenius-mlx -destination 'platform=macOS,arch=arm64' \
  -configuration Release -derivedDataPath .build/xcode \
  -skipPackagePluginValidation -skipMacroValidation
export LRG_MLX_SIDECAR=$PWD/.build/xcode/Build/Products/Release/lrgenius-mlx
```

Without it, `/llm/catalog` reports `mlx.supported: false` with a reason naming what is missing. See [native/mlx-sidecar/README.md](native/mlx-sidecar/README.md) for the protocol, the model layout (a directory, not a GGUF file), and why MLX has no pinned prompt prefix.

Point llama.cpp at a model with `LRG_LLAMA_MODEL_GGUF` + `LRG_LLAMA_MMPROJ_GGUF` (a GGUF needs both the weights and an `mmproj` vision projector), or download one from the plugin's settings. Discovery also picks up GGUFs already under `~/.lmstudio/models`. The `lrg-llama` tests that exercise a real model are `#[ignore]`d and need `LRG_TEST_MODEL_GGUF`/`LRG_TEST_MMPROJ_GGUF`:

```bash
cargo test -p lrg-llama --test engine_smoke -- --ignored
```

When Lightroom is involved, note the plugin auto-launches the *installed* binary (`/Applications/LrGeniusAI/Server/lrgenius-server`), which is not your dev build. `startServer` pings port 19819 first and short-circuits if something already answers, so start your feature-enabled build by hand and the plugin will use it.

`POST /clip/download/start` fetches the SigLIP2 fp16 ONNX assets from the fixed `model-assets-v1` release tag (not the build's own version tag); that release exists with all three assets, so this works on any build. To export them yourself instead, see [server-rs/README.md](server-rs/README.md) for the commands and the env vars that point the server at the files (also covers InsightFace's buffalo_l, which needs no export step). The export/verify script has its own standalone `uv` project at `server-rs/scripts/` (torch/open_clip/onnxruntime — a build-time tool, not a runtime dependency of the Rust binary).

### Pre-commit hooks (formatting + linting)

```bash
uv tool install pre-commit   # installs pre-commit as a uv-managed tool
pre-commit install           # registers the git hook in this repo
```

Hooks: StyLua for the plugin, and local `cargo fmt`/`cargo clippy` hooks for `server-rs/` (require a working `rustup`/`cargo` on PATH — see `.pre-commit-config.yaml`).

---

## Common Commands

### Backend — lint, test, run

```bash
cd server-rs
cargo fmt
cargo clippy --workspace --all-targets   # must be clean, zero warnings
cargo test --workspace
cargo run -p lrg-server -- --db-path /path/to/lrgenius.db --debug
```

### Plugin — load into Lightroom

Add (or symlink) `plugin/LrGeniusAI.lrdevplugin` via Lightroom **Plug-in Manager**. Smoke tests run inside Lightroom via `TaskAutomatedTests.lua`.

### Plugin — headless unit tests

Pure Lua logic (string/table helpers, keyword and photo-id handling) is tested outside Lightroom with [busted]. The Lightroom SDK environment is stubbed in `plugin/spec/spec_helper.lua`.

```bash
busted                       # runs plugin/spec/*_spec.lua (config in /.busted)
```

Add tests when you touch pure helpers in `Util.lua` (or any logic that doesn't require live LR objects). CI runs this via `.github/workflows/lua-tests.yml`.

---

## Architecture

### Plugin (Lua)

Entry point: `Init.lua` — sets up globals, imports all Lightroom SDK modules, loads shared modules (`Util`, `Defaults`, `ErrorHandler`, `APISearchIndex`, etc.).

**`Task*.lua` files** are the top-level actions triggered from *Library → Plug-in Extras*:
- `TaskAnalyzeAndIndex.lua` — AI tagging & description
- `TaskAiEditPhotos.lua` — generate & apply Lightroom develop edits
- `TaskSemanticSearch.lua` — semantic free-text search
- `TaskCullPhotos.lua` — burst/duplicate grouping
- `TaskAutomatedTests.lua` — smoke tests (plugin ↔ backend connectivity)

All long-running operations run inside `LrTasks.startAsyncTask`. Use `LrTasks.pcall` (never native `pcall`) so tasks can yield.

Photo identity uses the stable `globalPhotoId` via `Util.getGlobalPhotoIdForPhoto` (metadata-based, cross-catalog consistent). Two globals are defined everywhere: `WIN_ENV` and `MAC_ENV`.

### Backend (Rust) — `server-rs/`

Cargo workspace, one binary (`geniusai-server`) across these crates:

- `lrg-common` — config/CLI, error→envelope, version (baked in via `LRG_BACKEND_*` build-time env vars, `option_env!`), logging+rotation, PID/OK lifecycle handshake.
- `lrg-store` — LanceDB wrapper, Arrow schemas, `db_path` bind state machine, backup, stats. `lrg-chroma-reader` is migration-only: reads an old ChromaDB directory (WAL + HNSW segment binaries) left over from the retired Python backend, to migrate its data into LanceDB — no Chroma dependency at runtime.
- `lrg-imaging` — image conversion, EXIF/IPTC/XMP, pHash, culling metrics.
- `lrg-ml` — ONNX Runtime (`ort` crate) session management, SigLIP2 pre/post-processing + `tokenizers`-crate Gemma tokenizer, SCRFD face detection + ArcFace embedding. Model file locations resolved in `model_paths.rs` (env vars, see [server-rs/README.md](server-rs/README.md)).
- `lrg-analysis` — clustering, person matching, group/cull grading, style engine, keyword clustering.
- `lrg-providers` — LLM provider trait + REST clients (OpenAI, Gemini, Ollama, LM Studio, and Vertex AI via `gcp_auth` — Vertex AI was removed from the plugin UI in August 2026, so the client is dormant but still compiled and functional), edit-recipe schemas. `local_provider.rs` serves *both* local backends off one `LocalEngine` trait; it has no llama.cpp or MLX dependency of its own.
- `lrg-mlx` — supervises the `lrgenius-mlx` Swift sidecar (`native/mlx-sidecar/`) and speaks its JSON-lines stdio protocol. No native build step, no cargo feature; Apple silicon only at runtime.
- `lrg-api` — axum routers (one module per API domain under `routes/`), `db_path` auto-bind middleware, jobs registry.
- `lrg-server` — the binary: CLI (`clap`), lifecycle, self-updater (`routes::update`), `migrate` subcommand.

`APISearchIndex.lua` on the plugin side defines the API contract; keep it in sync with any endpoint changes here.

### Data & Identity

- Primary photo identity: file-based `photo_id` (replaces legacy Lightroom UUIDs).
- Vector search: LanceDB tables `IMAGE_TABLE`/`VERTEX_TABLE`/`FACE_TABLE` in `lrg_store` (SigLIP2, Vertex AI, and face embeddings respectively). `VERTEX_TABLE` is legacy: the plugin stopped writing and querying it when Vertex AI was removed (August 2026), existing rows are kept.
- Multi-catalog support: photos track `catalog_ids`; reads are catalog-scoped when a `catalog_id` is provided. The server never physically deletes photo data.

---

## Key Rules

### Lua / Plugin

- Use `LrTasks.pcall` — never native `pcall`.
- All GUI strings must use `LOC(...)`. The plugin ships no `TranslatedStrings_*.txt` files — the default string written inline in the `LOC()` call is what the UI shows, so write it as finished user-facing English.
- Surface all errors to the user via `ErrorHandler.handleError`; no silent failures.
- Logging: `log:error`, `log:warn`, `log:info`, `log:trace`.
- New top-level actions must follow the `Task*.lua` naming convention.
- `APISearchIndex.lua` must be kept in sync with any backend API changes.

### Rust / Backend (`server-rs/`)

- Routes in `lrg-api::routes` (one axum `Router` per domain, merged in `lrg_api::build_router`); business logic in `lrg-analysis`/`lrg-imaging`/`lrg-ml`; LLM/cloud clients in `lrg-providers`.
- Always use the `log` facade (`log::info!`/`warn!`/`error!`).
- Manage dependencies via `cargo add`/`cargo remove` (updates `Cargo.toml` + `Cargo.lock`); commit both.
- Code must pass `cargo fmt` and `cargo clippy --workspace --all-targets` with zero warnings before considering a change done.
- After changing an endpoint, prefer live-testing against the actual running binary (`cargo run -p lrg-server -- --db-path ... --debug` + `curl`) in addition to unit tests — this has repeatedly caught real bugs unit tests alone missed.

### Docs

- Backend port is 19819 by default
- **Docs are part of the change, not a follow-up.** `docs/doc-sources.toml` maps each wiki page to the source files it describes; when you touch one of those files, update the page in the same change. If the page is still accurate, say so — either in the PR or with a `Docs-Reviewed: <Page-Name>.md` commit trailer, which is what clears the staleness warning.
- `scripts/check-docs.py` enforces the parts that can be checked mechanically, and runs in pre-commit and CI (`lint-format.yml`):
  - every axum route has a heading in `docs/wiki/Dev-Backend-API.md`, and every heading names a real route (method and path both) — **hard failure**
  - every endpoint in `APISearchIndex.lua`'s `ENDPOINTS` table exists on the backend — **hard failure**, unless it is listed under `[[contract.known_gap]]` in `docs/doc-sources.toml` with a written reason
  - pages whose sources have newer commits than the page — **warning**, reported in the CI job summary
- `scripts/check-docs.py --for <path>` prints the pages that document a file.

### Editor automation (Claude Code)

- A `PostToolUse` hook (`.claude/hooks/lint-edited-file.py`, wired in `.claude/settings.json`) lints every file right after it is edited: `luacheck` + `stylua` for plugin Lua, and `cargo fmt`/`cargo clippy` for `server-rs/` Rust. Fix anything it reports before moving on — don't disable it to get past a warning.
- The same hook names the wiki pages documenting the edited file (once per page per session). Treat that as part of the task: update the page now, or state that it is still correct.

@.claude/skills/lrc-plugin-dev.md
