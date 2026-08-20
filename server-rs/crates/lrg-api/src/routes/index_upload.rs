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
use futures_util::StreamExt;
use serde_json::{json, Map, Value};

use lrg_analysis::face_aggregate::{aggregate_face_culling_metrics, FaceMetricsInput};
use lrg_imaging::convert::{normalize_image_bytes, UnsupportedImageError};
use lrg_imaging::cull_config::{FaceMetricsConfig, ImageMetricsConfig};
use lrg_imaging::metrics::{culling_metrics, perceptual_hash, RgbImage};
use lrg_ml::faces::FacePass;
use lrg_providers::provider::{build_provider, ProviderSelection};
use lrg_providers::types::{KeywordCategories, KeywordTree};
use lrg_store::{meta, StoreRecord, FACE_TABLE, IMAGE_TABLE, SPECIES_TABLE, VERTEX_TABLE};

use crate::routes::route_util::parse_multipart;
use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route("/index", axum::routing::post(index_batch))
}

pub(crate) struct ParsedOptions {
    compute_embeddings: bool,
    compute_faces: bool,
    /// How much of the face pipeline to run. The `cull` task only reads the
    /// quality proxies, so it skips FaceNet and the thumbnail encode.
    face_pass: FacePass,
    compute_metadata: bool,
    compute_vertexai: bool,
    /// Run BioCLIP 2 and write a taxonomic identification.
    compute_species: bool,
    /// Only run BioCLIP on photos the organism prompt gate lets through.
    ///
    /// On by default because BioCLIP is a ViT-L/14 — a few hundred ms per
    /// photo on CPU — and most of a general photo library has no organism in
    /// it. The gate itself is free: it scores the SigLIP2 embedding indexing
    /// already computed. Users who know their selection is all wildlife can
    /// turn it off and skip the (small) chance of a false negative.
    species_prefilter: bool,
    /// Gate threshold in `0..1`. Deliberately low: a false positive costs one
    /// wasted BioCLIP pass whose result the rank floor then discards, a false
    /// negative silently loses a species the user wanted.
    species_prefilter_threshold: f64,
    species_classify: lrg_ml::bioclip::ClassifyConfig,
    regenerate_metadata: bool,
    replace_ss: bool,
    catalog_id: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    capture_time: Option<f64>,
    /// Batch-level exposure compensation, from the single-photo multipart
    /// path. `/index_by_reference` carries it per photo instead — see
    /// [`PhotoOverrides::exposure_bias`].
    exposure_bias: Option<f64>,
    /// Whether the original is a raw file, from the multipart path. Stored
    /// with the photo rather than used during indexing: style training has to
    /// keep raw and rendered originals apart, because Lightroom's `Temp` is
    /// Kelvin for one and a relative -100..100 for the other. Absent when the
    /// plugin could not read the format.
    is_raw: Option<bool>,
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
/// fields arrive flat, one set for the whole request. These are exactly the
/// ones `prompts.rs` classifies as volatile — reusing one photo's capture
/// time, keywords or folders for the whole group would feed the model context
/// belonging to a different photo. Absent entries fall back to the
/// batch-level options, which is what every single-photo request does.
#[derive(Default, Clone)]
pub(crate) struct PhotoOverrides {
    pub capture_time: Option<f64>,
    pub date_time: Option<String>,
    pub existing_keywords: Option<Vec<String>>,
    pub folder_names: Option<String>,
    /// Exposure compensation in EV (`exposureBias` in the Lightroom SDK).
    ///
    /// Not prompt context like the others — it is the decisive evidence for
    /// bracket detection during culling, which has no other way to tell an AEB
    /// sequence from a burst, and there is no batch-level fallback because a
    /// per-photo EV shared across a group would be actively misleading.
    pub exposure_bias: Option<f64>,
    /// Per-photo raw flag, for `/index_by_reference`. Falls back to the
    /// batch-level [`ParsedOptions::is_raw`] when absent.
    pub is_raw: Option<bool>,
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
    /// Whether the photo's own GPS may be turned into place names and sent to
    /// the model along with the image.
    ///
    /// The plugin has always sent this field and the backend has always
    /// ignored it, reading location from the EXIF regardless — so the switch
    /// existed on the wire but not in behaviour. Defaults to true so existing
    /// installs keep the context they have been getting.
    submit_gps: bool,
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

    let default_classify = lrg_ml::bioclip::ClassifyConfig::default();

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
        compute_species: has_task("species"),
        species_prefilter: bool_field(fields, "species_prefilter", true),
        species_prefilter_threshold: fields
            .get("species_prefilter_threshold")
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|v| (0.0..=1.0).contains(v))
            .unwrap_or(0.35),
        species_classify: lrg_ml::bioclip::ClassifyConfig {
            min_confidence: fields
                .get("species_min_confidence")
                .and_then(|s| s.trim().parse::<f32>().ok())
                .filter(|v| (0.0..=1.0).contains(v))
                .unwrap_or(default_classify.min_confidence),
            top_k: fields
                .get("species_top_k")
                .and_then(|s| s.trim().parse::<usize>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(default_classify.top_k),
        },
        regenerate_metadata,
        replace_ss: bool_field(fields, "replace_ss", false),
        catalog_id,
        provider: fields.get("provider").cloned(),
        model: fields.get("model").cloned(),
        api_key: fields.get("api_key").cloned(),
        capture_time,
        exposure_bias: fields
            .get("exposure_bias")
            .and_then(|s| s.trim().parse::<f64>().ok()),
        is_raw: fields
            .get("is_raw")
            .map(|s| s.trim().eq_ignore_ascii_case("true")),
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
            submit_gps: bool_field(fields, "submit_gps", true),
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

/// Where [`process_batch`] gets each photo's bytes.
///
/// The distinction is about peak memory, not convenience. `/index_by_reference`
/// points at the user's originals — 25–50 MB raw files — and reading the whole
/// group before normalising any of it kept every one of them resident at once.
/// The normalised JPEG is a few hundred KB and the raw bytes are dead the
/// moment it exists, so only a bounded number of files is ever in flight (see
/// [`decode_concurrency`]). The normalised results still all live until the
/// batch finishes, because the LLM call needs them together.
pub(crate) enum ImageSource {
    /// Already in memory: the multipart path, where axum buffered the request
    /// body before the handler ran. Nothing to gain by being lazy here.
    Loaded(Vec<UploadedImage>),
    /// Paths on disk, read on demand.
    Paths(Vec<String>),
}

impl ImageSource {
    fn len(&self) -> usize {
        match self {
            ImageSource::Loaded(v) => v.len(),
            ImageSource::Paths(v) => v.len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Splits the source into one independently loadable item per photo.
    ///
    /// Consuming, and per-photo rather than indexed, because the loading is no
    /// longer a `&mut self` loop: several photos are read and normalised at
    /// once, which cannot be expressed while one borrow of the whole source is
    /// held across every await.
    fn into_pending(self) -> Vec<PendingImage> {
        match self {
            ImageSource::Loaded(images) => images.into_iter().map(PendingImage::Loaded).collect(),
            ImageSource::Paths(paths) => paths.into_iter().map(PendingImage::Path).collect(),
        }
    }
}

/// One photo's bytes, or where to find them.
enum PendingImage {
    Loaded(UploadedImage),
    Path(String),
}

/// How many photos are read and normalised at once.
///
/// Decode is pure CPU and embarrassingly parallel — the same reasoning that
/// already applies to [`precompute_cull_signals`] — and for a batch of raw
/// originals it is the single largest sequential cost in the request.
///
/// Deliberately small, and small by default. This is the one place a bigger
/// batch could reintroduce the memory blowup the by-reference path was built
/// to avoid: `k` raw originals are resident at once, so at 50 MB apiece the
/// default already accounts for ~150 MB before anything is normalised. Raising
/// it trades that directly against wall time.
///
/// `GENIUSAI_INDEX_DECODE_CONCURRENCY=1` restores the strictly sequential
/// behaviour this replaced.
fn decode_concurrency() -> usize {
    concurrency_from(std::env::var("GENIUSAI_INDEX_DECODE_CONCURRENCY").ok())
}

/// The parsing half of [`decode_concurrency`], split out so it can be tested
/// without reaching into the process environment.
fn concurrency_from(configured: Option<String>) -> usize {
    if let Some(raw) = configured {
        match raw.trim().parse::<usize>() {
            Ok(n) if n >= 1 => return n,
            _ => log::warn!(
                "GENIUSAI_INDEX_DECODE_CONCURRENCY={raw:?} is not a positive integer; ignoring"
            ),
        }
    }
    std::thread::available_parallelism()
        .map(|n| n.get().min(3))
        .unwrap_or(1)
        .max(1)
}

/// Produces one photo's bytes, reading from disk only for the path variant.
///
/// A read failure names the file, because it is reported against that photo's
/// own `photo_id` and the caller retries exactly that photo.
async fn load_pending(pending: PendingImage) -> Result<UploadedImage, String> {
    match pending {
        PendingImage::Loaded(img) => Ok(img),
        PendingImage::Path(path) => {
            let bytes = tokio::fs::read(&path).await.map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    format!("File not found: {path}")
                } else {
                    format!("Error reading {path}: {e}")
                }
            })?;
            let filename = std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or(path);
            Ok(UploadedImage { bytes, filename })
        }
    }
}

/// Reads one photo if needed and normalises it, off the async worker.
///
/// The raw bytes are dropped inside the blocking task, the moment the
/// normalised JPEG exists, so they never overlap with the next photo's read
/// any longer than they have to.
async fn load_and_normalize(
    pending: PendingImage,
    photo_id: String,
    max_edge: u32,
    quality: u8,
) -> Result<(Vec<u8>, String), String> {
    let img = load_pending(pending).await?;

    let t0 = Instant::now();
    let filename = img.filename;
    let bytes = img.bytes;
    // `normalize_image_bytes` is hundreds of milliseconds of pure CPU for a raw
    // original; it used to run directly on a tokio worker thread.
    let (result, filename) = tokio::task::spawn_blocking(move || {
        let out = normalize_image_bytes(&bytes, Some(&filename), max_edge, quality);
        (out, filename)
    })
    .await
    .map_err(|e| format!("image conversion panicked: {e}"))?;

    log::debug!(
        "Photo {photo_id} ({filename}): decode+resize+encode took {:?}",
        t0.elapsed()
    );
    result.map_err(|UnsupportedImageError(msg)| msg)
}

async fn index_batch(State(state): State<Arc<AppState>>, mut multipart: Multipart) -> Response {
    log::info!("Index request received");

    let form = match parse_multipart(&mut multipart).await {
        Ok(form) => form,
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error": e})),
            )
                .into_response()
        }
    };
    let photo_ids = form.photo_ids_or_uuids();
    let fields = form.fields;
    // `"photo"` is this route's own default for an unnamed part; the parser
    // keeps `filename` optional precisely so it does not impose one.
    let images: Vec<UploadedImage> = form
        .images
        .into_iter()
        .map(|img| UploadedImage {
            bytes: img.bytes,
            filename: img.filename.unwrap_or_else(|| "photo".to_string()),
        })
        .collect();

    // Multipart uploads carry no per-image context, so every photo falls back
    // to the batch-level options exactly as before.
    process_batch(
        state,
        fields,
        ImageSource::Loaded(images),
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
    images: ImageSource,
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

    // `Arc<[u8]>` rather than `Vec<u8>`: the normalised JPEG for every photo in
    // the batch is handed to the parallel cull pass, kept in `PreparedPhoto`
    // for phase 2, and read again by face detection. Each of those used to be
    // a full copy, so a 200-photo batch held the batch three times over.
    let mut triplets: Vec<(Arc<[u8]>, String, String, PhotoOverrides)> = Vec::new();
    let mut conversion_errors: Vec<String> = pre_failures;
    // A bounded number of photos in flight: read, normalise, drop the original.
    // With `ImageSource::Paths` that is what keeps a group of raw files from
    // all being resident at once. `buffered` preserves order, so a failure is
    // still recorded against its own photo_id — which is what the caller needs
    // in order to retry that photo alone.
    let concurrency = decode_concurrency();
    let t_decode = Instant::now();
    let normalized: Vec<Result<(Vec<u8>, String), String>> = futures_util::stream::iter(
        images
            .into_pending()
            .into_iter()
            .zip(photo_ids.iter().cloned()),
    )
    .map(|(pending, photo_id)| load_and_normalize(pending, photo_id, max_edge, quality))
    .buffered(concurrency)
    .collect()
    .await;
    log::debug!(
        "Read and normalised {} photo(s) at concurrency {concurrency} in {:?}",
        normalized.len(),
        t_decode.elapsed()
    );

    for ((photo_id, photo_overrides), result) in
        photo_ids.into_iter().zip(overrides).zip(normalized)
    {
        match result {
            Ok((bytes, filename)) => {
                triplets.push((Arc::from(bytes), photo_id, filename, photo_overrides))
            }
            Err(msg) => {
                log::warn!("Skipping {photo_id}: {msg}");
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
    let image_blobs: Vec<Arc<[u8]>> = triplets.iter().map(|(b, _, _, _)| Arc::clone(b)).collect();
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
        // Token accounting for the run. The per-photo lines are `debug` — a
        // few thousand photos would drown the log — but the aggregate below
        // is `info`, because it is the number to compare across changes to
        // the prompt or the response schema. Output tokens are the expensive
        // ones: each costs a full pass over the model weights, so the
        // out-tokens-per-second figure is what says whether decode dominates.
        let mut llm_photos: u64 = 0;
        let mut total_in: u64 = 0;
        let mut total_out: u64 = 0;
        let mut total_llm_secs: f64 = 0.0;

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
                    let elapsed = t0.elapsed();
                    total_llm_secs += elapsed.as_secs_f64();
                    log::debug!(
                        "LLM batch of {} photo(s) via {} took {:?}",
                        group.len(),
                        provider.name(),
                        elapsed
                    );
                    for (&i, response) in group.iter().zip(responses) {
                        log::debug!(
                            "Photo {}: LLM tokens in={} out={}",
                            prepared[i].photo_id,
                            response.input_tokens,
                            response.output_tokens
                        );
                        llm_photos += 1;
                        total_in += u64::from(response.input_tokens);
                        total_out += u64::from(response.output_tokens);
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

        if llm_photos > 0 {
            let photos = llm_photos as f64;
            log::info!(
                "LLM tokens over {llm_photos} photo(s): in={total_in} out={total_out} \
                 (mean per photo: in={:.0} out={:.0}); {:.1}s total, {:.1} output tok/s",
                total_in as f64 / photos,
                total_out as f64 / photos,
                total_llm_secs,
                if total_llm_secs > 0.0 {
                    total_out as f64 / total_llm_secs
                } else {
                    0.0
                }
            );
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
                    // The photo's own warnings ride on its result too, not just
                    // in the request-level list: a grouped caller reads
                    // `results` per photo, and a degradation nobody can pin to
                    // a photo is one nobody acts on.
                    results.push(json!({
                        "photo_id": photo_id,
                        "success": true,
                        "error": Value::Null,
                        "warnings": &warn,
                    }));
                    warnings.extend(warn);
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
            Json(json!({"error": msg, "results": results, "warnings": warnings})),
        )
            .into_response();
    }

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
    image_bytes: Arc<[u8]>,
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
fn precompute_cull_signals(images: &[Arc<[u8]>]) -> Vec<Result<CullSignals, String>> {
    use rayon::prelude::*;
    let cfg = ImageMetricsConfig::default();
    images
        .par_iter()
        .map(|bytes| {
            let decoded = image::load_from_memory(bytes.as_ref())
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

// Six of these are the per-photo inputs the batch loop already carries as a
// tuple; bundling them into a struct here would just move the same list one
// level out without making any call site clearer.
#[allow(clippy::too_many_arguments)]
async fn prepare_one(
    state: &AppState,
    store: &Arc<lrg_store::Store>,
    options: &ParsedOptions,
    overrides: &PhotoOverrides,
    image_bytes: &Arc<[u8]>,
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
    // Only written when the plugin sent it. Never defaulted to 0.0: bracket
    // detection requires *every* frame in a group to carry a value, and a
    // fabricated zero would turn "we do not know" into "all frames identical",
    // which is exactly the shape a focus stack is matched on.
    if let Some(ev) = overrides.exposure_bias.or(options.exposure_bias) {
        main_metadata.insert("exposure_bias".into(), json!(ev));
    }
    // Only written when the plugin knew the answer. Never defaulted: a
    // fabricated `false` would tell the training side a raw file's Kelvin
    // white balance is a relative value, which is the mistake the flag exists
    // to prevent.
    if let Some(is_raw) = overrides.is_raw.or(options.is_raw) {
        main_metadata.insert("is_raw".into(), json!(is_raw));
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
    // Subject-region signals. Older rows predate these; every consumer treats
    // an absent value as "fall back to the frame-wide number", so a catalog
    // indexed before this shipped keeps working and simply does not benefit
    // until it is re-indexed.
    main_metadata.insert(
        "cull_sharpness_peak".into(),
        json!(metrics.cull_sharpness_peak),
    );
    main_metadata.insert(
        "cull_focus_concentration".into(),
        json!(metrics.cull_focus_concentration),
    );
    main_metadata.insert(
        "cull_sharp_region_x".into(),
        json!(metrics.cull_sharp_region_x),
    );
    main_metadata.insert(
        "cull_sharp_region_y".into(),
        json!(metrics.cull_sharp_region_y),
    );
    main_metadata.insert(
        "cull_motion_anisotropy".into(),
        json!(metrics.cull_motion_anisotropy),
    );
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
        image_bytes: Arc::clone(image_bytes),
        main_metadata,
        existing_vector: existing.as_ref().and_then(|r| r.vector.clone()),
        existing_has_embedding,
        new_embedding,
        llm_request,
    })
}

/// Fields the "Replace ß with ss" option applies to: the generated text, and
/// nothing else.
///
/// `keywords` holds a serialized JSON tree rather than a plain string; a
/// literal ß→ss substitution is safe there because neither character needs
/// escaping, so the document stays valid and only the leaf words change.
const SHARP_S_FIELDS: [&str; 5] = [
    "title",
    "caption",
    "alt_text",
    "keywords",
    "flattened_keywords",
];

fn replace_sharp_s(metadata: &mut Map<String, Value>) {
    for field in SHARP_S_FIELDS {
        if let Some(Value::String(s)) = metadata.get(field) {
            if s.contains('\u{df}') {
                let replaced = s.replace('\u{df}', "ss");
                metadata.insert(field.into(), json!(replaced));
            }
        }
    }
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
) -> Result<Vec<String>, String> {
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

    // A vec, not one slot: metadata generation, faces and species can each
    // degrade independently, and a single `Option` meant whichever failed
    // second erased the other's report — the user was told about the species
    // model while the missing face model went unmentioned.
    let mut warnings: Vec<String> = Vec::new();

    if llm_request.is_some() {
        let response =
            llm_response.ok_or_else(|| "metadata generation returned no response".to_string())?;
        if !response.success {
            return Err(response
                .error
                .unwrap_or_else(|| "Unknown metadata generation error".to_string()));
        }
        // The provider says `success: true` even when it dropped a field the
        // user asked for; `warning` is where it says so. Reading it here is
        // the whole reason the field exists — it was written as `None` by
        // every provider and read by nobody, so a photo that came back
        // without its caption counted as a clean success.
        if let Some(w) = response.warning.filter(|s| !s.trim().is_empty()) {
            warnings.push(format!("{filename}: {w}"));
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

    // "Replace ß with ss" used to run in `prepare_one`, i.e. *before* the LLM
    // answered — so it only ever saw metadata imported from the catalog and
    // never touched a single generated word, which is the entire point of the
    // option. It also ran over every value in the map, including `filename`,
    // quietly rewriting the stored name of a file called `Straße.jpg`. Hence
    // both changes: after the merge, and only over the fields the option is
    // about.
    if options.replace_ss {
        replace_sharp_s(&mut main_metadata);
    }

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
    if options.compute_faces {
        // "Checked" means checked *by the models this build runs*. A photo
        // whose faces were found by the retired SCRFD/ArcFace pair carries
        // embeddings from a different space, so trusting the flag would leave
        // it permanently un-re-detected while its rows quietly poisoned every
        // clustering run. Rows written before `face_model` existed have no
        // marker at all, which is correctly treated as "not this model".
        let already_checked = main_metadata
            .get("faces_checked")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && main_metadata.get("face_model").and_then(Value::as_str)
                == Some(state.face.model_id());
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
                //
                // Names the fix, not the state. The overwhelmingly common
                // cause is a run started before the models finished
                // downloading, and "model not loaded" left the user with a
                // symptom and nowhere to go.
                Err(
                    "face model is not downloaded yet — run \"Download AI models\" \
                     in Plug-in Manager and index these photos again"
                        .to_string(),
                )
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
                    let prefix = format!("{photo_id}_");
                    // Read before deleting: detection replaces this photo's rows
                    // wholesale, and a fresh row starts with an empty
                    // `person_id`, so without this every re-index wipes the
                    // identities the user established.
                    let previous = store
                        .get_by_id_prefix(FACE_TABLE, &prefix)
                        .await
                        .unwrap_or_else(|e| {
                            // Losing the carry-over degrades the result; failing
                            // the photo over it would be worse.
                            log::warn!(
                                "Photo {photo_id}: could not read previous faces ({e}); \
                                 person assignments will not be carried over."
                            );
                            Vec::new()
                        });

                    let t_scan = Instant::now();
                    // Face ids are `{photo_id}_{n}`; a prefix delete clears
                    // this photo's stale rows in one shot without pulling
                    // the whole table's metadata (including every face's
                    // base64 thumbnail) into memory — see
                    // `Store::delete_by_id_prefix` for why that mattered.
                    store
                        .delete_by_id_prefix(FACE_TABLE, &prefix)
                        .await
                        .map_err(|e| e.to_string())?;
                    log::debug!(
                        "Photo {photo_id}: stale FACE_TABLE rows cleared in {:?}",
                        t_scan.elapsed()
                    );

                    if !faces.is_empty() {
                        let embeddings: Vec<&[f32]> =
                            faces.iter().map(|f| f.embedding.as_slice()).collect();
                        // Only rows from the current models can be matched by
                        // embedding distance. A cosine distance between an
                        // ArcFace vector and a FaceNet one is a number, not a
                        // measurement, and `FACE_CARRY_OVER_MAX_DISTANCE` is
                        // tight enough that it would mostly refuse to carry
                        // anything over — but "mostly" is how one person's name
                        // ends up on another person's face.
                        let previous: Vec<StoreRecord> = previous
                            .into_iter()
                            .filter(|r| {
                                r.metadata.get("face_model").and_then(Value::as_str)
                                    == Some(state.face.model_id())
                            })
                            .collect();
                        let carried = carry_over_person_ids(&previous, &embeddings);
                        let carried_count = carried.iter().filter(|p| !p.is_empty()).count();
                        if carried_count > 0 {
                            log::debug!(
                                "Photo {photo_id}: carried {carried_count} person assignment(s) \
                                 across re-detection."
                            );
                        }

                        let face_records: Vec<StoreRecord> = faces
                            .iter()
                            .enumerate()
                            .map(|(i, f)| {
                                let mut m = Map::new();
                                m.insert("photo_id".into(), json!(photo_id));
                                m.insert("photo_uuid".into(), json!(photo_id));
                                m.insert("thumbnail".into(), json!(f.thumbnail_base64));
                                m.insert(
                                    "person_id".into(),
                                    json!(carried.get(i).cloned().unwrap_or_default()),
                                );
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
                                m.insert("face_model".into(), json!(state.face.model_id()));
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
                    }
                    // Set for *both* outcomes. Flagging only the faceless branch
                    // meant a photo that has faces never counted as checked, so
                    // every later pass re-ran detection on it — and that path
                    // replaces the photo's rows, which is how person assignments
                    // used to disappear on a plain re-index.
                    main_metadata.insert("faces_checked".into(), json!(true));
                    main_metadata.insert("face_model".into(), json!(state.face.model_id()));
                }
                Err(e) => {
                    log::warn!("Face detection/indexing failed for {photo_id}: {e}");
                    warnings.push(format!("{filename} faces: {e}"));
                }
            }
        }
    }

    // Species identification. Runs before the coalesced write below so its
    // results land in the same upsert as everything else — see the comment
    // above the face block for why nothing here gets its own `merge_insert`.
    // The 768-d BioCLIP vector is the one thing that cannot go in that write,
    // and it goes to SPECIES_TABLE after it, like the Vertex embedding.
    let mut species_vector: Option<Vec<f32>> = None;
    if options.compute_species {
        let already_checked = main_metadata
            .get("species_checked")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if options.regenerate_metadata || !already_checked {
            match run_species(
                state,
                options,
                photo_id,
                &image_bytes,
                final_vector.as_deref(),
            ) {
                Ok((prediction, vector)) => {
                    apply_species_metadata(&mut main_metadata, &prediction, &state.bioclip);
                    species_vector = vector;
                }
                Err(e) => {
                    // Same contract as faces: an optional signal must never
                    // sink the photo, and `species_checked` stays unset so a
                    // later run retries rather than treating this as done.
                    log::warn!("Species identification failed for {photo_id}: {e}");
                    warnings.push(format!("{filename} species: {e}"));
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

    if let Some(vector) = species_vector {
        let mut species_meta = Map::new();
        species_meta.insert("photo_id".into(), json!(photo_id));
        species_meta.insert("uuid".into(), json!(photo_id));
        let species_record = StoreRecord {
            id: photo_id.to_string(),
            vector: Some(vector),
            metadata: species_meta,
        };
        if let Err(e) = store
            .upsert(SPECIES_TABLE, std::slice::from_ref(&species_record))
            .await
        {
            // Logged, not fatal: the identification itself is already stored
            // in IMAGE_TABLE. Losing the vector only costs a re-inference if
            // the taxonomy head is ever swapped.
            log::error!("Species embedding upsert failed for {photo_id}: {e}");
        }
    }

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

    Ok(warnings)
}

/// Run the organism gate and, if it passes, BioCLIP 2.
///
/// Returns the prediction plus the L2-normalized BioCLIP embedding, or `None`
/// for the vector when the gate rejected the photo — a rejected photo is still
/// *checked*, it just has nothing to store.
fn run_species(
    state: &AppState,
    options: &ParsedOptions,
    photo_id: &str,
    image_bytes: &[u8],
    siglip_embedding: Option<&[f32]>,
) -> Result<(lrg_ml::bioclip::TaxonomyPrediction, Option<Vec<f32>>), String> {
    // Checked before decoding, like the face path: without the model files the
    // classifier fails anyway and the decode would be pure waste.
    if !state.bioclip.is_cached() {
        // Names the fix, like the face path above.
        return Err(
            "species model is not downloaded yet — run \"Download AI models\" \
                    in Plug-in Manager and index these photos again"
                .to_string(),
        );
    }

    if options.species_prefilter {
        match siglip_embedding {
            Some(embedding) => {
                let score = crate::routes::route_util::score_prompt_set(
                    state,
                    lrg_ml::clip_iqa::PromptSet::Organism,
                    embedding,
                );
                // A missing score means the text tower would not load. Fall
                // through rather than skip: the gate is an optimization, and
                // failing it closed would silently lose species.
                if let Some(score) = score {
                    if score < options.species_prefilter_threshold {
                        log::debug!(
                            "Photo {photo_id}: organism gate {score:.2} < {:.2}, skipping BioCLIP",
                            options.species_prefilter_threshold
                        );
                        return Ok((
                            lrg_ml::bioclip::TaxonomyPrediction {
                                rank: None,
                                best: None,
                                alternatives: Vec::new(),
                            },
                            None,
                        ));
                    }
                }
            }
            // The fast `tasks=cull` ingest stores no embedding, so there is
            // nothing to gate on. Running BioCLIP is the safe direction.
            None => log::debug!(
                "Photo {photo_id}: no SigLIP embedding to gate on, running BioCLIP anyway"
            ),
        }
    }

    let t_decode = Instant::now();
    let decoded = image::load_from_memory(image_bytes)
        .map(|img| img.to_rgb8())
        .map_err(|e| format!("could not decode image: {e}"))?;
    let (width, height) = (decoded.width() as usize, decoded.height() as usize);
    let pixels = decoded.into_raw();
    log::debug!(
        "Photo {photo_id}: re-decode for species took {:?}",
        t_decode.elapsed()
    );

    let t0 = Instant::now();
    let mut vector = state
        .bioclip
        .embed_image(&pixels, width, height)
        .map_err(|e| e.to_string())?;
    lrg_ml::siglip::l2_normalize(&mut vector);
    let prediction = state
        .bioclip
        .classify(&vector, &options.species_classify)
        .map_err(|e| e.to_string())?;
    log::debug!("Photo {photo_id}: BioCLIP took {:?}", t0.elapsed());

    Ok((prediction, Some(vector)))
}

/// Fold a prediction into the photo's metadata blob.
///
/// Every key is written on every run, including the empty ones. Leaving stale
/// values behind when a re-run downgrades a confident species to "none" would
/// be worse than writing nothing at all — the plugin would keep showing an
/// identification the backend no longer stands behind.
fn apply_species_metadata(
    metadata: &mut Map<String, Value>,
    prediction: &lrg_ml::bioclip::TaxonomyPrediction,
    model: &lrg_ml::bioclip::BioclipModel,
) {
    let best = prediction.best.as_ref();
    metadata.insert("species_rank".into(), json!(prediction.rank_label()));
    metadata.insert(
        "species_taxonomy".into(),
        json!(best.map(|c| c.taxonomy.as_str()).unwrap_or("")),
    );
    metadata.insert(
        "species_scientific_name".into(),
        json!(best.map(|c| c.scientific_name.as_str()).unwrap_or("")),
    );
    metadata.insert(
        "species_common_name".into(),
        json!(best.map(|c| c.common_name.as_str()).unwrap_or("")),
    );
    metadata.insert(
        "species_confidence".into(),
        json!(best.map(|c| c.confidence).unwrap_or(0.0)),
    );
    metadata.insert(
        "species_alternatives".into(),
        json!(prediction
            .alternatives
            .iter()
            .map(|c| json!({
                "name": c.scientific_name,
                "common_name": c.common_name,
                "taxonomy": c.taxonomy,
                "confidence": c.confidence,
            }))
            .collect::<Vec<_>>()),
    );
    metadata.insert("species_checked".into(), json!(true));
    // `None` only before the head has ever loaded, which cannot happen on a
    // path that just classified something — but a gate-rejected photo takes
    // this branch without loading, and stamping nothing is correct there:
    // `check_unprocessed` then re-runs it once a head version is known.
    if let Some(id) = model.model_id() {
        metadata.insert("species_model".into(), json!(id));
    }
}

/// Cosine distance below which a re-detected face counts as the same face as
/// one of the photo's previous rows.
///
/// Deliberately far tighter than the clustering threshold (0.5). That one asks
/// "are these the same person"; this one asks "is this the same detection",
/// re-run on the same pixels through the same model, so a genuine match lands
/// near zero. Anything looser would hand one person's identity to a different
/// face in the same frame, which is worse than carrying nothing over.
const FACE_CARRY_OVER_MAX_DISTANCE: f64 = 0.2;

/// Re-attach the `person_id`s from a photo's previous FACE_TABLE rows to its
/// freshly detected faces, matched by nearest embedding.
///
/// Pairs are assigned globally best-first rather than in detection order, so
/// the result does not depend on which face the detector happened to report
/// first. Each old row is claimed at most once, so two faces in one frame
/// cannot both inherit the same identity. Faces with no match keep an empty
/// `person_id` and are picked up by the next clustering run.
fn carry_over_person_ids(previous: &[StoreRecord], embeddings: &[&[f32]]) -> Vec<String> {
    let mut out = vec![String::new(); embeddings.len()];
    if previous.is_empty() || embeddings.is_empty() {
        return out;
    }

    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, emb) in embeddings.iter().enumerate() {
        for (j, old) in previous.iter().enumerate() {
            let Some(old_vec) = old.vector.as_ref() else {
                continue;
            };
            // An empty assignment is what a fresh row already carries, so
            // matching against one buys nothing and would only consume a slot.
            if old
                .metadata
                .get("person_id")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                continue;
            }
            let distance = crate::routes::edit::cosine_distance(emb, old_vec);
            if distance <= FACE_CARRY_OVER_MAX_DISTANCE {
                pairs.push((distance, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut new_taken = vec![false; embeddings.len()];
    let mut old_taken = vec![false; previous.len()];
    for (_, i, j) in pairs {
        if new_taken[i] || old_taken[j] {
            continue;
        }
        if let Some(person) = previous[j]
            .metadata
            .get("person_id")
            .and_then(Value::as_str)
        {
            out[i] = person.to_string();
        }
        new_taken[i] = true;
        old_taken[j] = true;
    }
    out
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

    // Reading the EXIF location is what turns "this photo was taken at these
    // coordinates" into a place name in the prompt, so it is gated on the
    // user's choice rather than done unconditionally.
    let location_data = if mo.submit_gps {
        lrg_imaging::location::extract_location_tags(image_bytes)
    } else {
        None
    };
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
mod face_carry_over_tests {
    use super::*;

    fn face_row(id: &str, person: &str, vector: Vec<f32>) -> StoreRecord {
        let mut m = Map::new();
        m.insert("person_id".into(), json!(person));
        StoreRecord {
            id: id.to_string(),
            vector: Some(vector),
            metadata: m,
        }
    }

    /// The regression this exists for: re-indexing a photo replaces its face
    /// rows, and a fresh row starts with an empty `person_id`, so every named
    /// person in the catalog used to be lost on a plain re-index.
    #[test]
    fn a_redetected_face_keeps_its_person() {
        let previous = vec![face_row("p_0", "person_7", vec![1.0, 0.0, 0.0])];
        // Detection is not bit-identical across runs; a slightly shifted box
        // still has to match.
        let embedding = [0.999_f32, 0.044, 0.0];
        let carried = carry_over_person_ids(&previous, &[&embedding]);
        assert_eq!(carried, vec!["person_7".to_string()]);
    }

    #[test]
    fn a_different_face_inherits_nothing() {
        let previous = vec![face_row("p_0", "person_7", vec![1.0, 0.0, 0.0])];
        let embedding = [0.0_f32, 1.0, 0.0];
        assert_eq!(
            carry_over_person_ids(&previous, &[&embedding]),
            vec![String::new()]
        );
    }

    /// Two people in one frame must not both end up as the same person just
    /// because one old row happened to be the nearest match for both.
    #[test]
    fn one_old_row_can_only_be_claimed_once() {
        let previous = vec![face_row("p_0", "person_7", vec![1.0, 0.0, 0.0])];
        let a = [1.0_f32, 0.0, 0.0];
        let b = [0.995_f32, 0.1, 0.0];
        let carried = carry_over_person_ids(&previous, &[&a, &b]);
        assert_eq!(carried[0], "person_7", "the closer face wins the identity");
        assert_eq!(carried[1], "", "the other one starts unassigned");
    }

    /// Assignment is global-best-first, not detection order, so the result must
    /// not depend on which face the detector reported first.
    #[test]
    fn assignment_does_not_depend_on_detection_order() {
        let previous = vec![face_row("p_0", "person_7", vec![1.0, 0.0, 0.0])];
        let near = [1.0_f32, 0.0, 0.0];
        let far = [0.995_f32, 0.1, 0.0];

        let forward = carry_over_person_ids(&previous, &[&near, &far]);
        let reversed = carry_over_person_ids(&previous, &[&far, &near]);
        assert_eq!(forward[0], "person_7");
        assert_eq!(reversed[1], "person_7");
        assert_eq!(reversed[0], "");
    }

    #[test]
    fn unassigned_previous_rows_are_ignored() {
        let previous = vec![face_row("p_0", "", vec![1.0, 0.0, 0.0])];
        let embedding = [1.0_f32, 0.0, 0.0];
        assert_eq!(
            carry_over_person_ids(&previous, &[&embedding]),
            vec![String::new()]
        );
    }

    #[test]
    fn no_previous_rows_yields_empty_assignments() {
        let embedding = [1.0_f32, 0.0, 0.0];
        assert_eq!(
            carry_over_person_ids(&[], &[&embedding]),
            vec![String::new()]
        );
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
            exposure_bias: None,
            is_raw: None,
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

#[cfg(test)]
mod sharp_s_tests {
    use super::*;

    fn meta(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), json!(v)))
            .collect()
    }

    #[test]
    fn generated_text_is_converted() {
        // The whole point of the option, and the case the old placement in
        // `prepare_one` could never reach: these fields do not exist yet
        // before the LLM answers.
        let mut m = meta(&[
            ("title", "Straße bei Nacht"),
            ("caption", "Ein Fußgänger auf der Straße"),
            ("alt_text", "Große Straße"),
            ("flattened_keywords", "Straße, Fußweg"),
        ]);
        replace_sharp_s(&mut m);

        assert_eq!(m["title"], json!("Strasse bei Nacht"));
        assert_eq!(m["caption"], json!("Ein Fussgänger auf der Strasse"));
        assert_eq!(m["alt_text"], json!("Grosse Strasse"));
        assert_eq!(m["flattened_keywords"], json!("Strasse, Fussweg"));
    }

    #[test]
    fn the_keyword_tree_stays_valid_json() {
        let tree = json!({"Orte": ["Straße", "Fußweg"]}).to_string();
        let mut m = meta(&[("keywords", tree.as_str())]);
        replace_sharp_s(&mut m);

        let parsed: Value = serde_json::from_str(m["keywords"].as_str().unwrap())
            .expect("substitution must not break the serialized tree");
        assert_eq!(parsed, json!({"Orte": ["Strasse", "Fussweg"]}));
    }

    #[test]
    fn the_filename_is_left_alone() {
        // The old blanket loop over `main_metadata.values_mut()` rewrote the
        // stored filename too, so a photo called `Straße.jpg` was recorded
        // under a name no file on disk has.
        let mut m = meta(&[("filename", "Straße.jpg"), ("title", "Straße")]);
        replace_sharp_s(&mut m);

        assert_eq!(m["filename"], json!("Straße.jpg"));
        assert_eq!(m["title"], json!("Strasse"));
    }

    #[test]
    fn text_without_the_character_is_untouched() {
        let mut m = meta(&[("title", "Bridge at night")]);
        replace_sharp_s(&mut m);
        assert_eq!(m["title"], json!("Bridge at night"));
    }
}

#[cfg(test)]
mod image_source_tests {
    use super::*;

    #[test]
    fn into_pending_preserves_order_for_both_variants() {
        let loaded = ImageSource::Loaded(vec![
            UploadedImage {
                bytes: vec![1, 2, 3],
                filename: "a.jpg".into(),
            },
            UploadedImage {
                bytes: vec![4, 5],
                filename: "b.jpg".into(),
            },
        ])
        .into_pending();
        assert_eq!(loaded.len(), 2);
        let PendingImage::Loaded(first) = &loaded[0] else {
            panic!("multipart images stay in memory")
        };
        assert_eq!(first.filename, "a.jpg", "order must survive the split");

        let paths =
            ImageSource::Paths(vec!["/one.cr3".to_string(), "/two.cr3".to_string()]).into_pending();
        let PendingImage::Path(second) = &paths[1] else {
            panic!("by-reference images stay unread")
        };
        assert_eq!(second, "/two.cr3");
    }

    #[tokio::test]
    async fn a_path_is_read_lazily_and_keeps_only_its_filename() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("Straße.jpg");
        std::fs::write(&path, b"jpegbytes").expect("write");

        let img = load_pending(PendingImage::Path(path.to_string_lossy().to_string()))
            .await
            .expect("existing file reads");
        assert_eq!(img.bytes, b"jpegbytes");
        assert_eq!(img.filename, "Straße.jpg", "filename, not the whole path");
    }

    #[tokio::test]
    async fn a_missing_file_is_an_error_for_that_photo_only() {
        // It used to be an anonymous string in `pre_failures` with no photo_id,
        // so the plugin could not tell which photo to retry.
        let Err(err) = load_pending(PendingImage::Path("/nonexistent/nope.cr3".to_string())).await
        else {
            panic!("a missing file must not read successfully")
        };
        assert!(
            err.contains("File not found") && err.contains("nope.cr3"),
            "the message has to name the file, got {err:?}"
        );
    }

    #[tokio::test]
    async fn concurrent_loads_stay_in_batch_order_around_a_failure() {
        // The whole per-photo error attribution downstream rests on `buffered`
        // handing results back in submission order, not completion order.
        let dir = tempfile::tempdir().expect("temp dir");
        let first = dir.path().join("first.jpg");
        let third = dir.path().join("third.jpg");
        std::fs::write(&first, b"one").expect("write");
        std::fs::write(&third, b"three").expect("write");

        let pending = vec![
            PendingImage::Path(first.to_string_lossy().to_string()),
            PendingImage::Path("/nonexistent/second.cr3".to_string()),
            PendingImage::Path(third.to_string_lossy().to_string()),
        ];

        let loaded: Vec<Result<UploadedImage, String>> = futures_util::stream::iter(pending)
            .map(load_pending)
            .buffered(3)
            .collect()
            .await;

        assert_eq!(loaded[0].as_ref().expect("first reads").bytes, b"one");
        let Err(second) = &loaded[1] else {
            panic!("the middle photo must not read successfully")
        };
        assert!(
            second.contains("second.cr3"),
            "the middle photo keeps its own failure, got {second:?}"
        );
        assert_eq!(loaded[2].as_ref().expect("third reads").bytes, b"three");
    }

    #[test]
    fn concurrency_override_is_honoured_but_never_zero() {
        assert_eq!(
            concurrency_from(Some("1".into())),
            1,
            "1 restores sequential"
        );
        assert_eq!(concurrency_from(Some(" 8 ".into())), 8);

        // A bad value must not become a batch that never decodes anything.
        for bad in ["0", "-2", "lots", ""] {
            assert!(
                concurrency_from(Some(bad.into())) >= 1,
                "{bad:?} must fall back to the default"
            );
        }
        assert!(concurrency_from(None) >= 1);
    }
}
