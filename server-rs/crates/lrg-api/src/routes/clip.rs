//! Clip blueprint — port of `routes/clip.py`. `/v1/models/clip` checks the
//! ONNX+tokenizer files are on disk (matching Python's `is_model_cached()`
//! "ready to lazy-load" semantics — distinct from `/health`'s "currently
//! loaded in memory" status).
//!
//! `/v1/models/clip/downloads` differs in *source* from Python (which pulls the
//! fp32 PyTorch checkpoint straight from Hugging Face Hub): this binary
//! has no PyTorch/onnxscript toolchain to run
//! `scripts/export_siglip2_fp16.py` itself, so it instead downloads the
//! fp16 ONNX assets CI already exported and verified.
//!
//! Those assets live at a **stable, separate release tag**
//! (`MODEL_ASSETS_RELEASE_TAG`), not the app's own version tag — the
//! model (`google/siglip2-so400m-patch16-naflex`) doesn't change per app
//! release, so re-exporting/re-publishing it on every `v*` tag would be
//! pure waste. The `.github/workflows/model-assets.yml` workflow
//! publishes to that tag only when the export script or its pinned
//! deps actually change. If the export ever changes in a way that isn't
//! backward compatible with older binaries, bump both the workflow's
//! tag and `MODEL_ASSETS_RELEASE_TAG` together.
//!
//! `model-assets-v2` is that bump: v1 holds the fixed-resolution
//! `ViT-SO400M-16-SigLIP2-384` export, which released builds still download,
//! so it stays published and untouched. The asset names changed along with the
//! tag (`siglip2_naflex_*`) so the two can never be mistaken for each other on
//! disk.

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
const MODEL_ASSETS_RELEASE_TAG: &str = "model-assets-v2";
const ASSET_NAMES: [&str; 3] = [
    "siglip2_naflex_image_fp16.onnx",
    "siglip2_naflex_text_fp16.onnx",
    "siglip2_naflex_tokenizer.json",
];

/// The AestheticSigLIP scorer, published next to the SigLIP2 towers under the
/// same tag (one export script run per model release, one release to bump).
const AESTHETIC_ASSET_NAME: &str = "aesthetic_siglip_fp16.onnx";

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/models/clip", get(clip_status)).route(
        "/models/clip/downloads",
        post(download_start).get(download_status),
    )
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
    /// Every failure of a download that goes on past one, each as
    /// `"<what>: <why>"`: the combined download in `routes::assets` skips a
    /// model family it cannot fetch and lists it here. `error` sums them up
    /// for a client that shows one message; this keeps each report whole.
    /// Left out of the JSON when empty, so the single-family routes answer
    /// exactly as before.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) error_messages: Vec<String>,
    /// The name the finished model is offered under (`model` in a request),
    /// so the plugin can offer to select it. Local LLM downloads only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) installed_name: Option<String>,
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

    /// Marks a download that went on past its failures as failed: `summary`
    /// is the message a client shows, `messages` holds one entry per failure.
    pub(crate) fn set_errors(&mut self, summary: String, messages: Vec<String>) {
        self.status = "error";
        self.error = Some(summary);
        self.error_messages = messages;
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
        self.error_messages.clear();
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
            error_messages: Vec::new(),
            installed_name: None,
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

pub(crate) fn with_status(state: &Downloads, key: &str, f: impl FnOnce(&mut ModelDownloadStatus)) {
    f(state.lock().unwrap().entry(key.to_string()).or_default());
}

/// One asset to fetch: which release tag holds it, its asset name, and where
/// it goes on disk.
pub(crate) type ReleaseAsset<'a> = (&'a str, &'a str, &'a std::path::Path);

/// Where each release asset is downloaded from.
///
/// The tag is per *asset* rather than per call so one progress bar can span
/// several model families, which live at different release tags — that is
/// what makes the combined "download everything" flow in `routes::assets` a
/// single bar instead of one per family.
pub(crate) fn release_urls<'a>(assets: &[ReleaseAsset<'a>]) -> Vec<UrlAsset<'a>> {
    assets
        .iter()
        .map(|&(tag, name, dest)| (asset_url(tag, name), name, dest))
        .collect()
}

/// Streams a set of GitHub release assets to their destinations, reporting
/// progress under `key` in the shared download map.
///
/// For one model: its files are all or nothing, so the first that cannot be
/// resolved or fetched ends the download with its reason. Used by this
/// module's route and `routes::bioclip`. The combined download in
/// `routes::assets` must not stop at one family's failure, so it goes family
/// by family over [`resolve_sizes`] and [`fetch_assets`] instead — the same
/// HEAD pass and the same transfer.
pub(crate) async fn download_release_assets(
    state: &Downloads,
    key: &str,
    label: &str,
    assets: &[ReleaseAsset<'_>],
) {
    download_url_assets(state, key, label, &release_urls(assets)).await;
}

/// One file to fetch from anywhere: its URL, the name progress reports it
/// under, and where it goes on disk.
pub(crate) type UrlAsset<'a> = (String, &'a str, &'a std::path::Path);

/// [`download_release_assets`] for files that are not GitHub release assets
/// (AuraFace comes from Hugging Face). Same HEAD pass, `.part` staging and
/// progress reporting.
pub(crate) async fn download_url_assets(
    state: &Downloads,
    key: &str,
    label: &str,
    assets: &[UrlAsset<'_>],
) {
    let client = reqwest::Client::new();

    let total = match resolve_sizes(&client, assets).await {
        Ok(total) => total,
        Err(e) => return set_error(state, key, label, e),
    };
    with_status(state, key, |s| s.total = total);

    let mut downloaded = 0;
    if let Err(e) = fetch_assets(&client, state, key, label, assets, &mut downloaded).await {
        return set_error(state, key, label, e);
    }

    log::info!("{label} download complete ({} asset(s)).", assets.len());
    with_status(state, key, |s| {
        s.current_file = None;
        s.set_done();
    });
}

/// The summed size of `assets`, from one HEAD request each, so the progress
/// bar has its denominator before the first byte arrives (matching Python's
/// up-front HfApi `files_metadata` call).
///
/// Stops at the first asset that cannot be fetched and says why. Any non-2xx
/// is taken for a release that is not there, not for a corrupt file.
pub(crate) async fn resolve_sizes(
    client: &reqwest::Client,
    assets: &[UrlAsset<'_>],
) -> Result<u64, String> {
    let mut total = 0;
    for (url, name, _) in assets {
        let resp = client
            .head(url.as_str())
            .send()
            .await
            .map_err(|e| format!("failed to resolve {name}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!(
                "{name} not found at {url} (HTTP {}) — the release may not exist yet",
                resp.status()
            ));
        }
        total += announced_size(&resp);
    }
    Ok(total)
}

/// The size a HEAD response announces, or 0 when it announces none.
///
/// Read from the `Content-Length` header itself. reqwest's `content_length()`
/// is the length of the body it received, and a HEAD response has none, so it
/// is 0 for every asset (seanmonstar/reqwest#1814) — which is how every
/// download here used to run with a `total` of 0 until it had finished.
fn announced_size(resp: &reqwest::Response) -> u64 {
    resp.headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

/// Streams each of `assets` to its destination, adding every chunk to
/// `downloaded` and reporting the sum as `progress` under `key`.
///
/// Each file is written beside its destination as `.part` and renamed into
/// place once complete, so an interrupted download can never be mistaken for
/// a complete model. Stops at the first asset that fails and says why; the
/// files before it stay in place.
pub(crate) async fn fetch_assets(
    client: &reqwest::Client,
    state: &Downloads,
    key: &str,
    label: &str,
    assets: &[UrlAsset<'_>],
    downloaded: &mut u64,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;

    for (url, name, dest) in assets {
        with_status(state, key, |s| s.current_file = Some(name.to_string()));

        log::info!("Starting {label} asset download: {name} from {url}");
        let resp = match client.get(url.as_str()).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => return Err(format!("download failed for {name}: HTTP {}", r.status())),
            Err(e) => return Err(format!("download failed for {name}: {e}")),
        };

        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                format!("failed to create model directory {}: {e}", parent.display())
            })?;
        }

        let tmp_dest = dest.with_file_name(format!(
            "{}.part",
            dest.file_name().unwrap_or_default().to_string_lossy()
        ));
        let file = tokio::fs::File::create(&tmp_dest)
            .await
            .map_err(|e| format!("failed to create {}: {e}", tmp_dest.display()))?;
        let mut writer = tokio::io::BufWriter::new(file);
        let mut stream = resp.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("download interrupted for {name}: {e}"))?;
            writer
                .write_all(&chunk)
                .await
                .map_err(|e| format!("failed writing {name}: {e}"))?;
            *downloaded += chunk.len() as u64;
            let progress = *downloaded;
            with_status(state, key, |s| s.progress = progress);
        }
        writer
            .flush()
            .await
            .map_err(|e| format!("failed writing {name}: {e}"))?;
        drop(writer);
        tokio::fs::rename(&tmp_dest, dest)
            .await
            .map_err(|e| format!("failed to place {name} at {}: {e}", dest.display()))?;
        log::info!("{label} asset ready: {}", dest.display());
    }
    Ok(())
}

/// The aesthetic scorer's asset and where it goes, for the combined download in
/// `routes::assets`. Not part of [`clip_assets`]: it is its own model family
/// with its own readiness, and a build with the search model but not this one
/// must still search.
pub(crate) fn aesthetic_assets(paths: &lrg_ml::aesthetic::AestheticPaths) -> Vec<ReleaseAsset<'_>> {
    vec![(
        MODEL_ASSETS_RELEASE_TAG,
        AESTHETIC_ASSET_NAME,
        paths.onnx.as_path(),
    )]
}

/// The SigLIP2 assets and where they go, for this route and for the combined
/// download in `routes::assets`.
pub(crate) fn clip_assets(paths: &lrg_ml::siglip::ModelPaths) -> Vec<ReleaseAsset<'_>> {
    vec![
        (
            MODEL_ASSETS_RELEASE_TAG,
            ASSET_NAMES[0],
            paths.image_onnx.as_path(),
        ),
        (
            MODEL_ASSETS_RELEASE_TAG,
            ASSET_NAMES[1],
            paths.text_onnx.as_path(),
        ),
        (
            MODEL_ASSETS_RELEASE_TAG,
            ASSET_NAMES[2],
            paths.tokenizer_json.as_path(),
        ),
    ]
}

async fn run_download(state: Arc<Downloads>) {
    let paths = lrg_ml::model_paths::resolve();
    download_release_assets(&state, DOWNLOAD_KEY, "CLIP model", &clip_assets(&paths)).await;
}

#[cfg(test)]
mod tests {
    use crate::fake_release::{asset, content, serve, size_of};
    use super::*;

    #[tokio::test]
    async fn sizes_are_what_the_server_announces() {
        let server = serve().await;
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.onnx"), dir.path().join("b.onnx"));
        let assets = [
            asset(&server, "ok", "a.onnx", &a),
            asset(&server, "ok", "b.onnx", &b),
        ];

        let total = resolve_sizes(&reqwest::Client::new(), &assets)
            .await
            .unwrap();

        assert_eq!(total, size_of(&["a.onnx", "b.onnx"]));
    }

    /// One model's files are all or nothing: the single-family routes (CLIP,
    /// BioCLIP, AuraFace) still stop at the first file that cannot be fetched,
    /// with the message they always gave.
    #[tokio::test]
    async fn one_model_still_stops_at_its_first_missing_file() {
        let server = serve().await;
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.onnx"), dir.path().join("b.onnx"));
        let assets = [
            asset(&server, "ok", "a.onnx", &a),
            asset(&server, "gone", "b.onnx", &b),
        ];
        let downloads = Downloads::default();

        download_url_assets(&downloads, "clip", "CLIP model", &assets).await;

        let status = downloads.lock().unwrap()["clip"].clone();
        assert_eq!(status.status, "error");
        assert_eq!(
            status.error.as_deref(),
            Some(
                format!(
                    "b.onnx not found at {server}/gone/b.onnx (HTTP 404 Not Found) — \
                     the release may not exist yet"
                )
                .as_str()
            )
        );
        assert!(status.error_messages.is_empty());
        assert!(
            !a.exists() && !b.exists(),
            "nothing is fetched once a file of the model is known to be missing"
        );
    }

    #[tokio::test]
    async fn one_model_lands_and_completes() {
        let server = serve().await;
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.onnx"), dir.path().join("b.onnx"));
        let assets = [
            asset(&server, "ok", "a.onnx", &a),
            asset(&server, "ok", "b.onnx", &b),
        ];
        let downloads = Downloads::default();

        download_url_assets(&downloads, "clip", "CLIP model", &assets).await;

        assert_eq!(std::fs::read(&a).unwrap(), content("a.onnx"));
        assert_eq!(std::fs::read(&b).unwrap(), content("b.onnx"));
        let status = downloads.lock().unwrap()["clip"].clone();
        assert_eq!(status.status, "completed");
        assert_eq!(status.error, None);
        let size = size_of(&["a.onnx", "b.onnx"]);
        assert_eq!((status.progress, status.total), (size, size));
        assert_eq!(status.current_file, None);
    }
}
