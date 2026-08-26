//! `/v1/index/photos/by-path` — port of `routes/index.py::index_images_batch_by_reference`.
//! Same processing pipeline as `/v1/index/photos`, but the plugin sends server-side
//! file paths in a JSON body instead of uploading image bytes (used when
//! the backend has filesystem access to the catalog's photos). Shares
//! `index_upload::process_batch` for everything past reading the files.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Map, Value};

use crate::routes::index_upload::{process_batch, ImageSource, PhotoOverrides};
use crate::state::AppState;

/// Reads one `images[]` key as an array of strings; `None` when it is absent
/// or not an array.
fn string_array(item: &Value, key: &str) -> Option<Vec<String>> {
    item.get(key).and_then(Value::as_array).map(|a| {
        a.iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    })
}

/// Pulls the per-photo context out of one `images[]` entry.
///
/// A grouped request carries several photos whose capture time, keywords and
/// folders all differ, but the option fields arrive flat; without these the
/// whole group would inherit the first photo's context. Absent keys stay
/// `None` and fall back to the batch-level options, which is exactly what a
/// single-photo request does today.
fn image_overrides(item: &Value) -> PhotoOverrides {
    PhotoOverrides {
        capture_time: item.get("date_time_unix").and_then(Value::as_f64),
        date_time: item
            .get("date_time")
            .and_then(Value::as_str)
            .map(str::to_string),
        existing_keywords: string_array(item, "existing_keywords"),
        existing_face_tags: string_array(item, "existing_face_tags"),
        folder_names: item
            .get("folder_names")
            .and_then(Value::as_str)
            .map(str::to_string),
        exposure_bias: item.get("exposure_bias").and_then(Value::as_f64),
        is_raw: item.get("is_raw").and_then(Value::as_bool),
    }
}

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route(
        "/index/photos/by-path",
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
    let mut per_image: Vec<PhotoOverrides> = Vec::with_capacity(images_data.len());
    for item in &images_data {
        let path = item.get("path").and_then(Value::as_str).map(str::to_string);
        let photo_id = item
            .get("photo_id")
            .and_then(Value::as_str)
            .or_else(|| item.get("uuid").and_then(Value::as_str))
            .map(str::to_string);
        paths.push(path);
        ids.push(photo_id);
        per_image.push(image_overrides(item));
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

    // Paths, not bytes: `process_batch` reads each file only when it is about
    // to normalise it, so a group of raw originals is never all resident at
    // once. It also runs the same `normalize_image_bytes` step on these that
    // `/v1/index/photos` runs on its multipart uploads — converting here too would
    // double-convert.
    //
    // Files that cannot be read are reported by `process_batch` against their
    // photo_id, which is what the plugin needs in order to retry that one photo
    // through its export fallback. This loop used to read them here and lose
    // the association.
    let mut file_paths: Vec<String> = Vec::new();
    let mut photo_ids: Vec<String> = Vec::new();
    let mut overrides: Vec<PhotoOverrides> = Vec::new();
    for ((path, photo_id), photo_overrides) in paths
        .into_iter()
        .flatten()
        .zip(ids.into_iter().flatten())
        .zip(per_image)
    {
        file_paths.push(path);
        photo_ids.push(photo_id);
        overrides.push(photo_overrides);
    }

    let fields = json_fields_to_string_map(obj);
    process_batch(
        state,
        fields,
        ImageSource::Paths(file_paths),
        photo_ids,
        overrides,
        Vec::new(),
        false,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Face tags travel per photo in their own key. Folding them back into
    /// `existing_keywords` would put a person's name back among the scenery,
    /// which is what made a model write "the rocky shore of Ivo Beach"
    /// (issue #315).
    #[test]
    fn per_image_face_tags_are_read_separately_from_keywords() {
        let overrides = image_overrides(&json!({
            "existing_keywords": ["beach", "sunset"],
            "existing_face_tags": ["Ivo"],
        }));
        assert_eq!(
            overrides.existing_keywords,
            Some(vec!["beach".to_string(), "sunset".to_string()])
        );
        assert_eq!(overrides.existing_face_tags, Some(vec!["Ivo".to_string()]));
    }

    /// An older plugin sends no face-tag key at all; that must stay a
    /// keywords-only request rather than becoming an empty people list.
    #[test]
    fn absent_face_tags_stay_none() {
        let overrides = image_overrides(&json!({ "existing_keywords": ["beach"] }));
        assert_eq!(overrides.existing_face_tags, None);
    }
}
