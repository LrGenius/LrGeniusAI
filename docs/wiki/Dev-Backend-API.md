# Backend API Reference

The `geniusai-server` exposes a REST API over HTTP (default port 19819). All responses use JSON with the structure `{ "results": ..., "error": ..., "warning": ... }`.

This reference is intended for developers integrating with or extending the backend. Regular users interact through the Lightroom plugin and do not need to call these endpoints directly.

---

## Server & Health

### `GET /ping`
Minimal liveness check. Returns `{"status": "ok"}`.

### `GET /v1/server/health`
Returns local model load state. Each `*_model` field is `"loaded"`,
`"not_loaded"`, or `"failed"`, with a matching `*_error` carrying the message
when it is `"failed"`.

| Field | What it covers |
|---|---|
| `clip_model` / `clip_error` | The SigLIP2 pair, as it has always been. `"failed"` if **either** tower failed, `"loaded"` if **either** is resident. |
| `clip_image_model` / `clip_image_error` | The SigLIP2 **image** tower alone — what indexing and the training path embed with. |
| `clip_text_model` / `clip_text_error` | The SigLIP2 **text** tower alone — what search, keyword lookup and the CLIP-IQA prompt sets embed with. |
| `aesthetic_model` / `aesthetic_error` | AestheticSigLIP, the photo quality scorer. Its own backbone, loaded and idle-unloaded on its own clock. |
| `face_model` / `face_error` | YuNet + FaceNet. |
| `species_model` / `species_error` | BioCLIP 2. |

The two SigLIP2 towers load and idle-unload **independently**, so an
embeddings-only indexing run leaves `clip_image_model: "loaded"` and
`clip_text_model: "not_loaded"`, and a semantic search does the opposite.
`clip_model` is kept as the pair so that a client reading only that field still
learns that something failed rather than missing a failure on the tower it does
not ask about; the per-tower fields are additive and a server older than them
does not send them.

It does **not** report cloud/local LLM provider availability —
the backend has no stored API keys or base URLs to probe on a bare GET. The
plugin checks provider availability itself (stored keys, an Other AI server
address, and whether `POST /v1/llm/providers/models` lists anything for the
built-in engines, Ollama and LM Studio); see `SearchIndexAPI.getDetailedHealth()` in
`APISearchIndex.lua`, which backs the Plugin Manager's "System Health" panel
and the Setup Wizard.

### `GET /version`
Returns the running backend version string.

### `POST /version/check`
Checks whether the backend version is compatible with the plugin version passed in the request body.

### `POST /v1/db/bind`
Called by the Lightroom plugin on first connect. Accepts catalog/configuration parameters and initializes per-catalog state.

### `POST /v1/llm/providers/models`
Returns the list of available AI models grouped by provider (`gemini`, `chatgpt`, `anthropic`, `ollama`, `lmstudio`, `openai_compatible`, `llamacpp`, `mlx`). The two local backends report what is installed on disk, so they are empty when no local model has been downloaded.

Body (all optional; POST so that no key ends up in a URL):

| Field | Description |
|---|---|
| `openai_apikey`, `gemini_apikey`, `anthropic_apikey` | Cloud keys. Without one, that provider's list is empty. |
| `ollama_base_url`, `lmstudio_base_url` | Where to probe Ollama and LM Studio. Absent means their default address on this computer — which is all the current plugin sends. |
| `server_url`, `server_apikey` | The user's own OpenAI-compatible server ("Other AI server" in the plugin): OpenRouter, llama.cpp `llama-server`, LiteLLM, vLLM, or LM Studio/Ollama on another machine. The address is normalised — no scheme means `http://` for local-network hosts and `https://` otherwise, a pasted `…/chat/completions` is cut back, and no path means `/v1`. The key is optional. |

```json
{
  "models":   { "openai_compatible": ["google/gemini-2.5-flash", "…"], "lmstudio": [], "…": [] },
  "servers":  { "openai_compatible": { "label": "OpenRouter" } },
  "aliases":  { "mlx": { "<old name>": "<name offered now>" } },
  "warnings": ["Other AI server: OpenRouter rejected the API key. …"]
}
```

`aliases` maps names a model used to be offered under, and that the backend still accepts as `model`, onto its current name: MLX models from the Hugging Face cache were listed under their snapshot hash before they were listed under their repo name. The plugin uses it to keep a saved choice selected.

A provider that is only probed and not running (Ollama, LM Studio) is simply empty. The user's own server is different: they entered it, so any failure to list it — unreachable, rejected key, malformed address — is reported in `warnings` in words the user can act on. So is an Anthropic key that is set but does not work (rejected, no credit, unreachable API): its listing failure comes back in `warnings` too. OpenAI, Gemini and Anthropic listings time out after 10 s so that a hanging network cannot hold up the whole list. The `anthropic` list is Anthropic's own `/v1/models`, filtered on each model's published capabilities: a model that cannot take images or structured outputs is left out. `servers.openai_compatible.label` (`OpenRouter`, or host and port) is what the plugin shows in front of that server's models. Only models that can take a photo are listed: entries a server marks as text-only (OpenRouter's `architecture.input_modalities`) and embedding models are dropped. An LM Studio entered as the user's server is not also listed under `lmstudio`.

### `GET /v1/server/logs`
Returns recent log lines from the server log.

### `GET /v1/server/logs/<log_type>/raw`
Streams raw log file content. `log_type` can be `server` or `plugin`.

### `POST /shutdown`
Gracefully shuts down the backend process.

### `POST /v1/server/restart`
Restarts the backend process. Note: this endpoint is known to be unreliable on Windows (see [Troubleshooting](Troubleshooting)).

### `POST /v1/server/unload`
Unloads heavy ML models from memory without shutting the server down (useful to reclaim VRAM/RAM between sessions).

### `POST /update/apply`
Triggers an in-place backend self-update from the latest GitHub release.

---

## Photo Indexing

### `POST /v1/index/photos`
Indexes a batch of photos sent as multipart file uploads. Generates embeddings and/or AI metadata depending on options.

**Key request fields:**

| Field | Type | Description |
|---|---|---|
| `photo_id` | string | Stable file-based photo identifier |
| `catalog_id` | string | Lightroom catalog identifier |
| `provider` | string | LLM provider (`gemini`, `chatgpt`, `anthropic`, `ollama`, `lmstudio`, `openai_compatible`, `llamacpp`, `mlx`) |
| `model` | string | Model name within the provider (for `llamacpp` the GGUF file name, for `mlx` the model directory name) |
| `api_key` | string | Key for `chatgpt`/`gemini`/`anthropic` (required) or `openai_compatible` (optional) |
| `server_url` | string | `openai_compatible` only: the server address, normalised as for `/v1/llm/providers/models` |
| `llm_n_ctx` | int | `llamacpp` only: context window override (0/absent = default) |
| `llm_n_parallel` | int | `llamacpp` only: photos decoded concurrently |
| `llm_gpu_layers` | int | `llamacpp` only: layers offloaded to the GPU (`0` = CPU only) |
| `temperature` | float | Sampling temperature, default `0.1`. Only the local providers (`llamacpp`, `ollama`, `lmstudio`, `openai_compatible`) use it: OpenAI reasoning models get a fixed `1.0`, Anthropic and Gemini 3 are sent none, and MLX's guided generation samples on its own settings |
| `reasoning_effort` | string | The plug-in's *Analysis depth*: `low` (default), `medium` or `high`. Sent as OpenAI `reasoning_effort`, Gemini `thinkingLevel` (3.x) or `thinkingBudget` (2.5), and Anthropic `output_config.effort` where the model lists that level; room for thinking (4096 / 8192 / 16384 tokens) is added to `max_tokens` for a model that thinks. Local providers ignore it. Any other value is a **400** naming it, not a silent fallback |
| `generate_metadata` | bool | Generate keywords/title/caption/alt_text |
| `create_embeddings` | bool | Create SigLIP2 semantic embeddings. Also scores the photo's aesthetic and names its genre (see the `aesthetic` task below) |
| `detect_faces` | bool | Run face detection on the photo |
| `regenerate` | bool | Re-process even if data already exists |
| `replace_ss` | bool | Rewrite `ß` as `ss` in the generated title, caption, alt text and keywords. Applied after the model answers, and only to those fields — never to the filename |
| `exposure_bias` | float | Exposure compensation in EV. Stored, and the decisive signal for culling's bracket detection. Omit when the camera did not record it; never default it to 0 |
| `is_raw` | bool | Whether the original is raw. Stored on the photo so later work (style training above all) can keep raw and rendered originals apart. Omit when unknown |
| `original_filename` | string | Single-photo upload only. The original file's name in the catalog (`018A0344.CR3`), for when the uploaded part is a rendition (`018A0344.jpg`). Stored and shown as the photo's `filename`; the part's own name still decides how the bytes are decoded. Ignored for a batch |
| `extra_context` | object | Optional context hints (folder, date, GPS, keywords) |
| `prompt` | string | The system prompt (the plug-in's "Instructions / Prompt" template). It *replaces* the built-in persona rather than extending it. Trimmed, and dropped when blank: a template the user emptied means "no persona of my own", so the built-in default applies instead of an empty system prompt. The edit routes read the same field the same way |
| `existing_keywords` | string[] | The photo's keywords, *without* face tags. Only reaches the prompt when `submit_keywords` is set |
| `existing_face_tags` | string[] | Names Lightroom's face recognition put on the photo. Sent apart from `existing_keywords` and labelled as people in the prompt, so a person's name is not read as a place (issue #315), and asked for by name in the title and caption rather than "a man and a woman" (issue #321) |
| `submit_face_tags` | bool | Whether `existing_face_tags` may reach the prompt. **Defaults to true**, unlike `submit_keywords`: a client old enough not to send this field only puts the names on the wire when its own keyword switch is on, so trusting what arrived leaves that client's behaviour unchanged. Governs the edit routes too |
| `location_sublocation`, `location_city`, `location_state`, `location_country`, `location_country_code` | string | Where the catalog says the photo was taken. Omit a field the catalog has no value for — an empty string counts as "a place is known" and suppresses the GPS lookup below |
| `gps_latitude`, `gps_longitude` | float | The photo's coordinates. Both or neither; a lone latitude is discarded |

All seven location fields are governed by `submit_gps` (default on) and, like the
keyword fields, are only read when metadata is generated. `submit_keywords` and
`submit_face_tags` are separate switches on purpose: sending "beach, sunset" and
naming the people in a private photo to a cloud provider are different
decisions, and each is expressible without the other.

`/v1/index/photos/by-path` carries `exposure_bias`, `is_raw`, `existing_keywords`,
`existing_face_tags` and the seven location fields per image inside the `images`
array instead, since they are properties of the individual photo.

**Where the location comes from, in order.** The catalog fields above win,
because they are the most current — a city corrected in Lightroom is never
written back into a raw original. What they leave open is filled from the file
itself: its EXIF/IPTC, read *before* normalisation, plus any XMP sidecar beside
it. Failing a place name entirely, the coordinates are reverse-geocoded offline
against GeoNames' `cities1000` and the prompt says the name was looked up and
how far away it is.

Reading the file's metadata before normalisation is the fix for issue #321:
normalising re-encodes every raw original and every JPEG over 2048 px, and the
JPEG that comes out has neither EXIF nor IPTC — so location context reached the
model only for small exported JPEGs, and silently vanished for anyone using
"submit originals" or an export size above 2048 px.

`/v1/edit/recipe` and `/v1/edit/recipe/base64` accept the same
`existing_keywords`/`existing_face_tags` pair, under the same switch.

**Organism gate.** `species_prefilter` (default true) runs BioCLIP only on photos whose SigLIP2
embedding scores at least `species_prefilter_threshold` on a fixed set of "is there an animal or
plant" prompts; the rest are checked and stored as `rank = none`. The default threshold
(`DEFAULT_SPECIES_PREFILTER_THRESHOLD`, 0.10) belongs to the current SigLIP2 model, because the score
depends on how far apart that model puts the prompt pairs. It was 0.35 for the 384 center-crop model,
where the same value would skip almost half of the identifiable photos on NaFlex. The NaFlex scores
separate organisms from other photos less cleanly, so the gate is generous and a few non-organisms
reach BioCLIP, which answers them at a coarse rank. Re-measure it with `examples/siglip_thresholds.rs`
whenever `lrg_ml::siglip::MODEL_ID` changes.

**Species reaches the prompt, not the request.** When `tasks` includes
`species`, BioCLIP 2 runs *before* the LLM call rather than after it, and the
identification is written into that photo's prompt as the answer to use (issue
#326 — a general vision model captioned two rabbits as sheep while the species
field on the same photo read "European Rabbit"). A coarse rank is passed on as
a ceiling instead of a name. There is no request field for this: a photo
identified by an earlier run contributes its *stored* identification, so a
metadata-only re-run over already-identified photos benefits too. The line sits
in the per-photo half of the split prompt, so the run-constant KV-cache prefix
is unaffected.

**Response:** `{status, success_count, failure_count, error_messages, warnings, results}`.

`warnings` is where a *degraded success* is reported: the photo was indexed, but
an optional signal was lost — most often `"<file> faces: face model is not
downloaded yet …"` or the same for species. Metadata generation reports here
too: when the LLM returns `success` but omits a field that was asked for, the
provider says so in its own `warning` and it arrives as
`"<file>: the model returned no caption for this photo — the rest was kept …"`. These never fail the photo
(`success_count` still counts it), so a caller that ignores `warnings` shows the
user a clean run that silently produced worse data. Each entry also rides on its
own photo's `results` element as `warnings`, so a grouped caller can attribute it.

### `POST /v1/index/photos/by-path`
Indexes photos using server-side file paths instead of uploading image data (for local-backend
setups where the server has filesystem access).

A bounded number of files is read and normalised at a time, so a group of raw originals is
never all resident at once — the normalised JPEG is a few hundred KB and the raw bytes are
released the moment it exists. Decode is pure CPU and parallel across the group, which for a
run without `metadata` is the largest cost in the request; `GENIUSAI_INDEX_DECODE_CONCURRENCY`
sets how many run at once (default `min(3, cores)`, `1` restores the strictly sequential
behaviour). Results come back in submission order regardless, so a file that cannot be read is
reported as a failure against its own `photo_id` in `results`, not as an anonymous error for
the batch.

`llm_batch_size` is only meaningful when the run generates metadata. The plugin sends it as
the group size for a `llamacpp` run, and omits it otherwise — a group formed for decode
throughput must not override a provider's own preferred batch size.

**Aesthetic score and genre.** The `embeddings` and `cull` tasks both imply the `aesthetic` task,
which can also be asked for alone. It scores the photo with AestheticSigLIP (1 to 9, higher is
better; its own So400m forward pass at 256 patches) and stores `aesthetic_score` with
`aesthetic_model`, and names the photo's genre from its SigLIP2 embedding (zero-shot, thirteen
genres or `other`) as `genre`, `genre_confidence` and `genre_version`. A photo that already has a
score from the current export is skipped unless `regenerate_metadata` is set. When the scorer is not
downloaded the run still indexes the photos and says so **once** in the response's `warnings` (not
once per photo); a genre that could not be made says why the same way.

### `POST /v1/photos/lookup`
Returns stored metadata and embedding status for one or more `photo_id` values.

The response carries an `aesthetic` block next to `species`: `score`, `model`, `outdated` (the score
is from another export than this build's, so not comparable with current ones), `genre` (the stable
id), `genre_label` and `genre_confidence`. It is `null` for a photo with neither. It is a block of
its own, not part of `metadata`, which is text the plug-in lets the user edit.

### `GET /v1/photos/ids`
Returns a list of all indexed `photo_id` values, optionally filtered by catalog.

### `POST /v1/index/unprocessed`
Given a list of `photo_id` values, returns which ones are not yet indexed or are missing specific data.

"Missing" is per task, and where the producer of a field can change it means *produced by the
producer this build uses*, not merely present:

| task | outstanding when |
| --- | --- |
| `embeddings` | no embedding, or a stored `embedding_model` that is not this build's |
| `faces` | no face rows and no `faces_checked`, or a `face_model` that is not this build's |
| `aesthetic` (also implied by `embeddings` and `cull`) | no `aesthetic_score`, or an `aesthetic_model` that is not this build's — and only when the scorer is on disk, since a missing model cannot be fixed by running again |
| genre (with `embeddings`, `cull` and `aesthetic`) | an embedding but no `genre`, or a `genre_version` that is not this build's (the genre list or its prompts changed), and only when the search model is on disk. Made again from the stored embedding, so the photo is not re-embedded |
| `species` | no `species_checked`, or a `species_model` that is not the head on disk |
| `cull` | no `cull_phash`, or no `cull_faces_present` key |
| `metadata` | no title, caption, alt_text or keywords |

`regenerate_metadata` overrides all of them.

It redoes only the tasks of that request. Fields written by a task that is *not* in the request — the
stored embedding, the species, the AI title, caption and keywords, the catalog membership — are kept: redoing
`cull` alone does not remove a photo from search.

The two identity fields differ in one respect, deliberately. For `faces`, a missing `face_model`
re-queues the photo. For `embeddings`, a missing `embedding_model` does **not**: `has_embedding` has
existed since before this field, so an absent identity is the normal state of every existing index,
and re-queueing on absence would re-embed a whole catalog to settle a maybe. A present identity that
differs always re-queues.

Absence is rare in practice now: binding a database runs a one-time store upgrade
(`lrg_store::upgrade`, step `0001-stamp-embedding-model`, recorded in `upgrades.json` in the lance
root) that stamps this build's `embedding_model` on every stored vector that has none. That is safe
because the field was added while one SigLIP2 export was current: the step stamps
`lrg_ml::siglip::LEGACY_MODEL_ID` (the 384 center-crop model), **not** this build's NaFlex model, so
those vectors are re-queued by `/v1/index/unprocessed` like any other stale one. A row naming another
model keeps its name, and the absent-is-current rule stays for any row an older build writes after
the upgrade has run.

### `POST /v1/photos/remove`
Removes all data (embeddings, metadata, face data) for a given `photo_id`.

### `POST /v1/photos/metadata/remove`
Removes only the AI-generated metadata fields for a given `photo_id`, leaving embeddings intact.

### `POST /v1/photos/catalogs/cleanup`
The index cleanup: takes photos deleted from a catalog out of its index. The
plugin runs it on its own (a couple of minutes after Lightroom starts, then once
a day, and when a search turns up photos the catalog no longer has) and from
*Plug-in Manager → Backend Server → Clean up index now* (`IndexCleanup.lua`).
The logic is `lrg_store::cleanup`.

**Request:** `catalog_id` (required), `photo_ids` — every photo ID in the
catalog — and `confirm_large_removal` (bool, default false).

The inventory is sent in one request (including for an empty catalog), since
separate batches would each release the photos outside that batch. The plugin
takes cached identities as they are (no file access) and resolves only photos
that have none yet; it aborts rather than send an incomplete inventory.

Against that inventory, every image row is one of:

| Row | Effect |
|---|---|
| claimed by this catalog, not in the inventory | the catalog lets go (`catalog_id` removed). Other catalogs keep theirs. A row left in no catalog is stamped `orphaned_at` (Unix seconds) |
| in the inventory, in no catalog | re-adopted: the catalog is added back and the stamp cleared — a photo removed by mistake and imported again gets its AI data back |
| in no catalog, stamped more than 30 days ago | purged for good: image row, its faces (`{photo_id}_n`) and its species embedding. Training examples stay |
| in no catalog, no stamp | left alone — rows from before `catalog_ids` existed look like this |

Every row is read again before it is written, and a purge candidate some
catalog claimed since the scan is skipped.

**Large removals:** when the catalog would let go of more than half of what it
has indexed (and at least 50 photos), nothing changes and the answer is a
**409**: `{"error", "needs_confirmation": true, "would_disassociate": n,
"checked": indexed}`. That is what an inventory resolved another way than at
indexing time looks like, and the grace period does not cover it — those IDs
never come back. The plugin asks the user and repeats the request with
`confirm_large_removal: true`.

**Response:**

| Field | Meaning |
|---|---|
| `checked` | photos this catalog had indexed |
| `disassociated` | photos the catalog let go of |
| `orphaned` | of those, how many are now in no catalog |
| `readopted` | photos that came back |
| `purged` | photos deleted for good in this run |
| `waiting` | photos in no catalog, still within the grace period |
| `next_purge_at` | Unix seconds when the oldest of those falls due, or `null` |
| `grace_days` | the grace period (30) |
| `warnings` | purges that failed; the next run retries them |

### `POST /v1/photos/catalogs/claim`
Associates an existing backend record with a (potentially new) catalog ID.
The plugin may scan the whole catalog for automatic or manual claiming, but
identity caching uses catalog plugin properties and never writes photo metadata
or Lightroom’s `lastEditTime`. The existing `photo_id` formats are unchanged.

---

## Semantic Search

### `GET /v1/search` / `POST /v1/search`
Runs a semantic search query against indexed photos.

**Key request fields:**

| Field | Type | Description |
|---|---|---|
| `term` | string | Natural language search query. Also accepted as a query-string parameter. **Optional**: without one the search is "the best N" of the scope, ordered by stored aesthetic score, from metadata alone (no text tower, no vectors) |
| `sort` | string | `relevance`, `aesthetic` or `combined`. Default `relevance` with a term, `aesthetic` without. `relevance` and `combined` without a term are a **400** ("need a search term"), as is an unknown value |
| `min_aesthetic` | float | Only photos whose current aesthetic score is at least this (1–9). A photo with no score from the current export cannot be judged and is left out |
| `genres` | array | Only photos whose `genre` is one of these ids (see `GET /v1/search/genres`). A photo with no genre is left out |
| `aesthetic_weight` | float | `combined` only, 0–1, default 0.5: the share of the ordering that is the aesthetic score rather than relevance |
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

**Response:** `results` array of `{photo_id, uuid, distance, aesthetic_score, genre, genre_label}`
(`aesthetic_score` is `null` for a photo with no score from the current export). By
default semantic matches come first, ordered by ascending distance; metadata matches follow with
`distance: null` and fill whatever is left of `max_results`.

`sort=aesthetic` orders every match by score, best first. `sort=combined` orders by
`(1 - w) * relevance + w * (score - 1) / 8`, where relevance is a semantic hit's distance scaled
between the closest and the farthest of the result set (1 best) and a metadata hit — a literal
match in the title, caption or keywords, which has no distance — counts as fully relevant. In both,
matches without a current score go **last**, in their earlier order: they are not on the scale, and
a warning says how many.

`min_aesthetic` and `genres` are applied **after** the relevance cut, so they never move the cut
(it is computed over the whole candidate list) and can leave fewer than `max_results`. What they
drop for want of data, rather than for its value, is counted in `warnings`, since that is a problem
the user can fix by indexing; a photo that was judged and found short of the minimum is simply not a
match. Without a term, photos in scope with no score are counted the same way.

`warnings` (a list) and `warning` (the same joined, for clients that predate it) appear whenever
there is something to say; before this the route only ever returned `warning` for a missing model.

### `GET /v1/search/genres`
The genres a search can be restricted to, `{"genres": [{"id": "macro", "label": "Macro"}, ...]}`,
`other` last. From `lrg_ml::genre::GENRES`, the list the classifier itself uses, so a UI never
offers one the backend cannot name.

### `POST /v1/search/similar`
Finds photos similar to a reference photo using perceptual hash (phash) or CLIP embeddings.

**Key request fields:**

| Field | Type | Description |
|---|---|---|
| `photo_id` | string | Reference photo |
| `mode` | string | `phash` or `clip` |
| `scope_ids` | array | Optional photo_id scope |
| `max_results` | int | Maximum results |
| `strictness` | string | `strict`, `normal`, or `loose` (phash mode) |

### `POST /v1/cull/groups`
Groups a set of photos into similarity clusters (used internally by the culling workflow).
Takes the same request fields as `/v1/cull/grade` and returns its `groups`,
`warnings` and `warning` (see below); no `summary`.

### `POST /v1/cull/grade`
Runs the full culling pipeline on a set of photos: grouping, scoring, and classification into picks/alternates/rejects.

**Key request fields:**

| Field | Type | Description |
|---|---|---|
| `photo_ids` | array | Photos to cull. Ids with no stored record are skipped and counted in `summary.unindexed_count` |
| `culling_preset` | string | `default`, `portrait`, `street`, `event` or `sports`. 400 with `available_presets` for anything else |
| `time_delta_seconds` | int | Burst window. Absent means the preset's own |
| `phash_threshold`, `clip_threshold` | float or `"auto"` | Grouping thresholds |
| `use_iqa` | bool | Score the CLIP-IQA signals (the moment and the kiss gate) over the stored SigLIP2 embeddings. Default true. The aesthetic term is not one of them: it is the AestheticSigLIP score stored with each photo, used whatever this says |
| `semantic_weight` | float | Overrides the preset's moment weight, for the ranking as well as for deciding whether to score it. Clamped to 0..1 |
| `include_stored_metadata` | bool | Echo each photo's stored `cull_*` inputs as `stored_metadata` (for fixtures) |
| `include_debug` | bool | Per-group thresholds and pairwise distances |

Every preset except `default` judges the **moment**: the stronger of the
preset's axis (`cull_semantic_iqa`: action, expression or candid) and, for
`sports` and `event`, an emotion question (`cull_emotion_iqa`). `event` and
`portrait` also ask whether closed eyes are a kiss or a laugh
(`cull_eyes_closed_intent_iqa`), which lifts the blink penalty. All three are
computed per request from the stored embedding against prompt vectors kept in
the database (below), and never stored themselves. Within a
group the moment applies only when every frame has it, and frames with a
defect no edit repairs (blur, unintended closed eyes, occlusion) always rank
below usable ones.

**Response:**

| Field | Type | Description |
|---|---|---|
| `summary` | object | `group_count`, `pick_count`, `alternate_count`, `reject_candidate_count`, `near_duplicate_group_count`, `intentional_set_group_count`, `intentional_set_photo_count`, `culling_preset`, `unindexed_count` |
| `groups[].winner_photo_id` | string or null | The best-ranked frame that is not a reject candidate. **`null`** when every frame is one: a blurred single, or a burst blurred throughout |
| `groups[].alternate_photo_ids`, `groups[].reject_candidate_photo_ids` | string[] | The rest of the group |
| `groups[].photos[]` | object[] | Per photo: `rank`, `cull_score`, `winner`, `reject_candidate`, `reason_codes`, `explanation`, `metrics`. `metrics` carries `semantic`/`semantic_axis`, `emotion`/`emotion_axis` and `moment` only when computed, `eyes_closed_intended` when true, and always `clear_defect` |
| `warnings` | string[] | Every caveat for the run, most fundamental first: unanalyzed photos skipped, no embeddings, AI signals whose prompt vectors could not be prepared (one message naming every lost signal and the fix), photos whose moment was not judged. Empty when there is nothing to say |
| `warning` | string or null | The same messages joined, for clients older than `warnings` |

The **aesthetic term** of every preset is the stored AestheticSigLIP score: `aesthetic_score` of a
photo whose `aesthetic_model` is this build's, scaled `(s - 1) / 8` to `0..1`, injected as
`cull_aesthetic_model` (so it appears in `stored_metadata`) and blended with the heuristic at
`aesthetic_model_weight` (0.9, a chosen value rather than a tuned one). A score from another export
counts as missing. It needs no embedding and no text tower, so it is independent of `use_iqa`. The
number of ranked photos without one is said once in `warnings`, with the fix that fits: scoring them,
or downloading the model first.

Both routes return `warnings`, every caveat for the run as a list, and `warning`, the same
caveats joined into the one string the plugin shows (`null` when there are none). Besides photos
that were never analyzed and photos without embeddings, a caveat is raised when an AI signal the
run asked for — quality, moment, kiss gate — could not be computed: each compares a photo's stored
embedding against fixed prompt vectors, and those were neither in the database nor obtainable from
the text tower, almost always because the SigLIP2 model is not downloaded. The signals lost to that
one cause share one message, and ranking rests on the technical measurements alone.

The prompt vectors — the CLIP-IQA moment and emotion sets, the genre classifier, the culling kiss gate, the organism
gate `/v1/index/photos` uses in front of BioCLIP, the scene probes of `/v1/edit/training` and
`/v1/edit/style` — live in the `text_embeddings` table (`model_id`, `text`, `vector`). A request prepares the prompts it needs
once, before any per-photo work: from memory, else from the table for this build's
`lrg_ml::siglip::MODEL_ID`, else with the text tower, whose results are stored and which is unloaded
again straight away if the request loaded it. So each prompt is embedded once per model per
database, and in normal use only a free-text search or keyword clustering loads the text tower.
Only the prompts the request needs: on a CPU without fast fp16 one text-tower pass can take many
seconds, and a first cull must stay inside the plugin's request timeout.

---

## AI Develop Edits

### `POST /v1/edit/recipe`
Generates a Lightroom develop recipe for a photo sent as a file upload, using an
LLM.

> No plugin workflow calls this any more. *AI Edit Photos* runs on
> `POST /v1/edit/style` alone and never asks for the LLM fallback, so this endpoint
> and `/v1/edit/recipe/base64` are currently reachable only by direct HTTP callers. Both
> are fully supported; `SearchIndexAPI.generateEditRecipePhoto` in the plugin
> still speaks to `/v1/edit/recipe` and is simply not called.

**Key request fields:**

| Field | Type | Description |
|---|---|---|
| `photo_id` | string | Photo identifier |
| `provider` | string | LLM provider |
| `model` | string | Model name |
| `api_key`, `server_url` | string | Connection fields, as for `/v1/index/photos` |
| `temperature`, `reasoning_effort` | float, string | As for `/v1/index/photos`, including the 400 for an unknown `reasoning_effort`. `/v1/edit/style` reads both for its LLM fallback |
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

### `POST /v1/edit/recipe/base64`
Same as `/v1/edit/recipe` but accepts image data as base64.

### `POST /v1/edit/style`
Produces a recipe from the user's own saved edits with no LLM involved: it
retrieves training examples similar to the photo, re-scores them on exposure,
scene and time of day, and interpolates their develop settings. Needs at least
five stored examples. The result passes through the same guardrail budget as
`/v1/edit/recipe` and carries the same `guardrail_reasons`.

`temperature` is blended only from examples whose `is_raw` matches the photo
being edited, because Lightroom's temperature is Kelvin for raw and a relative
-100..100 for everything else — averaging the two produces a number that is
meaningless on either scale. When no matched example qualifies the field is
omitted entirely, and either way the reason is in `warning`. Examples saved
before `is_raw` was recorded count as compatible with anything. Every other
develop setting means the same on both kinds of file and is blended normally;
`tint` differs only in range, which the clamp handles.

Only examples embedded with the current search model are compared with the photo
(`embedding_model` in the example's metadata, written by `POST /v1/edit/training`); a vector
from another model means nothing next to this one. Examples saved before the identity was
recorded count as outdated. When *every* stored example is outdated, the endpoint says so
(`error` on the 422, or `style_engine_note` when the LLM fallback runs) instead of reporting
that there is nothing stored, and `/v1/edit/recipe` skips style matching with the same
explanation in `warning`. Saving an example again replaces it.

The `/v1/edit/recipe` few-shot path applies the same rule: an example whose raw status
conflicts with the target has its white balance stripped before it reaches the
prompt, so the model is never anchored on a number the schema it must answer in
cannot express.

---

## Face Detection & Persons

Two metadata fields on a face row carry a person, and the difference between
them decides what a re-cluster is allowed to touch:

- `person_id` — who the face belongs to. Empty or `person_unassigned` means
  nobody.
- `person_manual` — true when a human said so on the People page. A guess the
  clusterer may revise versus a fact it must preserve. Every editing endpoint
  below sets it, `POST /v1/faces/cluster` honours it, and re-detection during
  indexing carries it over.

### `POST /v1/faces/detect`
Detects faces in an uploaded image and stores their embeddings.

### `POST /v1/faces/search`
Finds photos containing faces similar to those in a reference image.

### `POST /v1/faces/cluster`
Re-clusters all stored face embeddings into person groups. The policy lives in
`lrg_analysis::face_cluster`; this endpoint is the store side of it.

Request fields (all optional): `distance_threshold` (cosine, default 0.5),
`min_faces_per_person` (default 2, `null` keeps singletons),
`linkage` (`"complete"` default, or `"average"`), `min_det_score`,
`min_area_ratio`, `min_sharpness`, `max_occlusion`, `attach_factor`,
`ignore_manual`, and `algorithm`.

What the default path does, and why:

- **Quality gate first.** A blurred, tiny, or half-occluded face is the one
  that sits between two people and welds them together. Faces below the gate
  may not *form* a person; after clustering they are attached to the nearest
  finished cluster whose centroid is within `distance_threshold × attach_factor`
  (0.9), or left unassigned. A face row indexed before this version measured
  quality has no scores at all — an unmeasured field never rejects, and the
  response warns that those rows were passed through ungated.
- **Complete linkage, not DBSCAN.** DBSCAN's density connectivity is
  transitive, so a single borderline face chains two people into one cluster.
  Complete linkage requires *every* pair across two clusters to be within the
  threshold. Passing `"algorithm": "dbscan"` restores the old behaviour and
  always returns a warning saying it chains.
- **Manual assignments are pinned.** Faces with `person_manual` enter as
  pre-formed seeds that keep their `person_id`, cannot be split, and cannot
  merge with a different pinned person. `"ignore_manual": true` opts out.
- **Canopy pre-partition above `AGGLOMERATIVE_MAX_POINTS`.** The agglomerative
  n×n distance matrix would be gigabytes, so a greedy centroid pass splits the
  faces into canopies at a loose threshold and each is clustered separately.
  This is reported in `diagnostics.canopies` and warned about, because a canopy
  boundary can split one person.

Faces whose row carries no usable embedding, or whose embedding came from a
superseded face model, are reported as unassigned rather than clustered — an
empty vector compares as distance 0 to everything and would merge unrelated
clusters.

The response carries `person_count`, `warnings`, and a `diagnostics` object —
`threshold`, `algorithm`, `clusters`, `pinned_faces`, `low_quality`,
`unmeasured`, `attached`, `dropped_small`, `canopies`, `mean_spread` and
`near_duplicates` — how many clusters have another cluster within 1.2× the
threshold, i.e. how many a slightly looser run would have merged. The last two
are what makes the threshold judgeable: a high `near_duplicates` means one
person is probably split across several clusters, and a `mean_spread`
approaching the threshold means clusters are being stretched over more than one
face.

### `POST /v1/faces/assign`
Moves specific faces to a person. Body: `face_ids` (required), plus either
`person_id`, or `"new_person": true` to allocate the next free id, optionally
with a `name`. Assigning to `person_unassigned` detaches the faces from
everyone. Every touched row gets `person_manual: true`, so the next
**Cluster faces** run preserves the decision. Face ids that no longer exist are
returned as warnings rather than failing the request.

### `GET /v1/faces/persons`
Returns all detected persons with `person_id`, `name`, `face_count`,
`photo_count`, `manual_count` (how many of the faces a human placed) and a
representative `thumbnail` (base64 JPEG, empty when the person has none). The
representative is the highest-scoring face by `det_score + sharpness`, not the
first row found.

Every face without a person — an empty `person_id` from a row that was never
clustered, or `person_unassigned` written by the clusterer — is reported as a
single entry with an empty `person_id`. The thumbnail is included here on
purpose: fetching it per person from `/v1/faces/persons/<id>/thumbnail` meant one
full table scan per person.

### `POST /v1/faces/persons/merge`
Merges two or more persons into one. Body: `person_ids` (at least two distinct
real persons — the unassigned pseudo-person is not one), optional `into` (the
survivor; defaults to whichever has the most faces) and optional `name`.

Every face of every person involved — including the target's own — is rewritten
to the surviving id with `person_manual: true`, so the merge holds through the
next re-cluster. The name is resolved in order: an explicit `name`, else the
target's existing name, else any name from a merged-away person. The merged-away
ids are removed from the names file.

### `GET /v1/faces/persons/<person_id>/thumbnail`
Returns the representative face thumbnail for a person as base64 JPEG. Kept for
older plugin builds; `GET /v1/faces/persons` already includes it, and this scans
the whole face table for one image, so it must not be called in a loop.

### `PUT /v1/faces/persons/<person_id>`
Names a person cluster, and by naming it confirms it. Body: `name` (empty or
absent clears it).

Every face of that person is marked `person_manual: true`, exactly as a merge
does, and the count comes back as `confirmed`. Naming is the user's claim that
the cluster really is one person, so the next **Cluster faces** run has to keep
it together — without the pin the name could end up on a different set of faces.

Clearing the name pins nothing (`confirmed: 0`) and unpins nothing: the same
flag also records hand-assignments and merges, and no row says which of those
wrote it, so undoing the naming would throw away decisions it never made.
Naming the unassigned pseudo-person pins nothing either. If the name saves but
the pinning fails, the request still succeeds and says so in `warnings`.

The People page uses this endpoint's name collision — a name another person
already has — as its merge gesture: it asks, then either posts to
`/v1/faces/persons/merge` or sends the name anyway when the user says the two
really are different people with the same name.

### `GET /v1/faces/persons/<person_id>/photos`
Returns the list of `photo_id` values associated with a specific person.

### `GET /v1/faces/persons/<person_id>/faces`
Returns the individual faces in one person, which is what the People page's
detail view is built on. Query parameters: `limit` (default 200, max 500),
`offset`, and `sort` — `typical` (closest to the cluster centroid first),
`outlier` (furthest first, which surfaces wrong faces) or `quality`.

Only this person's vectors are fetched, so the endpoint is cheap enough to page
through. Each face carries `face_id`, `photo_id`, `thumbnail`, `distance` from
the cluster centroid, `manual`, the four quality measurements
(`det_score`, `area_ratio`, `sharpness`, `occlusion`) and a `quality_note` when
one of them is below the gate. The envelope adds `total`, `offset`, `limit`,
`returned`, `name` and `spread` (mean distance to the centroid).

`distance` is what lets a threshold be judged by eye rather than guessed: a
face further from the centroid than the clustering threshold is drawn as an
outlier on the page.

---

## Browser UI (`/v1/ui/`)

Pages the plugin opens in the browser instead of rebuilding them out of
Lightroom's view factory, plus the queue they talk back through. The pages are
compiled into the binary from `crates/lrg-api/src/ui/`, so they can never go
stale against the server serving them.

A page can call the rest of this API directly — it is served from the
backend's own origin, so no CORS setup is involved — and it forwards the
`db_path` the plugin put in its URL on every request, which the auto-bind
middleware honours exactly like the plugin's own calls.

What a page *cannot* do is touch the Lightroom catalog. Those actions are
queued instead: the page POSTs one, and the plugin task that is waiting on it
drains the queue and performs it. Both ends stamp when they were last heard
from, so each can tell the user the other one is gone instead of appearing to
hang.

### `GET /v1/ui/people`
Serves the People page: person grid, renaming, re-clustering, filtering, and
the selection that becomes a Lightroom collection. Replaced the plugin's LrView
People dialog.

### `GET /v1/ui/status`
Page heartbeat. Returns `plugin_connected` — false once the plugin has not
polled for 5 seconds, which is how the page reports that People was closed in
Lightroom instead of silently queueing work nobody will run.

### `GET /v1/ui/actions`
Plugin poll. Returns every queued action and clears the queue, so each action
is handed out exactly once — a caller that drops the response drops the work.
Also returns `page_open`, false when no page has been seen for 90 seconds —
wide enough that a page throttled in a hidden browser tab still counts —
which lets the plugin report a browser that never opened.

### `POST /v1/ui/actions`
Queues one action for the plugin. The body needs an `action` naming one the
backend knows (`show_in_library`, which additionally needs a non-empty
`person_ids` array); anything else is a 400 rather than an entry nothing will
ever run. Returns `plugin_connected` as it was *before* the enqueue — the
answer to "will anything pick this up".

---

## AI Agents: MCP endpoint and agent bridge

An AI agent — Claude Code, Claude Desktop, LM Studio, Cursor, any MCP client —
connects to `/mcp` and gets tools over LrGeniusAI's index (search, people,
culling) and over Lightroom itself (catalog queries, previews, metadata,
collections, develop settings, the plugin's own tests). The backend cannot
touch the catalog, so the Lightroom tools queue commands on the **agent
bridge** and the plugin carries them out. Full design, the command list and how
to test it: [Dev: Agent Bridge](Dev-Agent-Bridge). For users:
[Help: AI Agents](Help-AI-Agents).

These routes refuse anything a web page could send: a request with an `Origin`
header (a browser — Lightroom's `LrHttp` sends none) or a `Host` that is not
this computer (a DNS-rebinding page) gets a 403. All but registration and the
status read also need the session token from registration, in the
`X-LrGenius-Bridge-Token` header; an unknown token gets a 401 with
`reregister: true`, which is how the plugin notices a backend restart.

### `POST /mcp`
### `GET /mcp`
### `DELETE /mcp`
The MCP endpoint (Streamable HTTP, served by `lrg-mcp` and mounted next to
these routes rather than under `/v1` — MCP negotiates its own version). `POST`
carries JSON-RPC messages and answers as JSON or as an event stream (tool
progress arrives as `notifications/progress` on it); `GET` opens the
server-to-client stream; `DELETE` ends the session named in `Mcp-Session-Id`.
Only answers when `Host` is this computer, rejects every browser `Origin`, and
needs the agent token as `Authorization: Bearer <token>`. A missing or wrong
token gets a 403 with a JSON-RPC error saying where to copy the setup, and so
does every request while no plugin session is registered. `geniusai-server
mcp` is a stdio proxy to it for clients that only start commands (Claude
Desktop); it takes the token from `LRGENIUS_MCP_TOKEN` or `--token`.

### `POST /v1/agent/bridge`
The plugin registers. Body: `port` (its LrSocket doorbell; absent when the
socket could not be opened — the plugin then polls), `previous_token` (keeps an
existing session when only the port changed), and what it reports about itself:
`plugin_version`, `lightroom_version`, `catalog_path`, `catalog_name`,
`catalog_id`, `db_path`, `allow_changes`, `dev_tools`, `platform`, plus
`mcp_token` (the agent token it keeps; absent on its first registration),
`rotate_mcp_token` (make a new one) and `started_at` (when this bridge loop
started). A registration whose `started_at` is older than the newest one seen
since the backend started gets **409** with `superseded: true`; the plugin's
loop then stops. That ends the fight between the old and the new loop after
"Reload Plug-in". Returns `token`, `session`, `resumed`,
`ping_interval_seconds` and `mcp_token`: the agent token `/mcp` now requires,
newly made when the plugin had none, asked for a new one, or sent something
too short to be a secret. The backend then
connects to the port and writes `wake` per queued command and `ping` on that
interval. Any other registration replaces the old session; its unfinished
commands fail as "plugin reloaded".

### `GET /v1/agent/bridge`
Bridge status: `connected`, `socket` (`none`/`connecting`/`connected`/`down`),
`plugin` (as registered), `registered_at`, `last_seen_seconds`, `queued`,
`in_progress`, `commands_done`. Never includes the token.

### `DELETE /v1/agent/bridge`
The plugin stops on purpose (the user cancelled the bridge, or Lightroom is
quitting). Pending commands fail with "the user stopped agent control" so a
waiting agent stops instead of retrying.

### `GET /v1/agent/commands`
The plugin claims every queued command, oldest first: `commands` is a list of
`{id, action, args, timeout_seconds}`. Claiming also counts as the plugin being
alive while its socket is down.

### `POST /v1/agent/commands/<id>/result`
The plugin's answer: `ok`, `result`, `error`, `warnings` (always passed on to
the agent, also on success), and `refused` (`"changes"` or `"dev_tools"` when
the plugin's settings forbid the command). Returns `accepted: false` when the
caller already gave up (timeout or cancellation).

### `POST /v1/agent/commands/<id>/progress`
`{done, total, message}` while a long command runs; forwarded to the MCP client
as progress notifications. The answer's `cancel` is true once nobody waits for
the command any more, and the plugin stops between photos.

---

## Agent chat

The in-app chat: a local or cloud AI model works the catalog through the same tools as
`/mcp`, and the user chats with it — and approves its changes — in the
browser. Design, the permission model and the event list:
[Dev: Agent Chat](Dev-Agent-Chat). Served by `lrg-chat`, merged next to `/mcp`
and written as served (not nested by `build_router`).

Opening a chat takes the agent token as `Authorization: Bearer <token>`, like
`/mcp`. Every other route takes the chat's own token the same way, and refuses
a request whose `Origin` is not this server even with the right token. A wrong
token, a foreign origin and a chat that has ended all get the same **403**.
Errors are `{"error": "<what to do>"}`.

### `GET /v1/ui/chat`
The chat page (`lrg-chat/src/ui/chat.html`), no token needed: it is the same
for everyone and holds nothing until it is given a chat. It reads `session` and
`token` from its fragment, keeps them in `sessionStorage` so a reload works,
and removes them from the address bar. Sent with a strict
`Content-Security-Policy` (`default-src 'none'`, `connect-src 'self'`,
`frame-ancestors 'none'`), `Cache-Control: no-store` and
`Referrer-Policy: no-referrer`. A new chat's address opened in the same tab
changes only the fragment; the page reloads itself for it.

### `POST /v1/chat/sessions`
Opens a chat. Body: `provider` and `model` as for indexing, and what that
provider needs:

* `llamacpp`, `mlx`: `llm_n_ctx`, `llm_n_parallel`, `llm_gpu_layers` as on the
  indexing routes. The model loads in the background; one that cannot load is
  reported as an `error` event.
* `chatgpt`, `anthropic`, `gemini`: `api_key`, required.
* `openai_compatible`: `server_url`, required, and `api_key` if the server
  wants one. `lmstudio`, `ollama`: optional `lmstudio_base_url`,
  `ollama_base_url`.

The key is kept in the chat's memory only — never logged, saved or returned.
A missing key, address or model is a **400** naming what to set. Returns
`session_id`, `token` and
`page`, the path to open with the token in its fragment
(`/v1/ui/chat#session=…&token=…`). **503** when 8 chats are open and none is
idle enough to close. No plugin session registered: 403 saying to turn the
agent bridge on.

### `GET /v1/chat/sessions/{id}`
### `DELETE /v1/chat/sessions/{id}`
`GET` returns what the page draws from: `model`, `busy`, `tools` (each with
`name`, `title`, `class` — `read`/`change`/`destructive` — and its `rule`),
`permissions`, `permissions_saved` (false while no index is bound, so a change
would not survive a restart), `sends_to` (who receives the conversation when
that is not this computer — "Anthropic", "your AI server (host:port)" — or
null) and `lightroom` (`catalog_name`,
`may_change_photos`; null when the plugin is not connected). `DELETE` stops the
chat and forgets it (**204**). A chat nobody has listened to for two hours is
closed by the backend.

### `GET /v1/chat/sessions/{id}/events`
Server-sent events, one JSON object per `data:` line, with a `type` and — for
everything except the streamed `delta` pieces — a `seq`. `?after=<seq>`
replays what was logged after it, then continues live. A page that falls too
far behind gets `event: resync` and the stream ends; reconnecting with `after`
catches up. The event types are listed in [Dev: Agent Chat](Dev-Agent-Chat#events).

### `POST /v1/chat/sessions/{id}/messages`
`{"text": "…"}`. **202** and the work starts; its progress arrives on the event
stream. **409** while the previous message is still being worked on, **400**
for an empty one.

### `POST /v1/chat/sessions/{id}/approvals/{call_id}`
The answer to an `approval_needed` event: `{"decision": "once"}`, `"chat"`
(this tool for the rest of the chat), `"always"` (sets the tool's rule to
`allow`), or `{"decision": "deny", "note": "…"}` — the note is passed to the
model. **204**; **409** when that call is not waiting any more.

### `POST /v1/chat/sessions/{id}/stop`
Stops after the step in progress: the model's turn is cancelled, calls waiting
for approval are denied, nothing new starts. A tool already running in
Lightroom finishes and its result is recorded. The event stream gets a
`stopped` event, and the model is told in the history that the user stopped
there. **204**.

### `GET /v1/chat/sessions/{id}/permissions`
### `PUT /v1/chat/sessions/{id}/permissions`
What runs without asking: `mode` (`ask_all`, `auto_read` — the default —,
`auto_safe`, `auto_all`), `tools` (tool name → `allow`, `ask` or `block`) and
`bulk_limit` (a change to photos aimed at more than this many asks even when
allowed; 0 turns it off). The settings are the user's, not the chat's: every
chat uses them, and they are saved as `chat_permissions.json` beside the index.
`PUT` takes the whole object and returns `{saved, permissions}`.

---

## Metadata Import

### `POST /v1/photos/metadata/import`
Imports existing Lightroom catalog metadata (title, caption, alt text and keywords) into the backend database for a batch of photos.

`metadata_items[].keywords` is the photo's keyword hierarchy
(`MetadataManager.getPhotoKeywordHierarchy`) without the plugin's top-level
container keyword ("LrGeniusAI", "ChatGPT", ...). Analysis results have no such
root either, and "Retrieve Metadata" writes a stored hierarchy back underneath
the container, so a root left in would come back nested inside itself.

---

## Keyword Management

### `POST /v1/keywords/clusters`
Synchronous keyword clustering: groups catalog keywords by semantic similarity.

### `POST /v1/keywords/clusters/jobs`
Async version: starts a background clustering job. Returns `202` with a
`job_id` and the `poll_url` to read it back from — see `GET /v1/jobs/<job_id>`.

## Async Jobs

Long-running work is enqueued by the domain that knows what to start, and read
back from one place. Every enqueue response carries a `job_id` and a `poll_url`
pointing here, so the poll location is never derived from the enqueue path.

### `GET /v1/jobs/<job_id>`
Returns `{status, result, error, progress}`, where `status` is `running`,
`done` or `error`.

A finished job is returned **once** and then dropped from the registry, and
jobs expire after 600s without activity (polling or reporting progress counts
as activity). A `404` therefore means the job never existed, was already
collected, or expired — not necessarily that the id was wrong.

### `POST /v1/keywords/merges`
Applies a list of approved keyword merge pairs to the backend's metadata records.

---

## Style Training

### `POST /v1/edit/training`
Stores one photo's develop settings as a training example. `is_raw` is recorded
with it and decides, later, whether the example may contribute a temperature —
see `POST /v1/edit/style`. The SigLIP2 vector is stored together with the
`embedding_model` that made it, which decides whether the example can still be
matched after the search model changes.

Saves a photo's Lightroom develop settings as a labeled training example.

### `GET /v1/edit/training`
Lists all stored training examples.

### `GET /v1/edit/training/stats`
Returns aggregated statistics about stored training examples (count per label, coverage).

### `GET /v1/edit/training/count`
Returns the total number of stored training examples.

### `DELETE /v1/edit/training/<photo_id>`
Removes the training example for a specific photo.

### `DELETE /v1/edit/training`
Clears all training examples.

---

## CLIP Model Management

### `GET /v1/models/clip`
Returns whether the SigLIP2 embedding model is downloaded and ready.

### `POST /v1/models/clip/downloads`
Triggers a background download of the CLIP model.

### `GET /v1/models/clip/downloads`
Returns the current progress of an ongoing CLIP model download.

---

## All Model Assets (combined)

The per-family routes below still exist and are what the detail view uses, but
the plugin's setup flows drive these instead: three downloads, three progress
bars and three ready indicators ask a photographer to care about which neural
network does which job.

### `GET /v1/models/assets`
Per-family readiness plus one overall `ready` flag and `missing_approx_bytes`
for the "this will download about N GB" line. All four families — `clip`, `aesthetic`
(the photo quality scorer), `bioclip` and `face` — are `downloadable: true` and all four gate `ready`.
Face detection used to be the exception: its `buffalo_l` weights came from
`INSIGHTFACE_ROOT`, were not redistributable, and had to be excluded from
`ready` so the button did not stay red forever over something the download
could not fix. Replacing them with YuNet + FaceNet removed that carve-out;
`downloadable` remains in the response because the plugin reads it.

### `POST /v1/models/assets/downloads`
Downloads every downloadable family that is **missing**, under one progress
entry keyed `assets`. Families already on disk are skipped, so this doubles as
"finish setting up" after an upgrade. With nothing missing it reports
`completed` rather than idling.

### `GET /v1/models/assets/downloads`
Same shape as the other download-status routes.

---

## Species Model Management (BioCLIP 2)

BioCLIP 2 identifies animals, plants and fungi down to species. It ships as
three assets — the ViT-L/14 image tower as fp16 ONNX, a pruned Tree-of-Life
zero-shot head (`bioclip2_taxa.bin`) and its interned labels
(`bioclip2_taxa.json`) — from their **own** release tag
(`bioclip-assets-v1`), independent of the SigLIP2 `model-assets-v2` tag.
Roughly 876 MB in total.

The head is pruned from upstream's 867,455 taxa; see
`server-rs/scripts/bioclip_taxa_filter.toml` for the rules and for what
pruning costs at the species rank.

### `GET /v1/models/bioclip`
Returns whether the BioCLIP assets are on disk (`bioclip: "ready" | "not_ready"`),
plus `model` — the head identifier, e.g. `bioclip-2/taxa-v2`, read from the
labels file when the head is not loaded, so it answers without pulling 866 MB
into memory. Deliberately distinct from `/v1/server/health`'s `species_model`, which
reports whether it is currently resident in memory.

### `POST /v1/models/bioclip/downloads`
Triggers a background download of the BioCLIP assets.

### `GET /v1/models/bioclip/downloads`
Returns the current progress of an ongoing BioCLIP download. Same shape as the
CLIP and LLM download status; all three share one progress map keyed by asset
group, so a species download and a GGUF download can be in flight at once.

---

## Species Links

The pruned Tree-of-Life head carries taxon *names* only, no identifiers, so a
link from an identification to a species database can either be a name-based
search or a deep link to the taxon's own page. These routes buy the deep link:
they resolve a name against GBIF's `/species/match` and iNaturalist's
`/v1/taxa` — both free and unauthenticated — and cache the answer in
`~/.cache/lrgenius/species_links.json`, so a given taxon costs one outbound
call per machine, ever.

Everything degrades to search URLs: an unreachable network, an unknown name,
or a fuzzy iNaturalist hit that does not match the queried name exactly all
return the same shape with `resolved: false`. Deliberately kept off the
indexing path — indexing must not need the network, and a catalog's taxa
repeat so heavily that resolving on demand costs a handful of calls for a whole
library. Outbound calls are spaced ~1.1 s apart to stay inside iNaturalist's
published rate limit.

### `GET /v1/species/links`
Query: `name` (scientific name, required), `rank` (`species`, `genus`, … —
narrows both lookups so a genus query cannot match a same-named species),
`lang` (two-letter; picks the Wikipedia edition and the iNaturalist
common-name locale, default `en`).

```json
{
  "status": "success",
  "links": {
    "name": "Panthera leo",
    "gbif_key": 5219404,
    "inat_id": 41964,
    "common_name": "Lion",
    "resolved": true,
    "urls": {
      "gbif":        "https://www.gbif.org/species/5219404",
      "inaturalist": "https://www.inaturalist.org/taxa/41964",
      "wikipedia":   "https://en.wikipedia.org/wiki/Lion",
      "wikispecies": "https://species.wikimedia.org/wiki/Panthera%20leo",
      "eol":         "https://eol.org/search?q=Panthera+leo",
      "col":         "https://www.catalogueoflife.org/data/search?q=Panthera+leo"
    }
  }
}
```

`urls` always holds every provider; `resolved` says whether any of them is a
deep link rather than a search. `wikispecies` deep-links without a lookup —
Wikispecies files scientific names as article titles verbatim. `wikipedia`
comes from iNaturalist and is therefore only a deep link for `lang=en`; other
languages get a scientific-name search on their own edition, which resolves
through the redirect each edition keeps.

The plugin calls this from `MetadataManager.applySpecies` and writes
`urls.inaturalist` / `urls.wikipedia` into the read-only `speciesInatUrl` /
`speciesWikipediaUrl` catalog fields, which Lightroom renders as clickable
links in the Metadata panel.

### `POST /v1/species/links/batch`
Same resolution for several taxa in one request. Resolved sequentially — the
rate gate would serialize concurrent calls anyway.

```json
{ "names": [{ "name": "Panthera leo", "rank": "species" }], "lang": "en" }
```

Returns `{ "status": "success", "links": [ … ] }`, one entry per requested
name, in request order.

---

## Local LLM Management (llama.cpp & MLX)

The backend hosts two local inference engines: `llamacpp` (in-process llama.cpp, GGUF models, present only in builds compiled with the `llamacpp` cargo feature) and `mlx` (an `lrgenius-mlx` helper process, Apple silicon only, no cargo feature). Both are exposed through the same routes — the MLX half is nested under an `mlx` key so that older plugins reading the top-level fields still see the llama.cpp engine.

### `GET /v1/llm/catalog`
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

### `GET /v1/llm/status`
Engine state (`status`, loaded `model_name`, …) for the llama.cpp engine, with the MLX engine's equivalent nested under `mlx`. `n_ctx` is the whole KV cache; `n_ctx_seq` is the share one photo gets, which is what a prompt plus its `max_tokens` has to fit inside. They differ whenever `n_parallel > 1`, because llama.cpp splits the cache per sequence. `n_parallel` is the effective value: the engine lowers a requested count that would leave each photo too little.

### `POST /v1/llm/downloads`
Starts a background download of a catalog entry. Body: `{ "id": "gemma4-e4b" }`. The id space is shared across both catalogs and the id alone selects the backend, so there is a single download queue: a GGUF pair is fetched as two files, an MLX entry as a repo snapshot staged into a hidden `.<name>.part` directory (which discovery skips) and renamed on success.

Or a model from Hugging Face that is not in a catalog: `{ "repo": "mlx-community/gemma-3-12b-it-qat-4bit", "engine": "mlx", "revision": "<sha from the check>" }`. The repo is checked again exactly as by `/v1/llm/downloads/check`, at that revision, and refused with the same 400/502 if it does not pass — so what is fetched is what was checked. A GGUF goes into a folder of its own under the llama.cpp model directory; an MLX model gets a `.lrgenius-source.json` naming its repo. When the model is already installed, the download completes at once with its `installed_name`.

### `POST /v1/llm/downloads/check`
Checks a Hugging Face model before anything is downloaded ("Other model from Hugging Face…" in the plugin). Body: `{ "repo": "<org/name, org/name:QUANT, or a huggingface.co address>", "engine": "mlx" | "llamacpp" }`.

- **400** `{ "error": … }` — the model cannot be used, with the reason and what to do instead: not a model name, no such public repo, gated, text-only, not an MLX conversion (or not a GGUF), an MLX `model_type` or `processor_class` the pinned mlx-swift-lm does not register (`hf_repo::MLX_VISION_MODEL_TYPES` / `MLX_VISION_PROCESSORS`), no tokenizer or chat template, no GGUF vision projector, a split GGUF, an unknown quantization. Also when the engine is not available on this machine.
- **502** `{ "error": … }` — Hugging Face could not be reached.
- **200** — what a download would fetch:

```json
{
  "engine": "mlx", "repo": "mlx-community/gemma-3-12b-it-qat-4bit",
  "revision": "<commit sha>", "dir_name": "gemma-3-12b-it-qat-4bit",
  "installed_name": "gemma-3-12b-it-qat-4bit",
  "files": [{ "path": "config.json", "size": 5000 }, "…"],
  "approx_bytes": 8031095500, "est_ram_gb": 11.1,
  "model_type": "gemma3", "quant": null,
  "already_installed": false,
  "warnings": ["This model is not in LrGeniusAI's tested list. …"]
}
```

For `llamacpp`, `files` is the model file then its projector (`Q4_K_M` and an `f16` projector unless a quantization follows a colon), `quant` is set and `model_type` is null. `installed_name` is the `model` to send once it is downloaded. The Hugging Face address can be overridden with `HF_ENDPOINT`.

Only one local model downloads at a time. While one is running, a second request gets **409** with `{ "error": "Another model download is still running. …" }` — it is refused, not queued.

### `GET /v1/llm/downloads`
Progress of the current local-model download (same shape as the CLIP download status). Once `status` is `"completed"`, `installed_name` is the name the model is now offered under — the `model` to send with provider `llamacpp` or `mlx`.

---

## Database Operations

### `GET /v1/db/stats`
Returns aggregate database statistics:
- total indexed photos
- photos with SigLIP embeddings (`with_embedding`), and how many of those were made by another
  search model than this build's (`with_outdated_embedding`; a search cannot find them until they
  are indexed again)
- photos with title / caption / keywords
- total detected faces
- total persons

### `POST /v1/db/backups`
Creates and streams a ZIP backup of the full persistent LanceDB data directory.

> **Not available:** `POST /db/migrate-photo-ids` existed on the retired Python backend and
> converted legacy Lightroom UUID-based IDs to `photo_id` values. It was deliberately not
> carried over (see the module comment in `routes/db.rs`), and the plugin no longer offers
> it: a database from before file-based `photo_id` values has to be re-indexed.
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
