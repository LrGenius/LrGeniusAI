//! `/group_similar` and `/cull` — port of `routes/search.py`'s
//! `group_similar_route`/`cull_route` + `services/search.py`'s
//! `group_similar_images`/`cull_images`, backed by
//! `lrg_analysis::grouping::group_and_sort_images`.

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Map, Value};

use lrg_analysis::culling_config::available_presets;
use lrg_analysis::grouping::{group_and_sort_images, Group, GroupingInput};
use lrg_store::IMAGE_TABLE;

use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route("/group_similar", axum::routing::post(group_similar))
        .route("/cull", axum::routing::post(cull))
}

struct GroupingParams {
    photo_ids: Vec<String>,
    phash_threshold: Option<f64>,
    clip_threshold: Option<f64>,
    time_delta_seconds: i64,
    culling_preset: String,
}

/// Mirrors `_parse_grouping_params`. `Err` carries the (status, body) to
/// return directly.
#[allow(clippy::result_large_err)] // error path only; not worth boxing for this
fn parse_grouping_params(data: &Value) -> Result<GroupingParams, Response> {
    let bad = |msg: &str| {
        Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": msg})),
        )
            .into_response())
    };

    let phash_threshold = match data.get("phash_threshold") {
        None => None,
        Some(Value::String(s)) if s == "auto" => None,
        Some(v) => match v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        {
            Some(n) => Some(n),
            None => return bad("Invalid phash_threshold value"),
        },
    };

    let clip_threshold = match data.get("clip_threshold") {
        None => None,
        Some(Value::String(s)) if s == "auto" => None,
        Some(v) => match v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        {
            Some(n) => Some(n),
            None => return bad("Invalid clip_threshold value"),
        },
    };

    let time_delta_seconds = match data.get("time_delta_seconds") {
        None => 1,
        Some(v) => match v
            .as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        {
            Some(n) => n,
            None => return bad("Invalid time_delta_seconds value"),
        },
    };

    let culling_preset = data
        .get("culling_preset")
        .and_then(Value::as_str)
        .unwrap_or("default")
        .trim()
        .to_lowercase();
    let culling_preset = if culling_preset.is_empty() {
        "default".to_string()
    } else {
        culling_preset
    };
    if !available_presets().contains(&culling_preset.as_str()) {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid culling_preset value", "available_presets": available_presets()})),
        )
            .into_response());
    }

    let photo_ids: Vec<String> = data
        .get("photo_ids")
        .or_else(|| data.get("uuids"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if photo_ids.is_empty() {
        return bad("Missing or invalid 'photo_ids' list in request body");
    }

    Ok(GroupingParams {
        photo_ids,
        phash_threshold,
        clip_threshold,
        time_delta_seconds,
        culling_preset,
    })
}

async fn load_grouping_inputs(
    store: &lrg_store::Store,
    photo_ids: &[String],
) -> Result<Vec<GroupingInput>, String> {
    // Dedup while preserving order, matching the Python set-based unique pass.
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<String> = photo_ids
        .iter()
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect();

    let records = store
        .get(IMAGE_TABLE, &unique)
        .await
        .map_err(|e| e.to_string())?;
    Ok(records
        .into_iter()
        .map(|r| {
            let filename = r
                .metadata
                .get("filename")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let capture_time = r.metadata.get("capture_time").and_then(Value::as_f64);
            let phash = r
                .metadata
                .get("cull_phash")
                .or_else(|| r.metadata.get("phash"))
                .and_then(Value::as_str)
                .and_then(|s| u64::from_str_radix(s, 16).ok());
            GroupingInput {
                photo_id: r.id,
                filename,
                capture_time,
                embedding: r.vector,
                phash,
                metadata: r.metadata,
            }
        })
        .collect())
}

fn group_to_json(g: &Group, culling_preset: &str) -> Value {
    let photos: Vec<Value> = g
        .photos
        .iter()
        .map(|p| {
            json!({
                "photo_id": p.photo_id,
                "rank": p.rank,
                "cull_score": p.cull_score,
                "winner": p.winner,
                "reject_candidate": p.reject_candidate,
                "reason_codes": p.reason_codes,
                "explanation": p.explanation,
                "metrics": Value::Object(p.metrics.clone()),
            })
        })
        .collect();
    let mut thresholds = Map::new();
    thresholds.insert("phash_hamming_threshold".into(), json!(g.thresholds.0));
    thresholds.insert("duplicate_distance".into(), json!(g.thresholds.1));
    thresholds.insert("burst_distance".into(), json!(g.thresholds.2));
    thresholds.insert(
        "duplicate_time_window_seconds".into(),
        json!(g.thresholds.3),
    );
    thresholds.insert("time_window_seconds".into(), json!(g.thresholds.4));

    json!({
        "group_id": g.group_id,
        "group_type": g.group_type,
        "group_size": g.group_size,
        "primary_photo_id": g.primary_photo_id,
        "photo_ids": g.photo_ids,
        "winner_photo_id": g.winner_photo_id,
        "alternate_photo_ids": g.alternate_photo_ids,
        "reject_candidate_photo_ids": g.reject_candidate_photo_ids,
        "photos": photos,
        "min_capture_time": g.min_capture_time,
        "max_capture_time": g.max_capture_time,
        "time_span_seconds": g.time_span_seconds,
        "debug": {
            "culling_preset": culling_preset,
            "thresholds": thresholds,
            "pairwise_distances": g.pairwise_distances,
            "pairwise_phash_distances": g.pairwise_phash_distances,
            "edge_types": g.edge_types,
        },
    })
}

async fn compute_groups(state: &AppState, params: &GroupingParams) -> Result<Vec<Group>, String> {
    let Some(store) = state.store() else {
        return Ok(Vec::new());
    };
    let records = load_grouping_inputs(&store, &params.photo_ids).await?;
    Ok(group_and_sort_images(
        records,
        params.phash_threshold,
        params.clip_threshold,
        params.time_delta_seconds,
        &params.culling_preset,
    ))
}

async fn group_similar(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let Some(Json(data)) = body else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "Request must be JSON"})),
        )
            .into_response();
    };
    let params = match parse_grouping_params(&data) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let warning = if state.siglip.status().0 != "loaded" {
        Some("SigLIP model not loaded. Similarity grouping based on visual content will be disabled (pHASH only). Download the model in the plugin manager.")
    } else {
        None
    };

    match compute_groups(&state, &params).await {
        Ok(groups) => {
            let json_groups: Vec<Value> = groups
                .iter()
                .map(|g| group_to_json(g, &params.culling_preset))
                .collect();
            let mut response = json!({"groups": json_groups});
            if let Some(w) = warning {
                response["warning"] = json!(w);
            }
            Json(response).into_response()
        }
        Err(e) => {
            log::error!("Error during similarity grouping: {e}");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e})),
            )
                .into_response()
        }
    }
}

async fn cull(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let Some(Json(data)) = body else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "Request must be JSON"})),
        )
            .into_response();
    };
    let params = match parse_grouping_params(&data) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let warning = if state.siglip.status().0 != "loaded" {
        Some("SigLIP model not loaded. Similarity grouping based on visual content will be disabled (pHASH only). Download the model in the plugin manager.")
    } else {
        None
    };

    match compute_groups(&state, &params).await {
        Ok(groups) => {
            let (mut picks, mut alternates, mut rejects, mut near_dup_groups) =
                (0i64, 0i64, 0i64, 0i64);
            for g in &groups {
                if g.group_type == "near_duplicate" {
                    near_dup_groups += 1;
                }
                for p in &g.photos {
                    if p.winner {
                        picks += 1;
                    } else if p.reject_candidate {
                        rejects += 1;
                    } else {
                        alternates += 1;
                    }
                }
            }
            let json_groups: Vec<Value> = groups
                .iter()
                .map(|g| group_to_json(g, &params.culling_preset))
                .collect();
            Json(json!({
                "status": "success",
                "warning": warning,
                "summary": {
                    "group_count": groups.len(),
                    "pick_count": picks,
                    "alternate_count": alternates,
                    "reject_candidate_count": rejects,
                    "near_duplicate_group_count": near_dup_groups,
                    "culling_preset": params.culling_preset,
                },
                "groups": json_groups,
            }))
            .into_response()
        }
        Err(e) => {
            log::error!("Error during culling: {e}");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e})),
            )
                .into_response()
        }
    }
}
