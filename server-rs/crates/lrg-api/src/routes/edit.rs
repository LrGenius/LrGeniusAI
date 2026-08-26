//! `/v1/edit/recipe` and `/v1/edit/recipe/base64` — port of `routes/edit.py`: single-photo
//! LLM-backed Lightroom edit-recipe generation across all four providers,
//! few-shot injection from the user's own training examples (best-effort,
//! brute-force cosine search over the small `edit_training` table — same
//! pattern as `faces.rs`'s face-similarity query), per-control filtering,
//! and persistence into the photo's existing metadata record.
//!
//! Note: mirrors the real Python behavior, not the aspirational one —
//! `_extract_options` never populates `location_data` for this endpoint
//! (only the `/v1/index/photos` batch path wires EXIF location through), so the
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

use crate::routes::route_util::{parse_multipart, SinglePhotoForm};
use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route("/edit/recipe", axum::routing::post(edit_multipart))
        .route("/edit/recipe/base64", axum::routing::post(edit_base64))
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
    /// Face tags, kept apart from `existing_keywords` — see
    /// [`lrg_providers::types::MetadataGenerationRequest::existing_face_tags`].
    existing_face_tags: Option<Vec<String>>,
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
    /// Engine tuning from the plugin's advanced fields; see `ParsedOptions`.
    engine: crate::llm_engine::EngineOverrides,
    catalog_id: Option<String>,
    use_training_style: bool,
    /// Whether the photo this edit is for is a raw file.
    ///
    /// Two consumers: the edit guardrails, to decide whether blown highlights
    /// have anything behind them, and the `temperature` scale — Lightroom
    /// exposes Kelvin for raw and a relative -100..100 for everything else.
    /// It cannot be recovered from the bytes the backend receives — those have
    /// already been normalised to JPEG — so the plugin sends it, and the
    /// filename is the fallback.
    pub(crate) is_raw: Option<bool>,
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
            existing_face_tags: None,
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
            is_raw: None,
            engine: crate::llm_engine::EngineOverrides::default(),
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
    let parse_terms = |key: &str| {
        fields.get(key).map(|raw| {
            serde_json::from_str::<Vec<String>>(raw).unwrap_or_else(|_| {
                raw.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
        })
    };
    let existing_keywords = parse_terms("existing_keywords");
    let existing_face_tags = parse_terms("existing_face_tags");
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
        existing_face_tags,
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
        engine: crate::routes::llm::engine_overrides_from_fields(fields),
        catalog_id: fields
            .get("catalog_id")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        use_training_style: bool_field(fields, "use_training_style", true),
        // Absent means "unknown", which is not the same as false: the plugin
        // may predate this field, and guessing raw for it would hand a JPEG
        // highlight headroom it does not have.
        is_raw: fields.get("is_raw").map(|v| v.to_lowercase() == "true"),
    }
}

fn parse_edit_options_json(data: &Value) -> EditOptions {
    let defaults = EditOptions::default();
    let get_str = |key: &str| data.get(key).and_then(Value::as_str).map(str::to_string);
    let get_bool =
        |key: &str, default: bool| data.get(key).and_then(Value::as_bool).unwrap_or(default);

    let json_terms = |key: &str| {
        data.get(key).and_then(|v| match v {
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
        })
    };
    let existing_keywords = json_terms("existing_keywords");
    let existing_face_tags = json_terms("existing_face_tags");
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
        engine: crate::routes::llm::engine_overrides_from_json(data),
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
        existing_face_tags,
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
        is_raw: data.get("is_raw").and_then(Value::as_bool),
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
/// Why a training-style request came back with nothing, for the user.
///
/// "Learn from my edits" silently doing nothing is worse than it not being
/// offered: the edit still arrives, just without the style the user asked for,
/// and there was no way to tell the two apart.
pub(crate) enum NoTrainingExamples {
    /// The photo has never been indexed, so there is no embedding to match
    /// training examples against.
    PhotoNotIndexed,
    /// The photo is fine; the user has not saved any usable examples yet.
    NoneStored,
}

impl NoTrainingExamples {
    pub(crate) fn message(&self) -> &'static str {
        match self {
            NoTrainingExamples::PhotoNotIndexed => {
                "Style matching was skipped: this photo is not indexed yet, so there is no \
                 embedding to compare your saved edits against. Run Analyze & Index with \
                 \"Enable smart photo search\" on it first."
            }
            NoTrainingExamples::NoneStored => {
                "Style matching was skipped: no usable training examples are stored yet. \
                 Use \"Save Edits as AI Training Examples\" on photos you have edited yourself."
            }
        }
    }
}

/// Lightroom's own key for white balance in a saved develop-settings blob, and
/// the recipe schema's spelling of the same thing, since examples can come from
/// either shape.
const TEMPERATURE_KEYS: [&str; 2] = ["Temp", "temperature"];

/// Strip the white balance from an example saved from a file whose temperature
/// scale differs from the photo being edited.
///
/// The few-shot block exists to give the model the user's own numbers to anchor
/// on. A Kelvin `Temp` offered as a reference for a JPEG — or the reverse —
/// anchors it on a number that is meaningless for the target, while the schema
/// it must answer in declares the other range entirely. Every other develop
/// setting means the same thing on both kinds of file, so only this one goes.
///
/// An unknown raw status on either side means no conflict can be established,
/// so nothing is removed.
fn drop_incompatible_temperature(
    settings: &mut Value,
    example_is_raw: Option<bool>,
    target_is_raw: Option<bool>,
) -> bool {
    let (Some(example), Some(target)) = (example_is_raw, target_is_raw) else {
        return false;
    };
    if example == target {
        return false;
    }
    let Some(map) = settings.as_object_mut() else {
        return false;
    };
    let mut removed = false;
    for key in TEMPERATURE_KEYS {
        removed |= map.remove(key).is_some();
    }
    removed
}

pub(crate) async fn fetch_training_examples(
    store: &Store,
    photo_id: &str,
    n_results: usize,
    target_is_raw: Option<bool>,
) -> Result<Vec<Value>, NoTrainingExamples> {
    let Ok(mut existing) = store.get(IMAGE_TABLE, &[photo_id.to_string()]).await else {
        return Err(NoTrainingExamples::PhotoNotIndexed);
    };
    let Some(query_embedding) = existing.pop().and_then(|r| r.vector) else {
        return Err(NoTrainingExamples::PhotoNotIndexed);
    };

    let Ok(records) = store.scan_all(TRAINING_TABLE).await else {
        return Err(NoTrainingExamples::NoneStored);
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
    if scored.is_empty() {
        return Err(NoTrainingExamples::NoneStored);
    }

    let mut dropped_temperature = 0usize;
    let examples: Vec<Value> = scored
        .into_iter()
        .map(|(_, r)| {
            let mut develop_settings = r
                .metadata
                .get("develop_settings")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or_else(|| json!({}));
            let example_is_raw = r.metadata.get("is_raw").and_then(Value::as_bool);
            if drop_incompatible_temperature(&mut develop_settings, example_is_raw, target_is_raw) {
                dropped_temperature += 1;
            }
            json!({
                "label": r.metadata.get("label").cloned().unwrap_or(json!("")),
                "filename": r.metadata.get("filename").cloned().unwrap_or(json!("")),
                "summary": r.metadata.get("summary").cloned().unwrap_or(json!("")),
                "develop_settings": develop_settings,
            })
        })
        .collect();
    if dropped_temperature > 0 {
        log::debug!(
            "Photo {photo_id}: dropped the white balance from {dropped_temperature} few-shot example(s) saved from files on the other temperature scale."
        );
    }
    Ok(examples)
}

pub(crate) async fn generate_edit_recipe_for_photo(
    state: &AppState,
    store: Option<&Store>,
    options: &EditOptions,
    image_bytes: &[u8],
    photo_id: &str,
    filename: Option<&str>,
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
    request.existing_face_tags = options.existing_face_tags.clone();
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
    // Decides which `temperature` scale the schema declares and the
    // normalizer clamps to: Kelvin for raw, -100..100 for everything else.
    request.is_raw = options.is_raw;

    // Surfaced rather than swallowed: the plugin already renders `warning`,
    // and "learn from my edits" quietly falling back to the generic prompt is
    // exactly the kind of silent no-op this endpoint should not have.
    let mut training_warning: Option<&'static str> = None;
    if options.use_training_style {
        if let Some(store) = store {
            match fetch_training_examples(store, photo_id, 3, options.is_raw).await {
                Ok(examples) => request.training_examples = examples,
                Err(reason) => training_warning = Some(reason.message()),
            }
        }
    }

    let local_engine = match crate::routes::llm::engine_for_request(
        state,
        &provider,
        &request.model,
        options.engine,
    )
    .await
    {
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
        if let Some(mut recipe) = response.recipe.take() {
            // Guardrails before the control filter, not after: a move the user
            // switched off must not first be scaled and then discarded, which
            // would spend budget on a field that never reaches the photo and
            // shrink the ones that do.
            response.guardrail_reasons = crate::edit_budget::measure_and_apply(
                &mut recipe,
                image_bytes,
                &capture_conditions(options, filename, image_bytes),
            );
            if !response.guardrail_reasons.is_empty() {
                log::info!(
                    "Photo {photo_id}: edit constrained by the frame ({}).",
                    response.guardrail_reasons.join(", ")
                );
            }
            let controls = controls_map(options);
            response.recipe = Some(lrg_providers::edit_recipe::filter_edit_recipe_by_controls(
                &recipe, &controls,
            ));
        }
    }
    // Appended rather than assigned: a provider-level warning says something
    // about the edit that was produced, and losing it to say the style was
    // skipped would trade one silent failure for another.
    if let Some(msg) = training_warning {
        response.warning = Some(match response.warning.take() {
            Some(existing) => format!("{existing}\n{msg}"),
            None => msg.to_string(),
        });
    }
    response
}

/// What the camera was doing, as far as the guardrails care.
///
/// `is_raw` prefers what the plugin said, because it reads Lightroom's own
/// `fileFormat` and is therefore authoritative. The filename is the fallback
/// for older plugins and for `/v1/edit/recipe/base64` callers that send neither — it is a
/// heuristic, and one whose unknown case reads as *not* raw, so a frame that
/// cannot be placed gets the conservative budget.
pub(crate) fn capture_conditions(
    options: &EditOptions,
    filename: Option<&str>,
    image_bytes: &[u8],
) -> lrg_analysis::edit_guardrails::CaptureConditions {
    lrg_analysis::edit_guardrails::CaptureConditions {
        iso: lrg_imaging::capture::read_iso(image_bytes),
        is_raw: options
            .is_raw
            .unwrap_or_else(|| filename.is_some_and(lrg_imaging::capture::is_raw_filename)),
    }
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
    // `json!(None::<String>)` is JSON `null`, not an absent field, so the
    // unconditional insert stored `"provider": null` on every edit made
    // without an explicit provider — a value every reader then has to special
    // case. Leave the field out instead.
    if let Some(provider) = &options.provider {
        metadata
            .entry("provider")
            .or_insert_with(|| json!(provider));
    }
    if let Some(model) = &options.model {
        metadata.entry("model").or_insert_with(|| json!(model));
    }
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
    guardrail_reasons: &[String],
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
        // Separate from `edit_warnings`, which are the model's own remarks
        // about what it could not do. These are the backend's, about what the
        // photograph would not take.
        "guardrail_reasons": guardrail_reasons,
        "guardrail_explanations": guardrail_explanations(guardrail_reasons),
    });
    if let Some(w) = warning {
        payload["warning"] = json!(w);
    }
    payload
}

/// The user-facing sentence for each reason code.
///
/// Resolved here rather than in the plugin so the wording lives next to the
/// rule that produces it — `GuardrailReason::explanation` is written for a
/// photographer, and a second copy in Lua would drift from the thresholds it
/// describes. Unknown codes are dropped rather than echoed: a plugin talking to
/// a newer backend should show nothing rather than a bare identifier.
fn guardrail_explanations(codes: &[String]) -> Vec<String> {
    use lrg_analysis::edit_guardrails::GuardrailReason::*;
    const ALL: [lrg_analysis::edit_guardrails::GuardrailReason; 6] = [
        HardLightNoAddedContrast,
        NoTonalHeadroom,
        FlatLightContrastAllowed,
        HighlightsUnrecoverable,
        HighIsoShadowsLimited,
        ShadowsAlreadyClipped,
    ];
    codes
        .iter()
        .filter_map(|code| {
            ALL.iter()
                .find(|reason| reason.code() == code)
                .map(|reason| reason.explanation().to_string())
        })
        .collect()
}

async fn edit_multipart(State(state): State<Arc<AppState>>, mut multipart: Multipart) -> Response {
    log::info!("Edit recipe request received");

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
    let SinglePhotoForm {
        photo_ids,
        image_bytes,
        filename,
        fields,
    } = form.into_single_photo();

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
    let response = generate_edit_recipe_for_photo(
        state,
        store.as_deref(),
        options,
        image_bytes,
        photo_id,
        filename,
    )
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

    let mut payload = success_payload(
        photo_id,
        &recipe,
        options,
        response.warning.as_deref(),
        &response.guardrail_reasons,
    );
    payload["input_tokens"] = json!(response.input_tokens);
    payload["output_tokens"] = json!(response.output_tokens);
    Json(payload).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exactly the multipart field names `SearchIndexAPI.generateEditRecipePhoto`
    /// and `SearchIndexAPI.styleEdit` put on the wire. Both plugin functions feed
    /// this same parser (`style_edit.rs` calls it too), so this is the one place
    /// the plugin/backend field-name contract can be pinned.
    fn plugin_fields() -> HashMap<String, String> {
        [
            ("style_strength", "0.8"),
            ("composition_mode", "aggressive"),
            ("adjust_white_balance", "false"),
            ("adjust_basic_tone", "false"),
            ("adjust_presence", "false"),
            ("adjust_color_mix", "false"),
            ("do_color_grading", "false"),
            ("use_tone_curve", "false"),
            ("use_point_curve", "false"),
            ("adjust_detail", "false"),
            ("adjust_effects", "false"),
            ("adjust_lens_corrections", "false"),
            ("allow_auto_crop", "false"),
            ("include_masks", "false"),
            ("is_raw", "true"),
            ("api_key", "sk-test"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn creative_controls_from_the_plugin_reach_the_options() {
        // Regression: the plugin assembled all of these and sent only
        // `include_masks`, so every checkbox in the Creative Controls group and the
        // style-strength slider silently did nothing.
        let opts = parse_edit_options_form(&plugin_fields());

        assert!(!opts.adjust_white_balance);
        assert!(!opts.adjust_basic_tone);
        assert!(!opts.adjust_presence);
        assert!(!opts.adjust_color_mix);
        assert!(!opts.do_color_grading);
        assert!(!opts.use_tone_curve);
        assert!(!opts.use_point_curve);
        assert!(!opts.adjust_detail);
        assert!(!opts.adjust_effects);
        assert!(!opts.adjust_lens_corrections);
        assert!(!opts.allow_auto_crop);
        assert!(!opts.include_masks);
        assert_eq!(opts.style_strength, 0.8);
        assert_eq!(opts.composition_mode, "aggressive");
        // Not a creative control, but it travels on the same form: the
        // guardrails need to know whether clipped highlights are recoverable,
        // and the exported JPEG no longer carries that.
        assert_eq!(opts.is_raw, Some(true));
    }

    #[test]
    fn api_key_survives_the_style_edit_fallback_path() {
        // The style-engine fallback used to omit `api_key`, and there is no
        // environment-variable fallback further down, so the first run of a new
        // user reached the provider with an empty bearer token.
        let opts = parse_edit_options_form(&plugin_fields());
        assert_eq!(opts.api_key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn omitted_toggles_default_to_on() {
        // Why the plugin must send an explicit "false" rather than omitting the
        // field: absent booleans are read as enabled.
        let opts = parse_edit_options_form(&HashMap::new());

        assert!(opts.adjust_white_balance);
        assert!(opts.do_color_grading);
        assert!(opts.allow_auto_crop);
        assert!(opts.include_masks);
        assert_eq!(opts.style_strength, 0.5);
        assert_eq!(opts.composition_mode, "subtle");
        // `is_raw` is the exception: absent means "unknown", not "yes". A
        // guessed raw flag would license highlight recovery the file cannot do.
        assert_eq!(opts.is_raw, None);
    }

    #[test]
    fn style_strength_is_clamped_and_composition_mode_validated() {
        let mut fields = HashMap::new();
        fields.insert("style_strength".to_string(), "9.5".to_string());
        fields.insert("composition_mode".to_string(), "wild".to_string());
        let opts = parse_edit_options_form(&fields);

        assert_eq!(opts.style_strength, 1.0);
        assert_eq!(opts.composition_mode, "subtle", "unknown mode falls back");
    }

    #[test]
    fn a_wrong_scale_white_balance_is_kept_out_of_the_few_shot_block() {
        // A Kelvin number offered to the model as a reference for a JPEG
        // anchors it on a value the target's schema cannot even express.
        let mut settings = json!({"Temp": 5600, "Exposure2012": 0.3});
        assert!(drop_incompatible_temperature(
            &mut settings,
            Some(true),
            Some(false)
        ));
        assert!(settings.get("Temp").is_none());
        assert_eq!(
            settings["Exposure2012"],
            json!(0.3),
            "everything else means the same on both kinds of file"
        );
    }

    #[test]
    fn a_matching_example_keeps_its_white_balance() {
        let mut settings = json!({"Temp": 5600});
        assert!(!drop_incompatible_temperature(
            &mut settings,
            Some(true),
            Some(true)
        ));
        assert_eq!(settings["Temp"], json!(5600));
    }

    #[test]
    fn an_unknown_raw_status_removes_nothing() {
        // Examples saved before the flag existed would otherwise lose their
        // white balance on every edit after upgrading.
        for (example, target) in [(None, Some(true)), (Some(true), None), (None, None)] {
            let mut settings = json!({"Temp": 5600});
            assert!(!drop_incompatible_temperature(
                &mut settings,
                example,
                target
            ));
            assert_eq!(settings["Temp"], json!(5600), "{example:?} vs {target:?}");
        }
    }
}
