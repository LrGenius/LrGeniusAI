//! `/keywords/*` — port of `routes/keywords.py`: CLIP-similarity keyword
//! clustering (optionally LLM-validated), async job polling for the
//! long-running variant, and applying user-approved merges across every
//! photo's stored metadata.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Value};

use lrg_analysis::keywords::{
    build_merge_map, clamp_threshold, cluster_by_similarity, dedupe_keywords,
    replace_in_flattened_keywords, replace_in_keyword_structure,
};
use lrg_ml::siglip::l2_normalize;
// Provider-name resolution lives in `lrg_providers::provider` so every route
// agrees on which names are valid; this module used to keep its own list, which
// is why `"openai"` worked for indexing but silently disabled LLM validation
// here.
use lrg_providers::provider::is_known_provider;
use lrg_providers::text_llm::{validate_clusters_with_llm, TextLlmConfig};
use lrg_store::IMAGE_TABLE;

use crate::state::AppState;

/// Sink for `run_clustering`'s stage updates. Async-job callers point this
/// at the job registry; the synchronous `/keywords/cluster` route passes
/// `None` because nobody can observe a job that hasn't been created.
type ProgressSink<'a> = Option<&'a (dyn Fn(Value) + Send + Sync)>;

fn report(sink: ProgressSink<'_>, progress: Value) {
    if let Some(sink) = sink {
        sink(progress);
    }
}

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route("/keywords/cluster", axum::routing::post(cluster_keywords))
        .route(
            "/keywords/cluster/start",
            axum::routing::post(cluster_keywords_start),
        )
        .route(
            "/keywords/cluster/status/{job_id}",
            axum::routing::get(cluster_keywords_status),
        )
        .route(
            "/keywords/apply-merges",
            axum::routing::post(keywords_apply_merges),
        )
}

struct ClusterRequest {
    unique: Vec<String>,
    threshold: f64,
    config: TextLlmConfig,
}

fn parse_cluster_request(
    data: &Value,
    local_engine: Option<lrg_providers::local::SharedLocalEngine>,
) -> Result<ClusterRequest, &'static str> {
    let Some(keyword_names) = data.get("keywords").and_then(Value::as_array) else {
        return Err("keywords must be a list");
    };
    let names: Vec<String> = keyword_names
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let unique = dedupe_keywords(&names);

    let provider = data
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let use_llm = is_known_provider(&provider);
    let default_threshold = if use_llm { 0.85 } else { 0.88 };
    let threshold = clamp_threshold(
        data.get("threshold")
            .and_then(Value::as_f64)
            .unwrap_or(default_threshold),
    );

    Ok(ClusterRequest {
        unique,
        threshold,
        config: TextLlmConfig {
            provider,
            model: data
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
            api_key: data
                .get("api_key")
                .and_then(Value::as_str)
                .map(str::to_string),
            ollama_base_url: data
                .get("ollama_base_url")
                .and_then(Value::as_str)
                .map(str::to_string),
            lmstudio_base_url: data
                .get("lmstudio_base_url")
                .and_then(Value::as_str)
                .map(str::to_string),
            local_engine,
        },
    })
}

/// Port of `_run_clustering`: embed with SigLIP2 (this project's CLIP
/// tower — the same model used for `image_embeddings`, not a separate
/// ViT-L/14 as `embed_keywords_batched`'s docstring vocabulary suggests
/// but the caller wires up via `server_lifecycle.get_model()` in
/// practice), cluster by cosine similarity, then optionally hand the
/// candidates to an LLM for validation.
async fn run_clustering(
    state: &AppState,
    req: &ClusterRequest,
    progress: ProgressSink<'_>,
) -> Value {
    if req.unique.len() < 2 {
        return json!({"results": [], "warning": Value::Null});
    }

    report(
        progress,
        json!({"stage": "embedding", "done": 0, "total": req.unique.len()}),
    );
    let mut embeddings = match state.siglip.embed_text(&req.unique) {
        Ok(e) => e,
        Err(e) => {
            return json!({"results": [], "warning": format!("Embedding failed: {e}")});
        }
    };
    for emb in &mut embeddings {
        l2_normalize(emb);
    }

    report(
        progress,
        json!({"stage": "clustering", "done": 0, "total": req.unique.len()}),
    );
    let n = embeddings.len();
    let mut sim = vec![0.0f64; n * n];
    for i in 0..n {
        for j in 0..n {
            sim[i * n + j] = embeddings[i]
                .iter()
                .zip(&embeddings[j])
                .map(|(a, b)| *a as f64 * *b as f64)
                .sum();
        }
    }
    let groups = cluster_by_similarity(&sim, n, req.threshold);
    let candidates: Vec<Vec<String>> = groups
        .into_iter()
        .map(|idxs| idxs.into_iter().map(|i| req.unique[i].clone()).collect())
        .collect();

    let use_llm = is_known_provider(&req.config.provider);
    log::info!(
        "cluster_keywords: {} keywords -> {} CLIP candidate(s) (threshold={}, llm={})",
        req.unique.len(),
        candidates.len(),
        req.threshold,
        if use_llm {
            req.config.provider.as_str()
        } else {
            "none"
        }
    );

    if use_llm && !candidates.is_empty() {
        let clusters = validate_clusters_with_llm(&candidates, &req.config, |done, total| {
            report(
                progress,
                json!({"stage": "llm", "done": done, "total": total}),
            );
        })
        .await;
        json!({"results": clusters, "warning": Value::Null})
    } else {
        json!({"results": candidates, "warning": Value::Null})
    }
}

async fn cluster_keywords(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Response {
    let data = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let req = match parse_cluster_request(&data, state.llm.current()) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": e, "results": [], "warning": null})),
            )
                .into_response()
        }
    };
    let result = run_clustering(&state, &req, None).await;
    Json(json!({"results": result["results"], "error": null, "warning": result["warning"]}))
        .into_response()
}

async fn cluster_keywords_start(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Response {
    let data = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let req = match parse_cluster_request(&data, state.llm.current()) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": e, "results": [], "warning": null})),
            )
                .into_response()
        }
    };

    let job_id = state.jobs.create_job();
    let job_id_for_log = job_id.clone();
    let unique_len = req.unique.len();
    let state_for_task = state.clone();
    tokio::spawn(async move {
        // Publish each stage on the job so a polling client can show what
        // is happening; LLM validation of a large branch can run for many
        // minutes and used to look indistinguishable from a hung job.
        let jobs = state_for_task.jobs.clone();
        let progress_job_id = job_id.clone();
        let sink = move |progress: Value| jobs.set_progress(&progress_job_id, progress);
        let result = run_clustering(&state_for_task, &req, Some(&sink)).await;
        state_for_task.jobs.complete_job(&job_id, result);
    });
    log::info!("cluster_keywords: started async job {job_id_for_log} for {unique_len} keywords");

    (
        StatusCode::ACCEPTED,
        Json(json!({"job_id": job_id_for_log, "error": null, "warning": null})),
    )
        .into_response()
}

async fn cluster_keywords_status(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    match state.jobs.get_job(&job_id) {
        Some(snapshot) => Json(snapshot.to_json()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "job not found", "status": null, "result": null})),
        )
            .into_response(),
    }
}

async fn keywords_apply_merges(
    State(state): State<Arc<AppState>>,
    body: Option<Json<Value>>,
) -> Response {
    let data = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let Some(merges_raw) = data.get("merges").and_then(Value::as_array) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "merges must be a list", "updated_photos": 0, "warning": null})),
        )
            .into_response();
    };
    let merges: Vec<(String, String)> = merges_raw
        .iter()
        .filter_map(|m| {
            let dup = m.get("duplicate").and_then(Value::as_str)?;
            let can = m.get("canonical").and_then(Value::as_str)?;
            Some((dup.to_string(), can.to_string()))
        })
        .collect();
    let merge_map = build_merge_map(&merges);
    if merge_map.is_empty() {
        return Json(json!({"updated_photos": 0, "error": null, "warning": null})).into_response();
    }

    let Some(store) = state.store() else {
        return Json(json!({"updated_photos": 0, "error": null, "warning": null})).into_response();
    };
    let rows = match store.scan_meta(IMAGE_TABLE).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string(), "updated_photos": 0, "warning": null})),
            )
                .into_response()
        }
    };

    let mut changed_metadata: HashMap<String, serde_json::Map<String, Value>> = HashMap::new();
    for (photo_id, mut meta) in rows {
        let mut changed = false;

        if let Some(flat) = meta.get("flattened_keywords").and_then(Value::as_str) {
            let (new_flat, flat_changed) = replace_in_flattened_keywords(flat, &merge_map);
            if flat_changed {
                meta.insert("flattened_keywords".into(), json!(new_flat));
                changed = true;
            }
        }

        if let Some(kw_json) = meta.get("keywords").and_then(Value::as_str) {
            if let Ok(kw_data) = serde_json::from_str::<Value>(kw_json) {
                let (new_kw, kw_changed) = replace_in_keyword_structure(&kw_data, &merge_map);
                if kw_changed {
                    meta.insert("keywords".into(), json!(new_kw.to_string()));
                    changed = true;
                }
            }
        }

        if changed {
            changed_metadata.insert(photo_id, meta);
        }
    }

    let updated_photos = changed_metadata.len();
    if updated_photos > 0 {
        let ids: Vec<String> = changed_metadata.keys().cloned().collect();
        for chunk in ids.chunks(500) {
            let existing =
                match store.get(IMAGE_TABLE, chunk).await {
                    Ok(r) => r,
                    Err(e) => return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": e.to_string(), "updated_photos": 0, "warning": null})),
                    )
                        .into_response(),
                };
            let records: Vec<lrg_store::StoreRecord> = existing
                .into_iter()
                .filter_map(|mut r| {
                    let new_meta = changed_metadata.remove(&r.id)?;
                    r.metadata = new_meta;
                    Some(r)
                })
                .collect();
            if let Err(e) = store.upsert(IMAGE_TABLE, &records).await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string(), "updated_photos": 0, "warning": null})),
                )
                    .into_response();
            }
        }
    }

    log::info!(
        "apply_keyword_merges: updated {updated_photos} photo(s) for {} merge(s)",
        merge_map.len()
    );
    Json(json!({"updated_photos": updated_photos, "error": null, "warning": null})).into_response()
}
