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
use crate::routes::clip::{
    clip_assets, download_release_assets, Downloads, ModelDownloadStatus, ReleaseAsset,
};
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
const FACE_APPROX_BYTES: u64 = 94_200_000;

/// Release tag holding the YuNet + FaceNet ONNX files.
///
/// Its own tag, for the same reason BioCLIP has one: the model families have
/// independent lifecycles, and re-exporting one should not force the others'
/// gigabytes to be re-uploaded. Bump this and
/// `.github/workflows/model-assets-face.yml`'s `tag_name` together if the
/// export ever changes in a way older binaries cannot read — which is a
/// different question from `lrg_ml::faces::MODEL_ID`, the marker that says
/// already-stored embeddings are no longer comparable.
const FACE_ASSETS_RELEASE_TAG: &str = "face-assets-v1";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/models/assets", get(assets_status))
        .route(
            "/models/assets/downloads",
            post(download_start).get(download_status),
        )
}

/// Per-family readiness plus one overall flag.
///
/// All three families are downloadable and all three gate `ready`. Face
/// detection used to be neither: its weights were InsightFace's `buffalo_l`,
/// resolved from `INSIGHTFACE_ROOT` — wherever that project's Python library
/// would have put them — and not redistributable, so there was nothing for
/// this endpoint to fetch and it had to be excluded from `ready` to avoid a
/// button that never turned green. YuNet and FaceNet replaced them precisely
/// so that carve-out could go away; `downloadable` stays in the response
/// because the plugin reads it.
async fn assets_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    let clip_ready = state.siglip.is_cached();
    let bioclip_ready = state.bioclip.is_cached();
    let face_ready = state.face.is_cached();

    let missing_bytes = if clip_ready { 0 } else { CLIP_APPROX_BYTES }
        + if bioclip_ready {
            0
        } else {
            BIOCLIP_APPROX_BYTES
        }
        + if face_ready { 0 } else { FACE_APPROX_BYTES };

    Json(json!({
        "ready": clip_ready && bioclip_ready && face_ready,
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
                "downloadable": true,
                "approx_bytes": FACE_APPROX_BYTES,
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
    let need_face = !state.face.is_cached();
    tokio::spawn(run_download(
        download_state,
        need_clip,
        need_bioclip,
        need_face,
    ));

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

async fn run_download(state: Arc<Downloads>, need_clip: bool, need_bioclip: bool, need_face: bool) {
    let clip_paths = lrg_ml::model_paths::resolve();
    let bioclip_paths = lrg_ml::model_paths::resolve_bioclip();
    let face_paths = lrg_ml::model_paths::resolve_face();

    let mut assets = Vec::new();
    if need_clip {
        assets.extend(clip_assets(&clip_paths));
    }
    if need_bioclip {
        assets.extend(bioclip_assets(&bioclip_paths));
    }
    if need_face {
        assets.extend(face_assets(&face_paths));
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
        "Downloading {} asset(s): clip={need_clip} bioclip={need_bioclip} face={need_face}",
        assets.len()
    );
    download_release_assets(&state, DOWNLOAD_KEY, "AI models", &assets).await;
}

/// The face-model assets and where they go.
///
/// Unlike SigLIP2 and BioCLIP this has no `/face/download/start` route of its
/// own — the two files together are ~112 MB and there is no UI that wants them
/// separately, so the combined download is the only caller.
fn face_assets(paths: &lrg_ml::faces::FaceModelPaths) -> Vec<ReleaseAsset<'_>> {
    vec![
        (
            FACE_ASSETS_RELEASE_TAG,
            "yunet_face_detection.onnx",
            paths.det_onnx.as_path(),
        ),
        (
            FACE_ASSETS_RELEASE_TAG,
            "facenet_vggface2.onnx",
            paths.rec_onnx.as_path(),
        ),
    ]
}
