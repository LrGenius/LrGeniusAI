# Backend API Reference

The `geniusai-server` exposes a REST API over HTTP (default port 19819). All responses use JSON with the structure `{ "results": ..., "error": ..., "warning": ... }`.

This reference is intended for developers integrating with or extending the backend. Regular users interact through the Lightroom plugin and do not need to call these endpoints directly.

---

## Server & Health

### `GET /ping`
Minimal liveness check. Returns `{"status": "ok"}`.

### `GET /health`
Returns local model load state: `clip_model`/`clip_error` (SigLIP2) and
`face_model`/`face_error` (SCRFD/ArcFace), each `"loaded"`, `"not_loaded"`, or
`"failed"`. It does **not** report cloud/local LLM provider availability —
the backend has no stored API keys or base URLs to probe on a bare GET. The
plugin checks provider availability itself (stored keys plus a direct ping to
Ollama/LM Studio); see `SearchIndexAPI.getDetailedHealth()` in
`APISearchIndex.lua`, which backs the Plugin Manager's "System Health" panel
and the Setup Wizard.

### `GET /version`
Returns the running backend version string.

### `POST /version/check`
Checks whether the backend version is compatible with the plugin version passed in the request body.

### `POST /initialize`
Called by the Lightroom plugin on first connect. Accepts catalog/configuration parameters and initializes per-catalog state.

### `GET /models` / `POST /models`
Returns the list of available AI models grouped by provider (`gemini`, `chatgpt`, `ollama`, `lmstudio`, `llamacpp`, `mlx`). Filters out providers that are not configured or not reachable; the two local backends report what is installed on disk, so they are empty when no local model has been downloaded.

### `GET /logs`
Returns recent log lines from the server log.

### `GET /logs/raw/<log_type>`
Streams raw log file content. `log_type` can be `server` or `plugin`.

### `POST /shutdown`
Gracefully shuts down the backend process.

### `POST /restart`
Restarts the backend process. Note: this endpoint is known to be unreliable on Windows (see [Troubleshooting](Troubleshooting)).

### `POST /unload`
Unloads heavy ML models from memory without shutting the server down (useful to reclaim VRAM/RAM between sessions).

### `POST /update/apply`
Triggers an in-place backend self-update from the latest GitHub release.

---

## Photo Indexing

### `POST /index`
Indexes a batch of photos sent as multipart file uploads. Generates embeddings and/or AI metadata depending on options.

**Key request fields:**

| Field | Type | Description |
|---|---|---|
| `photo_id` | string | Stable file-based photo identifier |
| `catalog_id` | string | Lightroom catalog identifier |
| `provider` | string | LLM provider (`gemini`, `chatgpt`, `ollama`, `lmstudio`, `llamacpp`, `mlx`) |
| `model` | string | Model name within the provider (for `llamacpp` the GGUF file name, for `mlx` the model directory name) |
| `llm_n_ctx` | int | `llamacpp` only: context window override (0/absent = default) |
| `llm_n_parallel` | int | `llamacpp` only: photos decoded concurrently |
| `llm_gpu_layers` | int | `llamacpp` only: layers offloaded to the GPU (`0` = CPU only) |
| `generate_metadata` | bool | Generate keywords/title/caption/alt_text |
| `create_embeddings` | bool | Create SigLIP2 semantic embeddings |
| `detect_faces` | bool | Run InsightFace detection on the photo |
| `regenerate` | bool | Re-process even if data already exists |
| `extra_context` | object | Optional context hints (folder, date, GPS, keywords) |

### `POST /index_base64`
Same as `/index` but accepts image data as base64 strings rather than file uploads.

### `POST /index_by_reference`
Indexes photos using server-side file paths instead of uploading image data (for local-backend setups where the server has filesystem access).

### `POST /get`
Returns stored metadata and embedding status for one or more `photo_id` values.

### `GET /get/ids`
Returns a list of all indexed `photo_id` values, optionally filtered by catalog.

### `POST /index/check-unprocessed`
Given a list of `photo_id` values, returns which ones are not yet indexed or are missing specific data.

### `POST /remove`
Removes all data (embeddings, metadata, face data) for a given `photo_id`.

### `POST /remove/metadata`
Removes only the AI-generated metadata fields for a given `photo_id`, leaving embeddings intact.

### `POST /sync/cleanup`
Removes backend records for photos that no longer exist in a given catalog.

### `POST /sync/claim`
Associates an existing backend record with a (potentially new) catalog ID.

---

## Semantic Search

### `GET /search` / `POST /search`
Runs a semantic search query against indexed photos.

**Key request fields:**

| Field | Type | Description |
|---|---|---|
| `query` | string | Natural language search query |
| `catalog_id` | string | Restrict to a specific catalog |
| `scope_ids` | array | Restrict search to specific `photo_id` values |
| `max_results` | int | Maximum number of results to return |
| `strictness` | float | Minimum similarity score threshold |
| `use_clip` | bool | Include visual embedding similarity |
| `use_metadata` | bool | Include keyword/caption/title text search |

**Response:** `results` array with `photo_id` and `score` fields, sorted by relevance descending.

### `POST /find_similar`
Finds photos similar to a reference photo using perceptual hash (phash) or CLIP embeddings.

**Key request fields:**

| Field | Type | Description |
|---|---|---|
| `photo_id` | string | Reference photo |
| `mode` | string | `phash` or `clip` |
| `scope_ids` | array | Optional photo_id scope |
| `max_results` | int | Maximum results |
| `strictness` | string | `strict`, `normal`, or `loose` (phash mode) |

### `POST /group_similar`
Groups a set of photos into similarity clusters (used internally by the culling workflow).

### `POST /cull`
Runs the full culling pipeline on a set of photos: grouping, scoring, and classification into picks/alternates/rejects.

---

## AI Develop Edits

### `POST /edit`
Generates a Lightroom develop recipe for a photo sent as a file upload.

**Key request fields:**

| Field | Type | Description |
|---|---|---|
| `photo_id` | string | Photo identifier |
| `provider` | string | LLM provider |
| `model` | string | Model name |
| `intent` | string | Style preset key (e.g. `natural_pro`, `moody_dramatic`) |
| `style_strength` | float | 0.0–1.0, how aggressively to apply the style |
| `composition_mode` | string | `none`, `subtle`, or `aggressive` |
| `instruction_override` | string | Optional per-photo free-text instruction |

**Response:** A develop recipe object with global adjustments and optional mask list.

### `POST /edit_base64`
Same as `/edit` but accepts image data as base64.

### `POST /style_edit`
Internal: applies style training context when building the edit prompt.

---

## Face Detection & Persons

### `POST /faces/detect`
Detects faces in an uploaded image and stores their embeddings.

### `POST /faces/query`
Finds photos containing faces similar to those in a reference image.

### `POST /faces/cluster`
Re-clusters all stored face embeddings into person groups.

### `GET /faces/persons`
Returns all detected persons with name, photo count, and optional thumbnails.

### `GET /faces/persons/<person_id>/thumbnail`
Returns the representative face thumbnail for a person as JPEG.

### `PUT /faces/persons/<person_id>`
Updates the name assigned to a person cluster.

### `GET /faces/persons/<person_id>/photos`
Returns the list of `photo_id` values associated with a specific person.

---

## Metadata Import

### `POST /import/metadata`
Imports existing Lightroom catalog metadata (keywords, title, caption, rating, etc.) into the backend database for a batch of photos.

---

## Keyword Management

### `POST /keywords/cluster`
Synchronous keyword clustering: groups catalog keywords by semantic similarity.

### `POST /keywords/cluster/start`
Async version: starts a background clustering job and returns a `job_id`.

### `GET /keywords/cluster/status/<job_id>`
Polls the status and result of an async clustering job.

### `POST /keywords/apply-merges`
Applies a list of approved keyword merge pairs to the backend's metadata records.

---

## Style Training

### `POST /training/add`
Saves a photo's Lightroom develop settings as a labeled training example.

### `GET /training/list`
Lists all stored training examples.

### `GET /training/stats`
Returns aggregated statistics about stored training examples (count per label, coverage).

### `GET /training/count`
Returns the total number of stored training examples.

### `DELETE /training/<photo_id>`
Removes the training example for a specific photo.

### `DELETE /training`
Clears all training examples.

---

## CLIP Model Management

### `GET /clip/status`
Returns whether the SigLIP2 embedding model is downloaded and ready.

### `POST /clip/download/start`
Triggers a background download of the CLIP model.

### `GET /clip/download/status`
Returns the current progress of an ongoing CLIP model download.

---

## Local LLM Management (llama.cpp & MLX)

The backend hosts two local inference engines: `llamacpp` (in-process llama.cpp, GGUF models, present only in builds compiled with the `llamacpp` cargo feature) and `mlx` (an `lrgenius-mlx` helper process, Apple silicon only, no cargo feature). Both are exposed through the same routes — the MLX half is nested under an `mlx` key so that older plugins reading the top-level fields still see the llama.cpp engine.

### `GET /llm/catalog`
Lists local models that are installed and those offered for download.

```json
{
  "installed":    [{ "name": "...", "model_path": "...", "mmproj_path": "...", "source": "downloaded|env|lmstudio" }],
  "downloadable": [{ "id": "gemma4-e4b", "label": "...", "approx_bytes": 0, "min_ram_gb": 16, "installed": false }],
  "supported":    true,
  "model_dir":    "~/.cache/lrgenius/models/llm",
  "mlx": {
    "installed":    [{ "name": "gemma-4-e4b-it-4bit", "model_dir": "...", "source": "downloaded|env|lmstudio|huggingface" }],
    "downloadable": [{ "id": "mlx-gemma4-e4b", "label": "...", "approx_bytes": 0, "min_ram_gb": 16, "installed": false }],
    "supported":    false,
    "reason":       "MLX runs only on Apple silicon Macs. …",
    "model_dir":    "~/.cache/lrgenius/models/mlx"
  }
}
```

`supported: false` always carries a `reason` for MLX; for llama.cpp it means the binary was built without the `llamacpp` feature.

### `GET /llm/status`
Engine state (`status`, loaded `model_name`, …) for the llama.cpp engine, with the MLX engine's equivalent nested under `mlx`.

### `POST /llm/download/start`
Starts a background download of a catalog entry. Body: `{ "id": "gemma4-e4b" }`. The id space is shared across both catalogs and the id alone selects the backend, so there is a single download queue: a GGUF pair is fetched as two files, an MLX entry as a repo snapshot staged into a `.part` directory and renamed on success.

### `GET /llm/download/status`
Progress of the current local-model download (same shape as the CLIP download status).

---

## Database Operations

### `GET /db/stats`
Returns aggregate database statistics:
- total indexed photos
- photos with SigLIP embeddings
- photos with Vertex AI embeddings
- photos with title / caption / keywords
- total detected faces
- total persons

### `GET /db/backup`
Creates and streams a ZIP backup of the full persistent LanceDB data directory.

### `POST /db/migrate-photo-ids`
One-time migration endpoint: converts legacy Lightroom UUID-based IDs to `photo_id` values.

**Request body:** `{ "mappings": [{ "old_id": "...", "new_id": "..." }] }`

---

## Response format

All endpoints return:

```json
{
  "results": { ... },
  "error": null,
  "warning": null
}
```

- `results` — the payload (varies per endpoint).
- `error` — a string if an error occurred, `null` otherwise.
- `warning` — a non-fatal warning string, `null` otherwise.

On HTTP error responses (4xx/5xx), `error` is always set.
