//! `/llm/*` — the catalog of local models, downloading them, and engine status.
//!
//! Modelled on `routes/clip.rs`'s download flow, with two deliberate changes:
//!
//! * downloads are keyed (`"llm"` vs `"clip"`) so a GGUF and a SigLIP2 asset can
//!   be fetched at once instead of one silently clobbering the other's progress;
//! * the finished file is size-checked against what the server said it would
//!   send, so a truncated download is caught here rather than as a baffling
//!   "failed to load model" an hour later.
//!
//! There is deliberately **no** hard-coded sha256 for catalog entries. Hugging
//! Face repos are mutable, so a digest pinned at compile time would either go
//! stale and block legitimate downloads, or be silently ignored — the transport
//! is TLS-authenticated, and a length check catches the failure mode that
//! actually occurs (an interrupted stream).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::{routing::get, routing::post, Json, Router};
use futures_util::StreamExt;
use serde_json::{json, Value};

use lrg_providers::local::SharedLocalEngine;

use crate::llm_engine::settings_for;
use crate::llm_models::{self, CatalogEntry};
use crate::routes::clip::ModelDownloadStatus;
use crate::state::AppState;

/// Key for this download group in `AppState::model_download`.
pub const DOWNLOAD_KEY: &str = "llm";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/llm/catalog", get(catalog))
        .route("/llm/status", get(status))
        .route("/llm/download/start", post(download_start))
        .route("/llm/download/status", get(download_status))
}

/// Resolve the engine for a request, loading the model if needed.
///
/// Returns `Ok(None)` for every provider other than `llamacpp`, so callers can
/// use it unconditionally. An unresolvable model name is an `Err` with a
/// user-facing message rather than a silent fallback: quietly indexing a whole
/// catalog with the wrong provider would be far worse than a clear failure.
pub async fn engine_for_request(
    state: &AppState,
    provider: &str,
    model: &str,
) -> Result<Option<SharedLocalEngine>, String> {
    if provider.trim().to_lowercase() != "llamacpp" {
        return Ok(None);
    }
    let Some(found) = llm_models::resolve_model(model) else {
        return Err(format!(
            "No local model named '{model}' was found. Download one from the plugin settings, \
             or set LRG_LLAMA_MODEL_GGUF to a GGUF file."
        ));
    };
    let settings = settings_for(found.model_path, found.mmproj_path);
    state.llm.get_or_load(&settings).await.map(Some)
}

async fn catalog(State(state): State<Arc<AppState>>) -> Json<Value> {
    let installed = llm_models::discover_local_models();
    let installed_names: std::collections::HashSet<&str> =
        installed.iter().map(|m| m.name.as_str()).collect();

    // A catalog entry whose file is already on disk is not offered for download
    // again — it shows up in `installed` instead.
    let downloadable: Vec<Value> = llm_models::CATALOG
        .iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "label": entry.label,
                "approx_bytes": entry.approx_bytes,
                "min_ram_gb": entry.min_ram_gb,
                "installed": installed_names.contains(entry.model_file),
            })
        })
        .collect();

    Json(json!({
        "installed": installed,
        "downloadable": downloadable,
        "supported": state.llm.is_supported(),
        "model_dir": lrg_ml::model_paths::resolve_llm().dir.display().to_string(),
    }))
}

async fn status(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!(state.llm.status()))
}

async fn download_start(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let data = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let id = data.get("id").and_then(Value::as_str).unwrap_or_default();

    let Some(entry) = llm_models::catalog_entry(id) else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("Unknown model id '{id}'.")})),
        )
            .into_response();
    };

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
        log::warn!("A local-model download is already running.");
        return Json(json!({"download": "started"})).into_response();
    }

    let downloads = state.model_download.clone();
    tokio::spawn(run_download(downloads, entry));
    Json(json!({"download": "started"})).into_response()
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

type Downloads = Arc<Mutex<std::collections::HashMap<String, ModelDownloadStatus>>>;

fn set_error(downloads: &Downloads, msg: String) {
    log::error!("Local model download failed: {msg}");
    let mut guard = downloads.lock().unwrap();
    let status = guard.entry(DOWNLOAD_KEY.to_string()).or_default();
    status.set_error(msg);
}

async fn run_download(downloads: Downloads, entry: &'static CatalogEntry) {
    let dir = lrg_ml::model_paths::resolve_llm().dir;
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return set_error(
            &downloads,
            format!("failed to create model directory {}: {e}", dir.display()),
        );
    }

    let client = reqwest::Client::new();
    let files = [entry.model_file, entry.mmproj_file];

    // Resolve sizes first so the progress bar has a denominator from step 0.
    // Hugging Face redirects `resolve` to a CDN, which reqwest follows.
    let mut total: u64 = 0;
    let mut sizes = Vec::with_capacity(files.len());
    for file in files {
        let url = llm_models::hf_url(entry.repo, entry.revision, file);
        let resp = match client.head(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                return set_error(
                    &downloads,
                    format!("{file} is not available (HTTP {})", r.status()),
                )
            }
            Err(e) => return set_error(&downloads, format!("failed to resolve {file}: {e}")),
        };
        let size = resp.content_length().unwrap_or(0);
        sizes.push(size);
        total += size;
    }
    downloads
        .lock()
        .unwrap()
        .entry(DOWNLOAD_KEY.to_string())
        .or_default()
        .total = total;

    let mut downloaded: u64 = 0;
    for (file, expected) in files.iter().zip(sizes) {
        let dest = dir.join(file);
        if dest.is_file() {
            // Already present from an earlier run; count it and move on.
            downloaded += expected;
            continue;
        }
        downloads
            .lock()
            .unwrap()
            .entry(DOWNLOAD_KEY.to_string())
            .or_default()
            .current_file = Some((*file).to_string());

        let url = llm_models::hf_url(entry.repo, entry.revision, file);
        log::info!("Downloading local model asset {file} from {url}");
        let resp = match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                return set_error(
                    &downloads,
                    format!("download failed for {file}: HTTP {}", r.status()),
                )
            }
            Err(e) => return set_error(&downloads, format!("download failed for {file}: {e}")),
        };

        // Write beside the destination and rename, so an interrupted download
        // never leaves a half-file that discovery would happily offer.
        let tmp = dest.with_extension("gguf.part");
        let handle = match tokio::fs::File::create(&tmp).await {
            Ok(f) => f,
            Err(e) => {
                return set_error(
                    &downloads,
                    format!("failed to create {}: {e}", tmp.display()),
                )
            }
        };
        let mut writer = tokio::io::BufWriter::new(handle);
        let mut stream = resp.bytes_stream();
        let mut written: u64 = 0;

        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    let _ = tokio::fs::remove_file(&tmp).await;
                    return set_error(&downloads, format!("download interrupted for {file}: {e}"));
                }
            };
            if let Err(e) = writer.write_all(&chunk).await {
                let _ = tokio::fs::remove_file(&tmp).await;
                return set_error(&downloads, format!("failed writing {file}: {e}"));
            }
            written += chunk.len() as u64;
            downloaded += chunk.len() as u64;
            downloads
                .lock()
                .unwrap()
                .entry(DOWNLOAD_KEY.to_string())
                .or_default()
                .progress = downloaded;
        }
        if let Err(e) = writer.flush().await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return set_error(&downloads, format!("failed writing {file}: {e}"));
        }
        drop(writer);

        if expected > 0 && written != expected {
            let _ = tokio::fs::remove_file(&tmp).await;
            return set_error(
                &downloads,
                format!(
                    "{file} was truncated: expected {expected} bytes but received {written}. \
                     Check your connection and try again."
                ),
            );
        }
        if let Err(e) = tokio::fs::rename(&tmp, &dest).await {
            return set_error(
                &downloads,
                format!("failed to place {file} at {}: {e}", dest.display()),
            );
        }
        log::info!("Local model asset ready: {}", dest.display());
    }

    let mut guard = downloads.lock().unwrap();
    let status = guard.entry(DOWNLOAD_KEY.to_string()).or_default();
    status.set_done();
    log::info!("Local model {} downloaded", entry.id);
}

/// Where a catalog entry's files end up, for tests and for the UI.
#[must_use]
pub fn destination_for(entry: &CatalogEntry) -> (PathBuf, PathBuf) {
    let dir = lrg_ml::model_paths::resolve_llm().dir;
    (dir.join(entry.model_file), dir.join(entry.mmproj_file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn engine_for_request_ignores_other_providers() {
        let state = AppState::new(None, false);
        for provider in ["ollama", "chatgpt", "gemini", "lmstudio"] {
            let resolved = engine_for_request(&state, provider, "whatever").await;
            assert!(
                matches!(resolved, Ok(None)),
                "{provider} must not touch the local engine"
            );
        }
    }

    #[tokio::test]
    async fn engine_for_request_explains_a_missing_model() {
        let state = AppState::new(None, false);
        let err = engine_for_request(&state, "llamacpp", "no-such-model.gguf")
            .await
            .err()
            .expect("a missing model must fail loudly, not fall back");
        assert!(err.contains("no-such-model.gguf"));
        assert!(err.contains("LRG_LLAMA_MODEL_GGUF"));
    }

    #[test]
    fn catalog_files_land_in_the_discovery_directory() {
        let dir = lrg_ml::model_paths::resolve_llm().dir;
        for entry in llm_models::CATALOG {
            let (model, mmproj) = destination_for(entry);
            // Discovery scans this directory, so a downloaded model must appear
            // in `/models` without any extra bookkeeping.
            assert_eq!(model.parent(), Some(dir.as_path()));
            assert_eq!(mmproj.parent(), Some(dir.as_path()));
        }
    }
}
