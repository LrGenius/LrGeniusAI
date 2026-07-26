//! `POST /import/metadata` — port of `routes/import_.py` +
//! `services/import_.py::import_metadata_task`: bulk-import previously
//! generated title/caption/alt_text/keywords into existing (or newly
//! created metadata-only) photo records.
//!
//! Deliberate correctness fix vs. the Python source: `import_metadata_task`
//! calls `chroma_service.update_image(photo_id, metadata_to_update, ...)`
//! with *only* the newly imported fields, not a copy of the existing
//! metadata first. Since Chroma's (and this store's) update/upsert
//! replaces a record's metadata wholesale — the exact reason every other
//! metadata-writing call site in the Python codebase (`services/index.py`,
//! `routes/edit.py`) always starts from `dict(existing_meta)` before
//! layering new fields on top — the real Python `/import/metadata`
//! silently wipes an already-indexed photo's other metadata (embedding
//! status, culling metrics, catalog_ids, provider/model, ...) on every
//! import of an existing photo. That is very likely an unintended data-
//! loss bug, not a deliberate behavior, so this port merges into the
//! existing metadata like every other write path instead of reproducing it.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use chrono::Utc;
use serde_json::{json, Map, Value};

use lrg_analysis::keywords::flatten_keywords_to_string;
use lrg_store::{meta, StoreRecord, IMAGE_TABLE};

use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route(
        "/import/metadata",
        axum::routing::post(import_metadata_batch),
    )
}

async fn import_metadata_batch(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Response {
    log::info!("Import metadata request received");
    let data = body.map(|Json(v)| v).unwrap_or(Value::Null);

    let Some(items) = data.get("metadata_items").and_then(Value::as_array) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "No data or metadata_items provided"})),
        )
            .into_response();
    };
    let catalog_id = data
        .get("catalog_id")
        .and_then(Value::as_str)
        .map(str::to_string);

    let Some(store) = state.store() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "database not initialized (no db_path bound)"})),
        )
            .into_response();
    };

    let mut success_count = 0usize;
    let mut failure_count = 0usize;
    log::info!("Starting metadata import task for {} items...", items.len());

    for item in items {
        let Some(item) = item.as_object() else {
            failure_count += 1;
            continue;
        };
        let photo_id = item
            .get("photo_id")
            .and_then(Value::as_str)
            .or_else(|| item.get("uuid").and_then(Value::as_str))
            .map(str::to_string);
        let Some(photo_id) = photo_id.filter(|s| !s.is_empty()) else {
            log::warn!("Skipping item due to missing photo_id.");
            failure_count += 1;
            continue;
        };

        let mut metadata_to_update = Map::new();
        if let Some(keywords) = item.get("keywords") {
            let has_content = match keywords {
                Value::Array(a) => !a.is_empty(),
                Value::Object(o) => !o.is_empty(),
                Value::Null => false,
                _ => true,
            };
            if has_content {
                metadata_to_update.insert("keywords".into(), json!(keywords.to_string()));
                metadata_to_update.insert(
                    "flattened_keywords".into(),
                    json!(flatten_keywords_to_string(keywords)),
                );
            }
        }
        for field in ["title", "caption", "alt_text"] {
            if let Some(v) = item.get(field).and_then(Value::as_str) {
                if !v.is_empty() {
                    metadata_to_update.insert(field.into(), json!(v));
                }
            }
        }

        if metadata_to_update.is_empty() {
            log::warn!("No metadata provided to update for photo_id {photo_id}. Skipping.");
            failure_count += 1;
            continue;
        }

        let existing = store
            .get(IMAGE_TABLE, std::slice::from_ref(&photo_id))
            .await
            .ok()
            .and_then(|mut v| v.pop());
        let run_date = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        metadata_to_update.insert("run_date".into(), json!(run_date));
        let uuid = item
            .get("uuid")
            .and_then(Value::as_str)
            .unwrap_or(&photo_id)
            .to_string();
        metadata_to_update.insert("uuid".into(), json!(uuid));

        let (mut metadata, vector) = match existing {
            Some(record) => {
                let mut m = record.metadata;
                m.extend(metadata_to_update);
                (m, record.vector)
            }
            None => (metadata_to_update, None),
        };
        meta::ensure_photo_metadata(&photo_id, &mut metadata);
        if let Some(cid) = &catalog_id {
            let mut ids_set = meta::parse_catalog_ids(&metadata);
            ids_set.insert(cid.clone());
            metadata.insert(
                meta::CATALOG_IDS_FIELD.into(),
                json!(meta::serialize_catalog_ids(&ids_set)),
            );
        }

        let record = StoreRecord {
            id: photo_id.clone(),
            vector,
            metadata,
        };
        match store
            .upsert(IMAGE_TABLE, std::slice::from_ref(&record))
            .await
        {
            Ok(()) => {
                log::info!("Successfully imported metadata for photo_id {photo_id}.");
                success_count += 1;
            }
            Err(e) => {
                log::error!("Error importing metadata for photo_id {photo_id}: {e}");
                failure_count += 1;
            }
        }
    }

    log::info!("Metadata import complete. Success: {success_count}, Failures: {failure_count}.");

    if success_count == 0 && failure_count > 0 {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "No metadata was successfully imported"})),
        )
            .into_response();
    }
    Json(json!({"status": "processed", "success_count": success_count, "failure_count": failure_count})).into_response()
}
