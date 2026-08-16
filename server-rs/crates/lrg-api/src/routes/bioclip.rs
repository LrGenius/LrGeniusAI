//! BioCLIP 2 asset status and download — the species-recognition counterpart
//! to [`crate::routes::clip`], sharing that module's download machinery.
//!
//! `/bioclip/status` reports whether the files are on disk ("ready to
//! lazy-load"), which is deliberately distinct from `/health`'s
//! `species_model`, which reports whether they are currently in memory. The
//! plugin gates its "Identify species" checkbox on this one.
//!
//! **Assets live at their own release tag** (`BIOCLIP_ASSETS_RELEASE_TAG`),
//! not the SigLIP2 one. The two model families have independent lifecycles:
//! re-exporting SigLIP2 incompatibly should not force BioCLIP's ~750 MB to be
//! re-uploaded, and widening the taxa whitelist should not touch SigLIP2. If
//! the BioCLIP export changes in a way older binaries cannot read, bump both
//! `.github/workflows/model-assets-bioclip.yml`'s `tag_name` and the constant
//! below together.
//!
//! Note the head is versioned *twice*, and the two answer different
//! questions: the release tag guards binary compatibility (can this build
//! parse these files at all), while `bioclip2_taxa.json`'s `taxa_version` —
//! surfaced as `species_model` on each photo — guards result validity (are
//! the predictions already on disk still the ones this head would give).

use std::sync::Arc;

use axum::extract::State;
use axum::{routing::get, routing::post, Json, Router};
use serde_json::{json, Value};

use crate::routes::clip::{download_release_assets, Downloads, ModelDownloadStatus};
use crate::state::AppState;

/// Key for this download group in `AppState::model_download`.
const DOWNLOAD_KEY: &str = "bioclip";

const BIOCLIP_ASSETS_RELEASE_TAG: &str = "bioclip-assets-v1";
const ASSET_NAMES: [&str; 3] = [
    "bioclip2_image_fp16.onnx",
    "bioclip2_taxa.bin",
    "bioclip2_taxa.json",
];

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/bioclip/status", get(bioclip_status))
        .route("/bioclip/download/start", post(download_start))
        .route("/bioclip/download/status", get(download_status))
}

async fn bioclip_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    if state.bioclip.is_cached() {
        Json(json!({
            "bioclip": "ready",
            "message": "BioCLIP species model is downloaded and ready.",
            // None until the head has been loaded once — the plugin shows the
            // ready state either way, this is only for diagnostics.
            "model": state.bioclip.model_id(),
        }))
    } else {
        Json(json!({
            "bioclip": "not_ready",
            "message": "BioCLIP species model is not downloaded.",
        }))
    }
}

async fn download_start(State(state): State<Arc<AppState>>) -> Json<Value> {
    log::info!("Download BioCLIP model request received");

    let already_running = {
        let mut downloads = state.model_download.lock().unwrap();
        let status = downloads.entry(DOWNLOAD_KEY.to_string()).or_default();
        if status.status == "downloading" {
            true
        } else {
            *status = ModelDownloadStatus::downloading();
            false
        }
    };
    if already_running {
        log::warn!("BioCLIP download thread is already running.");
        return Json(json!({"download": "started"}));
    }

    let download_state = state.model_download.clone();
    tokio::spawn(run_download(download_state));

    Json(json!({"download": "started"}))
}

async fn download_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    let status = state
        .model_download
        .lock()
        .unwrap()
        .get(DOWNLOAD_KEY)
        .cloned()
        .unwrap_or_default();
    Json(json!(status))
}

async fn run_download(state: Arc<Downloads>) {
    let paths = lrg_ml::model_paths::resolve_bioclip();
    let assets = [
        (ASSET_NAMES[0], paths.image_onnx.as_path()),
        (ASSET_NAMES[1], paths.taxa_bin.as_path()),
        (ASSET_NAMES[2], paths.taxa_json.as_path()),
    ];
    download_release_assets(
        &state,
        DOWNLOAD_KEY,
        "BioCLIP model",
        BIOCLIP_ASSETS_RELEASE_TAG,
        &assets,
    )
    .await;
}
