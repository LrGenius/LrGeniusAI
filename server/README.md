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

## ⚠️ Breaking Change: `photo_id` Migration

The server switched primary IDs from legacy Lightroom UUIDs to file-based `photo_id` values.

If you run an existing database, perform a one-time migration.

### Migration options

- Trigger from Lightroom plugin UI:
  - `File -> Plug-in Manager -> LrGeniusAI -> Backend Server -> Migrate existing DB IDs to photo_id`
- Trigger via API:
  - `POST /database/migrate-photo-ids`
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
