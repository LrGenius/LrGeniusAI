//! `/index` — port of `routes/index.py::index_images_batch` +
//! `services/index.py::process_image_task`'s core loop: embeddings
//! (SigLIP2), pHash + culling metrics (always computed), faces (when
//! requested), catalog association, existing-record merge for
//! `regenerate_metadata=false`, LLM metadata generation across all four
//! providers (Ollama, OpenAI, Gemini, LM Studio), and optional Vertex AI
//! embeddings (silently skipped, not an error, when no project is
//! configured — matching Python's behavior exactly).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Multipart, State};
use axum::response::{IntoResponse, Json, Response};
use chrono::Utc;
use serde_json::{json, Map, Value};

use lrg_analysis::face_aggregate::{aggregate_face_culling_metrics, FaceMetricsInput};
use lrg_imaging::convert::{normalize_image_bytes, UnsupportedImageError};
use lrg_imaging::cull_config::{FaceMetricsConfig, ImageMetricsConfig};
use lrg_imaging::metrics::{culling_metrics, perceptual_hash, RgbImage};
use lrg_ml::faces::FacePass;
use lrg_providers::provider::{build_provider, ProviderSelection};
use lrg_providers::types::{KeywordCategories, KeywordTree};
use lrg_store::{meta, StoreRecord, FACE_TABLE, IMAGE_TABLE, VERTEX_TABLE};

use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route("/index", axum::routing::post(index_batch))
}

pub(crate) struct ParsedOptions {
    compute_embeddings: bool,
    compute_faces: bool,
    /// How much of the face pipeline to run. The `cull` task only reads the
    /// quality proxies, so it skips ArcFace and the thumbnail encode.
    face_pass: FacePass,
    compute_metadata: bool,
    compute_vertexai: bool,
    regenerate_metadata: bool,
    replace_ss: bool,
    catalog_id: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    capture_time: Option<f64>,
    vertex_project_id: Option<String>,
    vertex_location: Option<String>,
    /// How many photos to hand the provider per LLM call. `None` means "ask
    /// the provider", which is what callers should normally do — the useful
    /// width is a property of the loaded engine, not of the request.
    llm_batch_size: Option<usize>,
    /// Engine tuning from the plugin's advanced fields. Changing any of these
    /// makes the next request reload the engine, so they are user settings
    /// rather than per-photo options.
    engine: crate::llm_engine::EngineOverrides,
    metadata_request: MetadataOptions,
}

/// Per-photo values that must not be shared across a grouped request.
///
/// `/index_by_reference` can carry several photos in one call, but the option
/// fields arrive flat, one set for the whole request. These four are exactly
/// the ones `prompts.rs` classifies as volatile — reusing one photo's capture
/// time, keywords or folders for the whole group would feed the model context
/// belonging to a different photo. Absent entries fall back to the
/// batch-level options, which is what every single-photo request does.
#[derive(Default, Clone)]
pub(crate) struct PhotoOverrides {
    pub capture_time: Option<f64>,
    pub date_time: Option<String>,
    pub existing_keywords: Option<Vec<String>>,
    pub folder_names: Option<String>,
}

/// The metadata-generation-specific subset of `_extract_options`, kept
/// separate since it's only consulted when `compute_metadata` is set.
struct MetadataOptions {
    language: String,
    temperature: f64,
    max_tokens: Option<u32>,
    generate_keywords: bool,
    generate_caption: bool,
    generate_title: bool,
    generate_alt_text: bool,
    submit_keywords: bool,
    submit_folder_names: bool,
    existing_keywords: Option<Vec<String>>,
    folder_names: Option<String>,
    user_context: Option<String>,
    /// Custom system prompt from the plugin's "Instructions / Prompt" field
    /// (multipart/JSON field `prompt`). `None` falls back to
    /// `DEFAULT_SYSTEM_PROMPT`.
    system_prompt: Option<String>,
    date_time: Option<String>,
    keyword_categories: Option<KeywordCategories>,
    bilingual_keywords: bool,
    keyword_secondary_language: Option<String>,
    generate_aliases: bool,
    catalog_keywords: Option<Vec<String>>,
    ollama_base_url: Option<String>,
    lmstudio_base_url: Option<String>,
}

/// Parses the `keyword_categories` multipart field (JSON, sent by the plugin
/// as either a flat array of category names or a nested object mapping
/// category -> subcategories) into `KeywordCategories`. Absent, unparsable,
/// or empty input yields `None` (matching "no hierarchy configured").
fn parse_keyword_categories(raw: &str) -> Option<KeywordCategories> {
    let value: Value = serde_json::from_str(raw).ok()?;
    value_to_keyword_categories(&value)
}

fn value_to_keyword_categories(value: &Value) -> Option<KeywordCategories> {
    match value {
        Value::Array(items) => {
            let list: Vec<String> = items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if list.is_empty() {
                None
            } else {
                Some(KeywordCategories::Flat(list))
            }
        }
        Value::Object(map) => {
            if map.is_empty() {
                None
            } else {
                Some(KeywordCategories::Nested(value_to_keyword_tree(value)))
            }
        }
        _ => None,
    }
}

fn value_to_keyword_tree(value: &Value) -> KeywordTree {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| (k.clone(), value_to_keyword_tree(v)))
            .collect(),
        _ => KeywordTree::default(),
    }
}

fn parse_string_list(raw: &str) -> Option<Vec<String>> {
    serde_json::from_str::<Vec<String>>(raw).ok().or_else(|| {
        let list: Vec<String> = raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if list.is_empty() {
            None
        } else {
            Some(list)
        }
    })
}

fn bool_field(fields: &HashMap<String, String>, key: &str, default: bool) -> bool {
    fields
        .get(key)
        .map(|v| v.to_lowercase() == "true")
        .unwrap_or(default)
}

pub(crate) fn parse_options(fields: &HashMap<String, String>) -> ParsedOptions {
    let tasks: Vec<String> = match fields.get("tasks") {
        Some(s) if !s.is_empty() => {
            if s.starts_with('[') {
                serde_json::from_str::<Vec<String>>(s)
                    .unwrap_or_else(|_| s.split(',').map(|t| t.trim().to_string()).collect())
            } else {
                s.split(',').map(|t| t.trim().to_string()).collect()
            }
        }
        _ => vec!["embeddings".to_string()],
    };
    let has_task = |t: &str| tasks.iter().any(|x| x == t);

    // `cull` is the fast ingest path: everything culling actually reads
    // (pHash, image metrics, face quality) and nothing it does not. It
    // deliberately does *not* imply `embeddings` or `metadata` — the SigLIP2
    // embedding is ~316-480ms/photo and the LLM is seconds, while culling uses
    // the embedding only as a secondary near-duplicate signal that pHash plus
    // capture time already covers for bursts. A later full Analyze & Index run
    // backfills embeddings without redoing any of this.
    let cull_pass = has_task("cull");

    let reg_val = fields
        .get("regenerate_metadata")
        .or_else(|| fields.get("regenerateMetadata"));
    let regenerate_metadata = reg_val.map(|v| v.to_lowercase() == "true").unwrap_or(true);

    let catalog_id = fields
        .get("catalog_id")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // Absent, unparseable or zero all mean "let the provider decide", so a
    // stray `llm_batch_size=0` cannot stall a run.
    let llm_batch_size = fields
        .get("llm_batch_size")
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|n| *n > 0);

    let capture_time = fields
        .get("date_time_unix")
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| {
            fields.get("date_time").and_then(|s| {
                let normalized = if let Some(stripped) = s.strip_suffix('Z') {
                    format!("{stripped}+00:00")
                } else {
                    s.clone()
                };
                chrono::DateTime::parse_from_rfc3339(&normalized)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc).timestamp() as f64)
            })
        });

    let existing_keywords = fields.get("existing_keywords").map(|raw| {
        serde_json::from_str::<Vec<String>>(raw).unwrap_or_else(|_| {
            raw.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
    });

    ParsedOptions {
        compute_embeddings: has_task("embeddings"),
        compute_faces: has_task("faces") || cull_pass,
        face_pass: if has_task("faces") {
            FacePass::Full
        } else {
            FacePass::QualityOnly
        },
        compute_metadata: has_task("metadata"),
        compute_vertexai: has_task("vertexai"),
        regenerate_metadata,
        replace_ss: bool_field(fields, "replace_ss", false),
        catalog_id,
        provider: fields.get("provider").cloned(),
        model: fields.get("model").cloned(),
        api_key: fields.get("api_key").cloned(),
        capture_time,
        llm_batch_size,
        engine: crate::routes::llm::engine_overrides_from_fields(fields),
        vertex_project_id: fields
            .get("vertex_project_id")
            .or_else(|| fields.get("vertexProjectId"))
            .cloned(),
        vertex_location: fields
            .get("vertex_location")
            .or_else(|| fields.get("vertexLocation"))
            .cloned(),
        metadata_request: MetadataOptions {
            language: fields
                .get("language")
                .cloned()
                .unwrap_or_else(|| "German".to_string()),
            temperature: fields
                .get("temperature")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.2),
            max_tokens: fields.get("max_tokens").and_then(|s| s.parse().ok()),
            generate_keywords: bool_field(fields, "generate_keywords", true),
            generate_caption: bool_field(fields, "generate_caption", true),
            generate_title: bool_field(fields, "generate_title", true),
            generate_alt_text: bool_field(fields, "generate_alt_text", true),
            submit_keywords: bool_field(fields, "submit_keywords", false),
            submit_folder_names: bool_field(fields, "submit_folder_names", false),
            existing_keywords,
            folder_names: fields.get("folder_names").cloned(),
            user_context: fields.get("user_context").cloned(),
            system_prompt: fields
                .get("prompt")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            date_time: fields
                .get("date_time")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            keyword_categories: fields
                .get("keyword_categories")
                .and_then(|raw| parse_keyword_categories(raw)),
            bilingual_keywords: bool_field(fields, "bilingual_keywords", false),
            keyword_secondary_language: fields
                .get("keyword_secondary_language")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            generate_aliases: bool_field(fields, "generate_aliases", false),
            catalog_keywords: fields
                .get("catalog_keywords")
                .and_then(|s| parse_string_list(s)),
            ollama_base_url: fields.get("ollama_base_url").cloned(),
            lmstudio_base_url: fields.get("lmstudio_base_url").cloned(),
        },
    }
}

pub(crate) struct UploadedImage {
    pub(crate) bytes: Vec<u8>,
    pub(crate) filename: String,
}

async fn index_batch(State(state): State<Arc<AppState>>, mut multipart: Multipart) -> Response {
    log::info!("Index request received");

    let mut images: Vec<UploadedImage> = Vec::new();
    let mut photo_ids: Vec<String> = Vec::new();
    let mut uuids_fallback: Vec<String> = Vec::new();
    let mut fields: HashMap<String, String> = HashMap::new();

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(json!({"error": format!("invalid multipart body: {e}")})),
                )
                    .into_response()
            }
        };
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "image" => {
                let filename = field.file_name().unwrap_or("photo").to_string();
                match field.bytes().await {
                    Ok(bytes) => images.push(UploadedImage {
                        bytes: bytes.to_vec(),
                        filename,
                    }),
                    Err(e) => log::warn!("Skipping unreadable image field: {e}"),
                }
            }
            "photo_id" => {
                if let Ok(text) = field.text().await {
                    photo_ids.push(text);
                }
            }
            "uuid" => {
                if let Ok(text) = field.text().await {
                    uuids_fallback.push(text);
                }
            }
            _ => {
                if let Ok(text) = field.text().await {
                    fields.insert(name, text);
                }
            }
        }
    }
    let photo_ids = if photo_ids.is_empty() {
        uuids_fallback
    } else {
        photo_ids
    };

    // Multipart uploads carry no per-image context, so every photo falls back
    // to the batch-level options exactly as before.
    process_batch(
        state,
        fields,
        images,
        photo_ids,
        Vec::new(),
        Vec::new(),
        true,
    )
    .await
}

/// Shared by `/index` (multipart) and `/index_by_reference` (JSON + on-disk
/// paths) once each has gathered its images/photo_ids/option-fields in its
/// own way — everything past that point (db_path auto-bind, validation,
/// per-photo processing, response shape) is identical.
///
/// `pre_failures` are errors the caller already knows about before this
/// point (e.g. `/index_by_reference`'s file-not-found reads) — folded in
/// exactly like an image-normalization failure so failure_count/
/// error_messages account for them the same way Python's
/// `read_failures + processing_failures` does.
///
/// `reject_empty_batch` matches Python's per-endpoint difference on a
/// genuinely empty batch: `/index` 400s ("no images provided"), while
/// `/index_by_reference` treats an all-paths-invalid batch as a normal
/// zero-success response (or a 500 quoting the read errors, if there
/// were any) rather than a client-error mismatch.
pub(crate) async fn process_batch(
    state: Arc<AppState>,
    fields: HashMap<String, String>,
    images: Vec<UploadedImage>,
    photo_ids: Vec<String>,
    overrides: Vec<PhotoOverrides>,
    pre_failures: Vec<String>,
    reject_empty_batch: bool,
) -> Response {
    let options = parse_options(&fields);

    // The generic auto-bind middleware only peeks JSON bodies and query
    // strings (a multipart body can't be cheaply peeked and replayed), so
    // multipart endpoints bind explicitly from their own parsed fields —
    // the plugin sends db_path as a form field here, same as every other
    // indexing option.
    if let Some(db_path) = fields.get("db_path") {
        if let Err(e) = state.ensure_db_path(db_path).await {
            log::error!("Auto-bind to db_path {db_path} failed: {e}");
        }
    }

    if images.len() != photo_ids.len() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "Mismatch between number of images and photo IDs, or no images provided"})),
        )
            .into_response();
    }
    if images.is_empty() {
        if reject_empty_batch {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error": "Mismatch between number of images and photo IDs, or no images provided"})),
            )
                .into_response();
        }
        if !pre_failures.is_empty() {
            let joined = pre_failures
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ");
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": joined})),
            )
                .into_response();
        }
        return Json(json!({"status": "processed", "success_count": 0, "failure_count": 0}))
            .into_response();
    }

    let max_edge = lrg_imaging::convert::default_max_long_edge();
    let quality = lrg_imaging::convert::default_jpeg_quality();

    // Short or absent override lists mean "no per-photo context", which is
    // the single-photo case and every multipart upload.
    let mut overrides = overrides;
    overrides.resize(images.len(), PhotoOverrides::default());

    // Per-photo outcomes, keyed by photo_id. A grouped request needs these:
    // aggregate counts tell the plugin that something in the group failed but
    // not which photo, and it has to know in order to run its per-photo export
    // fallback and fire `onPhotoAnalyzed` only for the ones that landed. Any
    // photo missing from this list did not get far enough to be judged; the
    // reason is in `error_messages`.
    let mut results: Vec<Value> = Vec::new();

    let mut triplets: Vec<(Vec<u8>, String, String, PhotoOverrides)> = Vec::new();
    let mut conversion_errors: Vec<String> = pre_failures;
    for ((img, photo_id), photo_overrides) in images.into_iter().zip(photo_ids).zip(overrides) {
        let t0 = Instant::now();
        let result = normalize_image_bytes(&img.bytes, Some(&img.filename), max_edge, quality);
        log::debug!(
            "Photo {photo_id} ({}): decode+resize+encode took {:?}",
            img.filename,
            t0.elapsed()
        );
        match result {
            Ok((bytes, filename)) => triplets.push((bytes, photo_id, filename, photo_overrides)),
            Err(UnsupportedImageError(msg)) => {
                log::warn!("Skipping {}: {msg}", img.filename);
                results.push(json!({
                    "photo_id": photo_id, "success": false, "error": msg.clone(),
                }));
                conversion_errors.push(msg);
            }
        }
    }

    if triplets.is_empty() {
        if !conversion_errors.is_empty() {
            let joined = conversion_errors
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ");
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": joined})),
            )
                .into_response();
        }
        return Json(json!({"status": "processed", "success_count": 0, "failure_count": 0}))
            .into_response();
    }

    let total = triplets.len();

    // Matches process_image_task's early, whole-batch failure: metadata
    // generation with no model configured can't succeed for any photo.
    if options.compute_metadata && options.model.as_deref().unwrap_or("").is_empty() {
        let msg = "AI metadata generation requires an LLM model to be configured. \
                    Disable 'AI metadata' in the indexing dialog or select a model. ";
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": msg})),
        )
            .into_response();
    }

    let mut success_count = 0usize;
    let mut failure_count = conversion_errors.len();
    let mut error_messages = conversion_errors;
    let mut warnings: Vec<String> = Vec::new();

    let store = state.store();

    // Phase 0 — decode, culling metrics and pHash for the whole batch, in
    // parallel. Pure CPU work with no model behind it, so unlike phase 1 there
    // is nothing here that wants the whole machine to itself.
    let t_cull = Instant::now();
    let image_blobs: Vec<Vec<u8>> = triplets.iter().map(|(b, _, _, _)| b.clone()).collect();
    let mut cull_signals =
        tokio::task::spawn_blocking(move || precompute_cull_signals(&image_blobs))
            .await
            .unwrap_or_else(|e| vec![Err(format!("cull metric pass panicked: {e}"))]);
    if !triplets.is_empty() {
        log::debug!(
            "Cull signals for {} photo(s) took {:?}",
            triplets.len(),
            t_cull.elapsed()
        );
    }
    // A join failure yields a single error; widen it so the zip below still
    // lines up one entry per photo.
    if cull_signals.len() != triplets.len() {
        let msg = cull_signals
            .first()
            .and_then(|r| r.as_ref().err().cloned())
            .unwrap_or_else(|| "cull metric pass produced no result".to_string());
        cull_signals = triplets.iter().map(|_| Err(msg.clone())).collect();
    }

    // Phase 1 — everything that does not need the LLM, still strictly
    // sequential (SigLIP2 and the face detector each want the whole machine).
    let mut prepared: Vec<PreparedPhoto> = Vec::with_capacity(triplets.len());
    for ((image_bytes, photo_id, filename, photo_overrides), cull) in
        triplets.into_iter().zip(cull_signals)
    {
        let Some(store_ref) = store.as_ref() else {
            failure_count += 1;
            let msg = "database not initialized (no db_path bound)";
            results.push(json!({
                "photo_id": photo_id, "success": false, "error": msg,
            }));
            error_messages.push(format!("{filename}: {msg}"));
            continue;
        };
        let cull = match cull {
            Ok(c) => c,
            Err(e) => {
                failure_count += 1;
                results.push(json!({
                    "photo_id": photo_id, "success": false, "error": e,
                }));
                error_messages.push(format!("{filename}: {e}"));
                continue;
            }
        };
        match prepare_one(
            &state,
            store_ref,
            &options,
            &photo_overrides,
            &image_bytes,
            &photo_id,
            &filename,
            cull,
        )
        .await
        {
            Ok(photo) => prepared.push(photo),
            Err(e) => {
                failure_count += 1;
                results.push(json!({
                    "photo_id": photo_id, "success": false, "error": e,
                }));
                error_messages.push(format!("{filename}: {e}"));
            }
        }
    }

    // Phase 2 — the LLM, in groups. One provider for the whole batch instead
    // of one per photo. `preferred_batch_size` is 1 for every HTTP provider,
    // so they still issue exactly one request per photo; only the in-process
    // backend asks for wider groups, where the group is what shares the
    // pinned prefix and decodes in parallel sequences.
    let mut llm_responses: Vec<Option<lrg_providers::types::MetadataGenerationResponse>> =
        (0..prepared.len()).map(|_| None).collect();
    let llm_indices: Vec<usize> = prepared
        .iter()
        .enumerate()
        .filter(|(_, p)| p.llm_request.is_some())
        .map(|(i, _)| i)
        .collect();

    if !llm_indices.is_empty() {
        match provider_for_batch(&state, &options).await {
            Ok(provider) => {
                let batch_size = options
                    .llm_batch_size
                    .unwrap_or_else(|| provider.preferred_batch_size())
                    .max(1);
                for group in llm_indices.chunks(batch_size) {
                    let requests: Vec<_> = group
                        .iter()
                        .filter_map(|&i| prepared[i].llm_request.clone())
                        .collect();
                    let t0 = Instant::now();
                    let responses = provider.generate_metadata_batch(&requests).await;
                    log::debug!(
                        "LLM batch of {} photo(s) via {} took {:?}",
                        group.len(),
                        provider.name(),
                        t0.elapsed()
                    );
                    for (&i, response) in group.iter().zip(responses) {
                        llm_responses[i] = Some(response);
                    }
                }
            }
            Err(e) => {
                // Resolving the provider is a whole-run problem, but it can
                // only sink the photos that actually wanted one.
                for &i in &llm_indices {
                    llm_responses[i] = Some(lrg_providers::types::MetadataGenerationResponse {
                        uuid: prepared[i].photo_id.clone(),
                        success: false,
                        error: Some(e.clone()),
                        ..Default::default()
                    });
                }
            }
        }
    }

    // Phase 3 — merge, faces, and the coalesced write. `prepared` is only
    // non-empty when the store bound, so the `if let` never skips work.
    if let Some(store_ref) = store.as_ref() {
        for (photo, llm_response) in prepared.into_iter().zip(llm_responses) {
            let filename = photo.filename.clone();
            let photo_id = photo.photo_id.clone();
            match finish_one(&state, store_ref, &options, photo, llm_response).await {
                Ok(warn) => {
                    success_count += 1;
                    results.push(json!({
                        "photo_id": photo_id, "success": true, "error": Value::Null,
                    }));
                    if let Some(w) = warn {
                        warnings.push(w);
                    }
                }
                Err(e) => {
                    failure_count += 1;
                    // Logged, not just returned: the count on the line below is
                    // all the log used to carry, so a run where every photo
                    // failed for one fixable reason looked identical to one
                    // where they each failed differently.
                    log::warn!("Indexing failed for {filename}: {e}");
                    results.push(json!({
                        "photo_id": photo_id, "success": false, "error": e,
                    }));
                    error_messages.push(format!("{filename}: {e}"));
                }
            }
        }
    }

    log::info!("Batch processing complete. Success: {success_count}, Failures: {failure_count}.");

    if success_count == 0 {
        let mut unique = Vec::new();
        for e in &error_messages {
            if !unique.contains(e) {
                unique.push(e.clone());
            }
        }
        let mut msg = "No images were successfully processed".to_string();
        if !unique.is_empty() {
            msg.push_str(": ");
            msg.push_str(
                &unique
                    .iter()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" | "),
            );
        }
        // `results` rides along even on the all-failed path: a grouped caller
        // still needs to know which photo hit which error.
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": msg, "results": results})),
        )
            .into_response();
    }

    let _ = total;
    Json(json!({
        "status": "processed",
        "success_count": success_count,
        "failure_count": failure_count,
        "error_messages": error_messages,
        "warnings": warnings,
        "results": results,
    }))
    .into_response()
}

/// One photo's state in flight between the two phases of [`process_batch`].
///
/// Indexing used to run a photo end to end before touching the next one, which
/// meant every photo paid a full prompt prefill. Splitting at the LLM call lets
/// the whole group's requests be handed to the provider at once — the only
/// point at which a pinned prefix and parallel decode sequences can be shared.
struct PreparedPhoto {
    photo_id: String,
    filename: String,
    image_bytes: Vec<u8>,
    // Deliberately no decoded pixels. Phase 1 needs full RGB for the culling
    // metrics, pHash and the SigLIP2 embedding, but it is done with them by the
    // time it returns, and carrying them would scale the largest allocation in
    // the request with the number of photos in flight — roughly 8 MB per photo
    // against a few hundred KB for the normalised JPEG. Face detection, the one
    // thing in phase 2 that wants pixels, decodes `image_bytes` again.
    main_metadata: Map<String, Value>,
    /// The stored embedding, kept so phase 2 can fall back to it when this
    /// pass did not compute a new one.
    existing_vector: Option<Vec<f32>>,
    existing_has_embedding: bool,
    new_embedding: Option<Vec<f32>>,
    /// `Some` when this photo still needs an LLM call. `None` covers both
    /// "metadata generation is off" and "it already has metadata".
    llm_request: Option<lrg_providers::types::MetadataGenerationRequest>,
}

/// Phase 1: everything that does not need the LLM.
///
/// Deliberately stops short of the `IMAGE_TABLE` write, so a photo whose LLM
/// call later fails is never persisted — same as when this was one function.
/// The per-photo culling signals, computed once per batch off the hot path.
pub(crate) struct CullSignals {
    metrics: lrg_imaging::metrics::CullingMetrics,
    phash: String,
}

/// Decodes each image and computes its culling metrics and pHash, in parallel
/// across the batch.
///
/// This used to happen inline in `prepare_one`, inside a loop the surrounding
/// comment describes as "strictly sequential (SigLIP2 and the face detector
/// each want the whole machine)". That reasoning holds for the two ONNX models
/// — both sit behind a `Mutex`, so concurrency there buys nothing — but it does
/// not hold for decode, resampling and the metric passes, which are pure CPU
/// and embarrassingly parallel. At ~51ms/photo measured on a 2048px frame
/// (39ms metrics + 12ms pHash) that was invisible next to a ~400ms embedding
/// and is the dominant cost once `tasks=cull` removes the embedding.
///
/// Decoded pixels are dropped immediately rather than carried forward: a
/// 2048px RGB8 frame is ~8MB, and holding a whole batch of them across the LLM
/// phase is exactly the memory blowup the phase split exists to avoid.
///
/// The config is deliberately the *default*, never a preset's: indexing happens
/// long before the user picks a culling preset, and these values are stored
/// once and read by every later cull run. Storing preset-flavoured numbers
/// would silently bind a catalog to whichever preset happened to be active
/// during import. Presets instead re-derive `cull_technical_score` and
/// `cull_aesthetic` from the stored sub-scores at rank time (see
/// `lrg_analysis::grouping::rank_group_records`), which is where their weights
/// belong. The threshold-shaped fields (denominators, exposure target, clip
/// thresholds) genuinely need pixels, so changing those still requires a
/// re-index.
fn precompute_cull_signals(images: &[Vec<u8>]) -> Vec<Result<CullSignals, String>> {
    use rayon::prelude::*;
    let cfg = ImageMetricsConfig::default();
    images
        .par_iter()
        .map(|bytes| {
            let decoded = image::load_from_memory(bytes)
                .map_err(|e| format!("could not decode image: {e}"))?
                .to_rgb8();
            let (width, height) = (decoded.width() as usize, decoded.height() as usize);
            let pixels = decoded.into_raw();
            let rgb = RgbImage {
                pixels: &pixels,
                width,
                height,
            };
            Ok(CullSignals {
                metrics: culling_metrics(&rgb, &cfg),
                phash: perceptual_hash(&rgb),
            })
        })
        .collect()
}

async fn prepare_one(
    state: &AppState,
    store: &Arc<lrg_store::Store>,
    options: &ParsedOptions,
    overrides: &PhotoOverrides,
    image_bytes: &[u8],
    photo_id: &str,
    filename: &str,
    cull: CullSignals,
) -> Result<PreparedPhoto, String> {
    // The cull signals arrive precomputed from the parallel pass, so the only
    // reason left to decode here is the SigLIP2 embedding. A cull-only run
    // therefore never decodes in this function at all.
    let decoded_for_embedding = if options.compute_embeddings {
        let decoded = image::load_from_memory(image_bytes)
            .map_err(|e| format!("could not decode image: {e}"))?
            .to_rgb8();
        let (width, height) = (decoded.width() as usize, decoded.height() as usize);
        Some((decoded.into_raw(), width, height))
    } else {
        None
    };

    let existing = if options.regenerate_metadata {
        None
    } else {
        store
            .get(IMAGE_TABLE, &[photo_id.to_string()])
            .await
            .ok()
            .and_then(|mut v| v.pop())
    };

    let mut main_metadata: Map<String, Value> = match &existing {
        Some(record) if !options.regenerate_metadata => {
            let mut m = record.metadata.clone();
            m.insert("filename".into(), json!(filename));
            m.insert("photo_id".into(), json!(photo_id));
            m.insert(
                "uuid".into(),
                m.get("uuid").cloned().unwrap_or(json!(photo_id)),
            );
            m
        }
        _ => {
            let mut m = Map::new();
            m.insert("filename".into(), json!(filename));
            m.insert("photo_id".into(), json!(photo_id));
            m.insert("uuid".into(), json!(photo_id));
            if let Some(p) = &options.provider {
                m.insert("provider".into(), json!(p));
            }
            if let Some(mo) = &options.model {
                m.insert("model".into(), json!(mo));
            }
            m
        }
    };

    if let Some(ct) = overrides.capture_time.or(options.capture_time) {
        main_metadata.insert("capture_time".into(), json!(ct));
    }

    // Culling metrics + pHash are cheap enough to compute on every pass.
    //
    // Deliberately the *default* config, never a preset's: indexing happens
    // long before the user picks a culling preset, and these values are stored
    // once and read by every later cull run. Storing preset-flavoured numbers
    // would silently bind a catalog to whichever preset happened to be active
    // during import. Presets instead re-derive `cull_technical_score` and
    // `cull_aesthetic` from the stored sub-scores at rank time (see
    // `lrg_analysis::grouping::rank_group_records`), which is where their
    // weights belong. The threshold-shaped fields here (denominators, exposure
    // target, clip thresholds) genuinely need pixels, so changing those still
    // requires a re-index.
    let CullSignals { metrics, phash } = cull;
    main_metadata.insert("cull_sharpness".into(), json!(metrics.cull_sharpness));
    main_metadata.insert("cull_exposure".into(), json!(metrics.cull_exposure));
    main_metadata.insert("cull_noise".into(), json!(metrics.cull_noise));
    main_metadata.insert(
        "cull_highlight_clip".into(),
        json!(metrics.cull_highlight_clip),
    );
    main_metadata.insert("cull_shadow_clip".into(), json!(metrics.cull_shadow_clip));
    main_metadata.insert(
        "cull_technical_score".into(),
        json!(metrics.cull_technical_score),
    );
    main_metadata.insert("cull_aesthetic".into(), json!(metrics.cull_aesthetic));
    if !phash.is_empty() {
        main_metadata.insert("cull_phash".into(), json!(phash));
        main_metadata.insert("phash".into(), json!(phash));
    }

    // Python's inline delta-check here defaults `has_embedding` to False
    // when absent — a different default than `meta::has_embedding`
    // (which defaults True, matching the stats/get_all_ids call sites).
    let existing_has_embedding = existing
        .as_ref()
        .and_then(|r| r.metadata.get("has_embedding"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let need_embedding =
        options.compute_embeddings && (options.regenerate_metadata || !existing_has_embedding);

    let mut new_embedding: Option<Vec<f32>> = None;
    if need_embedding {
        let t0 = Instant::now();
        let (pixels, width, height) = decoded_for_embedding
            .as_ref()
            .map(|(p, w, h)| (p.as_slice(), *w, *h))
            .ok_or_else(|| "internal: embedding requested without decoded pixels".to_string())?;
        let mut emb = state
            .siglip
            .embed_image(pixels, width, height)
            .map_err(|e| format!("Embedding generation failed: {e}"))?;
        log::debug!("Photo {photo_id}: SigLIP2 embed took {:?}", t0.elapsed());
        lrg_ml::siglip::l2_normalize(&mut emb);
        new_embedding = Some(emb);
    }

    if options.replace_ss {
        for v in main_metadata.values_mut() {
            if let Value::String(s) = v {
                if s.contains('\u{df}') {
                    *v = json!(s.replace('\u{df}', "ss"));
                }
            }
        }
    }

    let llm_request = if options.compute_metadata {
        let has_any_metadata = ["title", "caption", "alt_text", "keywords"]
            .iter()
            .any(|k| {
                existing.as_ref().is_some_and(|r| {
                    r.metadata
                        .get(*k)
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty())
                })
            });
        let need_metadata = options.regenerate_metadata || !has_any_metadata;
        need_metadata.then(|| build_metadata_request(options, overrides, image_bytes, photo_id))
    } else {
        None
    };

    Ok(PreparedPhoto {
        photo_id: photo_id.to_string(),
        filename: filename.to_string(),
        image_bytes: image_bytes.to_vec(),
        main_metadata,
        existing_vector: existing.as_ref().and_then(|r| r.vector.clone()),
        existing_has_embedding,
        new_embedding,
        llm_request,
    })
}

/// Phase 2: fold in the LLM result, detect faces, and write the photo.
///
/// `llm_response` is `Some` exactly when [`prepare_one`] asked for one.
async fn finish_one(
    state: &AppState,
    store: &Arc<lrg_store::Store>,
    options: &ParsedOptions,
    prepared: PreparedPhoto,
    llm_response: Option<lrg_providers::types::MetadataGenerationResponse>,
) -> Result<Option<String>, String> {
    let PreparedPhoto {
        photo_id,
        filename,
        image_bytes,
        mut main_metadata,
        existing_vector,
        existing_has_embedding,
        new_embedding,
        llm_request,
    } = prepared;
    let photo_id = photo_id.as_str();
    let filename = filename.as_str();

    if llm_request.is_some() {
        let response =
            llm_response.ok_or_else(|| "metadata generation returned no response".to_string())?;
        if !response.success {
            return Err(response
                .error
                .unwrap_or_else(|| "Unknown metadata generation error".to_string()));
        }
        if let Some(title) = response.title.filter(|s| !s.is_empty()) {
            main_metadata.insert("title".into(), json!(title));
        }
        if let Some(caption) = response.caption.filter(|s| !s.is_empty()) {
            main_metadata.insert("caption".into(), json!(caption));
        }
        if let Some(alt) = response.alt_text.filter(|s| !s.is_empty()) {
            main_metadata.insert("alt_text".into(), json!(alt));
        }
        if let Some(keywords) = response.keywords {
            if !matches!(&keywords, Value::Array(a) if a.is_empty())
                && !matches!(&keywords, Value::Object(o) if o.is_empty())
            {
                main_metadata.insert("keywords".into(), json!(keywords.to_string()));
                main_metadata.insert(
                    "flattened_keywords".into(),
                    json!(lrg_analysis::keywords::flatten_keywords_to_string(
                        &keywords
                    )),
                );
            }
        }
        main_metadata.insert(
            "provider".into(),
            json!(options.provider.clone().unwrap_or_default()),
        );
        main_metadata.insert(
            "model".into(),
            json!(options.model.clone().unwrap_or_default()),
        );
    }
    main_metadata.insert(
        "run_date".into(),
        json!(chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()),
    );

    let final_vector = match &new_embedding {
        Some(v) => Some(v.clone()),
        None => existing_vector.filter(|_| existing_has_embedding),
    };
    main_metadata.insert("has_embedding".into(), json!(final_vector.is_some()));
    meta::ensure_photo_metadata(photo_id, &mut main_metadata);

    if let Some(catalog_id) = &options.catalog_id {
        let mut ids_set = meta::parse_catalog_ids(&main_metadata);
        ids_set.insert(catalog_id.clone());
        main_metadata.insert(
            meta::CATALOG_IDS_FIELD.into(),
            json!(meta::serialize_catalog_ids(&ids_set)),
        );
    }

    // Everything below folds into `main_metadata` in place — faces (and
    // catalog_id, above) used to each cost their own IMAGE_TABLE
    // `merge_insert`, which scans the whole table to match the key and
    // mints a new dataset version every time; back-to-back per-photo
    // scans on a growing table is what drove the unbounded memory growth
    // during large indexing runs. One coalesced write at the end fixes
    // that, and also fixes a real bug: the old per-branch writes based
    // themselves on the stale pre-catalog_id `record`, so indexing with
    // both faces and catalog_id set silently dropped the catalog_ids
    // field once face processing's write landed last.
    let mut warning = None;
    if options.compute_faces {
        let already_checked = main_metadata
            .get("faces_checked")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if options.regenerate_metadata || !already_checked {
            // Decoded here rather than carried from phase 1: full RGB is by far
            // the largest per-photo allocation, and holding one per photo in
            // flight is how a bigger group would turn into a memory problem.
            // The decode is small next to the detector itself, and this is the
            // only place in phase 2 that needs pixels at all.
            // A decode failure is reported the same way a detector failure is,
            // as a warning: faces are optional and must not sink the photo.
            let t_decode = Instant::now();
            let face_result: Result<Vec<_>, String> = if !state.face.is_cached() {
                // Checked before decoding: without the model files the detector
                // fails anyway, and the decode would be pure waste.
                Err("model not loaded".to_string())
            } else {
                match image::load_from_memory(&image_bytes).map(|img| img.to_rgb8()) {
                    Ok(decoded) => {
                        let (width, height) = (decoded.width() as usize, decoded.height() as usize);
                        let pixels = decoded.into_raw();
                        log::debug!(
                            "Photo {photo_id}: re-decode for faces took {:?}",
                            t_decode.elapsed()
                        );
                        let t0 = Instant::now();
                        let result = state
                            .face
                            .detect_faces(
                                &pixels,
                                width,
                                height,
                                &FaceMetricsConfig::defaults(),
                                options.face_pass,
                            )
                            .map_err(|e| e.to_string());
                        log::debug!("Photo {photo_id}: face detect took {:?}", t0.elapsed());
                        result
                    }
                    Err(e) => Err(format!("could not decode image: {e}")),
                }
            };
            match face_result {
                // The quality-only pass has no identity embedding and no
                // thumbnail, so it must not touch FACE_TABLE at all: writing
                // empty vectors would corrupt person clustering, and clearing
                // the photo's existing rows would destroy identities a previous
                // full run had established. It also deliberately leaves
                // `faces_checked` unset, so `/index/check-unprocessed` still
                // reports the photo as needing a real face pass later.
                Ok(faces) if options.face_pass == FacePass::QualityOnly => {
                    let inputs: Vec<FaceMetricsInput> = faces
                        .iter()
                        .map(|f| FaceMetricsInput {
                            sharpness: f.sharpness,
                            area_ratio: f.area_ratio,
                            det_score: f.det_score,
                            center_proximity: f.center_proximity,
                            eye_openness: f.eye_openness,
                            blink_penalty: f.blink_penalty,
                            occlusion: Some(f.occlusion),
                        })
                        .collect();
                    apply_face_aggregate(&mut main_metadata, &inputs);
                    log::debug!(
                        "Photo {photo_id}: cull pass scored {} face(s), no identity written.",
                        faces.len()
                    );
                }
                Ok(faces) => {
                    let t_scan = Instant::now();
                    // Face ids are `{photo_id}_{n}`; a prefix delete clears
                    // this photo's stale rows in one shot without pulling
                    // the whole table's metadata (including every face's
                    // base64 thumbnail) into memory — see
                    // `Store::delete_by_id_prefix` for why that mattered.
                    store
                        .delete_by_id_prefix(FACE_TABLE, &format!("{photo_id}_"))
                        .await
                        .map_err(|e| e.to_string())?;
                    log::debug!(
                        "Photo {photo_id}: stale FACE_TABLE rows cleared in {:?}",
                        t_scan.elapsed()
                    );

                    if !faces.is_empty() {
                        let face_records: Vec<StoreRecord> = faces
                            .iter()
                            .enumerate()
                            .map(|(i, f)| {
                                let mut m = Map::new();
                                m.insert("photo_id".into(), json!(photo_id));
                                m.insert("photo_uuid".into(), json!(photo_id));
                                m.insert("thumbnail".into(), json!(f.thumbnail_base64));
                                m.insert("person_id".into(), json!(""));
                                m.insert(
                                    "bbox".into(),
                                    json!(serde_json::to_string(&f.bbox).unwrap()),
                                );
                                m.insert("face_area_ratio".into(), json!(f.area_ratio));
                                m.insert("face_sharpness".into(), json!(f.sharpness));
                                m.insert("face_det_score".into(), json!(f.det_score));
                                m.insert("face_center_proximity".into(), json!(f.center_proximity));
                                m.insert("face_eye_openness".into(), json!(f.eye_openness));
                                m.insert("face_blink_penalty".into(), json!(f.blink_penalty));
                                m.insert("face_occlusion".into(), json!(f.occlusion));
                                StoreRecord {
                                    id: format!("{photo_id}_{i}"),
                                    vector: Some(f.embedding.clone()),
                                    metadata: m,
                                }
                            })
                            .collect();
                        store
                            .upsert(FACE_TABLE, &face_records)
                            .await
                            .map_err(|e| e.to_string())?;

                        let inputs: Vec<FaceMetricsInput> = faces
                            .iter()
                            .map(|f| FaceMetricsInput {
                                sharpness: f.sharpness,
                                area_ratio: f.area_ratio,
                                det_score: f.det_score,
                                center_proximity: f.center_proximity,
                                eye_openness: f.eye_openness,
                                blink_penalty: f.blink_penalty,
                                occlusion: Some(f.occlusion),
                            })
                            .collect();
                        apply_face_aggregate(&mut main_metadata, &inputs);
                        log::info!("Photo {photo_id}: indexed {} face(s).", faces.len());
                    } else {
                        apply_face_aggregate(&mut main_metadata, &[]);
                        main_metadata.insert("faces_checked".into(), json!(true));
                    }
                }
                Err(e) => {
                    log::warn!("Face detection/indexing failed for {photo_id}: {e}");
                    warning = Some(format!("{filename} faces: {e}"));
                }
            }
        }
    }

    let record = StoreRecord {
        id: photo_id.to_string(),
        vector: final_vector,
        metadata: main_metadata,
    };
    store
        .upsert(IMAGE_TABLE, std::slice::from_ref(&record))
        .await
        .map_err(|e| format!("Database update failed: {e}"))?;

    // Vertex AI embeddings: optional, separate table. Silently skipped
    // (no warning) when no project is configured, matching Python's
    // `if vertexai_service.is_available(...):` guard with no else branch.
    if options.compute_vertexai {
        let already_has_vertex = store
            .get(VERTEX_TABLE, std::slice::from_ref(&photo_id.to_string()))
            .await
            .ok()
            .is_some_and(|v| v.first().is_some_and(|r| r.vector.is_some()));
        if options.regenerate_metadata || !already_has_vertex {
            if let Some(client) = lrg_providers::vertexai::VertexAiProvider::new(
                options.vertex_project_id.as_deref(),
                options.vertex_location.as_deref(),
            ) {
                let embeddings = client
                    .get_image_embeddings(std::slice::from_ref(&image_bytes.to_vec()))
                    .await;
                if let Some(Some(embedding)) = embeddings.into_iter().next() {
                    let mut vertex_meta = Map::new();
                    vertex_meta.insert("photo_id".into(), json!(photo_id));
                    vertex_meta.insert("uuid".into(), json!(photo_id));
                    let vertex_record = StoreRecord {
                        id: photo_id.to_string(),
                        vector: Some(embedding),
                        metadata: vertex_meta,
                    };
                    if let Err(e) = store
                        .upsert(VERTEX_TABLE, std::slice::from_ref(&vertex_record))
                        .await
                    {
                        log::error!("Vertex AI embedding upsert failed for {photo_id}: {e}");
                    } else {
                        log::debug!("Photo {photo_id}: Vertex AI embedding stored.");
                    }
                }
            }
        }
    }

    Ok(warning)
}

fn apply_face_aggregate(metadata: &mut Map<String, Value>, faces: &[FaceMetricsInput]) {
    let agg = aggregate_face_culling_metrics(faces, &FaceMetricsConfig::defaults());
    metadata.insert("cull_face_count".into(), json!(agg.cull_face_count));
    metadata.insert("cull_face_sharpness".into(), json!(agg.cull_face_sharpness));
    metadata.insert(
        "cull_face_prominence".into(),
        json!(agg.cull_face_prominence),
    );
    metadata.insert(
        "cull_face_visibility".into(),
        json!(agg.cull_face_visibility),
    );
    metadata.insert("cull_face_score".into(), json!(agg.cull_face_score));
    metadata.insert("cull_eye_openness".into(), json!(agg.cull_eye_openness));
    metadata.insert("cull_blink_penalty".into(), json!(agg.cull_blink_penalty));
    metadata.insert("cull_occlusion".into(), json!(agg.cull_occlusion));
    metadata.insert("cull_faces_present".into(), json!(agg.cull_faces_present));
}

/// The wire provider name for this run, lowercased, defaulting to "ollama".
fn provider_name(options: &ParsedOptions) -> String {
    options
        .provider
        .as_deref()
        .unwrap_or("ollama")
        .to_lowercase()
}

/// Resolve the provider once for a whole batch.
///
/// This used to happen inside the per-photo path, which for the in-process
/// backend meant re-resolving the engine for every single photo.
async fn provider_for_batch(
    state: &AppState,
    options: &ParsedOptions,
) -> Result<Arc<dyn lrg_providers::provider::LlmProvider>, String> {
    let provider = provider_name(options);
    let model = options.model.clone().unwrap_or_default();
    let mo = &options.metadata_request;
    build_provider(&ProviderSelection {
        local_engine: crate::routes::llm::engine_for_request(
            state,
            &provider,
            &model,
            options.engine,
        )
        .await?,
        name: provider,
        api_key: options.api_key.clone(),
        ollama_base_url: mo.ollama_base_url.clone(),
        lmstudio_base_url: mo.lmstudio_base_url.clone(),
    })
}

/// Builds one photo's `MetadataGenerationRequest` from the parsed options.
///
/// Pure and cheap on purpose: phase 1 builds these for the whole group, and
/// only then does the provider see them.
fn build_metadata_request(
    options: &ParsedOptions,
    overrides: &PhotoOverrides,
    image_bytes: &[u8],
    photo_id: &str,
) -> lrg_providers::types::MetadataGenerationRequest {
    let provider = provider_name(options);
    let model = options.model.clone().unwrap_or_default();
    let mo = &options.metadata_request;

    let location_data = lrg_imaging::location::extract_location_tags(image_bytes);
    lrg_providers::types::MetadataGenerationRequest {
        image_data: image_bytes.to_vec(),
        uuid: photo_id.to_string(),
        provider: provider.clone(),
        model,
        api_key: options.api_key.clone(),
        generate_keywords: mo.generate_keywords,
        generate_caption: mo.generate_caption,
        generate_title: mo.generate_title,
        generate_alt_text: mo.generate_alt_text,
        language: mo.language.clone(),
        temperature: mo.temperature,
        max_tokens: mo.max_tokens,
        system_prompt: mo.system_prompt.clone(),
        user_prompt: None,
        submit_keywords: mo.submit_keywords,
        submit_folder_names: mo.submit_folder_names,
        existing_keywords: overrides
            .existing_keywords
            .clone()
            .or_else(|| mo.existing_keywords.clone()),
        location_data,
        folder_names: overrides
            .folder_names
            .clone()
            .or_else(|| mo.folder_names.clone()),
        user_context: mo.user_context.clone(),
        date_time: overrides.date_time.clone().or_else(|| mo.date_time.clone()),
        keyword_categories: mo.keyword_categories.clone(),
        bilingual_keywords: mo.bilingual_keywords,
        keyword_secondary_language: mo.keyword_secondary_language.clone(),
        generate_aliases: mo.generate_aliases,
        catalog_keywords: mo.catalog_keywords.clone(),
        ollama_base_url: mo.ollama_base_url.clone(),
        lmstudio_base_url: mo.lmstudio_base_url.clone(),
    }
}

#[cfg(test)]
mod keyword_option_tests {
    use super::*;

    fn fields(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Without this, a grouped request would hand every photo the first
    /// photo's capture time, keywords and folders.
    #[test]
    fn per_image_context_wins_over_the_batch_wide_fields() {
        let opts = parse_options(&fields(&[
            ("date_time", "2026-01-01 00:00:00"),
            ("existing_keywords", r#"["batch"]"#),
            ("folder_names", "BatchFolder"),
        ]));
        let overrides = PhotoOverrides {
            capture_time: Some(1234.0),
            date_time: Some("2026-08-07 12:00:00".to_string()),
            existing_keywords: Some(vec!["mine".to_string()]),
            folder_names: Some("MyFolder".to_string()),
        };
        let req = build_metadata_request(&opts, &overrides, &[], "p1");
        assert_eq!(req.date_time.as_deref(), Some("2026-08-07 12:00:00"));
        assert_eq!(req.existing_keywords, Some(vec!["mine".to_string()]));
        assert_eq!(req.folder_names.as_deref(), Some("MyFolder"));
    }

    /// The single-photo path sends no per-image context, so it must keep
    /// reading the flat fields exactly as it always has.
    #[test]
    fn absent_per_image_context_falls_back_to_the_batch_fields() {
        let opts = parse_options(&fields(&[
            ("date_time", "2026-01-01 00:00:00"),
            ("existing_keywords", r#"["batch"]"#),
            ("folder_names", "BatchFolder"),
        ]));
        let req = build_metadata_request(&opts, &PhotoOverrides::default(), &[], "p1");
        assert_eq!(req.date_time.as_deref(), Some("2026-01-01 00:00:00"));
        assert_eq!(req.existing_keywords, Some(vec!["batch".to_string()]));
        assert_eq!(req.folder_names.as_deref(), Some("BatchFolder"));
    }

    #[test]
    fn llm_batch_size_defaults_to_asking_the_provider() {
        assert_eq!(parse_options(&fields(&[])).llm_batch_size, None);
    }

    #[test]
    fn llm_batch_size_parses_an_explicit_override() {
        assert_eq!(
            parse_options(&fields(&[("llm_batch_size", " 4 ")])).llm_batch_size,
            Some(4)
        );
    }

    #[test]
    fn llm_batch_size_rejects_zero_and_garbage() {
        // Both fall back to the provider's own width; a batch of zero photos
        // would otherwise loop forever producing nothing.
        assert_eq!(
            parse_options(&fields(&[("llm_batch_size", "0")])).llm_batch_size,
            None
        );
        assert_eq!(
            parse_options(&fields(&[("llm_batch_size", "lots")])).llm_batch_size,
            None
        );
    }

    #[test]
    fn flat_keyword_categories_json_array_parses() {
        let opts = parse_options(&fields(&[(
            "keyword_categories",
            r#"["People","Places","Nature"]"#,
        )]));
        match opts.metadata_request.keyword_categories {
            Some(KeywordCategories::Flat(list)) => {
                assert_eq!(list, vec!["People", "Places", "Nature"]);
            }
            other => panic!("expected Flat categories, got {other:?}"),
        }
    }

    #[test]
    fn nested_keyword_categories_json_object_parses() {
        let opts = parse_options(&fields(&[(
            "keyword_categories",
            r#"{"Nature":{"Flower":{},"Animal":{}},"Places":{}}"#,
        )]));
        match opts.metadata_request.keyword_categories {
            Some(KeywordCategories::Nested(tree)) => {
                assert_eq!(tree.len(), 2);
                let nature = tree.iter().find(|(name, _)| name == "Nature").unwrap();
                assert_eq!(nature.1.len(), 2);
            }
            other => panic!("expected Nested categories, got {other:?}"),
        }
    }

    #[test]
    fn empty_or_absent_keyword_categories_is_none() {
        assert!(parse_options(&fields(&[("keyword_categories", "{}")]))
            .metadata_request
            .keyword_categories
            .is_none());
        assert!(parse_options(&fields(&[("keyword_categories", "[]")]))
            .metadata_request
            .keyword_categories
            .is_none());
        assert!(parse_options(&fields(&[]))
            .metadata_request
            .keyword_categories
            .is_none());
    }

    #[test]
    fn custom_prompt_and_date_time_are_wired() {
        let opts = parse_options(&fields(&[
            ("prompt", "  You are a bird expert.  "),
            ("date_time", "2026-07-28 11:18:22"),
        ]));
        let mo = &opts.metadata_request;
        assert_eq!(mo.system_prompt.as_deref(), Some("You are a bird expert."));
        assert_eq!(mo.date_time.as_deref(), Some("2026-07-28 11:18:22"));
    }

    #[test]
    fn blank_or_absent_prompt_falls_back_to_default() {
        assert!(parse_options(&fields(&[("prompt", "   ")]))
            .metadata_request
            .system_prompt
            .is_none());
        assert!(parse_options(&fields(&[]))
            .metadata_request
            .system_prompt
            .is_none());
    }

    #[test]
    fn bilingual_and_alias_and_language_and_catalog_keywords_are_wired() {
        let opts = parse_options(&fields(&[
            ("bilingual_keywords", "true"),
            ("keyword_secondary_language", "French"),
            ("generate_aliases", "true"),
            ("catalog_keywords", r#"["cat","dog"]"#),
        ]));
        let mo = &opts.metadata_request;
        assert!(mo.bilingual_keywords);
        assert_eq!(mo.keyword_secondary_language.as_deref(), Some("French"));
        assert!(mo.generate_aliases);
        assert_eq!(
            mo.catalog_keywords,
            Some(vec!["cat".to_string(), "dog".to_string()])
        );
    }
}
