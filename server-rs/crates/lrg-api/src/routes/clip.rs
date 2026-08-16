//! Clip blueprint — port of `routes/clip.py`. `/clip/status` checks the
//! ONNX+tokenizer files are on disk (matching Python's `is_model_cached()`
//! "ready to lazy-load" semantics — distinct from `/health`'s "currently
//! loaded in memory" status).
//!
//! `/clip/download/*` differs in *source* from Python (which pulls the
//! fp32 PyTorch checkpoint straight from Hugging Face Hub): this binary
//! has no PyTorch/onnxscript toolchain to run
//! `scripts/export_siglip2_fp16.py` itself, so it instead downloads the
//! fp16 ONNX assets CI already exported and verified.
//!
//! Those assets live at a **stable, separate release tag**
//! (`MODEL_ASSETS_RELEASE_TAG`), not the app's own version tag — the
//! model (`timm/ViT-SO400M-16-SigLIP2-384`) doesn't change per app
//! release, so re-exporting/re-publishing it on every `v*` tag would be
//! pure waste. The `.github/workflows/model-assets.yml` workflow
//! publishes to that tag only when the export script or its pinned
//! deps actually change. If the export ever changes in a way that isn't
//! backward compatible with older binaries, bump both the workflow's
//! tag and `MODEL_ASSETS_RELEASE_TAG` together (e.g. `model-assets-v2`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::{routing::get, routing::post, Json, Router};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};

use crate::state::AppState;

/// Key for this download group in `AppState::model_download`.
const DOWNLOAD_KEY: &str = "clip";

const RELEASE_REPO: &str = "LrGenius/LrGeniusAI";
const MODEL_ASSETS_RELEASE_TAG: &str = "model-assets-v1";
const ASSET_NAMES: [&str; 3] = [
    "siglip2_image_fp16.onnx",
    "siglip2_text_fp16.onnx",
    "tokenizer.json",
];

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/clip/status", get(clip_status))
        .route("/clip/download/start", post(download_start))
        .route("/clip/download/status", get(download_status))
}

async fn clip_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    if state.siglip.is_cached() {
        Json(json!({"clip": "ready", "message": "CLIP model is loaded and ready."}))
    } else {
        Json(json!({"clip": "not_ready", "message": "CLIP model is not loaded."}))
    }
}

/// Progress for one download group. Shared with `routes::llm`, which tracks
/// GGUF downloads under its own key in the same map.
#[derive(Clone, Serialize)]
pub struct ModelDownloadStatus {
    pub(crate) status: &'static str,
    pub(crate) progress: u64,
    pub(crate) total: u64,
    pub(crate) current_file: Option<String>,
    pub(crate) error: Option<String>,
}

impl ModelDownloadStatus {
    pub(crate) fn downloading() -> Self {
        Self {
            status: "downloading",
            ..Default::default()
        }
    }

    pub(crate) fn set_error(&mut self, msg: String) {
        self.status = "error";
        self.error = Some(msg);
    }

    /// Marks success.
    ///
    /// The plugin polls for `"completed"`; the server used to report `"done"`,
    /// so a successful download fell through the plugin's status match, logged
    /// "unexpected status" and never showed the success dialog. Both spellings
    /// are emitted now: `status` stays `"done"` for anything already parsing
    /// it, and `completed` is the boolean the plugin should key off.
    pub(crate) fn set_done(&mut self) {
        self.status = "completed";
        self.error = None;
        if self.total == 0 {
            self.total = self.progress;
        }
    }
}

impl Default for ModelDownloadStatus {
    fn default() -> Self {
        Self {
            status: "not_started",
            progress: 0,
            total: 0,
            current_file: None,
            error: None,
        }
    }
}

async fn download_start(State(state): State<Arc<AppState>>) -> Json<Value> {
    log::info!("Download CLIP model request received");

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
        log::warn!("Download thread is already running.");
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

fn asset_url(tag: &str, name: &str) -> String {
    format!("https://github.com/{RELEASE_REPO}/releases/download/{tag}/{name}")
}

pub(crate) type Downloads = Mutex<HashMap<String, ModelDownloadStatus>>;

fn set_error(state: &Downloads, key: &str, label: &str, msg: String) {
    log::error!("{label} download failed: {msg}");
    state
        .lock()
        .unwrap()
        .entry(key.to_string())
        .or_default()
        .set_error(msg);
}

fn with_status(state: &Downloads, key: &str, f: impl FnOnce(&mut ModelDownloadStatus)) {
    f(state.lock().unwrap().entry(key.to_string()).or_default());
}

/// Streams a set of GitHub release assets to their destinations, reporting
/// progress under `key` in the shared download map.
///
/// Shared with `routes::bioclip`, which pulls a different asset set from its
/// own release tag. Everything that differs between the two model families is
/// a parameter; everything that must not differ — the up-front HEAD pass so
/// the progress bar has a denominator, `.part` staging so an interrupted
/// download can never be mistaken for a complete model, treating any non-2xx
/// as a missing release rather than a corrupt file — lives here once.
pub(crate) async fn download_release_assets(
    state: &Downloads,
    key: &str,
    label: &str,
    tag: &str,
    assets: &[(&str, &std::path::Path)],
) {
    let client = reqwest::Client::new();

    // Resolve sizes up front (HEAD per asset) so total progress is known
    // from the start, matching Python's up-front HfApi files_metadata call.
    let mut total: u64 = 0;
    for (name, _) in assets {
        let url = asset_url(tag, name);
        let resp = match client.head(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                return set_error(state, key, label, format!("failed to resolve {name}: {e}"))
            }
        };
        if !resp.status().is_success() {
            return set_error(
                state,
                key,
                label,
                format!(
                    "release asset {name} not found for {tag} (HTTP {}) — the model-assets release may not exist yet",
                    resp.status()
                ),
            );
        }
        total += resp.content_length().unwrap_or(0);
    }
    with_status(state, key, |s| s.total = total);

    let mut downloaded: u64 = 0;
    for (name, dest) in assets {
        with_status(state, key, |s| s.current_file = Some(name.to_string()));

        let url = asset_url(tag, name);
        log::info!("Starting {label} asset download: {name} from {url}");
        let resp = match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                return set_error(
                    state,
                    key,
                    label,
                    format!("download failed for {name}: HTTP {}", r.status()),
                )
            }
            Err(e) => {
                return set_error(
                    state,
                    key,
                    label,
                    format!("download failed for {name}: {e}"),
                )
            }
        };

        if let Some(parent) = dest.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return set_error(
                    state,
                    key,
                    label,
                    format!("failed to create model directory {}: {e}", parent.display()),
                );
            }
        }

        let tmp_dest = dest.with_file_name(format!(
            "{}.part",
            dest.file_name().unwrap_or_default().to_string_lossy()
        ));
        let file = match tokio::fs::File::create(&tmp_dest).await {
            Ok(f) => f,
            Err(e) => {
                return set_error(
                    state,
                    key,
                    label,
                    format!("failed to create {}: {e}", tmp_dest.display()),
                )
            }
        };
        let mut writer = tokio::io::BufWriter::new(file);
        let mut stream = resp.bytes_stream();

        use tokio::io::AsyncWriteExt;
        loop {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    if let Err(e) = writer.write_all(&chunk).await {
                        return set_error(state, key, label, format!("failed writing {name}: {e}"));
                    }
                    downloaded += chunk.len() as u64;
                    with_status(state, key, |s| s.progress = downloaded);
                }
                Some(Err(e)) => {
                    return set_error(
                        state,
                        key,
                        label,
                        format!("download interrupted for {name}: {e}"),
                    )
                }
                None => break,
            }
        }
        if let Err(e) = writer.flush().await {
            return set_error(state, key, label, format!("failed writing {name}: {e}"));
        }
        drop(writer);
        if let Err(e) = tokio::fs::rename(&tmp_dest, dest).await {
            return set_error(
                state,
                key,
                label,
                format!("failed to place {name} at {}: {e}", dest.display()),
            );
        }
        log::info!("{label} asset ready: {}", dest.display());
    }

    log::info!("{label} download complete ({tag}).");
    with_status(state, key, |s| {
        s.current_file = None;
        s.set_done();
    });
}

async fn run_download(state: Arc<Downloads>) {
    let paths = lrg_ml::model_paths::resolve();
    let assets = [
        (ASSET_NAMES[0], paths.image_onnx.as_path()),
        (ASSET_NAMES[1], paths.text_onnx.as_path()),
        (ASSET_NAMES[2], paths.tokenizer_json.as_path()),
    ];
    download_release_assets(
        &state,
        DOWNLOAD_KEY,
        "CLIP model",
        MODEL_ASSETS_RELEASE_TAG,
        &assets,
    )
    .await;
}
