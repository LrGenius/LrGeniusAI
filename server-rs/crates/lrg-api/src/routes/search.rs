//! `/search` — port of `routes/search.py::search_route` +
//! `services/search.py::search_images`. Scope for M6: SigLIP2 semantic
//! search (brute-force cosine over `image_embeddings` — small enough
//! per-catalog to skip building a LanceDB ANN index for now) + in-memory
//! metadata substring search, merged and knee-filtered exactly like
//! Python. Vertex AI semantic search is M8 (needs cloud auth wiring);
//! omitting it changes result composition when a user has Vertex
//! embeddings, not correctness of what IS implemented.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Value};

use lrg_analysis::relevance_filter::{
    clamp_max_results, clamp_strictness, smart_filter_by_relevance, ScoredResult,
};
use lrg_store::{meta, IMAGE_TABLE};

use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route("/search", axum::routing::get(search).post(search))
}

const DEFAULT_METADATA_FIELDS: [&str; 4] = ["flattened_keywords", "alt_text", "caption", "title"];

#[derive(serde::Deserialize, Default)]
struct SearchQuery {
    term: Option<String>,
    catalog_id: Option<String>,
    relevance_strictness: Option<i64>,
    max_results: Option<i64>,
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

fn round4(v: f64) -> f64 {
    (v * 10000.0).round_ties_even() / 10000.0
}

async fn search(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchQuery>,
    body: Option<Json<Value>>,
) -> Response {
    log::info!("Search request received");
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);

    let term = q
        .term
        .clone()
        .or_else(|| body.get("term").and_then(Value::as_str).map(str::to_string));
    let Some(term) = term.filter(|t| !t.is_empty()) else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "No search term provided"})),
        )
            .into_response();
    };

    let photo_ids_to_search: Option<HashSet<String>> = body
        .get("photo_ids")
        .or_else(|| body.get("uuids"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        });

    let catalog_id = body
        .get("catalog_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(q.catalog_id)
        .filter(|s| !s.is_empty());

    let relevance_strictness = body
        .get("relevance_strictness")
        .and_then(Value::as_i64)
        .or(q.relevance_strictness);
    let max_results = body
        .get("max_results")
        .and_then(Value::as_i64)
        .or(q.max_results);
    let n_results = clamp_max_results(max_results) as usize;
    let strictness = clamp_strictness(relevance_strictness);

    let sources = body.get("search_sources");
    let semantic_siglip = sources
        .and_then(|s| s.get("semantic_siglip"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let metadata_enabled = sources
        .and_then(|s| s.get("metadata"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let metadata_fields: Vec<String> = sources
        .and_then(|s| s.get("metadata_fields"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .filter(|f| DEFAULT_METADATA_FIELDS.contains(f))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            DEFAULT_METADATA_FIELDS
                .iter()
                .map(|s| s.to_string())
                .collect()
        });

    let mut warning: Option<String> = None;
    let Some(store) = state.store() else {
        return Json(json!({"results": Value::Array(vec![])})).into_response();
    };

    // 1. Semantic search (SigLIP2).
    let mut semantic_results: Vec<ScoredResult> = Vec::new();
    let mut semantic_ids: HashSet<String> = HashSet::new();
    if semantic_siglip {
        match state.siglip.embed_text(std::slice::from_ref(&term)) {
            Ok(mut batch) => {
                let mut query_vec = batch.remove(0);
                lrg_ml::siglip::l2_normalize(&mut query_vec);

                let records = match store.scan_all(IMAGE_TABLE).await {
                    Ok(r) => r,
                    Err(e) => {
                        return (
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"error": e.to_string()})),
                        )
                            .into_response()
                    }
                };
                let mut scored: Vec<ScoredResult> = records
                    .iter()
                    .filter(|r| meta::has_embedding(&r.metadata))
                    .filter(|r| {
                        photo_ids_to_search
                            .as_ref()
                            .is_none_or(|ids| ids.contains(&r.id))
                    })
                    .filter(|r| {
                        catalog_id.as_ref().is_none_or(|cid| {
                            meta::parse_catalog_ids(&r.metadata).contains(cid.as_str())
                        })
                    })
                    .filter_map(|r| {
                        let v = r.vector.as_ref()?;
                        Some(ScoredResult {
                            photo_id: r.id.clone(),
                            distance: round4(cosine_distance(&query_vec, v)),
                        })
                    })
                    .collect();
                scored.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
                scored.truncate(n_results);

                semantic_results = smart_filter_by_relevance(scored, Some(strictness));
                semantic_ids = semantic_results
                    .iter()
                    .map(|r| r.photo_id.clone())
                    .collect();
            }
            Err(e) => {
                let msg = format!("SigLIP model not available: {e}");
                log::info!("{msg}");
                warning = Some(msg);
            }
        }
    }

    // 2. Metadata substring search (in-memory over metadata only, not vectors).
    let mut metadata_results: Vec<Value> = Vec::new();
    if metadata_enabled {
        let term_lower = term.to_lowercase();
        let rows = match store.scan_meta(IMAGE_TABLE).await {
            Ok(r) => r,
            Err(e) => {
                return (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
                    .into_response()
            }
        };
        for (photo_id, m) in rows {
            if let Some(ids) = &photo_ids_to_search {
                if !ids.contains(&photo_id) {
                    continue;
                }
            } else if let Some(cid) = &catalog_id {
                if !meta::parse_catalog_ids(&m).contains(cid.as_str()) {
                    continue;
                }
            }
            if semantic_ids.contains(&photo_id) {
                continue;
            }
            let matched = metadata_fields.iter().any(|field| {
                m.get(field)
                    .and_then(Value::as_str)
                    .is_some_and(|v| v.to_lowercase().contains(&term_lower))
            });
            if matched {
                metadata_results
                    .push(json!({"photo_id": photo_id, "uuid": photo_id, "distance": null}));
            }
        }
    }

    let mut results: Vec<Value> = semantic_results
        .into_iter()
        .map(|r| json!({"photo_id": r.photo_id, "uuid": r.photo_id, "distance": r.distance}))
        .collect();
    results.extend(metadata_results);

    let mut response = json!({"results": results});
    if let Some(w) = warning {
        response["warning"] = json!(w);
    }
    Json(response).into_response()
}
