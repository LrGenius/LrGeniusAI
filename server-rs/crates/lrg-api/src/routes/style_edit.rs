//! `POST /v1/edit/style` — port of `routes/style_edit.py`: the LLM-free
//! style-matched edit endpoint. Retrieves the photo's own CLIP embedding
//! (if already indexed), scores training examples on visual + exposure +
//! scene + time-of-day proximity via `lrg-analysis::style_engine`, and
//! falls back to the regular LLM edit-recipe path (with the same
//! few-shot training injection) when confidence is too low and the
//! caller opted in.

use std::sync::Arc;

use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Map, Value};

use lrg_analysis::style_engine::{
    generate_style_edit, ExposureFeatures, StyleEngineResult, StyleQuery, TrainingCandidate,
    CANDIDATE_POOL, CONFIDENCE_LOW,
};
use lrg_analysis::training::{
    canonicalize_develop_settings_str, compute_exposure_metrics, time_of_day_bucket_for_hour,
    white_balance_from_json, CANONICAL_VERSION,
};
use lrg_develop::model::WbSetting;
use lrg_develop::{FileKindHint, WarningKind};
use lrg_store::{StoreRecord, IMAGE_TABLE, TRAINING_TABLE};

use super::edit::{
    cosine_distance, generate_edit_recipe_for_photo, parse_edit_options_form, persist_edit_recipe,
    success_payload, warning_entries,
};
use crate::routes::route_util::{
    compute_scene_tags, local_hour, parse_multipart, reasoning_effort_field, SinglePhotoForm,
};
use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route("/edit/style", axum::routing::post(style_edit))
}

/// The learnable form of a stored training example: its frozen
/// `canonical_settings`/`white_balance` when they were written by the current
/// [`CANONICAL_VERSION`], otherwise derived again from its `develop_settings`
/// blob (rows saved before white balance was read from the right keys carry
/// none, and a stale tint). Only the retrieved candidates (at most
/// `CANDIDATE_POOL`) are re-derived, and nothing is written back.
///
/// The flag is true when a re-derived row teaches nothing: its blob is
/// missing or unreadable, or older than PV2012. Training told the user
/// about those only if the row was saved by a build that already knew; an
/// older row reaches the user through the style route's warning instead.
fn canonical_of(
    id: &str,
    meta: &Map<String, Value>,
) -> (Map<String, Value>, Option<WbSetting>, bool) {
    let stored_json = |key: &str| {
        meta.get(key)
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
    };
    if meta.get("canonical_version").and_then(Value::as_u64) == Some(CANONICAL_VERSION) {
        let settings = match stored_json("canonical_settings") {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        };
        let wb = stored_json("white_balance").and_then(|v| white_balance_from_json(&v));
        return (settings, wb, false);
    }
    let Some(blob) = meta.get("develop_settings").and_then(Value::as_str) else {
        log::warn!("Training example {id}: no stored develop settings; it contributes nothing to the style blend");
        return (Map::new(), None, true);
    };
    log::debug!("Training example {id}: stored canonical form is outdated, re-deriving it");
    let hint = FileKindHint::from(meta.get("is_raw").and_then(Value::as_bool));
    match canonicalize_develop_settings_str(blob, hint) {
        Ok(c) => {
            let too_old = c
                .warnings
                .iter()
                .any(|w| matches!(w.kind, WarningKind::UnsupportedProcessVersion { .. }));
            if too_old {
                log::warn!("Training example {id}: process version older than PV2012; it contributes nothing to the style blend");
            }
            (c.settings, c.white_balance, too_old)
        }
        Err(e) => {
            log::warn!("Training example {id}: stored develop settings are unreadable ({e}); it contributes nothing to the style blend");
            (Map::new(), None, true)
        }
    }
}

/// A stored training row as a style-engine candidate, and whether it had
/// to be re-derived into nothing (see [`canonical_of`]).
fn record_to_candidate(
    id: &str,
    meta: &Map<String, Value>,
    distance: f64,
) -> (TrainingCandidate, bool) {
    let (canonical_settings, white_balance, unlearned) = canonical_of(id, meta);
    let scene_tags: Vec<String> = meta
        .get("scene_tags")
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    let candidate = TrainingCandidate {
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
        white_balance,
    };
    (candidate, unlearned)
}

/// Port of the candidate-retrieval half of `generate_style_edit`: CLIP
/// similarity search when an embedding is available (converting our
/// plain cosine distance to the Chroma-style squared-L2 value
/// `clip_distance_to_similarity` expects), otherwise the most-recent-
/// examples fallback with Python's neutral 0.5 distance.
///
/// Also returns how many of the candidates teach nothing (see
/// [`canonical_of`]), so the caller can tell the user.
async fn fetch_style_candidates(
    store: &lrg_store::Store,
    query_embedding: Option<&[f32]>,
    n_results: usize,
) -> (Vec<TrainingCandidate>, usize) {
    let pairs: Vec<(TrainingCandidate, bool)> = if let Some(q) = query_embedding {
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
    };
    let unlearned = pairs.iter().filter(|(_, u)| *u).count();
    (pairs.into_iter().map(|(c, _)| c).collect(), unlearned)
}

/// The note for candidates that were retrieved but teach nothing. The count
/// goes to the log, not into the text: the plugin merges warnings by exact
/// text into one "(N photos)" line, and a per-photo number would defeat it.
const UNLEARNED_EXAMPLES_WARNING: &str = "Some of your matching training examples use a Lightroom process version older than PV2012, or their develop settings could not be read, so they were not used. Update them to the current process version in the Develop module and save them as training examples again.";

async fn style_edit(State(state): State<Arc<AppState>>, mut multipart: Multipart) -> Response {
    log::info!("Style edit request received");

    let form = match parse_multipart(&mut multipart).await {
        Ok(form) => form,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
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

    let (Some(image_bytes), true) = (image_bytes, photo_ids.len() == 1) else {
        return Json(json!({"error": "Mismatch between number of images and photo IDs, or no images provided"})).into_response();
    };
    let photo_id = photo_ids[0].clone();

    if let Err(e) = reasoning_effort_field(fields.get("reasoning_effort").map(String::as_str)) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
    }

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
    let (candidates, unlearned) = fetch_style_candidates(
        &store,
        clip_embedding.as_deref(),
        CANDIDATE_POOL.min(training_count.max(1)),
    )
    .await;

    // Parsed before the style engine runs, not after: the engine needs to know
    // whether this photo is raw to decide which examples' white balance
    // applies (Kelvin for raw, an offset for everything else).
    let options = parse_edit_options_form(&fields);

    let query = StyleQuery {
        exposure: ExposureFeatures {
            luminance_mean: Some(query_exposure.exp_luminance_mean),
            contrast: Some(query_exposure.exp_contrast),
            warmth_proxy: Some(query_exposure.exp_warmth_proxy),
        },
        scene_tags: query_scene_tags,
        time_of_day_bucket: query_tod,
        // Decides the white-balance family: Kelvin (`Temperature`) for raw,
        // an offset (`IncrementalTemperature`) for everything else. Unknown
        // means no white balance is transferred.
        is_raw: options.is_raw,
    };
    let mut result: StyleEngineResult = generate_style_edit(training_count, &candidates, &query);
    if unlearned > 0 && result.engine != "none" {
        log::warn!(
            "Photo {photo_id}: {unlearned} of {} matching training examples taught nothing (older than PV2012 or unreadable)",
            candidates.len()
        );
        result.warnings.push(UNLEARNED_EXAMPLES_WARNING.to_string());
    }

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
        let llm_warnings = warning_entries(llm_response.warning);
        let mut payload = success_payload(
            &photo_id,
            &recipe,
            &options,
            &llm_warnings,
            &llm_response.guardrail_reasons,
        );
        payload["engine"] = json!("llm");
        payload["confidence"] = json!(round3(result.confidence));
        payload["matched_examples"] = json!(result.matched_count);
        payload["style_engine_note"] = if result.warnings.is_empty() {
            Value::Null
        } else {
            json!(result.warnings.join("\n"))
        };
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
                "error": if result.warnings.is_empty() {
                    "Style engine could not produce a result.".to_string()
                } else {
                    result.warnings.join("\n")
                },
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
    // White balance needs no sanitising here: `blend_white_balance` only
    // sends numbers of the target's own key family, as `white_balance` at
    // recipe level, never as `global.temperature`.
    let mut styled_recipe = result.recipe;
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
        &result.warnings,
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

#[cfg(test)]
mod tests {
    use super::*;
    use lrg_develop::model::{WbFamily, WbMode};

    fn meta(pairs: Value) -> Map<String, Value> {
        match pairs {
            Value::Object(m) => m,
            _ => unreachable!(),
        }
    }

    #[test]
    fn an_outdated_row_is_re_derived_from_its_blob() {
        // Version 1 (no `canonical_version`): the frozen form has the old
        // tint number and no white balance, the blob has the truth.
        let blob = json!({
            "ProcessVersion": "15.4",
            "WhiteBalance": "Custom",
            "Temperature": 5600,
            "Tint": 5,
            "Contrast2012": 20,
        });
        let row = meta(json!({
            "canonical_settings": json!({"contrast": 99.0, "tint": 5.0}).to_string(),
            "develop_settings": blob.to_string(),
            "is_raw": true,
        }));
        let (c, unlearned) = record_to_candidate("old", &row, 0.5);
        assert!(!unlearned);
        assert_eq!(c.canonical_settings["contrast"], json!(20));
        assert!(c.canonical_settings.get("tint").is_none());
        let wb = c.white_balance.unwrap();
        assert_eq!((wb.mode, wb.family), (WbMode::Custom, WbFamily::Raw));
        assert_eq!(wb.temperature.map(|t| t.get()), Some(5600.0));
    }

    #[test]
    fn a_current_row_uses_its_stored_form() {
        let row = meta(json!({
            "canonical_version": CANONICAL_VERSION,
            "canonical_settings": json!({"contrast": 7}).to_string(),
            "white_balance": json!({"mode": "As Shot", "family": "raw"}).to_string(),
            // Would say otherwise; not read for a current row.
            "develop_settings": json!({"Contrast2012": 50}).to_string(),
        }));
        let (c, unlearned) = record_to_candidate("new", &row, 0.5);
        assert!(!unlearned);
        assert_eq!(c.canonical_settings["contrast"], json!(7));
        assert_eq!(c.white_balance.unwrap().mode, WbMode::AsShot);
    }

    #[test]
    fn an_unreadable_or_missing_blob_contributes_nothing_and_says_so() {
        let (c, unlearned) =
            record_to_candidate("x", &meta(json!({"develop_settings": "not json"})), 0.5);
        assert!(c.canonical_settings.is_empty() && c.white_balance.is_none());
        assert!(unlearned);
        let (c, unlearned) = record_to_candidate("y", &meta(json!({})), 0.5);
        assert!(c.canonical_settings.is_empty() && c.white_balance.is_none());
        assert!(unlearned);
    }

    #[test]
    fn an_outdated_row_older_than_pv2012_is_flagged_as_unlearned() {
        // No `canonical_version`, so the blob is read again, and PV 5.7
        // (PV2010) teaches nothing.
        let blob = json!({
            "ProcessVersion": "5.7",
            "WhiteBalance": "Custom",
            "Temperature": 5600,
            "Tint": 5,
            "Contrast": 20,
        });
        let row = meta(json!({
            "canonical_settings": json!({"contrast": 20.0}).to_string(),
            "develop_settings": blob.to_string(),
            "is_raw": true,
        }));
        let (c, unlearned) = record_to_candidate("pv2010", &row, 0.5);
        assert!(unlearned);
        assert!(c.canonical_settings.is_empty() && c.white_balance.is_none());
    }
}
