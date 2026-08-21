//! `GET /v1/jobs/{job_id}` — polling for any async job.
//!
//! The registry in `lrg_common::jobs` was always generic, but the only way to
//! read it used to be `/keywords/cluster/status/{job_id}`, a
//! keyword-clustering-shaped path whose handler contained nothing specific to
//! keyword clustering. The next long-running operation would have had to grow
//! its own near-identical poll route, and clients would have needed to know
//! which one to call for which job.
//!
//! So enqueueing stays with the domain that knows what work to start (e.g.
//! `POST /v1/keywords/clusters/jobs`), and every one of those returns a
//! `job_id` plus the `poll_url` to read it back here.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

use crate::state::AppState;

/// The path a job is polled at. Handlers that create jobs hand this to the
/// client so the poll location is never guessed from the enqueue path.
pub fn poll_url(job_id: &str) -> String {
    format!("/v1/jobs/{job_id}")
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/jobs/{job_id}", get(job_status))
}

/// A finished job is returned once and then dropped, so a 404 here means
/// either "never existed", "already collected" or "expired after 600s idle" —
/// which is why the message says so rather than claiming the job is unknown.
async fn job_status(State(state): State<Arc<AppState>>, Path(job_id): Path<String>) -> Response {
    match state.jobs.get_job(&job_id) {
        Some(snapshot) => Json(snapshot.to_json()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "job not found — it may have already been collected or expired",
                "status": null,
                "result": null,
            })),
        )
            .into_response(),
    }
}
