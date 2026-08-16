//! `POST /style_edit` — port of `routes/style_edit.py`: the LLM-free
//! style-matched edit endpoint. Retrieves the photo's own CLIP embedding
//! (if already indexed), scores training examples on visual + exposure +
//! scene + time-of-day proximity via `lrg-analysis::style_engine`, and
//! falls back to the regular LLM edit-recipe path (with the same
//! few-shot training injection) when confidence is too low and the
//! caller opted in.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Map, Value};

use lrg_analysis::style_engine::{
    generate_style_edit, ExposureFeatures, StyleEngineResult, StyleQuery, TrainingCandidate,
    CANDIDATE_POOL, CONFIDENCE_LOW,
};
use lrg_analysis::training::{compute_exposure_metrics, time_of_day_bucket_for_hour};
use lrg_store::{StoreRecord, IMAGE_TABLE, TRAINING_TABLE};

use super::edit::{
    cosine_distance, generate_edit_recipe_for_photo, parse_edit_options_form, persist_edit_recipe,
    success_payload,
};
use crate::routes::route_util::{compute_scene_tags, local_hour};
use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route("/style_edit", axum::routing::post(style_edit))
}

fn record_to_candidate(id: &str, meta: &Map<String, Value>, distance: f64) -> TrainingCandidate {
    let canonical_settings = meta
        .get("canonical_settings")
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default();
    let scene_tags: Vec<String> = meta
        .get("scene_tags")
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    TrainingCandidate {
        photo_id: id.to_string(),
        filename: meta
            .get("filename")
            .and_then(Value::as_str)
            .map(str::to_string),
        label: meta
            .get("label")
            .and_then(Value::as_str)
            .map(str::to_string),
        summary: meta
            .get("summary")
            .and_then(Value::as_str)
            .map(str::to_string),
        distance,
        canonical_settings,
        scene_tags,
        exposure: ExposureFeatures {
            luminance_mean: meta.get("exp_luminance_mean").and_then(Value::as_f64),
            contrast: meta.get("exp_contrast").and_then(Value::as_f64),
            warmth_proxy: meta.get("exp_warmth_proxy").and_then(Value::as_f64),
        },
        time_of_day_bucket: meta
            .get("time_of_day_bucket")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        // Absent on examples saved before the flag was recorded, which the
        // style engine treats as compatible with anything.
        is_raw: meta.get("is_raw").and_then(Value::as_bool),
    }
}

/// Port of the candidate-retrieval half of `generate_style_edit`: CLIP
/// similarity search when an embedding is available (converting our
/// plain cosine distance to the Chroma-style squared-L2 value
/// `clip_distance_to_similarity` expects), otherwise the most-recent-
/// examples fallback with Python's neutral 0.5 distance.
async fn fetch_style_candidates(
    store: &lrg_store::Store,
    query_embedding: Option<&[f32]>,
    n_results: usize,
) -> Vec<TrainingCandidate> {
    if let Some(q) = query_embedding {
        let records = store.scan_all(TRAINING_TABLE).await.unwrap_or_default();
        let mut scored: Vec<(f64, StoreRecord)> = records
            .into_iter()
            .filter_map(|r| {
                let v = r.vector.clone()?;
                Some((cosine_distance(q, &v), r))
            })
            .collect();
        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        scored.truncate(n_results);
        scored
            .into_iter()
            .map(|(cos_dist, r)| record_to_candidate(&r.id, &r.metadata, 2.0 * cos_dist))
            .collect()
    } else {
        let mut rows = store.scan_meta(TRAINING_TABLE).await.unwrap_or_default();
        rows.sort_by(|a, b| {
            let ca = a.1.get("captured_at").and_then(Value::as_str).unwrap_or("");
            let cb = b.1.get("captured_at").and_then(Value::as_str).unwrap_or("");
            cb.cmp(ca)
        });
        rows.truncate(n_results);
        rows.into_iter()
            .map(|(id, meta)| record_to_candidate(&id, &meta, 0.5))
            .collect()
    }
}

async fn style_edit(State(state): State<Arc<AppState>>, mut multipart: Multipart) -> Response {
    log::info!("Style edit request received");

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
                    StatusCode::BAD_REQUEST,
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

    let (Some(image_bytes), true) = (image_bytes, photo_ids.len() == 1) else {
        return Json(json!({"error": "Mismatch between number of images and photo IDs, or no images provided"})).into_response();
    };
    let photo_id = photo_ids[0].clone();

    let Some(store) = state.store() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "database not initialized (no db_path bound)"})),
        )
            .into_response();
    };

    let use_llm_fallback = fields
        .get("use_llm_fallback")
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let focal_length = fields
        .get("focal_length")
        .and_then(|s| s.trim().parse::<f64>().ok());
    let capture_time_unix = fields
        .get("capture_time")
        .and_then(|s| s.trim().parse::<f64>().ok());

    // Re-use the CLIP embedding already stored from indexing (best-effort).
    let clip_embedding: Option<Vec<f32>> = store
        .get(IMAGE_TABLE, std::slice::from_ref(&photo_id))
        .await
        .ok()
        .and_then(|mut v| v.pop())
        .and_then(|r| r.vector);

    let query_exposure = match image::load_from_memory(&image_bytes) {
        Ok(img) => {
            let rgb = img.to_rgb8();
            let (w, h) = (rgb.width() as usize, rgb.height() as usize);
            compute_exposure_metrics(&rgb.into_raw(), w, h)
        }
        Err(e) => {
            log::warn!("Failed to decode style_edit image for {photo_id}: {e}");
            compute_exposure_metrics(&[], 0, 0)
        }
    };
    let query_scene_tags = clip_embedding
        .as_deref()
        .map(|e| compute_scene_tags(&state.siglip, e))
        .unwrap_or_default();
    let query_tod = time_of_day_bucket_for_hour(local_hour(capture_time_unix)).to_string();
    let focal_bucket = lrg_analysis::training::focal_length_bucket(focal_length);
    log::info!(
        "Style engine query: photo_id={photo_id} lum={:.3} contrast={:.3} tags={:?} tod={query_tod} focal={focal_bucket}",
        query_exposure.exp_luminance_mean,
        query_exposure.exp_contrast,
        query_scene_tags,
    );

    let training_count = store.count(TRAINING_TABLE).await.unwrap_or(0);
    let candidates = fetch_style_candidates(
        &store,
        clip_embedding.as_deref(),
        CANDIDATE_POOL.min(training_count.max(1)),
    )
    .await;

    // Parsed before the style engine runs, not after: the engine needs to know
    // whether this photo is raw in order to decide which examples may
    // contribute a temperature.
    let options = parse_edit_options_form(&fields);

    let query = StyleQuery {
        exposure: ExposureFeatures {
            luminance_mean: Some(query_exposure.exp_luminance_mean),
            contrast: Some(query_exposure.exp_contrast),
            warmth_proxy: Some(query_exposure.exp_warmth_proxy),
        },
        scene_tags: query_scene_tags,
        time_of_day_bucket: query_tod,
        // Decides which examples may contribute a temperature: Lightroom's is
        // Kelvin for raw and relative for everything else, and the two cannot
        // be averaged together.
        is_raw: options.is_raw,
    };
    let result: StyleEngineResult = generate_style_edit(training_count, &candidates, &query);

    if (result.engine == "none" || result.confidence < CONFIDENCE_LOW) && use_llm_fallback {
        log::info!(
            "Style engine confidence {:.3} below threshold for photo_id={photo_id}, falling back to LLM",
            result.confidence
        );
        let llm_response = generate_edit_recipe_for_photo(
            &state,
            Some(&store),
            &options,
            &image_bytes,
            &photo_id,
            filename.as_deref(),
        )
        .await;
        let Some(recipe) = llm_response.recipe.filter(|_| llm_response.success) else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"status": "error", "engine": "llm", "error": llm_response.error.unwrap_or_else(|| "LLM edit generation failed".to_string())})),
            )
                .into_response();
        };
        if let Err(e) =
            persist_edit_recipe(&store, &photo_id, filename.as_deref(), &recipe, &options).await
        {
            log::error!("Failed to persist style_edit LLM-fallback recipe for {photo_id}: {e}");
        }
        let mut payload = success_payload(
            &photo_id,
            &recipe,
            &options,
            llm_response.warning.as_deref(),
            &llm_response.guardrail_reasons,
        );
        payload["engine"] = json!("llm");
        payload["confidence"] = json!(round3(result.confidence));
        payload["matched_examples"] = json!(result.matched_count);
        payload["style_engine_note"] = result.warning.map(|w| json!(w)).unwrap_or(Value::Null);
        payload["input_tokens"] = json!(llm_response.input_tokens);
        payload["output_tokens"] = json!(llm_response.output_tokens);
        return Json(payload).into_response();
    }

    if result.engine == "none" {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "status": "error",
                "engine": "none",
                "confidence": 0.0,
                "matched_examples": 0,
                "error": result.warning.unwrap_or_else(|| "Style engine could not produce a result.".to_string()),
            })),
        )
            .into_response();
    }

    let global_is_empty = result
        .recipe
        .get("global")
        .map(|g| g.as_object().is_some_and(|m| m.is_empty()))
        .unwrap_or(true);
    if global_is_empty {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "error",
                "engine": "style",
                "confidence": round3(result.confidence),
                "matched_examples": result.matched_count,
                "error": "Style engine returned an empty recipe.",
            })),
        )
            .into_response();
    }

    // The style engine needs the guardrails more than the LLM path does, not
    // less: its recipe *is* the photographer's own average edit, and that
    // average was formed across frames this one may have nothing in common
    // with. A habitual +25 contrast learnt on softly lit material is exactly
    // what `derive_budget` exists to keep off a harshly lit frame.
    let mut styled_recipe = result.recipe;
    // The interpolated settings come straight from the user's own edits, so
    // `temperature` carries whatever scale *those* photos used. Applying a
    // Kelvin average to a JPEG is the maximum-warmth accident this guards
    // against; the LLM paths get the same treatment inside
    // `normalize_edit_recipe`, which the style engine never goes through.
    if lrg_providers::edit_recipe::sanitize_recipe_temperature(&mut styled_recipe, options.is_raw) {
        log::info!(
            "Photo {photo_id}: dropped the style engine's temperature — \
             it is on the raw Kelvin scale and this photo is not raw."
        );
    }
    let guardrail_reasons = crate::edit_budget::measure_and_apply(
        &mut styled_recipe,
        &image_bytes,
        &crate::routes::edit::capture_conditions(&options, filename.as_deref(), &image_bytes),
    );
    if !guardrail_reasons.is_empty() {
        log::info!(
            "Photo {photo_id}: style-engine edit constrained by the frame ({}).",
            guardrail_reasons.join(", ")
        );
    }

    if let Err(e) = persist_edit_recipe(
        &store,
        &photo_id,
        filename.as_deref(),
        &styled_recipe,
        &options,
    )
    .await
    {
        log::error!("Failed to persist style_edit recipe for {photo_id}: {e}");
    }
    let mut payload = success_payload(
        &photo_id,
        &styled_recipe,
        &options,
        result.warning.as_deref(),
        &guardrail_reasons,
    );
    payload["engine"] = json!("style");
    payload["confidence"] = json!(round3(result.confidence));
    payload["matched_examples"] = json!(result.matched_count);
    payload["matched_filenames"] = json!(result.matched_filenames);
    Json(payload).into_response()
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}
