//! `/v1/search/similar` — port of `routes/search.py::find_similar_route` +
//! `services/chroma.py::find_similar_to_photo`/`find_similar_to_photo_by_clip`.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Value};

use lrg_store::{meta, IMAGE_TABLE};

use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route("/search/similar", axum::routing::post(find_similar))
}

fn phash_to_int(value: Option<&Value>) -> Option<u64> {
    let s = value?.as_str()?.trim();
    if s.is_empty() {
        return None;
    }
    u64::from_str_radix(s, 16).ok()
}

fn cosine_distance(a: &[f32], b: &[f32]) -> f64 {
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

async fn find_similar(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let data = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let photo_id = data
        .get("photo_id")
        .or_else(|| data.get("uuid"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(photo_id) = photo_id else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "Missing or invalid 'photo_id' in request body"})),
        )
            .into_response();
    };

    let scope_photo_ids: Option<HashSet<String>> = data
        .get("scope_photo_ids")
        .or_else(|| data.get("scope_uuids"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        });
    let max_results = data
        .get("max_results")
        .and_then(Value::as_i64)
        .unwrap_or(100)
        .clamp(1, 2000) as usize;
    let phash_max_hamming = data
        .get("phash_max_hamming")
        .and_then(Value::as_i64)
        .unwrap_or(10)
        .clamp(0, 64) as u32;
    let use_clip = data
        .get("use_clip")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let similarity_mode = data
        .get("similarity_mode")
        .and_then(Value::as_str)
        .unwrap_or("phash")
        .to_lowercase();
    let catalog_id = data
        .get("catalog_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty());

    log::info!(
        "find_similar: photo_id={photo_id} similarity_mode={similarity_mode} max_results={max_results} \
         phash_max_hamming={phash_max_hamming} use_clip={use_clip}"
    );

    let Some(store) = state.store() else {
        return Json(json!({"results": []})).into_response();
    };

    let not_indexed = || {
        Json(json!({
            "results": [],
            "warning": "This photo is not in the search index. Run 'Analyze & Index' first.",
        }))
        .into_response()
    };

    let target = match store.get(IMAGE_TABLE, &[photo_id.to_string()]).await {
        Ok(mut v) => v.pop(),
        Err(e) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };
    let Some(target) = target else {
        return not_indexed();
    };
    if let Some(cid) = &catalog_id {
        if !meta::parse_catalog_ids(&target.metadata).contains(cid.as_str()) {
            return not_indexed();
        }
    }

    let all_records = match store.scan_all(IMAGE_TABLE).await {
        Ok(r) => r,
        Err(e) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };
    let candidates: Vec<_> = all_records
        .into_iter()
        .filter(|r| r.id != photo_id)
        .filter(|r| {
            scope_photo_ids
                .as_ref()
                .is_none_or(|ids| ids.contains(&r.id))
        })
        .filter(|r| {
            scope_photo_ids.is_some()
                || catalog_id
                    .as_ref()
                    .is_none_or(|cid| meta::parse_catalog_ids(&r.metadata).contains(cid.as_str()))
        })
        .collect();

    if similarity_mode == "clip" {
        let Some(target_emb) = target.vector.as_ref() else {
            return Json(json!({
                "results": [],
                "warning": "This photo has no image embedding. Run 'Analyze & Index' with embeddings enabled.",
            }))
            .into_response();
        };
        let mut scored: Vec<(String, f64)> = candidates
            .iter()
            .filter_map(|r| {
                Some((
                    r.id.clone(),
                    cosine_distance(target_emb, r.vector.as_ref()?),
                ))
            })
            .collect();
        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        scored.truncate(max_results);
        let results: Vec<Value> = scored
            .into_iter()
            .map(|(pid, d)| json!({"photo_id": pid, "phash_distance": null, "clip_distance": d}))
            .collect();
        log::info!("find_similar: returning {} results", results.len());
        return Json(json!({"results": results})).into_response();
    }

    // Default: pHash mode, optionally re-ranked by CLIP distance.
    let target_phash = phash_to_int(
        target
            .metadata
            .get("cull_phash")
            .or_else(|| target.metadata.get("phash")),
    );
    let Some(target_phash) = target_phash else {
        return Json(json!({
            "results": [],
            "warning": "This photo has no perceptual hash. Run 'Analyze & Index' first.",
        }))
        .into_response();
    };
    let target_emb = if use_clip {
        target.vector.as_ref()
    } else {
        None
    };

    let mut results: Vec<(String, u32, Option<f64>)> = candidates
        .iter()
        .filter_map(|r| {
            let cand_phash = phash_to_int(
                r.metadata
                    .get("cull_phash")
                    .or_else(|| r.metadata.get("phash")),
            )?;
            let phash_dist = (target_phash ^ cand_phash).count_ones();
            if phash_dist > phash_max_hamming {
                return None;
            }
            let clip_dist =
                target_emb.and_then(|te| r.vector.as_ref().map(|ce| cosine_distance(te, ce)));
            Some((r.id.clone(), phash_dist, clip_dist))
        })
        .collect();
    results.sort_by(|a, b| {
        a.1.cmp(&b.1).then(
            a.2.unwrap_or(f64::INFINITY)
                .partial_cmp(&b.2.unwrap_or(f64::INFINITY))
                .unwrap(),
        )
    });
    results.truncate(max_results);
    let out: Vec<Value> = results
        .into_iter()
        .map(|(pid, phash_dist, clip_dist)| json!({"photo_id": pid, "phash_distance": phash_dist, "clip_distance": clip_dist}))
        .collect();
    log::info!("find_similar: returning {} results", out.len());
    Json(json!({"results": out})).into_response()
}
