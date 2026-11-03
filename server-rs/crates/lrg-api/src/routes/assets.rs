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
//! The combined download goes **family by family** over the download machinery
//! in [`crate::routes::clip`]: one progress bar over one summed total, but a
//! family whose files cannot be resolved or fetched is skipped and reported,
//! and the others still download. It used to be one list of assets that the
//! first failure ended, so a single release that was not reachable — every
//! family has its own tag, published by its own workflow — cost a new user
//! every model, the search model included.
//!
//! **Families already on disk are skipped**, so this is also the natural
//! "finish setting up" button: a user who has SigLIP2 from an older version
//! only downloads BioCLIP, and after a download that skipped a family, pressing
//! it again fetches only that one.

use std::sync::Arc;

use axum::extract::State;
use axum::{routing::get, routing::post, Json, Router};
use serde_json::{json, Value};

use crate::routes::bioclip::bioclip_assets;
use crate::routes::clip::{
    aesthetic_assets, clip_assets, fetch_assets, release_urls, resolve_sizes, with_status,
    Downloads, ModelDownloadStatus, ReleaseAsset, UrlAsset,
};
use crate::state::AppState;

/// Key for this download group in `AppState::model_download`. Distinct from
/// the per-family keys so a combined run and a single-family run cannot
/// overwrite each other's progress.
const DOWNLOAD_KEY: &str = "assets";

/// What the log calls this download.
const LABEL: &str = "AI models";

/// Rough download sizes, for the "this will fetch about N GB" line before the
/// user commits to it.
///
/// Approximate on purpose: the exact total comes from the HEAD pass at
/// download time, and plumbing that through a status endpoint would mean
/// issuing six HEAD requests every time the Plug-in Manager polls.
const CLIP_APPROX_BYTES: u64 = 2_310_000_000;
const AESTHETIC_APPROX_BYTES: u64 = 868_000_000;
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

/// A model family as the user meets it: one indicator in the Plug-in Manager,
/// one entry in `/v1/models/assets`, and the unit the combined download skips
/// when it cannot be fetched.
#[derive(Clone, Copy, Debug)]
enum Family {
    Clip,
    Aesthetic,
    Bioclip,
    Face,
}

impl Family {
    /// Every family, in the order `/v1/models/assets` lists them and the
    /// combined download fetches them.
    const ALL: [Family; 4] = [
        Family::Clip,
        Family::Aesthetic,
        Family::Bioclip,
        Family::Face,
    ];

    fn id(self) -> &'static str {
        match self {
            Family::Clip => "clip",
            Family::Aesthetic => "aesthetic",
            Family::Bioclip => "bioclip",
            Family::Face => "face",
        }
    }

    /// What the user is shown, in the status and in a failed download's
    /// message: the job the family does, not the network that does it.
    fn name(self) -> &'static str {
        match self {
            Family::Clip => "Smart photo search",
            Family::Aesthetic => "Photo quality scoring",
            Family::Bioclip => "Species identification",
            Family::Face => "Face detection",
        }
    }

    fn approx_bytes(self) -> u64 {
        match self {
            Family::Clip => CLIP_APPROX_BYTES,
            Family::Aesthetic => AESTHETIC_APPROX_BYTES,
            Family::Bioclip => BIOCLIP_APPROX_BYTES,
            Family::Face => FACE_APPROX_BYTES,
        }
    }

    /// Whether its files are on disk: "ready to lazy-load", not "loaded".
    fn is_ready(self, state: &AppState) -> bool {
        match self {
            Family::Clip => state.siglip.is_cached(),
            Family::Aesthetic => state.aesthetic.is_cached(),
            Family::Bioclip => state.bioclip.is_cached(),
            Family::Face => state.face.is_cached(),
        }
    }
}

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
/// Every family is downloadable and every family gates `ready`. Face
/// detection used to be neither: its weights were InsightFace's `buffalo_l`,
/// resolved from `INSIGHTFACE_ROOT` — wherever that project's Python library
/// would have put them — and not redistributable, so there was nothing for
/// this endpoint to fetch and it had to be excluded from `ready` to avoid a
/// button that never turned green. YuNet and FaceNet replaced them precisely
/// so that carve-out could go away; `downloadable` stays in the response
/// because the plugin reads it.
async fn assets_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    let families: Vec<(Family, bool)> = Family::ALL
        .into_iter()
        .map(|family| (family, family.is_ready(&state)))
        .collect();
    let missing_bytes: u64 = families
        .iter()
        .filter(|(_, ready)| !ready)
        .map(|(family, _)| family.approx_bytes())
        .sum();

    Json(json!({
        "ready": families.iter().all(|(_, ready)| *ready),
        "missing_approx_bytes": missing_bytes,
        "families": families
            .iter()
            .map(|&(family, ready)| json!({
                "id": family.id(),
                "name": family.name(),
                "ready": ready,
                "downloadable": true,
                "approx_bytes": family.approx_bytes(),
            }))
            .collect::<Vec<_>>(),
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

    let missing: Vec<Family> = Family::ALL
        .into_iter()
        .filter(|family| !family.is_ready(&state))
        .collect();
    tokio::spawn(run_download(state.model_download.clone(), missing));

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

async fn run_download(state: Arc<Downloads>, missing: Vec<Family>) {
    let clip_paths = lrg_ml::model_paths::resolve();
    let aesthetic_paths = lrg_ml::model_paths::resolve_aesthetic();
    let bioclip_paths = lrg_ml::model_paths::resolve_bioclip();
    let face_paths = lrg_ml::model_paths::resolve_face();

    let families: Vec<FamilyDownload<'_>> = missing
        .iter()
        .map(|&family| {
            let assets = match family {
                Family::Clip => clip_assets(&clip_paths),
                Family::Aesthetic => aesthetic_assets(&aesthetic_paths),
                Family::Bioclip => bioclip_assets(&bioclip_paths),
                Family::Face => face_assets(&face_paths),
            };
            FamilyDownload {
                name: family.name(),
                assets: release_urls(&assets),
            }
        })
        .collect();

    if families.is_empty() {
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

    let ids: Vec<&str> = missing.iter().map(|family| family.id()).collect();
    log::info!("Downloading model families: {}", ids.join(", "));
    let already_on_disk = Family::ALL.len() - missing.len();
    download_families(&state, &families, already_on_disk).await;
}

/// One family of a combined download: the name the user knows it by, and its
/// files.
struct FamilyDownload<'a> {
    name: &'a str,
    assets: Vec<UrlAsset<'a>>,
}

/// Downloads `families` under one progress entry, one family at a time.
///
/// A family whose files cannot be resolved or fetched is skipped, and the
/// others still download: a release that is not published yet, a workflow run
/// that failed or a blocked host costs that family, not every model.
///
/// Every family's size is resolved before the first byte, so the bar has its
/// denominator from the start and covers only what will be fetched. A family
/// that fails part-way takes the bytes it will not fetch out of the total
/// again, so the bar still ends full.
///
/// Ends `completed` only when every family landed. Otherwise `error`, with one
/// `"<family name>: <reason>"` per family that did not in `error_messages`,
/// and `error` naming each of them and what to do, which is what the plugin
/// shows. Until then the status stays `downloading`, so the plugin keeps
/// polling and a second request cannot start a run next to this one.
///
/// `already_on_disk` counts the families this run left out because they were
/// there, which the message counts among the ready ones.
async fn download_families(
    state: &Downloads,
    families: &[FamilyDownload<'_>],
    already_on_disk: usize,
) {
    let client = reqwest::Client::new();
    let mut failed: Vec<String> = Vec::new();

    let mut resolved = Vec::with_capacity(families.len());
    let mut total = 0;
    for family in families {
        match resolve_sizes(&client, &family.assets).await {
            Ok(size) => {
                total += size;
                resolved.push((family, size));
            }
            Err(reason) => give_up_on(&mut failed, family, reason),
        }
    }
    with_status(state, DOWNLOAD_KEY, |s| s.total = total);

    let mut downloaded = 0;
    let mut landed = 0;
    for (family, size) in resolved {
        let before = downloaded;
        match fetch_assets(
            &client,
            state,
            DOWNLOAD_KEY,
            LABEL,
            &family.assets,
            &mut downloaded,
        )
        .await
        {
            Ok(()) => landed += 1,
            Err(reason) => {
                let unfetched = size.saturating_sub(downloaded - before);
                with_status(state, DOWNLOAD_KEY, |s| {
                    s.total = s.total.saturating_sub(unfetched);
                });
                give_up_on(&mut failed, family, reason);
            }
        }
    }

    if failed.is_empty() {
        log::info!("{LABEL} download complete ({landed} model families).");
        with_status(state, DOWNLOAD_KEY, |s| {
            s.current_file = None;
            s.set_done();
        });
    } else {
        let summary = failure_summary(&failed, already_on_disk + landed > 0);
        log::error!("{LABEL} download incomplete. {summary}");
        with_status(state, DOWNLOAD_KEY, |s| {
            s.current_file = None;
            s.set_errors(summary, failed);
        });
    }
}

/// Records that `family` is skipped, and why.
fn give_up_on(failed: &mut Vec<String>, family: &FamilyDownload<'_>, reason: String) {
    log::warn!(
        "{LABEL} download: skipping {}, the others go on: {reason}",
        family.name
    );
    failed.push(format!("{}: {reason}", family.name));
}

/// The job's `error` when families did not download: each of them with its
/// reason, then what to do. `others_ready` says whether any other model is on
/// disk, so the message only says so when it is true.
fn failure_summary(failed: &[String], others_ready: bool) -> String {
    let mut text = String::from(if failed.len() == 1 {
        "This AI model could not be downloaded:"
    } else {
        "These AI models could not be downloaded:"
    });
    for line in failed {
        text.push_str("\n- ");
        text.push_str(line);
    }
    text.push_str("\n\n");
    if others_ready {
        text.push_str("The other AI models are downloaded and ready. ");
    }
    text.push_str(
        "Press \"Download AI models\" in the Plug-in Manager again later; it fetches only \
         what is still missing. If it keeps failing, check that the computer the backend \
         runs on can reach github.com.",
    );
    text
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_release::{asset, content, serve, size_of};

    fn status_of(downloads: &Downloads) -> ModelDownloadStatus {
        downloads.lock().unwrap()[DOWNLOAD_KEY].clone()
    }

    /// What this module was changed for: one family's release cannot be
    /// reached (a 404, as for a family whose release is not published yet),
    /// and the families around it download anyway.
    #[tokio::test]
    async fn a_family_whose_release_is_missing_does_not_stop_the_others() {
        let server = serve().await;
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("image.onnx");
        let text = dir.path().join("text.onnx");
        let mesh = dir.path().join("face_mesh_v2.onnx");
        let yunet = dir.path().join("yunet.onnx");
        let families = [
            FamilyDownload {
                name: "Smart photo search",
                assets: vec![
                    asset(&server, "ok", "image.onnx", &image),
                    asset(&server, "ok", "text.onnx", &text),
                ],
            },
            FamilyDownload {
                name: "Eye and expression analysis",
                assets: vec![asset(&server, "gone", "face_mesh_v2.onnx", &mesh)],
            },
            FamilyDownload {
                name: "Face detection",
                assets: vec![asset(&server, "ok", "yunet.onnx", &yunet)],
            },
        ];
        let downloads = Downloads::default();

        download_families(&downloads, &families, 0).await;

        for (dest, name) in [
            (&image, "image.onnx"),
            (&text, "text.onnx"),
            (&yunet, "yunet.onnx"),
        ] {
            assert_eq!(std::fs::read(dest).unwrap(), content(name), "{name}");
        }
        assert!(!mesh.exists());

        let status = status_of(&downloads);
        assert_eq!(status.status, "error");
        assert_eq!(
            status.error_messages,
            [format!(
                "Eye and expression analysis: face_mesh_v2.onnx not found at \
                 {server}/gone/face_mesh_v2.onnx (HTTP 404 Not Found) — the release may \
                 not exist yet"
            )]
        );
        let error = status.error.unwrap();
        assert!(error.contains(&status.error_messages[0]), "{error}");
        assert!(
            !error.contains("Smart photo search") && !error.contains("Face detection"),
            "only the failed family is named: {error}"
        );
        assert!(
            error.contains("The other AI models are downloaded and ready."),
            "{error}"
        );

        // The bar covered what was fetched from the start, and ended full.
        let landed = size_of(&["image.onnx", "text.onnx", "yunet.onnx"]);
        assert_eq!((status.progress, status.total), (landed, landed));
        assert_eq!(status.current_file, None);
    }

    /// A family whose transfer breaks after its size was resolved is skipped
    /// too: the next family still downloads, and the bytes the broken one will
    /// not fetch leave the total, so the bar ends full.
    #[tokio::test]
    async fn a_family_that_breaks_during_the_transfer_is_skipped() {
        let server = serve().await;
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("bioclip_image.onnx");
        let taxa = dir.path().join("bioclip_taxa.bin");
        let yunet = dir.path().join("yunet.onnx");
        let families = [
            FamilyDownload {
                name: "Species identification",
                assets: vec![
                    asset(&server, "ok", "bioclip_image.onnx", &image),
                    asset(&server, "flaky", "bioclip_taxa.bin", &taxa),
                ],
            },
            FamilyDownload {
                name: "Face detection",
                assets: vec![asset(&server, "ok", "yunet.onnx", &yunet)],
            },
        ];
        let downloads = Downloads::default();

        download_families(&downloads, &families, 0).await;

        assert_eq!(std::fs::read(&yunet).unwrap(), content("yunet.onnx"));
        assert!(!taxa.exists());

        let status = status_of(&downloads);
        assert_eq!(status.status, "error");
        assert_eq!(
            status.error_messages,
            ["Species identification: download failed for bioclip_taxa.bin: HTTP 502 Bad Gateway"]
        );
        let fetched = size_of(&["bioclip_image.onnx", "yunet.onnx"]);
        assert_eq!((status.progress, status.total), (fetched, fetched));
    }

    #[tokio::test]
    async fn every_family_landing_completes_without_errors() {
        let server = serve().await;
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("image.onnx");
        let yunet = dir.path().join("yunet.onnx");
        let families = [
            FamilyDownload {
                name: "Smart photo search",
                assets: vec![asset(&server, "ok", "image.onnx", &image)],
            },
            FamilyDownload {
                name: "Face detection",
                assets: vec![asset(&server, "ok", "yunet.onnx", &yunet)],
            },
        ];
        let downloads = Downloads::default();

        download_families(&downloads, &families, 2).await;

        assert_eq!(std::fs::read(&image).unwrap(), content("image.onnx"));
        assert_eq!(std::fs::read(&yunet).unwrap(), content("yunet.onnx"));
        let status = status_of(&downloads);
        assert_eq!(status.status, "completed");
        assert_eq!(status.error, None);
        assert!(status.error_messages.is_empty());
        let size = size_of(&["image.onnx", "yunet.onnx"]);
        assert_eq!((status.progress, status.total), (size, size));
    }

    #[test]
    fn the_summary_names_each_failure_and_what_to_do() {
        let one = failure_summary(&["Species identification: HTTP 404".to_string()], true);
        assert!(
            one.starts_with(
                "This AI model could not be downloaded:\n- Species identification: HTTP 404\n\n\
                 The other AI models are downloaded and ready. Press \"Download AI models\""
            ),
            "{one}"
        );

        // Nothing else on disk: no claim that something is ready.
        let all = failure_summary(
            &[
                "Smart photo search: no route to host".to_string(),
                "Face detection: no route to host".to_string(),
            ],
            false,
        );
        assert!(
            all.starts_with(
                "These AI models could not be downloaded:\n- Smart photo search: no route to \
                 host\n- Face detection: no route to host\n\nPress \"Download AI models\""
            ),
            "{all}"
        );
        assert!(!all.contains("ready"), "{all}");
    }
}
