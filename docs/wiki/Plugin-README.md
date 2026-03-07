# Plugin README

> Auto-generated from `plugin/README.md`. Do not edit this page manually.

# LrGeniusAI Lightroom Plugin

AI-powered metadata, semantic search, and face workflows for Adobe Lightroom Classic.

---

## What It Does

LrGeniusAI adds a backend-powered AI layer to Lightroom Classic. It helps you:

- Generate metadata (`title`, `caption`, `keywords`, `alt_text`)
- Run semantic search on your catalog
- Detect, cluster, and browse people/faces
- Re-import generated metadata back into Lightroom

The plugin is designed to work with local and cloud providers, while keeping Lightroom as your main workspace.

---

## Core Features

### Analyze and Index

- Batch-process selected, visible, all, or missing photos
- Generate embeddings for semantic retrieval
- Generate metadata
- Optional face detection and clustering

### Advanced Search

- Semantic search using image/text embeddings
- Metadata field search (`keywords`, `caption`, `title`, `alt_text`)
- Scope search to current selection/view/catalog

### People Workflows

- Cluster faces into persons
- Rename persons
- Jump from a person directly to a Lightroom collection

### Metadata Sync

- Import existing Lightroom metadata to backend
- Retrieve generated metadata from backend
- Apply validated values back to catalog

---

## Requirements

- Adobe Lightroom Classic (supported by plugin SDK settings)
- LrGeniusAI backend server reachable from Lightroom
- Optional API keys depending on provider:
  - Gemini
  - OpenAI / ChatGPT
  - Vertex AI (project + location)

---

## Installation

1. Build or download the plugin package.
2. In Lightroom Classic, open `File -> Plug-in Manager`.
3. Click `Add` and select the `LrGeniusAI.lrdevplugin` folder.
4. Configure server URL and provider settings in plugin preferences.

---

## Breaking Change: ID Migration Required

The plugin/backend now use file-based `photo_id` values instead of Lightroom catalog UUIDs as primary IDs.
The stable ID algorithm was updated again to avoid ID changes when metadata is written into files (for example DNG metadata updates).

If you already have an indexed backend database from older versions, run this one-time migration:

1. Open `File -> Plug-in Manager`
2. Select `LrGeniusAI`
3. In the `Backend Server` section, click **Migrate existing DB IDs to photo_id**
4. Wait for the `LrProgressScope` migration to finish

Notes:

- Migration is incremental and skips photos that are not indexed in backend.
- Existing migrated entries are skipped automatically.
- Main embeddings, vertex embeddings, and face references are migrated.

## Identity Scope Note

The current `photo_id` / hash / derived `canonicalId` strategy is more stable than Lightroom catalog UUIDs, but it is still not guaranteed to be 100% cross-catalog safe in every workflow.

Treat backend identity as best-effort and primarily catalog-scoped for now, especially when:

- the same files exist in multiple Lightroom catalogs
- files were duplicated, re-exported, or rewritten outside Lightroom
- the plugin had to fall back to partial file hashes because stable metadata IDs were unavailable

If strict cross-catalog identity is important for your workflow, plan for re-indexing or migration checks when moving photos between catalogs or restoring older databases.

---

## Configuration (Plugin Manager)

In the plugin settings dialog you can configure:

- Backend server URL
- Ollama base URL
- API keys and Vertex settings
- Export size and quality used for AI processing
- Prompt presets
- Optional CLIP model download for advanced search

---

## Google Vertex AI Login (gcloud)

If you want to use Vertex AI from LrGeniusAI, run the login on the machine where the backend server runs.

### macOS

1. Install Google Cloud CLI (if not installed):
   - [https://cloud.google.com/sdk/docs/install](https://cloud.google.com/sdk/docs/install)
2. Open Terminal and run:

```bash
gcloud init
gcloud config set project YOUR_PROJECT_ID
gcloud auth application-default login
```

3. Optional verification:

```bash
gcloud auth application-default print-access-token
```

### Windows (PowerShell)

1. Install Google Cloud CLI (if not installed):
   - [https://cloud.google.com/sdk/docs/install](https://cloud.google.com/sdk/docs/install)
2. Open **Google Cloud SDK Shell** (or PowerShell with gcloud in PATH) and run:

```powershell
gcloud init
gcloud config set project YOUR_PROJECT_ID
gcloud auth application-default login
```

3. Optional verification:

```powershell
gcloud auth application-default print-access-token
```

### Notes

- `gcloud auth application-default login` creates local Application Default Credentials (ADC) used by the backend.
- In plugin settings, set `Vertex AI Project ID` and `Vertex AI Location` (for example `us-central1`).
- For headless/server deployments, prefer a service account with `GOOGLE_APPLICATION_CREDENTIALS`.

---

## Typical Workflow

1. Run **Analyze and Index Photos**
2. Optionally validate generated metadata
3. Use **Advanced Search** to find related images
4. Use **People** and **Find Similar Faces** for portrait-heavy catalogs
5. Re-run **Import Metadata from Catalog** if needed for sync

---

## Migration Notes

If you migrated from legacy UUID-based IDs to `photo_id`:

- The plugin can trigger backend migration from the Plugin Manager UI.
- Migration uses a progress scope and batch requests.
- Existing collections (main embeddings, vertex embeddings, faces) are migrated through backend migration endpoints.

---

## Troubleshooting

- Verify backend connectivity in plugin settings (`backendServerUrl`).
- Check log files from Plugin Manager (`Show logfile` / copy logs to desktop).
- If search returns no results, confirm photos were indexed with embeddings.
- If faces are missing, ensure face processing was enabled during indexing.

---

## Documentation

- Help: [https://lrgenius.com/help/](https://lrgenius.com/help/)
- Repository: [https://github.com/LrGenius/LrGeniusAI](https://github.com/LrGenius/LrGeniusAI)
