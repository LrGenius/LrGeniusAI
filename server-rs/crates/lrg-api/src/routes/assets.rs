//! One status and one download for every local model the plugin needs.
//!
//! The per-family routes (`/clip/*`, `/bioclip/*`) still exist and still work
//! — the Plug-in Manager uses them for the detail view, and they are the only
//! way to fetch just one family. But presenting an end user with three
//! separate downloads, three progress bars and three "ready" indicators asks
//! them to care about which neural network does which job, which is not a
//! thing a photographer should have to know. These routes collapse that into
//! "download the AI models".
//!
//! The combined download reuses [`crate::routes::clip::download_release_assets`]
//! with assets from several release tags at once, so it is a single progress
//! bar over one summed total rather than three sequential ones.
//!
//! **Families already on disk are skipped**, so this is also the natural
//! "finish setting up" button: a user who has SigLIP2 from an older version
//! only downloads BioCLIP.

use std::sync::Arc;

use axum::extract::State;
use axum::{routing::get, routing::post, Json, Router};
use serde_json::{json, Value};

use crate::routes::bioclip::bioclip_assets;
use crate::routes::clip::{clip_assets, download_release_assets, Downloads, ModelDownloadStatus};
use crate::state::AppState;

/// Key for this download group in `AppState::model_download`. Distinct from
/// the per-family keys so a combined run and a single-family run cannot
/// overwrite each other's progress.
const DOWNLOAD_KEY: &str = "assets";

/// Rough download sizes, for the "this will fetch about N GB" line before the
/// user commits to it.
///
/// Approximate on purpose: the exact total comes from the HEAD pass at
/// download time, and plumbing that through a status endpoint would mean
/// issuing six HEAD requests every time the Plug-in Manager polls.
const CLIP_APPROX_BYTES: u64 = 2_310_000_000;
const BIOCLIP_APPROX_BYTES: u64 = 876_000_000;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/assets/status", get(assets_status))
        .route("/assets/download/start", post(download_start))
        .route("/assets/download/status", get(download_status))
}

/// Per-family readiness plus one overall flag.
///
/// `downloadable` is not decoration: face detection's `buffalo_l` weights are
/// resolved from `INSIGHTFACE_ROOT` — wherever the InsightFace Python library
/// would have put them — and are not published to any release of this project,
/// so there is nothing for this endpoint to fetch. Reporting it as a family
/// that is simply "not ready" would make the download button look broken. It
/// is listed so the UI can show the state and explain it, and it is excluded
/// from `ready` for the same reason: the combined download cannot fix it, so
/// letting it hold the overall flag down would leave the user with a button
/// that never turns green.
async fn assets_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    let clip_ready = state.siglip.is_cached();
    let bioclip_ready = state.bioclip.is_cached();
    let face_ready = state.face.is_cached();

    let missing_bytes = if clip_ready { 0 } else { CLIP_APPROX_BYTES }
        + if bioclip_ready {
            0
        } else {
            BIOCLIP_APPROX_BYTES
        };

    Json(json!({
        "ready": clip_ready && bioclip_ready,
        "missing_approx_bytes": missing_bytes,
        "families": [
            {
                "id": "clip",
                "name": "Smart photo search",
                "ready": clip_ready,
                "downloadable": true,
                "approx_bytes": CLIP_APPROX_BYTES,
            },
            {
                "id": "bioclip",
                "name": "Species identification",
                "ready": bioclip_ready,
                "downloadable": true,
                "approx_bytes": BIOCLIP_APPROX_BYTES,
            },
            {
                "id": "face",
                "name": "Face detection",
                "ready": face_ready,
                "downloadable": false,
                "reason": "buffalo_l is resolved from INSIGHTFACE_ROOT and is not \
                           published with this project yet",
            },
        ],
    }))
}

async fn download_start(State(state): State<Arc<AppState>>) -> Json<Value> {
    log::info!("Download all model assets request received");

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
        log::warn!("Combined model download is already running.");
        return Json(json!({"download": "started"}));
    }

    let download_state = state.model_download.clone();
    let need_clip = !state.siglip.is_cached();
    let need_bioclip = !state.bioclip.is_cached();
    tokio::spawn(run_download(download_state, need_clip, need_bioclip));

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

async fn run_download(state: Arc<Downloads>, need_clip: bool, need_bioclip: bool) {
    let clip_paths = lrg_ml::model_paths::resolve();
    let bioclip_paths = lrg_ml::model_paths::resolve_bioclip();

    let mut assets = Vec::new();
    if need_clip {
        assets.extend(clip_assets(&clip_paths));
    }
    if need_bioclip {
        assets.extend(bioclip_assets(&bioclip_paths));
    }

    if assets.is_empty() {
        // Nothing missing. Reporting "completed" rather than "not_started" is
        // what lets the plugin treat pressing the button twice as a no-op
        // instead of a hang waiting for progress that will never arrive.
        log::info!("All model assets are already on disk; nothing to download.");
        state
            .lock()
            .unwrap()
            .entry(DOWNLOAD_KEY.to_string())
            .or_default()
            .set_done();
        return;
    }

    log::info!(
        "Downloading {} asset(s): clip={need_clip} bioclip={need_bioclip}",
        assets.len()
    );
    download_release_assets(&state, DOWNLOAD_KEY, "AI models", &assets).await;
}
