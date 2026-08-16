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
| `replace_ss` | bool | Rewrite `ß` as `ss` in the generated title, caption, alt text and keywords. Applied after the model answers, and only to those fields — never to the filename |
| `exposure_bias` | float | Exposure compensation in EV. Stored, and the decisive signal for culling's bracket detection. Omit when the camera did not record it; never default it to 0 |
| `is_raw` | bool | Whether the original is raw. Stored on the photo so later work (style training above all) can keep raw and rendered originals apart. Omit when unknown |
| `extra_context` | object | Optional context hints (folder, date, GPS, keywords) |

`/index_by_reference` carries `exposure_bias` and `is_raw` per image inside the
`images` array instead, since both are properties of the individual photo.

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
| `term` | string | Natural language search query. Also accepted as a query-string parameter |
| `catalog_id` | string | Restrict to a specific catalog |
| `photo_ids` | array | Restrict search to specific `photo_id` values (`uuids` is accepted as an alias) |
| `max_results` | int | Upper bound on results, across **both** halves of the search |
| `relevance_strictness` | int | 0–100. `0` disables the knee filter |
| `search_sources.semantic_siglip` | bool | Include SigLIP embedding similarity |
| `search_sources.metadata` | bool | Include substring search over the AI metadata |
| `search_sources.metadata_fields` | array | Which fields to match; defaults to `flattened_keywords`, `alt_text`, `caption`, `title` |

`catalog_id` and `photo_ids` are independent filters: passing both narrows to
their intersection. (Until August 2026 the metadata half skipped the catalog
check whenever `photo_ids` was present, leaking results from other catalogs.)

**Response:** `results` array of `{photo_id, uuid, distance}`. Semantic matches
come first, ordered by ascending distance; metadata matches follow with
`distance: null` and fill whatever is left of `max_results`.

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
| `is_raw` | bool | Whether the *original* is a raw file. Optional; absent means unknown |
| `adjust_*`, `use_*`, `allow_auto_crop`, `include_masks` | bool | Creative controls. **Absent means enabled** — an unchecked box must travel as an explicit `"false"` |

`is_raw` cannot be recovered on the server: the plugin exports to JPEG before
uploading, so the original encoding is gone by the time the bytes arrive. It
decides two things:

- **How far the guardrails let a recipe push.** Raw files still hold detail
  behind clipped highlights; a rendered file does not.
- **The unit of `temperature`.** Lightroom's `Temp` is Kelvin for raw and a
  relative −100..100 for JPEG/TIFF/PNG. The declared JSON schema follows the
  flag so the model answers in the right unit, and normalization clamps to the
  matching range. A Kelvin-looking value returned for a non-raw photo is
  **dropped, not clamped** — clamping 6200 into −100..100 yields +100, a hard
  orange cast — and the reason lands in `warnings`.

Absent means unknown, which is treated as raw: that is what every catalog
indexed before the flag existed assumed.

**Response:** A develop recipe object with global adjustments and an optional
mask list, plus:

| Field | Type | Description |
|---|---|---|
| `guardrail_reasons` | string[] | Machine-readable codes for what the frame allowed or refused, e.g. `hard_light_no_added_contrast`, `flat_light_contrast_allowed`. Empty when the budget changed nothing |
| `guardrail_explanations` | string[] | The same, as sentences for the UI |
| `edit_warnings` | string[] | Unrelated problems worth surfacing (e.g. no training examples found) |

Before the recipe is returned, the backend decodes the image, measures the
scene (light hardness, dynamic range, specular fraction, shadow clipping),
derives a budget from it, and caps contrast-raising fields — `contrast`,
`clarity`, `dehaze`, the S-strength of the tone curve — plus `shadows` and
`whites` against it. The cap runs *before* the creative-control filter, so a
disabled field is never capped and then discarded.

### `POST /edit_base64`
Same as `/edit` but accepts image data as base64.

### `POST /style_edit`
Produces a recipe from the user's own saved edits with no LLM involved: it
retrieves training examples similar to the photo, re-scores them on exposure,
scene and time of day, and interpolates their develop settings. Needs at least
five stored examples. The result passes through the same guardrail budget as
`/edit` and carries the same `guardrail_reasons`.

---

## Face Detection & Persons

### `POST /faces/detect`
Detects faces in an uploaded image and stores their embeddings.

### `POST /faces/query`
Finds photos containing faces similar to those in a reference image.

### `POST /faces/cluster`
Re-clusters all stored face embeddings into person groups. Faces whose row carries no
usable embedding are reported as unassigned rather than clustered — an empty
vector compares as distance 0 to everything and would merge unrelated clusters.
Above `AGGLOMERATIVE_MAX_POINTS` faces the agglomerative algorithm is skipped
(its n×n distance matrix would be gigabytes) and DBSCAN is used instead.

### `GET /faces/persons`
Returns all detected persons with `person_id`, `name`, `face_count`,
`photo_count` and a representative `thumbnail` (base64 JPEG, empty when the
person has none).

Every face without a person — an empty `person_id` from a row that was never
clustered, or `person_unassigned` written by the clusterer — is reported as a
single entry with an empty `person_id`. The thumbnail is included here on
purpose: fetching it per person from `/faces/persons/<id>/thumbnail` meant one
full table scan per person.

### `GET /faces/persons/<person_id>/thumbnail`
Returns the representative face thumbnail for a person as base64 JPEG. Kept for
older plugin builds; `GET /faces/persons` already includes it, and this scans
the whole face table for one image, so it must not be called in a loop.

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
- photos with Vertex AI embeddings *(legacy — Vertex AI was removed from the plugin in
  August 2026; the field is still returned but the plugin no longer displays it and the
  count no longer grows)*
- photos with title / caption / keywords
- total detected faces
- total persons

### `GET /db/backup`
Creates and streams a ZIP backup of the full persistent LanceDB data directory.

> **Not available:** `POST /db/migrate-photo-ids` existed on the retired Python backend and
> converted legacy Lightroom UUID-based IDs to `photo_id` values. It was deliberately not
> carried over to the Rust backend (see the module comment in `routes/db.rs`). The plugin
> still calls it from `SearchIndexAPI.migratePhotoIdsFromCatalog`, which therefore fails —
> see the note in [Troubleshooting](Troubleshooting).
>
> The unrelated one-time ChromaDB → LanceDB migration is a CLI subcommand, not an
> endpoint: `geniusai-server migrate --db-path <path>`.

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
