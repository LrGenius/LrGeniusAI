//! `/training/*` — port of `routes/training.py`: CRUD over the
//! `edit_training` LanceDB table that backs the few-shot injection in
//! `routes/edit.rs`. Pure aggregation/bucketing logic lives in
//! `lrg-analysis::training`; this file is the thin HTTP + Store + CLIP
//! embedding layer, mirroring the split already used for `/faces/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Map, Value};

use lrg_analysis::training::{
    aggregate_training_stats, canonicalize_develop_settings, compute_exposure_metrics,
    focal_length_bucket, time_of_day_bucket_for_hour, white_balance_json, CANONICAL_VERSION,
};
use lrg_develop::{FileKind, WarningKind};
use lrg_store::TRAINING_TABLE;

use crate::routes::route_util::{compute_scene_tags, local_hour, parse_multipart, SinglePhotoForm};
use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route(
            "/edit/training",
            axum::routing::post(add_training_example)
                .get(list_training_examples)
                .delete(clear_training_examples),
        )
        .route(
            "/edit/training/stats",
            axum::routing::get(get_training_stats),
        )
        .route(
            "/edit/training/count",
            axum::routing::get(get_training_count),
        )
        .route(
            "/edit/training/{*photo_id}",
            axum::routing::delete(delete_training_example),
        )
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({"error": msg.into()}))).into_response()
}

fn opt_str(fields: &HashMap<String, String>, key: &str) -> Option<String> {
    fields
        .get(key)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn opt_f64(fields: &HashMap<String, String>, key: &str) -> Option<f64> {
    fields.get(key).and_then(|s| s.trim().parse::<f64>().ok())
}

async fn add_training_example(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Response {
    log::info!("Training add request received");

    let form = match parse_multipart(&mut multipart).await {
        Ok(form) => form,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
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
    let Some(store) = state.store() else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database not initialized (no db_path bound)",
        );
    };

    // Reads the parsed `photo_id` part rather than `fields`: the shared
    // parser gives ids their own slot, so they no longer arrive in the
    // catch-all map this used to search.
    let photo_id = photo_ids
        .first()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if photo_id.is_empty() {
        return err(StatusCode::BAD_REQUEST, "photo_id is required");
    }

    // `[]` is how the plugin's JSON.lua encodes an empty Lua table, so a
    // photo without develop settings arrives as `"[]"`; it is the empty table,
    // not an error.
    let develop_settings: Value = match fields.get("develop_settings") {
        Some(raw) if !raw.trim().is_empty() => match serde_json::from_str::<Value>(raw) {
            Ok(Value::Array(a)) if a.is_empty() => Value::Object(Map::new()),
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    "develop_settings must be a JSON object (or [] for none)",
                )
            }
            Err(_) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    "develop_settings must be valid JSON",
                )
            }
        },
        _ => Value::Object(Map::new()),
    };

    let label = opt_str(&fields, "label");
    let summary = opt_str(&fields, "summary");
    let focal_length = opt_f64(&fields, "focal_length");
    let capture_time_unix = opt_f64(&fields, "capture_time");
    let camera_make = opt_str(&fields, "camera_make");
    let camera_model = opt_str(&fields, "camera_model");
    let shutter_speed = opt_str(&fields, "shutter_speed");
    let iso = opt_f64(&fields, "iso");
    let aperture = opt_f64(&fields, "aperture");
    // What the plugin believes. The develop settings' own white-balance keys
    // win over it below: `Temperature` exists only on raw files and
    // `IncrementalTemperature` only on the others, while the plugin's flag
    // counts every DNG as raw, including DNGs converted from JPEGs.
    let is_raw_hint = fields
        .get("is_raw")
        .map(|s| s.trim().eq_ignore_ascii_case("true"));

    let canonical = match canonicalize_develop_settings(&develop_settings, is_raw_hint.into()) {
        Ok(c) => c,
        Err(e) => return err(StatusCode::BAD_REQUEST, e.to_string()),
    };
    let mut warnings: Vec<String> = Vec::new();
    for w in &canonical.warnings {
        match &w.kind {
            // The keys are the better source and the stored flag follows them.
            // Nothing for the user to do about it, so the log only.
            WarningKind::FileKindMismatch { .. } => log::info!(
                "Training example {photo_id}: {w}; storing is_raw from the white-balance keys"
            ),
            WarningKind::UnsupportedProcessVersion { found } => warnings.push(format!(
                "This example uses Lightroom process version {found}, older than PV2012 (6.7), so its develop settings are not learned. Update it to the current process version in the Develop module and save it again."
            )),
            WarningKind::ConflictingFileKind => warnings.push(
                "The white balance of this example was not learned: its develop settings carry both raw (Temperature/Tint) and non-raw (IncrementalTemperature/IncrementalTint) keys.".to_string(),
            ),
            // Unknown keys, odd types and the like are kept verbatim and do
            // not affect what is learned today; the user cannot act on them.
            _ => log::debug!("Training example {photo_id}: {w}"),
        }
    }
    let is_raw = canonical
        .file_kind
        .map(|k| k == FileKind::Raw)
        .or(is_raw_hint);

    let mut embedding: Option<Vec<f32>> = None;
    let mut decoded_rgb: Option<(Vec<u8>, usize, usize)> = None;
    if let Some(bytes) = &image_bytes {
        match image::load_from_memory(bytes) {
            Ok(img) => {
                let rgb = img.to_rgb8();
                let (w, h) = (rgb.width() as usize, rgb.height() as usize);
                let pixels = rgb.into_raw();
                match state.siglip.embed_image(&pixels, w, h) {
                    Ok(emb) => embedding = Some(emb),
                    Err(e) => {
                        log::warn!("Could not compute CLIP embedding for training example: {e}")
                    }
                }
                decoded_rgb = Some((pixels, w, h));
            }
            Err(e) => log::warn!("Failed to decode training image: {e}"),
        }
        if embedding.is_none() {
            warnings.push(
                "Could not compute CLIP embedding for training example. AI style prediction may be less accurate."
                    .to_string(),
            );
        }
    }

    let mut metadata = Map::new();
    metadata.insert("photo_id".into(), json!(photo_id));
    metadata.insert(
        "develop_settings".into(),
        json!(develop_settings.to_string()),
    );
    metadata.insert(
        "canonical_settings".into(),
        json!(Value::Object(canonical.settings).to_string()),
    );
    if let Some(wb) = &canonical.white_balance {
        metadata.insert(
            "white_balance".into(),
            json!(white_balance_json(wb).to_string()),
        );
    }
    metadata.insert("canonical_version".into(), json!(CANONICAL_VERSION));
    metadata.insert(
        "captured_at".into(),
        json!(chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()),
    );
    metadata.insert("has_embedding".into(), json!(embedding.is_some()));
    if let Some(l) = &label {
        metadata.insert("label".into(), json!(l));
    }
    if let Some(f) = &filename {
        metadata.insert("filename".into(), json!(f));
    }
    if let Some(s) = &summary {
        metadata.insert("summary".into(), json!(s));
    }
    if let Some(raw) = is_raw {
        metadata.insert("is_raw".into(), json!(raw));
    }
    metadata.insert(
        "focal_length_bucket".into(),
        json!(focal_length_bucket(focal_length)),
    );
    metadata.insert(
        "time_of_day_bucket".into(),
        json!(time_of_day_bucket_for_hour(local_hour(capture_time_unix))),
    );
    if let Some(cm) = &camera_make {
        metadata.insert(
            "camera_make".into(),
            json!(cm.chars().take(64).collect::<String>()),
        );
    }
    if let Some(cm) = &camera_model {
        metadata.insert(
            "camera_model".into(),
            json!(cm.chars().take(64).collect::<String>()),
        );
    }
    if let Some(iso) = iso {
        metadata.insert("iso".into(), json!(iso));
    }
    if let Some(ap) = aperture {
        metadata.insert("aperture".into(), json!(ap));
    }
    if let Some(ss) = &shutter_speed {
        metadata.insert(
            "shutter_speed".into(),
            json!(ss.chars().take(16).collect::<String>()),
        );
    }

    if let Some((pixels, w, h)) = &decoded_rgb {
        let exp = compute_exposure_metrics(pixels, *w, *h);
        if let Value::Object(exp_map) = exp.to_json() {
            metadata.extend(exp_map);
        }
    }

    let scene_tags: Vec<String> = embedding
        .as_ref()
        .map(|img_emb| compute_scene_tags(&state.siglip, img_emb))
        .unwrap_or_default();
    metadata.insert("scene_tags".into(), json!(json!(scene_tags).to_string()));

    let record = lrg_store::StoreRecord {
        id: photo_id.clone(),
        vector: embedding,
        metadata,
    };
    if let Err(e) = store
        .upsert(TRAINING_TABLE, std::slice::from_ref(&record))
        .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    let total_count = store.count(TRAINING_TABLE).await.unwrap_or(0);
    let mut response = json!({
        "status": "ok",
        "photo_id": photo_id,
        "total_count": total_count,
        "warnings": warnings,
    });
    // Legacy, for plugins that read only the single `warning` string; remove
    // once every supported plugin reads `warnings`.
    if !warnings.is_empty() {
        response["warning"] = json!(warnings.join("\n"));
    }
    Json(response).into_response()
}

async fn list_training_examples(State(state): State<Arc<AppState>>) -> Response {
    let Some(store) = state.store() else {
        return Json(json!({"status": "ok", "examples": [], "count": 0})).into_response();
    };
    let rows = match store.scan_meta(TRAINING_TABLE).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut examples: Vec<Value> = rows
        .iter()
        .map(|(id, meta)| lrg_analysis::training::training_example_view(id, meta))
        .collect();
    examples.sort_by(|a, b| {
        let ca = a.get("captured_at").and_then(Value::as_str).unwrap_or("");
        let cb = b.get("captured_at").and_then(Value::as_str).unwrap_or("");
        cb.cmp(ca)
    });
    let count = examples.len();
    Json(json!({"status": "ok", "examples": examples, "count": count})).into_response()
}

async fn get_training_stats(State(state): State<Arc<AppState>>) -> Response {
    let Some(store) = state.store() else {
        let mut stats = aggregate_training_stats(&[]);
        stats["status"] = json!("ok");
        return Json(stats).into_response();
    };
    let rows = match store.scan_meta(TRAINING_TABLE).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let metadatas: Vec<Map<String, Value>> = rows.into_iter().map(|(_, m)| m).collect();
    let mut stats = aggregate_training_stats(&metadatas);
    stats["status"] = json!("ok");
    Json(stats).into_response()
}

async fn get_training_count(State(state): State<Arc<AppState>>) -> Response {
    let Some(store) = state.store() else {
        return Json(json!({"count": 0})).into_response();
    };
    match store.count(TRAINING_TABLE).await {
        Ok(n) => Json(json!({"count": n})).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn delete_training_example(
    State(state): State<Arc<AppState>>,
    Path(photo_id): Path<String>,
) -> Response {
    let Some(store) = state.store() else {
        return err(
            StatusCode::NOT_FOUND,
            format!("No training example found for photo_id={photo_id}"),
        );
    };
    let existing = store
        .get(TRAINING_TABLE, std::slice::from_ref(&photo_id))
        .await
        .unwrap_or_default();
    if existing.is_empty() {
        return err(
            StatusCode::NOT_FOUND,
            format!("No training example found for photo_id={photo_id}"),
        );
    }
    if let Err(e) = store
        .delete(TRAINING_TABLE, std::slice::from_ref(&photo_id))
        .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let total_count = store.count(TRAINING_TABLE).await.unwrap_or(0);
    Json(json!({"status": "ok", "photo_id": photo_id, "total_count": total_count})).into_response()
}

async fn clear_training_examples(State(state): State<Arc<AppState>>) -> Response {
    let Some(store) = state.store() else {
        return Json(json!({"status": "ok", "removed": 0})).into_response();
    };
    let rows = match store.scan_meta(TRAINING_TABLE).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let ids: Vec<String> = rows.into_iter().map(|(id, _)| id).collect();
    let removed = ids.len();
    if let Err(e) = store.delete(TRAINING_TABLE, &ids).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    Json(json!({"status": "ok", "removed": removed})).into_response()
}
