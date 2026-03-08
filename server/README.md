# 🖥️ LrGeniusAI - Backend Server

This is the Python-based core of **LrGeniusAI**. It acts as the bridge between Adobe Lightroom Classic (Lua) and various AI models, handling image processing, metadata storage, and high-speed semantic search.

---

## 🛠️ Core Responsibilities

- **📂 Database Management:** Stores image metadata, AI-generated descriptions, and vector embeddings in a local SQLite database to ensure blazing-fast retrieval without re-scanning images.
- **🧠 AI Orchestration:** - Interfaces with **Cloud APIs** (Google Gemini, Vertex AI, OpenAI).
    - Connects to **Local LLMs** via Ollama and LM Studio (enhancing privacy and saving costs).
- **🔎 Semantic Search Engine:** Uses `vertexai` / `SigLip2` to generate image embeddings. This allows users to search their Lightroom catalog using natural language descriptions instead of just keywords.
- **🎭 Face Intelligence:** Provides face detection and recognition capabilities (powered by `Insightface`).
- **⚙️ Metadata Sync:** Handles the import of existing keywords and metadata from Lightroom to build a comprehensive search index.

---

## 🚀 Technical Architecture

The server is built with **Python** and designed to run as a local background process (provided as a standalone `.exe` for Windows or a binary for macOS).

### Key Components:
- **API Framework:** FastAPI / Flask-based REST interface for communication with the Lightroom Lua plugin.
- **Computer Vision:** - `open-clip-torch`: For generating semantic embeddings.
    - `Insightface`: For advanced face detection and recognition.
    - `vertexai`: For even better semantic embeddings.
- **Database:** SQLite (local-first approach).
- **Task Handling:** Efficient processing of batch image analysis.

---

## 🗄️ Database API

The backend exposes dedicated database endpoints for status and backup operations.

### `GET /db/stats`

Returns aggregated counters for the current backend database, including:

- total indexed photos
- photos with SigLIP embeddings
- photos with title / caption / keywords
- photos with Vertex AI embeddings
- total indexed faces
- total detected persons

### `GET /db/backup`

Creates and returns a ZIP backup of the persistent backend data directory. This includes the Chroma data as well as accompanying JSON and SQLite files stored under the configured DB path.

Recommended use cases:

- before one-time DB migrations
- before moving or rebuilding the backend host
- before larger maintenance work on the server

The ZIP is created temporarily on the server for download and removed again after the response is sent.

### Lightroom plugin integration

In `Plug-in Manager -> LrGeniusAI -> Backend Server`, the button `Download DB backup` downloads this ZIP from the backend and reveals the saved file in Finder or Explorer.

---

## ☁️ Persistent Vertex AI Login In Docker Compose

If you run the backend remotely via Docker Compose, authenticate inside the container so Vertex AI uses Application Default Credentials (ADC) from the same runtime that executes the Python code.

The Compose file mounts `./gcloud` to `/root/.config/gcloud`, which keeps the active `gcloud` config and `application_default_credentials.json` persistent across container restarts and rebuilds.

### Recommended setup

```bash
mkdir -p gcloud
docker compose up -d --build
docker compose exec geniusai-server gcloud config set project YOUR_PROJECT_ID
docker compose exec geniusai-server gcloud auth application-default login
docker compose exec geniusai-server gcloud auth application-default print-access-token
```

### Headless remote servers

If the server is only reachable via SSH and has no browser, use:

```bash
docker compose exec geniusai-server gcloud auth application-default login --no-browser
```

Then complete the remote bootstrap flow on a second trusted machine with a browser and Google Cloud CLI installed.

### Important notes

- Do not set `GOOGLE_APPLICATION_CREDENTIALS` if you want the backend to use ADC created by `gcloud auth application-default login`.
- Set `Vertex AI Project ID` and `Vertex AI Location` in the Lightroom plugin settings, or provide `GOOGLE_CLOUD_PROJECT` / `VERTEX_LOCATION` as environment variables.
- For fully non-interactive deployments, a service account via `GOOGLE_APPLICATION_CREDENTIALS` is still the better option.

---

## ⚠️ Breaking Change: `photo_id` Migration

The server switched primary IDs from legacy Lightroom UUIDs to file-based `photo_id` values.

If you run an existing database, perform a one-time migration.

### Migration options

- Trigger from Lightroom plugin UI:
  - `File -> Plug-in Manager -> LrGeniusAI -> Backend Server -> Migrate existing DB IDs to photo_id`
- Trigger via API:
  - `POST /db/migrate-photo-ids`
  - Body: `{ "mappings": [{ "old_id": "...", "new_id": "..." }] }`
- Trigger on server startup:
  - Set `GENIUSAI_MIGRATION_FILE` to a JSON mapping file path

### Affected collections

- `image_embeddings`
- `image_embeddings_vertex`
- `face_embeddings` (metadata references)

### Identity scope note

The current `photo_id` / hash / derived `canonicalId` strategy is more stable than legacy Lightroom UUIDs, but it is still not guaranteed to be 100% cross-catalog safe in every workflow.

In practice, backend identity should still be treated as best-effort and mostly catalog-scoped, especially when:

- the same source files exist in multiple Lightroom catalogs
- files were duplicated, re-exported, or rewritten outside Lightroom
- ID generation had to fall back to partial file hashes because stable metadata IDs were unavailable

For workflows that depend on strict cross-catalog identity, re-indexing and migration validation are still recommended when moving photos between catalogs or restoring older backend databases.
