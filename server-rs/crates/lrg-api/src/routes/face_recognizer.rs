//! Which network makes the face identity embeddings: FaceNet (the default,
//! part of "Download AI models") or AuraFace (opt-in, its own download).
//!
//! The choice is the backend's, not a request option, because it decides what
//! every stored face row means: `lrg_ml::faces::Recognizer::model_id` is
//! stamped on each row, clustering and "Find Similar Faces" only compare rows
//! of the active model, and `/v1/index/unprocessed` re-queues photos whose
//! faces another model found. So a switch is safe — nothing is mixed — but it
//! only takes full effect once the photos with faces are indexed again (with
//! face detection on). The names the user gave survive that re-index: the
//! detector is the same, so `index_upload` carries them over by box.
//!
//! The choice is saved in `lrg_ml::model_paths::face_recognizer_setting()`
//! and restored when the backend starts ([`restore`]).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{routing::get, routing::post, Json, Router};
use serde_json::{json, Value};

use lrg_ml::faces::{FaceModel, Recognizer};

use crate::routes::clip::{download_url_assets, Downloads, ModelDownloadStatus};
use crate::state::AppState;

/// Key for the AuraFace download in `AppState::model_download`.
const DOWNLOAD_KEY: &str = "face_recognizer";

/// Where AuraFace is published, Apache-2.0.
const AURAFACE_REPO: &str = "fal/AuraFace-v1";
/// The repository revision downloaded. `main` until the exact commit has been
/// verified against `scripts/auraface_reference.py`'s reference embedding;
/// pin it then, like YuNet's commit in `export_face_models.py`.
const AURAFACE_REVISION: &str = "main";
/// The recognition network in that repository (the others are InsightFace's
/// detector and attribute models, which this pipeline does not use).
const AURAFACE_FILE: &str = "glintr100.onnx";
/// Rough size for the "this downloads about N MB" line; the HEAD pass at
/// download time has the exact one.
const AURAFACE_APPROX_BYTES: u64 = 261_000_000;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/faces/recognizer", get(status).post(select))
        .route(
            "/faces/recognizer/downloads",
            post(download_start).get(download_status),
        )
}

/// Applies the saved choice at startup. A saved AuraFace whose file is gone
/// falls back to FaceNet, loudly in the log; `GET /v1/faces/recognizer` then
/// reports `saved: "auraface"` next to `active: "facenet"`, which the
/// Plug-in Manager shows.
pub fn restore(face: &FaceModel) {
    let path = lrg_ml::model_paths::face_recognizer_setting();
    let Some(saved) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| Recognizer::parse(&s))
    else {
        return;
    };
    if let Err(e) = face.set_recognizer(saved) {
        log::warn!(
            "The saved face recognizer ({}) cannot be used, staying on {}: {e}",
            saved.as_str(),
            face.recognizer().as_str()
        );
    }
}

fn saved_choice() -> Option<Recognizer> {
    std::fs::read_to_string(lrg_ml::model_paths::face_recognizer_setting())
        .ok()
        .and_then(|s| Recognizer::parse(&s))
}

fn describe(state: &AppState) -> Value {
    let face = &state.face;
    let active = face.recognizer();
    let saved = saved_choice().unwrap_or_default();
    let paths = face.paths();
    let entry = |r: Recognizer, name: &str, about: &str, approx: u64, downloadable: bool| {
        json!({
            "id": r.as_str(),
            "name": name,
            "description": about,
            "ready": paths.ready_for(r),
            "active": r == active,
            "model_id": r.model_id(),
            "approx_bytes": approx,
            "downloadable": downloadable,
        })
    };
    let mut warnings = Vec::new();
    if saved != active {
        warnings.push(format!(
            "{} was chosen, but its model file is missing, so {} is in use. Download it again, or choose {}.",
            name_of(saved),
            name_of(active),
            name_of(active),
        ));
    }
    json!({
        "active": active.as_str(),
        "saved": saved.as_str(),
        "model_id": active.model_id(),
        "recognizers": [
            entry(
                Recognizer::FaceNet,
                name_of(Recognizer::FaceNet),
                "Included in the AI model download. Good for telling family and friends apart.",
                0,
                false,
            ),
            entry(
                Recognizer::AuraFace,
                name_of(Recognizer::AuraFace),
                "More accurate, especially for similar-looking people, across ages and in \
                 difficult light. A separate download of about 260 MB; slower per face.",
                AURAFACE_APPROX_BYTES,
                true,
            ),
        ],
        "warnings": warnings,
    })
}

fn name_of(r: Recognizer) -> &'static str {
    match r {
        Recognizer::FaceNet => "FaceNet",
        Recognizer::AuraFace => "AuraFace",
    }
}

async fn status(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(describe(&state))
}

/// `{"recognizer": "facenet" | "auraface"}`: switches and saves the choice.
async fn select(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let data = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let Some(wanted) = data
        .get("recognizer")
        .and_then(Value::as_str)
        .and_then(Recognizer::parse)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "recognizer must be \"facenet\" or \"auraface\""})),
        )
            .into_response();
    };
    let before = state.face.recognizer();
    if let Err(e) = state.face.set_recognizer(wanted) {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!("{} cannot be used yet: {e}. Download it first.", name_of(wanted)),
                "error_code": "not_downloaded",
            })),
        )
            .into_response();
    }

    let setting = lrg_ml::model_paths::face_recognizer_setting();
    let saved = setting
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&setting, wanted.as_str()));

    let mut body = describe(&state);
    let mut warnings: Vec<Value> = Vec::new();
    if let Err(e) = saved {
        log::error!(
            "Could not save the face recognizer choice to {}: {e}",
            setting.display()
        );
        warnings.push(json!(format!(
            "{} is in use now, but the choice could not be saved ({e}), so the backend \
             goes back to its previous one when it restarts.",
            name_of(wanted)
        )));
    } else if let Some(w) = body["warnings"].as_array() {
        warnings.extend(w.iter().cloned());
    }
    if before != wanted {
        warnings.push(json!(format!(
            "Faces found with {} are set aside until their photos are indexed again with \
             face detection on. Run Analyze & Index Photos with \"New or unprocessed photos\" \
             to do that; the names you gave carry over.",
            name_of(before)
        )));
    }
    body["warnings"] = Value::Array(warnings);
    body["changed"] = json!(before != wanted);
    Json(body).into_response()
}

async fn download_start(State(state): State<Arc<AppState>>) -> Json<Value> {
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
    if !already_running {
        tokio::spawn(run_download(state.model_download.clone()));
    }
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

async fn run_download(downloads: Arc<Downloads>) {
    let paths = lrg_ml::model_paths::resolve_face();
    let url = crate::llm_models::hf_url(AURAFACE_REPO, AURAFACE_REVISION, AURAFACE_FILE);
    let assets = [(url, AURAFACE_FILE, paths.auraface_onnx.as_path())];
    download_url_assets(&downloads, DOWNLOAD_KEY, "AuraFace", &assets).await;
}
