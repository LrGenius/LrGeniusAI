//! Clip blueprint — port of `routes/clip.py`. `/clip/status` is real
//! (checks the ONNX+tokenizer files are on disk, matching Python's
//! `is_model_cached()` "ready to lazy-load" semantics — distinct from
//! `/health`'s "currently loaded in memory" status). The download
//! endpoints are stubs until M9 ships the model distribution pipeline
//! (there's nothing to download yet: production ONNX/fp16 assets don't
//! exist as a release artifact).

use std::sync::Arc;

use axum::{routing::get, routing::post, Json, Router};
use serde_json::{json, Value};

use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/clip/status", get(clip_status))
        .route("/clip/download/start", post(download_start))
        .route("/clip/download/status", get(download_status))
}

async fn clip_status(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Json<Value> {
    if state.siglip.is_cached() {
        Json(json!({"clip": "ready", "message": "CLIP model is loaded and ready."}))
    } else {
        Json(json!({"clip": "not_ready", "message": "CLIP model is not loaded."}))
    }
}

async fn download_start() -> Json<Value> {
    Json(json!({
        "error": "model download is not yet implemented in the Rust backend (M9)"
    }))
}

async fn download_status() -> Json<Value> {
    Json(json!({"status": "not_started"}))
}
