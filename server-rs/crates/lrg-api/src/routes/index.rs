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
        .route("/sync/claim", post(sync_claim))
        .route("/sync/cleanup", post(sync_cleanup))
        .route("/index/check-unprocessed", post(check_unprocessed))
}

/// Port of `get_photo_ids_needing_processing` + the option subset that
/// matters for the delta check (tasks, regenerate_metadata, catalog_id).
async fn check_unprocessed(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Response {
    let data = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let photo_ids: Vec<String> = data
        .get("photo_ids")
        .or_else(|| data.get("uuids"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .filter_map(|s| meta::normalize_photo_id(Some(s), None))
                .collect()
        })
        .unwrap_or_default();
    if photo_ids.is_empty() {
        return Json(json!({"photo_ids": [], "uuids": []})).into_response();
    }

    // tasks: JSON array, comma string, or list; default ["embeddings"].
    let tasks: Vec<String> = match data.get("tasks") {
        Some(Value::String(s)) if !s.is_empty() => {
            if s.starts_with('[') {
                serde_json::from_str::<Vec<String>>(s)
                    .unwrap_or_else(|_| s.split(',').map(|t| t.trim().to_string()).collect())
            } else {
                s.split(',').map(|t| t.trim().to_string()).collect()
            }
        }
        Some(Value::Array(list)) if !list.is_empty() => list
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => vec!["embeddings".to_string()],
    };
    let has_task = |t: &str| tasks.iter().any(|x| x == t);
    let compute_embeddings = has_task("embeddings");
    let compute_metadata = has_task("metadata");
    let compute_faces = has_task("faces");
    let compute_vertexai = has_task("vertexai");
    let compute_species = has_task("species");
    // The fast cull ingest. Deliberately does not set `compute_faces`: that
    // pass writes no FACE_TABLE rows (it has no identity embedding), so keying
    // off stored faces would report every cull-processed photo as outstanding
    // forever. It has its own completion flag below.
    let compute_cull = has_task("cull");
    let any_task = compute_embeddings
        || compute_metadata
        || compute_faces
        || compute_vertexai
        || compute_cull
        || compute_species;

    let regenerate_metadata = data
        .get("regenerate_metadata")
        .or_else(|| data.get("regenerateMetadata"))
        .map(|v| match v {
            Value::String(s) => s.to_lowercase() == "true",
            Value::Bool(b) => *b,
            other => other.to_string().to_lowercase() == "true",
        })
        .unwrap_or(true);
    let catalog_id = data
        .get("catalog_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    // Which taxonomy head is on disk right now. `None` until BioCLIP has
    // loaded once this process, in which case there is nothing to compare
    // against and stored results are taken at face value — the alternative
    // would re-queue the whole catalog on every server restart.
    let current_species_model = state.bioclip.model_id();
    // Which detector/recognizer pair this build writes. A compile-time
    // constant rather than something read off disk, unlike the BioCLIP head
    // above — see `lrg_ml::faces::MODEL_ID`.
    let current_face_model = state.face.model_id();

    let Some(store) = state.store() else {
        // No store bound: everything needs processing (matches empty existing).
        return Json(json!({"photo_ids": photo_ids, "uuids": photo_ids})).into_response();
    };

    let result: Result<Vec<String>, String> = async {
        // Single batch query replaces N individual gets (get_images_batch).
        let mut existing: Map<String, Value> = Map::new();
        for (id, m) in store
            .get_meta(IMAGE_TABLE, &photo_ids)
            .await
            .map_err(|e| e.to_string())?
        {
            if let Some(cat) = catalog_id {
                if !meta::parse_catalog_ids(&m).contains(cat) {
                    continue;
                }
            }
            existing.insert(id, Value::Object(m));
        }

        let mut vertex_present: std::collections::HashSet<String> = Default::default();
        if compute_vertexai && !regenerate_metadata {
            vertex_present = store
                .get_meta(VERTEX_TABLE, &photo_ids)
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|(id, _)| id)
                .collect();
        }

        let mut faces_checked: std::collections::HashSet<String> = Default::default();
        if compute_faces && !regenerate_metadata {
            let wanted: std::collections::HashSet<&str> =
                photo_ids.iter().map(String::as_str).collect();
            // Photos with stored faces: face ids are always `{photo_id}_{n}`,
            // so an id-only scan (no metadata decode, no base64 thumbnails)
            // is enough to know which wanted photos have one.
            for id in store
                .scan_ids(FACE_TABLE)
                .await
                .map_err(|e| e.to_string())?
            {
                if let Some((pid, _)) = id.rsplit_once('_') {
                    if wanted.contains(pid) {
                        faces_checked.insert(pid.to_string());
                    }
                }
            }
            // ...plus photos flagged faces_checked (no faces found).
            for (id, obj) in &existing {
                if obj
                    .get("faces_checked")
                    .is_some_and(|v| v.as_bool().unwrap_or(false) || v == "true")
                {
                    faces_checked.insert(id.clone());
                }
            }
            // A photo is done for `faces` only if it was checked *by the models
            // this build runs*. Swapping the detector/recognizer pair — as the
            // move off InsightFace's buffalo_l did — leaves 512-d embeddings
            // that are the right shape and the wrong space, and no shape check
            // downstream can tell. Dropping those photos back out of
            // `faces_checked` is what re-derives them; the rows themselves stay
            // put until detection replaces them, per the store's no-delete rule.
            faces_checked.retain(|id| {
                existing
                    .get(id)
                    .and_then(Value::as_object)
                    .and_then(|m| m.get("face_model"))
                    .and_then(Value::as_str)
                    == Some(current_face_model)
            });
        }

        let needing: Vec<String> = photo_ids
            .iter()
            .filter(|pid| {
                let m = existing
                    .get(*pid)
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let has_flag = |k: &str| {
                    m.get(k).is_some_and(|v| match v {
                        Value::String(s) => !s.is_empty(),
                        Value::Bool(b) => *b,
                        Value::Null => false,
                        _ => true,
                    })
                };
                let needs_embedding =
                    compute_embeddings && (regenerate_metadata || !has_flag("has_embedding"));
                let has_any_metadata = has_flag("title")
                    || has_flag("caption")
                    || has_flag("alt_text")
                    || has_flag("keywords");
                let needs_metadata = compute_metadata && (regenerate_metadata || !has_any_metadata);
                let needs_faces =
                    compute_faces && (regenerate_metadata || !faces_checked.contains(*pid));
                let needs_vertexai =
                    compute_vertexai && (regenerate_metadata || !vertex_present.contains(*pid));
                let needs_cull_phash = any_task && (regenerate_metadata || !has_flag("cull_phash"));
                // `cull_faces_present` is legitimately `false` for a photo with
                // no faces, so completion is "the key exists", not "it is
                // truthy" — otherwise every faceless photo would be reported as
                // outstanding on every run.
                let needs_cull_faces =
                    compute_cull && (regenerate_metadata || !m.contains_key("cull_faces_present"));
                // A photo is done for `species` only if it was checked *and*
                // checked by the head currently on disk. Widening the taxa
                // whitelist changes what BioCLIP can answer, so results from an
                // older head have to be re-derived rather than trusted.
                let species_head_current = match current_species_model.as_deref() {
                    Some(current) => {
                        m.get("species_model").and_then(Value::as_str) == Some(current)
                    }
                    None => true,
                };
                let needs_species = compute_species
                    && (regenerate_metadata
                        || !has_flag("species_checked")
                        || !species_head_current);
                needs_embedding
                    || needs_metadata
                    || needs_faces
                    || needs_vertexai
                    || needs_cull_phash
                    || needs_cull_faces
                    || needs_species
            })
            .cloned()
            .collect();
        Ok(needing)
    }
    .await;

    match result {
        Ok(needing) => {
            log::info!(
                "check-unprocessed: {} of {} photos need processing",
                needing.len(),
                photo_ids.len()
            );
            Json(json!({"photo_ids": needing, "uuids": needing})).into_response()
        }
        Err(e) => {
            log::error!("check-unprocessed failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e, "results": null, "warning": null})),
            )
                .into_response()
        }
    }
}

/// Port of `chroma_service.sync_claim`: add catalog_id to each photo's
/// catalog_ids (claim existing backend photos for this catalog).
async fn sync_claim(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let data = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let catalog_id = data
        .get("catalog_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(catalog_id) = catalog_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "catalog_id is required"})),
        )
            .into_response();
    };
    let Some(photo_ids) = data.get("photo_ids").and_then(Value::as_array) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "photo_ids must be a list"})),
        )
            .into_response();
    };
    log::info!(
        "sync/claim request: catalog_id={catalog_id}, photo_ids count={}",
        photo_ids.len()
    );

    // Deduplicate (virtual copies share a file-based id), preserve order.
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<String> = photo_ids
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty() && seen.insert(s.to_string()))
        .map(str::to_string)
        .collect();

    let Some(store) = state.store() else {
        return Json(json!({"status": "ok", "claimed": 0, "errors": 0})).into_response();
    };

    let mut claimed = 0usize;
    let mut errors = 0usize;
    for chunk in unique.chunks(200) {
        let result: Result<usize, String> = async {
            let mut records = store
                .get(IMAGE_TABLE, chunk)
                .await
                .map_err(|e| e.to_string())?;
            for record in &mut records {
                let mut ids_set = meta::parse_catalog_ids(&record.metadata);
                ids_set.insert(catalog_id.to_string());
                record.metadata.insert(
                    meta::CATALOG_IDS_FIELD.into(),
                    Value::String(meta::serialize_catalog_ids(&ids_set)),
                );
                let id = record.id.clone();
                meta::ensure_photo_metadata(&id, &mut record.metadata);
            }
            let n = records.len();
            store
                .upsert(IMAGE_TABLE, &records)
                .await
                .map_err(|e| e.to_string())?;
            Ok(n)
        }
        .await;
        match result {
            Ok(n) => claimed += n,
            Err(e) => {
                log::warn!("sync_claim batch failed: {e}");
                errors += chunk.len();
            }
        }
    }
    Json(json!({"status": "ok", "claimed": claimed, "errors": errors})).into_response()
}

/// Port of `chroma_service.sync_cleanup`: disassociate catalog_id from
/// photos no longer in the active list (soft state only, never deletes).
async fn sync_cleanup(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let data = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let catalog_id = data
        .get("catalog_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(catalog_id) = catalog_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "catalog_id is required"})),
        )
            .into_response();
    };
    let photo_ids = data.get("photo_ids");
    if photo_ids.is_some_and(|v| !v.is_null() && !v.is_array()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "photo_ids must be a list or omit for empty"})),
        )
            .into_response();
    }
    let active: std::collections::HashSet<&str> = photo_ids
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let Some(store) = state.store() else {
        return Json(json!({"status": "ok", "checked": 0, "disassociated": 0})).into_response();
    };

    let result: Result<(usize, usize), String> = async {
        let rows = store
            .scan_meta(IMAGE_TABLE)
            .await
            .map_err(|e| e.to_string())?;
        let mut checked = 0usize;
        let mut disassociated = 0usize;
        for (id, metadata) in rows {
            let mut ids_set = meta::parse_catalog_ids(&metadata);
            if !ids_set.contains(catalog_id) {
                continue;
            }
            checked += 1;
            if active.contains(id.as_str()) {
                continue;
            }
            // Read-modify-write keeping the vector (Python _remove_catalog_id).
            let mut records = store
                .get(IMAGE_TABLE, std::slice::from_ref(&id))
                .await
                .map_err(|e| e.to_string())?;
            let Some(record) = records.first_mut() else {
                continue;
            };
            ids_set.remove(catalog_id);
            record.metadata.insert(
                meta::CATALOG_IDS_FIELD.into(),
                Value::String(meta::serialize_catalog_ids(&ids_set)),
            );
            meta::ensure_photo_metadata(&id, &mut record.metadata);
            store
                .upsert(IMAGE_TABLE, &records)
                .await
                .map_err(|e| e.to_string())?;
            disassociated += 1;
        }
        Ok((checked, disassociated))
    }
    .await;

    match result {
        Ok((checked, disassociated)) => {
            Json(json!({"status": "ok", "checked": checked, "disassociated": disassociated}))
                .into_response()
        }
        Err(e) => {
            log::error!("Sync cleanup failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response()
        }
    }
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
        // delete_faces_by_photo_uuid: face ids are always `{photo_id}_{n}`
        // (index_upload.rs stamps both photo_id and photo_uuid to the same
        // value), so a prefix delete finds them without a full-table
        // scan_meta — see Store::delete_by_id_prefix for why that scan
        // (decoding every face's metadata, including its base64 thumbnail,
        // on every call) was a real memory-growth hazard.
        let _ = store
            .delete_by_id_prefix(FACE_TABLE, &format!("{photo_id}_"))
            .await;
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

    let records = match store
        .get(IMAGE_TABLE, std::slice::from_ref(&photo_id))
        .await
    {
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

    // Species goes in its own top-level block rather than into `metadata`.
    // That object is LLM output: the plugin runs it through the review dialog
    // and lets the user edit every field. A taxonomic identification is not
    // something to hand-edit in that dialog, and it is applied on a different
    // path (`MetadataManager.applySpecies`).
    //
    // `null` rather than an empty object when the photo has never been through
    // BioCLIP, so the plugin can tell "not identified" from "not checked".
    let species = if metadata_dict.contains_key("species_checked") {
        json!({
            "rank": metadata_dict.get("species_rank").cloned().unwrap_or(Value::Null),
            "taxonomy": metadata_dict.get("species_taxonomy").cloned().unwrap_or(Value::Null),
            "scientific_name": metadata_dict
                .get("species_scientific_name")
                .cloned()
                .unwrap_or(Value::Null),
            "common_name": metadata_dict
                .get("species_common_name")
                .cloned()
                .unwrap_or(Value::Null),
            "confidence": metadata_dict.get("species_confidence").cloned().unwrap_or(Value::Null),
            "alternatives": metadata_dict
                .get("species_alternatives")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new())),
            "model": metadata_dict.get("species_model").cloned().unwrap_or(Value::Null),
        })
    } else {
        Value::Null
    };

    log::info!(
        "Retrieved data for photo {photo_id}: {} metadata fields",
        metadata_fields.len()
    );
    Json(json!({
        "status": "success",
        "photo_id": photo_id,
        "uuid": photo_id,
        "metadata": metadata_fields,
        "species": species,
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
