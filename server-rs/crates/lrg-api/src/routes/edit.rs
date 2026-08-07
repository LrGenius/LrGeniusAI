//! `/edit` and `/edit_base64` — port of `routes/edit.py`: single-photo
//! LLM-backed Lightroom edit-recipe generation across all four providers,
//! few-shot injection from the user's own training examples (best-effort,
//! brute-force cosine search over the small `edit_training` table — same
//! pattern as `faces.rs`'s face-similarity query), per-control filtering,
//! and persistence into the photo's existing metadata record.
//!
//! Note: mirrors the real Python behavior, not the aspirational one —
//! `_extract_options` never populates `location_data` for this endpoint
//! (only the `/index` batch path wires EXIF location through), so the
//! edit prompt's "Photo taken in: ..." context is deliberately never
//! filled here either, matching production.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Multipart, State};
use axum::response::{IntoResponse, Json, Response};
use base64::Engine;
use chrono::Utc;
use serde_json::{json, Map, Value};

use lrg_providers::provider::{build_provider, ProviderSelection};
use lrg_providers::types::{EditGenerationRequest, EditGenerationResponse};
use lrg_store::{meta, Store, StoreRecord, IMAGE_TABLE, TRAINING_TABLE};

use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route("/edit", axum::routing::post(edit_multipart))
        .route("/edit_base64", axum::routing::post(edit_base64))
}

pub(crate) struct EditOptions {
    provider: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    language: String,
    temperature: f64,
    max_tokens: Option<u32>,
    prompt: Option<String>,
    submit_keywords: bool,
    submit_folder_names: bool,
    existing_keywords: Option<Vec<String>>,
    folder_names: Option<String>,
    user_context: Option<String>,
    date_time: Option<String>,
    edit_intent: Option<String>,
    style_strength: f64,
    include_masks: bool,
    adjust_white_balance: bool,
    adjust_basic_tone: bool,
    adjust_presence: bool,
    adjust_color_mix: bool,
    do_color_grading: bool,
    use_tone_curve: bool,
    use_point_curve: bool,
    adjust_detail: bool,
    adjust_effects: bool,
    adjust_lens_corrections: bool,
    allow_auto_crop: bool,
    composition_mode: String,
    ollama_base_url: Option<String>,
    lmstudio_base_url: Option<String>,
    catalog_id: Option<String>,
    use_training_style: bool,
}

impl Default for EditOptions {
    fn default() -> Self {
        EditOptions {
            provider: None,
            model: None,
            api_key: None,
            language: "German".to_string(),
            temperature: 0.2,
            max_tokens: None,
            prompt: None,
            submit_keywords: false,
            submit_folder_names: false,
            existing_keywords: None,
            folder_names: None,
            user_context: None,
            date_time: None,
            edit_intent: None,
            style_strength: 0.5,
            include_masks: true,
            adjust_white_balance: true,
            adjust_basic_tone: true,
            adjust_presence: true,
            adjust_color_mix: true,
            do_color_grading: true,
            use_tone_curve: true,
            use_point_curve: true,
            adjust_detail: true,
            adjust_effects: true,
            adjust_lens_corrections: true,
            allow_auto_crop: true,
            composition_mode: "subtle".to_string(),
            ollama_base_url: None,
            lmstudio_base_url: None,
            catalog_id: None,
            use_training_style: true,
        }
    }
}

fn bool_field(fields: &HashMap<String, String>, key: &str, default: bool) -> bool {
    fields
        .get(key)
        .map(|v| v.to_lowercase() == "true")
        .unwrap_or(default)
}

pub(crate) fn parse_edit_options_form(fields: &HashMap<String, String>) -> EditOptions {
    let defaults = EditOptions::default();
    let existing_keywords = fields.get("existing_keywords").map(|raw| {
        serde_json::from_str::<Vec<String>>(raw).unwrap_or_else(|_| {
            raw.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
    });
    let composition_mode = fields
        .get("composition_mode")
        .map(|s| s.to_lowercase())
        .filter(|s| matches!(s.as_str(), "none" | "subtle" | "aggressive"))
        .unwrap_or(defaults.composition_mode);
    let style_strength = fields
        .get("style_strength")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(defaults.style_strength)
        .clamp(0.0, 1.0);

    EditOptions {
        provider: fields.get("provider").cloned(),
        model: fields.get("model").cloned(),
        api_key: fields.get("api_key").cloned(),
        language: fields.get("language").cloned().unwrap_or(defaults.language),
        temperature: fields
            .get("temperature")
            .and_then(|s| s.parse().ok())
            .unwrap_or(defaults.temperature),
        max_tokens: fields.get("max_tokens").and_then(|s| s.parse().ok()),
        prompt: fields.get("prompt").cloned(),
        submit_keywords: bool_field(fields, "submit_keywords", false),
        submit_folder_names: bool_field(fields, "submit_folder_names", false),
        existing_keywords,
        folder_names: fields.get("folder_names").cloned(),
        user_context: fields.get("user_context").cloned(),
        date_time: fields.get("date_time").cloned(),
        edit_intent: fields.get("edit_intent").cloned(),
        style_strength,
        include_masks: bool_field(fields, "include_masks", true),
        adjust_white_balance: bool_field(fields, "adjust_white_balance", true),
        adjust_basic_tone: bool_field(fields, "adjust_basic_tone", true),
        adjust_presence: bool_field(fields, "adjust_presence", true),
        adjust_color_mix: bool_field(fields, "adjust_color_mix", true),
        do_color_grading: bool_field(fields, "do_color_grading", true),
        use_tone_curve: bool_field(fields, "use_tone_curve", true),
        use_point_curve: bool_field(fields, "use_point_curve", true),
        adjust_detail: bool_field(fields, "adjust_detail", true),
        adjust_effects: bool_field(fields, "adjust_effects", true),
        adjust_lens_corrections: bool_field(fields, "adjust_lens_corrections", true),
        allow_auto_crop: bool_field(fields, "allow_auto_crop", true),
        composition_mode,
        ollama_base_url: fields.get("ollama_base_url").cloned(),
        lmstudio_base_url: fields.get("lmstudio_base_url").cloned(),
        catalog_id: fields
            .get("catalog_id")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        use_training_style: bool_field(fields, "use_training_style", true),
    }
}

fn parse_edit_options_json(data: &Value) -> EditOptions {
    let defaults = EditOptions::default();
    let get_str = |key: &str| data.get(key).and_then(Value::as_str).map(str::to_string);
    let get_bool =
        |key: &str, default: bool| data.get(key).and_then(Value::as_bool).unwrap_or(default);

    let existing_keywords = data.get("existing_keywords").and_then(|v| match v {
        Value::Array(arr) => Some(
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
        ),
        Value::String(s) => Some(
            s.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        ),
        _ => None,
    });
    let composition_mode = get_str("composition_mode")
        .map(|s| s.to_lowercase())
        .filter(|s| matches!(s.as_str(), "none" | "subtle" | "aggressive"))
        .unwrap_or(defaults.composition_mode);
    let style_strength = data
        .get("style_strength")
        .and_then(Value::as_f64)
        .unwrap_or(defaults.style_strength)
        .clamp(0.0, 1.0);

    EditOptions {
        provider: get_str("provider"),
        model: get_str("model"),
        api_key: get_str("api_key"),
        language: get_str("language").unwrap_or(defaults.language),
        temperature: data
            .get("temperature")
            .and_then(Value::as_f64)
            .unwrap_or(defaults.temperature),
        max_tokens: data
            .get("max_tokens")
            .and_then(Value::as_u64)
            .map(|v| v as u32),
        prompt: get_str("prompt"),
        submit_keywords: get_bool("submit_keywords", false),
        submit_folder_names: get_bool("submit_folder_names", false),
        existing_keywords,
        folder_names: get_str("folder_names"),
        user_context: get_str("user_context"),
        date_time: get_str("date_time"),
        edit_intent: get_str("edit_intent"),
        style_strength,
        include_masks: get_bool("include_masks", true),
        adjust_white_balance: get_bool("adjust_white_balance", true),
        adjust_basic_tone: get_bool("adjust_basic_tone", true),
        adjust_presence: get_bool("adjust_presence", true),
        adjust_color_mix: get_bool("adjust_color_mix", true),
        do_color_grading: get_bool("do_color_grading", true),
        use_tone_curve: get_bool("use_tone_curve", true),
        use_point_curve: get_bool("use_point_curve", true),
        adjust_detail: get_bool("adjust_detail", true),
        adjust_effects: get_bool("adjust_effects", true),
        adjust_lens_corrections: get_bool("adjust_lens_corrections", true),
        allow_auto_crop: get_bool("allow_auto_crop", true),
        composition_mode,
        ollama_base_url: get_str("ollama_base_url"),
        lmstudio_base_url: get_str("lmstudio_base_url"),
        catalog_id: get_str("catalog_id")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        use_training_style: get_bool("use_training_style", true),
    }
}

fn controls_map(options: &EditOptions) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("include_masks".into(), json!(options.include_masks));
    m.insert(
        "adjust_white_balance".into(),
        json!(options.adjust_white_balance),
    );
    m.insert("adjust_basic_tone".into(), json!(options.adjust_basic_tone));
    m.insert("adjust_presence".into(), json!(options.adjust_presence));
    m.insert("adjust_color_mix".into(), json!(options.adjust_color_mix));
    m.insert("do_color_grading".into(), json!(options.do_color_grading));
    m.insert("use_tone_curve".into(), json!(options.use_tone_curve));
    m.insert("use_point_curve".into(), json!(options.use_point_curve));
    m.insert("adjust_detail".into(), json!(options.adjust_detail));
    m.insert("adjust_effects".into(), json!(options.adjust_effects));
    m.insert(
        "adjust_lens_corrections".into(),
        json!(options.adjust_lens_corrections),
    );
    m.insert("allow_auto_crop".into(), json!(options.allow_auto_crop));
    m.insert("composition_mode".into(), json!(options.composition_mode));
    m
}

pub(crate) fn cosine_distance(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
    let na: f64 = a
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt();
    let nb: f64 = b
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt();
    if na <= 0.0 || nb <= 0.0 {
        1.0
    } else {
        1.0 - dot / (na * nb)
    }
}

/// Best-effort port of `query_similar_training_examples`: reuse the
/// photo's own stored CLIP embedding (skip silently if absent — no image
/// re-embedding here, same as Python's best-effort try/except), brute-force
/// cosine-rank the (typically small) `edit_training` table, and shape the
/// top matches as the few-shot JSON `prompts::format_training_example` expects.
pub(crate) async fn fetch_training_examples(
    store: &Store,
    photo_id: &str,
    n_results: usize,
) -> Vec<Value> {
    let Ok(mut existing) = store.get(IMAGE_TABLE, &[photo_id.to_string()]).await else {
        return Vec::new();
    };
    let Some(query_embedding) = existing.pop().and_then(|r| r.vector) else {
        return Vec::new();
    };

    let Ok(records) = store.scan_all(TRAINING_TABLE).await else {
        return Vec::new();
    };
    let mut scored: Vec<(f64, StoreRecord)> = records
        .into_iter()
        .filter_map(|r| {
            let v = r.vector.clone()?;
            Some((cosine_distance(&query_embedding, &v), r))
        })
        .collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    scored.truncate(n_results);

    scored
        .into_iter()
        .map(|(_, r)| {
            let develop_settings = r
                .metadata
                .get("develop_settings")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or_else(|| json!({}));
            json!({
                "label": r.metadata.get("label").cloned().unwrap_or(json!("")),
                "filename": r.metadata.get("filename").cloned().unwrap_or(json!("")),
                "summary": r.metadata.get("summary").cloned().unwrap_or(json!("")),
                "develop_settings": develop_settings,
            })
        })
        .collect()
}

pub(crate) async fn generate_edit_recipe_for_photo(
    state: &AppState,
    store: Option<&Store>,
    options: &EditOptions,
    image_bytes: &[u8],
    photo_id: &str,
) -> EditGenerationResponse {
    let provider = options
        .provider
        .as_deref()
        .unwrap_or("ollama")
        .to_lowercase();
    let model = options.model.clone().unwrap_or_default();

    let mut request = EditGenerationRequest::new(
        image_bytes.to_vec(),
        photo_id.to_string(),
        provider.clone(),
        model,
    );
    request.api_key = options.api_key.clone();
    request.language = options.language.clone();
    request.temperature = options.temperature;
    request.max_tokens = options.max_tokens;
    request.system_prompt = options.prompt.clone();
    request.submit_keywords = options.submit_keywords;
    request.submit_folder_names = options.submit_folder_names;
    request.existing_keywords = options.existing_keywords.clone();
    request.folder_names = options.folder_names.clone();
    request.user_context = options.user_context.clone();
    request.date_time = options.date_time.clone();
    request.edit_intent = options.edit_intent.clone();
    request.style_strength = options.style_strength;
    request.include_masks = options.include_masks;
    request.adjust_white_balance = options.adjust_white_balance;
    request.adjust_basic_tone = options.adjust_basic_tone;
    request.adjust_presence = options.adjust_presence;
    request.adjust_color_mix = options.adjust_color_mix;
    request.do_color_grading = options.do_color_grading;
    request.use_tone_curve = options.use_tone_curve;
    request.use_point_curve = options.use_point_curve;
    request.adjust_detail = options.adjust_detail;
    request.adjust_effects = options.adjust_effects;
    request.adjust_lens_corrections = options.adjust_lens_corrections;
    request.allow_auto_crop = options.allow_auto_crop;
    request.composition_mode = options.composition_mode.clone();
    request.ollama_base_url = options.ollama_base_url.clone();
    request.lmstudio_base_url = options.lmstudio_base_url.clone();

    if options.use_training_style {
        if let Some(store) = store {
            request.training_examples = fetch_training_examples(store, photo_id, 3).await;
        }
    }

    let local_engine =
        match crate::routes::llm::engine_for_request(state, &provider, &request.model).await {
            Ok(engine) => engine,
            Err(e) => return edit_fail(photo_id, e),
        };
    let mut response = match build_provider(&ProviderSelection {
        local_engine,
        name: provider,
        api_key: options.api_key.clone(),
        ollama_base_url: options.ollama_base_url.clone(),
        lmstudio_base_url: options.lmstudio_base_url.clone(),
    }) {
        Ok(client) => client.generate_edit_recipe(&request).await,
        Err(e) => edit_fail(photo_id, e),
    };

    if response.success {
        if let Some(recipe) = response.recipe.take() {
            let controls = controls_map(options);
            response.recipe = Some(lrg_providers::edit_recipe::filter_edit_recipe_by_controls(
                &recipe, &controls,
            ));
        }
    }
    response
}

fn edit_fail(uuid: &str, error: String) -> EditGenerationResponse {
    EditGenerationResponse {
        uuid: uuid.to_string(),
        success: false,
        error: Some(error),
        ..Default::default()
    }
}

/// Port of `_persist_edit_recipe`: merge the recipe into the photo's
/// existing metadata record (preserving its embedding untouched), or
/// create a metadata-only record if the photo was never indexed.
pub(crate) async fn persist_edit_recipe(
    store: &Store,
    photo_id: &str,
    filename: Option<&str>,
    recipe: &Value,
    options: &EditOptions,
) -> Result<(), String> {
    let existing = store
        .get(IMAGE_TABLE, &[photo_id.to_string()])
        .await
        .ok()
        .and_then(|mut v| v.pop());

    let mut metadata = existing
        .as_ref()
        .map(|r| r.metadata.clone())
        .unwrap_or_default();
    let existing_vector = existing.and_then(|r| r.vector);

    if let Some(f) = filename {
        metadata.insert("filename".into(), json!(f));
    }
    metadata.insert("edit_recipe".into(), json!(recipe.to_string()));
    metadata.insert(
        "edit_summary".into(),
        recipe.get("summary").cloned().unwrap_or(json!("")),
    );
    metadata.insert(
        "edit_warnings".into(),
        json!(recipe
            .get("warnings")
            .cloned()
            .unwrap_or(json!([]))
            .to_string()),
    );
    let edit_model = options
        .model
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            metadata
                .get("edit_model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            metadata
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    if let Some(m) = edit_model {
        metadata.insert("edit_model".into(), json!(m));
    }
    if let Some(p) = &options.provider {
        metadata.insert("edit_provider".into(), json!(p));
    }
    metadata.insert(
        "edit_run_date".into(),
        json!(Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()),
    );
    metadata
        .entry("provider")
        .or_insert(json!(options.provider));
    metadata.entry("model").or_insert(json!(options.model));
    let has_embedding_existing = metadata
        .get("has_embedding")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    metadata
        .entry("has_embedding")
        .or_insert(json!(has_embedding_existing));

    meta::ensure_photo_metadata(photo_id, &mut metadata);

    if let Some(catalog_id) = &options.catalog_id {
        let mut ids_set = meta::parse_catalog_ids(&metadata);
        ids_set.insert(catalog_id.clone());
        metadata.insert(
            meta::CATALOG_IDS_FIELD.into(),
            json!(meta::serialize_catalog_ids(&ids_set)),
        );
    }

    let record = StoreRecord {
        id: photo_id.to_string(),
        vector: existing_vector,
        metadata,
    };
    store
        .upsert(IMAGE_TABLE, std::slice::from_ref(&record))
        .await
        .map_err(|e| e.to_string())
}

pub(crate) fn success_payload(
    photo_id: &str,
    recipe: &Value,
    options: &EditOptions,
    warning: Option<&str>,
) -> Value {
    let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut payload = json!({
        "status": "success",
        "photo_id": photo_id,
        "uuid": photo_id,
        "edit": recipe,
        "edit_summary": recipe.get("summary").cloned().unwrap_or(json!("")),
        "edit_warnings": recipe.get("warnings").cloned().unwrap_or(json!([])),
        "edit_model": options.model,
        "edit_rundate": now,
    });
    if let Some(w) = warning {
        payload["warning"] = json!(w);
    }
    payload
}

async fn edit_multipart(State(state): State<Arc<AppState>>, mut multipart: Multipart) -> Response {
    log::info!("Edit recipe request received");

    let mut image_bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut photo_ids: Vec<String> = Vec::new();
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
                filename = field.file_name().map(str::to_string);
                if let Ok(bytes) = field.bytes().await {
                    image_bytes = Some(bytes.to_vec());
                }
            }
            "photo_id" | "uuid" => {
                if let Ok(text) = field.text().await {
                    if !text.is_empty() {
                        photo_ids.push(text);
                    }
                }
            }
            _ => {
                if let Ok(text) = field.text().await {
                    fields.insert(name, text);
                }
            }
        }
    }

    if let Some(db_path) = fields.get("db_path") {
        if let Err(e) = state.ensure_db_path(db_path).await {
            log::error!("Auto-bind to db_path {db_path} failed: {e}");
        }
    }

    let Some(image_bytes) = image_bytes else {
        return Json(json!({"error": "Mismatch between number of images and photo IDs, or no images provided"}))
            .into_response();
    };
    if photo_ids.len() != 1 {
        return Json(json!({"error": "Mismatch between number of images and photo IDs, or no images provided"}))
            .into_response();
    }
    let photo_id = &photo_ids[0];

    let options = parse_edit_options_form(&fields);
    finish_edit(
        &state,
        &options,
        &image_bytes,
        photo_id,
        filename.as_deref(),
    )
    .await
}

async fn edit_base64(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    log::info!("Edit recipe base64 request received");
    let data = body.map(|Json(v)| v).unwrap_or(Value::Null);

    let image_b64 = data.get("image").and_then(Value::as_str);
    let photo_id = data
        .get("photo_id")
        .and_then(Value::as_str)
        .or_else(|| data.get("uuid").and_then(Value::as_str));
    let filename = data.get("filename").and_then(Value::as_str);

    let (Some(image_b64), Some(photo_id), Some(_filename)) = (image_b64, photo_id, filename) else {
        return Json(json!({"error": "Missing required fields: image, photo_id, filename"}))
            .into_response();
    };
    let Ok(image_bytes) = base64::engine::general_purpose::STANDARD.decode(image_b64) else {
        return Json(json!({"error": "Missing required fields: image, photo_id, filename"}))
            .into_response();
    };

    let options = parse_edit_options_json(&data);
    finish_edit(&state, &options, &image_bytes, photo_id, filename).await
}

async fn finish_edit(
    state: &Arc<AppState>,
    options: &EditOptions,
    image_bytes: &[u8],
    photo_id: &str,
    filename: Option<&str>,
) -> Response {
    let store = state.store();
    let response =
        generate_edit_recipe_for_photo(state, store.as_deref(), options, image_bytes, photo_id)
            .await;

    let Some(recipe) = response.recipe.filter(|_| response.success) else {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "error", "error": response.error.unwrap_or_else(|| "Edit generation failed".to_string())})),
        )
            .into_response();
    };

    if let Some(store) = &store {
        if let Err(e) = persist_edit_recipe(store, photo_id, filename, &recipe, options).await {
            log::error!("Failed to persist edit recipe for {photo_id}: {e}");
        }
    }

    let mut payload = success_payload(photo_id, &recipe, options, response.warning.as_deref());
    payload["input_tokens"] = json!(response.input_tokens);
    payload["output_tokens"] = json!(response.output_tokens);
    Json(payload).into_response()
}
