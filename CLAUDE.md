# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## Project Overview

**LrGeniusAI** is an Adobe Lightroom Classic plugin that brings AI-powered photo analysis (tagging, descriptions, semantic search, develop edits, face recognition) into Lightroom. It consists of two main components:

- **Plugin** (`plugin/LrGeniusAI.lrdevplugin/`) — Lua frontend using the Lightroom SDK
- **Backend** — a local background process the plugin talks to over HTTP on port 19819. Two implementations currently coexist in this repo on the `rust-rewrite` branch:
  - **`server-rs/`** — the Rust rewrite (axum + LanceDB + `ort`/ONNX Runtime), single binary `geniusai-server`, byte-compatible with the same API contract. See [server-rs/README.md](server-rs/README.md).
  - **`server/`** — the original Python/Flask server, still the one that ships until the rewrite is validated end-to-end in Lightroom (see the plan file referenced in project memory for milestone status).

When asked to work on "the backend" with no further qualifier, check which of `server/` or `server-rs/` the task actually concerns — they implement the same API but are separate codebases in different languages; changes to one do not apply to the other.

---

## Development Environment Setup

### Backend (Python)

Dependencies are managed by [uv](https://docs.astral.sh/uv/). The lockfile (`server/uv.lock`) and project metadata (`server/pyproject.toml`) are the source of truth — there are no `requirements*.txt` files.

```bash
cd server
uv sync                  # creates .venv and installs locked deps (incl. dev group)
uv sync --no-dev         # production-equivalent install (matches the Dockerfile)
```

To add or upgrade a dependency, use `uv add <pkg>` (or `uv add --dev <pkg>` for dev-only). This updates both `pyproject.toml` and `uv.lock`; commit both. The Dockerfile picks them up automatically via `uv sync --locked` — no Dockerfile edit needed for routine dependency changes.

### Backend (Rust)

Standard cargo workspace, no extra tooling beyond `rustup`/`cargo`.

```bash
cd server-rs
cargo build --release -p lrg-server
```

`POST /clip/download/start` fetches the SigLIP2 fp16 ONNX assets from this build's own GitHub release; until a release with those assets exists, export them yourself — see [server-rs/README.md](server-rs/README.md) for the exact commands and the env vars that point the server at the files (also covers InsightFace's buffalo_l, which needs no export step).

### Pre-commit hooks (formatting + linting)

```bash
uv tool install pre-commit   # installs pre-commit as a uv-managed tool
pre-commit install           # registers the git hook in this repo
```

---

## Common Commands

### Backend — lint & format

```bash
# Format
uv run ruff format

# Lint + format check (what CI runs)
bash server/scripts/lint_format.sh
```

### Backend — run tests

```bash
cd server
uv run pytest test/                        # all tests
uv run pytest test/test_api_endpoints.py   # single file
```

### Backend — start server locally

```bash
cd server
uv run python src/geniusai_server.py
```

### Backend (Rust) — lint, test, run

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

### Translations sync & check

```bash
python sync_translations.py            # regenerate all three TranslatedStrings_*.txt from LOC() keys
python3 scripts/check_translations.py  # read-only parity check (used by CI and the edit hook)
```

The `.txt` files are UTF-16 — never hand-edit one in isolation; go through the scripts. See the `sync-translations` skill.

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

### Backend (Python/Flask)

Entry point: `server/src/geniusai_server.py` — registers Flask Blueprints and starts via `waitress`.

**Routing layer** (`routes/`) — thin HTTP handlers, one Blueprint per domain:
`routes/index.py`, `routes/search.py`, `routes/edit.py`, `routes/faces.py`, `routes/clip.py`, `routes/db.py`, `routes/import_.py`, `routes/server.py`, `routes/style_edit.py`, `routes/training.py` (the trailing underscore on `import_` avoids the Python keyword).

**Service layer** (`services/`) — business logic:
- `services/chroma.py` — ChromaDB vector store (semantic embeddings)
- `services/clip.py` / `services/vertexai.py` — embedding generation (SigLIP2 / Vertex AI)
- `services/face.py` / `services/persons.py` — InsightFace detection & clustering
- `services/db.py` — SQLite metadata store
- `services/index.py` / `services/search.py` — photo indexing & semantic search
- `services/style_engine.py` — develop edit recipe generation
- `services/update.py` — code-update orchestration (spawns `src/scripts/updater.py`)

**LLM providers** (`providers/`): `providers/chatgpt.py`, `providers/gemini.py`, `providers/lmstudio.py`, `providers/ollama.py`, with the shared base class in `providers/base.py`.

**Shared helpers** (`utils/`): `utils/edit_recipe.py` (recipe schemas and filtering), `utils/open_clip_compat.py` (open_clip tokenizer shim).

Imports use sibling-relative form within a subpackage (`from .face import …` inside `services/`) and absolute form across subpackages (`from services.face import …` from a route). `from config import …` and other root-level modules are unchanged.

**API response format**: always return JSON with `results`, `error`, and `warning` fields.

**Lifecycle**: `server_lifecycle.py` handles PID file and the "OK" signal file used by the plugin to detect when the server is ready.

**Configuration** is driven by environment variables (e.g. `GENIUSAI_PORT`, `GENIUSAI_BACKUP_ENABLED`, `GENIUSAI_FACES_CLUSTER_ENABLED`).

### Backend (Rust) — `server-rs/`

Cargo workspace, one binary (`geniusai-server`) across these crates:

- `lrg-common` — config/CLI, error→envelope, version (baked in via `LRG_BACKEND_*` build-time env vars, `option_env!`), logging+rotation, PID/OK lifecycle handshake.
- `lrg-store` — LanceDB wrapper (replaces ChromaDB), Arrow schemas, `db_path` bind state machine, backup, stats. `lrg-chroma-reader` is migration-only: reads an existing Chroma dir (WAL + HNSW segment binaries) to migrate into LanceDB, no Chroma dependency at runtime.
- `lrg-imaging` — image_convert, EXIF/IPTC/XMP, pHash, culling metrics (port of `utils/image_convert.py` + related).
- `lrg-ml` — ONNX Runtime (`ort` crate) session management, SigLIP2 pre/post-processing + `tokenizers`-crate Gemma tokenizer, SCRFD face detection + ArcFace embedding. Model file locations resolved in `model_paths.rs` (env vars, see [server-rs/README.md](server-rs/README.md)).
- `lrg-analysis` — clustering, person matching, group/cull grading, style engine, keyword clustering.
- `lrg-providers` — LLM provider trait + REST clients (OpenAI, Gemini, Ollama, LM Studio, Vertex AI via `gcp_auth`), edit-recipe schemas.
- `lrg-api` — axum routers (one module per Flask blueprint equivalent under `routes/`), `db_path` auto-bind middleware, jobs registry.
- `lrg-server` — the binary: CLI (`clap`), lifecycle, self-updater (`routes::update`), `migrate` subcommand.

Same API envelope and endpoint contract as the Python backend — `APISearchIndex.lua` does not need to know which one it's talking to. When porting an endpoint from Python to Rust (or fixing one), read the real Python source behavior first rather than assuming from docs/config — this codebase has repeatedly found real backend behavior diverging from what config files or docstrings imply (see project memory for specific examples caught this way).

### Data & Identity

- Primary photo identity: file-based `photo_id` (replaces legacy Lightroom UUIDs).
- Vector search: ChromaDB collections `image_embeddings` (SigLIP2) and `image_embeddings_vertex` (Vertex AI) in the Python backend; the equivalent LanceDB tables (`IMAGE_TABLE`/`VERTEX_TABLE`/`FACE_TABLE` in `lrg_store`) in the Rust backend.
- Multi-catalog support: photos track `catalog_ids`; reads are catalog-scoped when a `catalog_id` is provided. The server never physically deletes photo data.

---

## Key Rules

### Lua / Plugin

- Use `LrTasks.pcall` — never native `pcall`.
- All GUI strings must use `LOC(...)`. Update **all three** translation files when adding/changing strings: `TranslatedStrings_en.txt`, `TranslatedStrings_de.txt`, `TranslatedStrings_fr.txt`.
- Surface all errors to the user via `ErrorHandler.handleError`; no silent failures.
- Logging: `log:error`, `log:warn`, `log:info`, `log:trace`.
- New top-level actions must follow the `Task*.lua` naming convention.
- `APISearchIndex.lua` must be kept in sync with any backend API changes.

### Python / Backend

- Endpoints in `routes/` (Blueprints); logic in `services/`. LLM provider implementations in `providers/`. Shared helpers in `utils/`.
- Always use the configured `logger`; include `exc_info=True` for exceptions.
- Manage dependencies via `uv add` / `uv remove` (updates `pyproject.toml` + `uv.lock`); commit both. The Dockerfile re-runs `uv sync --locked` automatically — only touch it for non-dependency changes (system packages, env vars, build steps).
- Code must pass `bash server/scripts/lint_format.sh` (ruff check + ruff format).

### Rust / Backend (`server-rs/`)

- Routes in `lrg-api::routes` (one axum `Router` per domain, merged in `lrg_api::build_router`); business logic in `lrg-analysis`/`lrg-imaging`/`lrg-ml`; LLM/cloud clients in `lrg-providers`.
- Always use the `log` facade (`log::info!`/`warn!`/`error!`), matching the Python backend's logging discipline.
- Manage dependencies via `cargo add`/`cargo remove` (updates `Cargo.toml` + `Cargo.lock`); commit both.
- Code must pass `cargo fmt` and `cargo clippy --workspace --all-targets` with zero warnings before considering a change done.
- After changing an endpoint, prefer live-testing against the actual running binary (`cargo run -p lrg-server -- --db-path ... --debug` + `curl`) in addition to unit tests — this has repeatedly caught real bugs unit tests alone missed.

### Docs

- Backend port is 19819 by default

### Editor automation (Claude Code)

- A `PostToolUse` hook (`.claude/hooks/lint-edited-file.py`, wired in `.claude/settings.json`) lints every file right after it is edited: `luacheck` + `stylua` for plugin Lua, `ruff check`/`ruff format` for server Python, and the translation parity check for `TranslatedStrings_*.txt`. Fix anything it reports before moving on — don't disable it to get past a warning.

@.claude/skills/lrc-plugin-dev.md
