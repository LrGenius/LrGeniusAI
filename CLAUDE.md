# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## Project Overview

**LrGeniusAI** is an Adobe Lightroom Classic plugin that brings AI-powered photo analysis (tagging, descriptions, semantic search, develop edits, face recognition) into Lightroom. It consists of two main components:

- **Plugin** (`plugin/LrGeniusAI.lrdevplugin/`) — Lua frontend using the Lightroom SDK
- **Backend** (`server/`) — Python/Flask server running as a local background process

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

### Plugin — load into Lightroom

Add (or symlink) `plugin/LrGeniusAI.lrdevplugin` via Lightroom **Plug-in Manager**. Smoke tests run inside Lightroom via `TaskAutomatedTests.lua`.

### Translations sync

```bash
python sync_translations.py
```

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

### Data & Identity

- Primary photo identity: file-based `photo_id` (replaces legacy Lightroom UUIDs).
- Vector search: ChromaDB collections `image_embeddings` (SigLIP2) and `image_embeddings_vertex` (Vertex AI).
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
