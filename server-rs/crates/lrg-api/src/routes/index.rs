//! Index blueprint (M2 subset) — port of `routes/index.py`: /get,
//! /get/ids, /remove, /remove/metadata. The upload/indexing endpoints
//! arrive with the ML pipeline in M4-M6.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Map, Value};

use lrg_store::{meta, StoreRecord, FACE_TABLE, IMAGE_TABLE, VERTEX_TABLE};

use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/get", post(get_photo_data))
        .route("/get/ids", get(get_ids))
        .route("/remove", post(remove_image))
        .route("/remove/metadata", post(remove_metadata))
}

fn body_photo_id(body: &Option<Json<Value>>) -> Option<String> {
    let data = body.as_ref()?.0.as_object()?;
    meta::normalize_photo_id(
        data.get("photo_id").and_then(Value::as_str),
        data.get("uuid").and_then(Value::as_str),
    )
}

async fn remove_image(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    log::info!("Remove request received");
    let Some(photo_id) = body_photo_id(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "No photo_id provided"})),
        )
            .into_response();
    };

    let Some(store) = state.store() else {
        // Python: collection is None -> delete is a silent no-op -> success.
        return Json(json!({"status": "removed", "photo_id": photo_id, "uuid": photo_id}))
            .into_response();
    };

    let result: Result<(), String> = async {
        let ids = [photo_id.clone()];
        store
            .delete(IMAGE_TABLE, &ids)
            .await
            .map_err(|e| e.to_string())?;
        // delete_vertex_image failures are swallowed in Python; same here.
        let _ = store.delete(VERTEX_TABLE, &ids).await;
        // delete_faces_by_photo_uuid: faces matching photo_id or photo_uuid.
        let face_rows = store
            .scan_meta(FACE_TABLE)
            .await
            .map_err(|e| e.to_string())?;
        let face_ids: Vec<String> = face_rows
            .into_iter()
            .filter(|(_, m)| {
                m.get("photo_id").and_then(Value::as_str) == Some(photo_id.as_str())
                    || m.get("photo_uuid").and_then(Value::as_str) == Some(photo_id.as_str())
            })
            .map(|(id, _)| id)
            .collect();
        let _ = store.delete(FACE_TABLE, &face_ids).await;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            log::info!("Image ID {photo_id} removed (including face embeddings).");
            Json(json!({"status": "removed", "photo_id": photo_id, "uuid": photo_id}))
                .into_response()
        }
        Err(e) => {
            log::error!("Error removing image {photo_id}: {e}");
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "photo_id not found or error during removal"})),
            )
                .into_response()
        }
    }
}

async fn remove_metadata(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Response {
    log::info!("Remove metadata request received");
    let Some(photo_id) = body_photo_id(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "No photo_id provided"})),
        )
            .into_response();
    };

    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "photo_id not found"})),
        )
            .into_response()
    };
    let Some(store) = state.store() else {
        return not_found();
    };

    let clear_table = |record: StoreRecord| {
        let mut metadata = record.metadata;
        for key in meta::AI_METADATA_KEYS {
            metadata.remove(key);
        }
        meta::ensure_photo_metadata(&record.id, &mut metadata);
        StoreRecord {
            id: record.id,
            vector: record.vector,
            metadata,
        }
    };

    let ids = [photo_id.clone()];
    match store.get(IMAGE_TABLE, &ids).await {
        Ok(records) => {
            let Some(record) = records.into_iter().next() else {
                return not_found();
            };
            if let Err(e) = store.upsert(IMAGE_TABLE, &[clear_table(record)]).await {
                log::error!("Error clearing metadata for {photo_id}: {e}");
                return not_found();
            }
            // Vertex collection: same if present; failures logged only.
            if let Ok(vrecords) = store.get(VERTEX_TABLE, &ids).await {
                if let Some(vrecord) = vrecords.into_iter().next() {
                    if let Err(e) = store.upsert(VERTEX_TABLE, &[clear_table(vrecord)]).await {
                        log::debug!("clear_image_metadata vertex {photo_id}: {e}");
                    }
                }
            }
            log::info!("Metadata cleared for photo_id {photo_id} (embeddings kept).");
            Json(json!({"status": "ok", "photo_id": photo_id, "uuid": photo_id})).into_response()
        }
        Err(e) => {
            log::error!("Error clearing metadata for {photo_id}: {e}");
            not_found()
        }
    }
}

async fn get_photo_data(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    log::info!("Get photo data request received");
    let Some(photo_id) = body_photo_id(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status": "error", "error": "No photo_id provided"})),
        )
            .into_response();
    };
    let catalog_id = body
        .as_ref()
        .and_then(|Json(v)| v.get("catalog_id"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"status": "error", "error": "Photo not found"})),
        )
            .into_response()
    };
    let Some(store) = state.store() else {
        return not_found();
    };

    let records = match store.get(IMAGE_TABLE, std::slice::from_ref(&photo_id)).await {
        Ok(r) => r,
        Err(e) => {
            log::error!("Error retrieving photo data for {photo_id}: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"status": "error", "error": e.to_string()})),
            )
                .into_response();
        }
    };
    let Some(record) = records.into_iter().next() else {
        log::warn!("Photo with photo_id {photo_id} not found in database");
        return not_found();
    };

    // Catalog scoping (get_image with catalog_id).
    if let Some(catalog_id) = &catalog_id {
        if !meta::parse_catalog_ids(&record.metadata).contains(catalog_id.trim()) {
            return not_found();
        }
    }

    let metadata_dict = record.metadata;
    let mut metadata_fields = Map::new();
    let mut edit_recipe = Value::Null;
    let mut edit_warnings = Value::Array(Vec::new());

    for (key, value) in &metadata_dict {
        match key.as_str() {
            "title" | "caption" | "alt_text" => {
                metadata_fields.insert(key.clone(), value.clone());
            }
            "keywords" => match value.as_str() {
                Some(s) if !s.is_empty() => match serde_json::from_str::<Value>(s) {
                    Ok(parsed) => {
                        metadata_fields.insert(key.clone(), parsed);
                    }
                    Err(e) => {
                        // Python lets json.loads raise -> 500 with the error.
                        log::error!("Error retrieving photo data for {photo_id}: {e}");
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"status": "error", "error": e.to_string()})),
                        )
                            .into_response();
                    }
                },
                _ => {
                    metadata_fields.insert(key.clone(), value.clone());
                }
            },
            "edit_recipe" => {
                if let Some(s) = value.as_str() {
                    if !s.is_empty() {
                        match serde_json::from_str::<Value>(s) {
                            Ok(parsed) => edit_recipe = parsed,
                            Err(_) => {
                                log::warn!("Error decoding edit_recipe JSON for {photo_id}")
                            }
                        }
                    }
                }
            }
            "edit_warnings" => {
                if let Some(s) = value.as_str() {
                    if !s.is_empty() {
                        if let Ok(Value::Array(list)) = serde_json::from_str::<Value>(s) {
                            edit_warnings = Value::Array(list);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    log::info!(
        "Retrieved data for photo {photo_id}: {} metadata fields",
        metadata_fields.len()
    );
    Json(json!({
        "status": "success",
        "photo_id": photo_id,
        "uuid": photo_id,
        "metadata": metadata_fields,
        "edit": edit_recipe,
        "edit_summary": metadata_dict.get("edit_summary").cloned().unwrap_or(Value::Null),
        "edit_warnings": edit_warnings,
        "edit_model": metadata_dict.get("edit_model").cloned().unwrap_or(Value::Null),
        "edit_rundate": metadata_dict.get("edit_run_date").cloned().unwrap_or(Value::Null),
        "ai_model": metadata_dict.get("model").cloned().unwrap_or(Value::Null),
        "ai_rundate": metadata_dict.get("run_date").cloned().unwrap_or(Value::Null),
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
struct GetIdsQuery {
    has_embedding: Option<String>,
    catalog_id: Option<String>,
    #[allow(dead_code)]
    db_path: Option<String>,
}

async fn get_ids(State(state): State<Arc<AppState>>, Query(query): Query<GetIdsQuery>) -> Response {
    log::info!("Get IDs request received");
    let Some(store) = state.store() else {
        return Json(json!([])).into_response();
    };

    let has_embedding = query
        .has_embedding
        .as_deref()
        .map(|v| v.eq_ignore_ascii_case("true"));
    let catalog_id = query
        .catalog_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let rows = match store.scan_meta(IMAGE_TABLE).await {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };

    let ids: Vec<String> = rows
        .into_iter()
        .filter(|(_, m)| {
            if let Some(want) = has_embedding {
                if meta::has_embedding(m) != want {
                    return false;
                }
            }
            if let Some(cat) = catalog_id {
                if !meta::parse_catalog_ids(m).contains(cat) {
                    return false;
                }
            }
            true
        })
        .map(|(id, _)| id)
        .collect();

    log::info!("Returning {} image IDs", ids.len());
    Json(json!(ids)).into_response()
}
