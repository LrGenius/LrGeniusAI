//! `/index_by_reference` — port of `routes/index.py::index_images_batch_by_reference`.
//! Same processing pipeline as `/index`, but the plugin sends server-side
//! file paths in a JSON body instead of uploading image bytes (used when
//! the backend has filesystem access to the catalog's photos). Shares
//! `index_upload::process_batch` for everything past reading the files.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Map, Value};

use crate::routes::index_upload::{process_batch, UploadedImage};
use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route(
        "/index_by_reference",
        axum::routing::post(index_by_reference),
    )
}

/// Flattens the request body's non-`images` fields into the same
/// string-keyed map `parse_options` already expects from multipart form
/// fields, so the option-parsing logic doesn't need a JSON-specific copy.
fn json_fields_to_string_map(data: &Map<String, Value>) -> HashMap<String, String> {
    data.iter()
        .filter(|(k, _)| k.as_str() != "images")
        .filter_map(|(k, v)| {
            let s = match v {
                Value::Null => return None,
                Value::String(s) => s.clone(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                other => other.to_string(),
            };
            Some((k.clone(), s))
        })
        .collect()
}

async fn index_by_reference(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Response {
    log::info!("Index by reference request received");

    let Some(Json(data)) = body else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "No JSON payload provided"})),
        )
            .into_response();
    };
    let Some(obj) = data.as_object() else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "No JSON payload provided"})),
        )
            .into_response();
    };

    let images_data = obj
        .get("images")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut paths: Vec<Option<String>> = Vec::with_capacity(images_data.len());
    let mut ids: Vec<Option<String>> = Vec::with_capacity(images_data.len());
    for item in &images_data {
        let path = item.get("path").and_then(Value::as_str).map(str::to_string);
        let photo_id = item
            .get("photo_id")
            .and_then(Value::as_str)
            .or_else(|| item.get("uuid").and_then(Value::as_str))
            .map(str::to_string);
        paths.push(path);
        ids.push(photo_id);
    }

    let mismatched = paths.len() != ids.len()
        || paths.iter().any(Option::is_none)
        || ids.iter().any(Option::is_none);
    if mismatched {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "Mismatch in data, or missing 'path' or 'photo_id' keys in some objects"})),
        )
            .into_response();
    }

    // Raw bytes only — `process_batch` runs the same `normalize_image_bytes`
    // step on these that `/index` runs on its multipart uploads; doing it
    // again here would double-convert.
    let mut images: Vec<UploadedImage> = Vec::new();
    let mut photo_ids: Vec<String> = Vec::new();
    let mut read_errors: Vec<String> = Vec::new();

    for (path, photo_id) in paths.into_iter().flatten().zip(ids.into_iter().flatten()) {
        let raw = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log::warn!("File not found at path: {path}. Skipping.");
                read_errors.push(format!("File not found: {path}"));
                continue;
            }
            Err(e) => {
                log::error!("Error reading file at path {path}: {e}");
                read_errors.push(format!("Error reading {path}: {e}"));
                continue;
            }
        };
        let filename = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        images.push(UploadedImage {
            bytes: raw,
            filename,
        });
        photo_ids.push(photo_id);
    }

    let fields = json_fields_to_string_map(obj);
    process_batch(state, fields, images, photo_ids, read_errors, false).await
}
