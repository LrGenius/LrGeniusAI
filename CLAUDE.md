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

```bash
bash scripts/setup-local-uv-env.sh   # creates .venv, installs deps via uv
```

### Pre-commit hooks (formatting + linting)

```bash
pip install pre-commit
pre-commit install
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

**Routing layer** (`routes_*.py`) — thin HTTP handlers, one Blueprint per domain:
`routes_index`, `routes_search`, `routes_edit`, `routes_faces`, `routes_clip`, `routes_db`, `routes_import`, `routes_server`, `routes_style_edit`, `routes_training`

**Service layer** (`service_*.py`) — business logic:
- `service_chroma.py` — ChromaDB vector store (semantic embeddings)
- `service_clip.py` / `service_vertexai.py` — embedding generation (SigLIP2 / Vertex AI)
- `service_face.py` / `service_persons.py` — InsightFace detection & clustering
- `service_db.py` — SQLite metadata store
- `service_index.py` / `service_search.py` — photo indexing & semantic search
- `service_edit.py` / `service_style_engine.py` — develop edit recipe generation

**LLM providers** (`llm_provider_*.py`): `chatgpt`, `gemini`, `lmstudio`, `ollama`

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

- Endpoints in `routes_*.py` (Blueprints); logic in `service_*.py`.
- Always use the configured `logger`; include `exc_info=True` for exceptions.
- Update `Dockerfile`, `docker-compose-dev.yml`, and `docker-compose-prod.yml` when changing dependencies.
- Code must pass `bash server/scripts/lint_format.sh` (ruff check + ruff format).
