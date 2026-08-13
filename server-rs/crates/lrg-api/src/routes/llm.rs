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

use crate::llm_engine::{settings_for, EngineOverrides};
use crate::llm_models::{self, CatalogEntry};
use crate::mlx_engine::MlxEngineSettings;
use crate::mlx_models;
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
/// Returns `Ok(None)` for every provider other than the two local backends, so
/// callers can use it unconditionally. An unresolvable model name is an `Err`
/// with a user-facing message rather than a silent fallback: quietly indexing a
/// whole catalog with the wrong provider would be far worse than a clear
/// failure.
pub async fn engine_for_request(
    state: &AppState,
    provider: &str,
    model: &str,
    overrides: EngineOverrides,
) -> Result<Option<SharedLocalEngine>, String> {
    match provider.trim().to_lowercase().as_str() {
        "llamacpp" => {
            let Some(found) = llm_models::resolve_model(model) else {
                return Err(format!(
                    "No local model named '{model}' was found. Download one from the plugin \
                     settings, or set LRG_LLAMA_MODEL_GGUF to a GGUF file."
                ));
            };
            let settings = settings_for(found.model_path, found.mmproj_path, overrides);
            state.llm.get_or_load(&settings).await.map(Some)
        }
        "mlx" => {
            // Refuse early on a host that cannot run MLX at all, so the user
            // gets "needs Apple silicon" rather than a sidecar spawn failure.
            let availability = state.mlx.availability();
            if !availability.supported {
                return Err(availability
                    .reason
                    .unwrap_or_else(|| "MLX is not available on this system.".to_string()));
            }
            let Some(found) = mlx_models::resolve_model(model) else {
                return Err(format!(
                    "No MLX model named '{model}' was found. Download one from the plugin \
                     settings, or set LRG_MLX_MODEL_DIR to a model directory."
                ));
            };
            // No `overrides`: MLX has no context-size or GPU-layer equivalent
            // to tune. See `crate::mlx_engine`.
            let settings = MlxEngineSettings {
                model_dir: found.model_dir,
            };
            state.mlx.get_or_load(&settings).await.map(Some)
        }
        _ => Ok(None),
    }
}

/// Read the engine tuning knobs the plugin's advanced fields send.
///
/// Absent, unparseable and zero all mean "unspecified", so a blank field in the
/// dialog falls back to the environment and then to the default rather than
/// asking for a zero-sized context window.
#[must_use]
pub fn engine_overrides_from_fields(
    fields: &std::collections::HashMap<String, String>,
) -> EngineOverrides {
    let raw = |key: &str| fields.get(key).and_then(|v| v.trim().parse::<u32>().ok());
    // Zero is nonsense for a context window or a sequence count, but it is a
    // real choice for GPU layers: it means "run entirely on the CPU".
    let positive = |key: &str| raw(key).filter(|n| *n > 0);
    EngineOverrides {
        n_ctx: positive("llm_n_ctx"),
        n_parallel: positive("llm_n_parallel"),
        n_gpu_layers: raw("llm_gpu_layers"),
    }
}

/// Same knobs, read from a JSON body.
///
/// The plugin sends numbers on the JSON routes and strings on the multipart
/// ones, so both forms are accepted here rather than making the caller care.
#[must_use]
pub fn engine_overrides_from_json(data: &Value) -> EngineOverrides {
    let raw = |key: &str| {
        data.get(key).and_then(|v| match v {
            Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
            Value::String(s) => s.trim().parse::<u32>().ok(),
            _ => None,
        })
    };
    let positive = |key: &str| raw(key).filter(|n| *n > 0);
    EngineOverrides {
        n_ctx: positive("llm_n_ctx"),
        n_parallel: positive("llm_n_parallel"),
        n_gpu_layers: raw("llm_gpu_layers"),
    }
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

    // The MLX half is reported alongside rather than as a separate route: the
    // plugin renders one "Local AI Model" panel and needs both backends'
    // availability to decide what to offer, and two round-trips to draw one
    // panel would only invite the two halves to disagree.
    let mlx_installed = mlx_models::discover_local_models();
    let mlx_installed_dirs: std::collections::HashSet<&str> = mlx_installed
        .iter()
        .map(|m| m.name.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mlx_downloadable: Vec<Value> = mlx_models::CATALOG
        .iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "label": entry.label,
                "approx_bytes": entry.approx_bytes,
                "min_ram_gb": entry.min_ram_gb,
                "installed": mlx_installed_dirs.contains(entry.dir_name),
            })
        })
        .collect();
    let mlx_availability = state.mlx.availability();

    Json(json!({
        "installed": installed,
        "downloadable": downloadable,
        "supported": state.llm.is_supported(),
        "model_dir": lrg_ml::model_paths::resolve_llm().dir.display().to_string(),
        "mlx": {
            "installed": mlx_installed,
            "downloadable": mlx_downloadable,
            "supported": mlx_availability.supported,
            "reason": mlx_availability.reason,
            "model_dir": lrg_ml::model_paths::resolve_mlx().dir.display().to_string(),
        },
    }))
}

async fn status(State(state): State<Arc<AppState>>) -> Json<Value> {
    let mut body = json!(state.llm.status());
    // Nested under `mlx` so the existing top-level keys keep meaning exactly
    // what they meant before: an older plugin reading `status` still sees the
    // llama.cpp engine and is not confused by a second one.
    if let Some(object) = body.as_object_mut() {
        object.insert("mlx".to_string(), json!(state.mlx.status()));
    }
    Json(body)
}

async fn download_start(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let data = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let id = data.get("id").and_then(Value::as_str).unwrap_or_default();

    // One id space across both catalogs (enforced by a test in `mlx_models`),
    // so the id alone picks the backend and the plugin needs no extra field.
    enum Wanted {
        Gguf(&'static CatalogEntry),
        Mlx(&'static mlx_models::CatalogEntry),
    }
    let wanted = llm_models::catalog_entry(id)
        .map(Wanted::Gguf)
        .or_else(|| mlx_models::catalog_entry(id).map(Wanted::Mlx));
    let Some(wanted) = wanted else {
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
    match wanted {
        Wanted::Gguf(entry) => tokio::spawn(run_download(downloads, entry)),
        Wanted::Mlx(entry) => tokio::spawn(run_mlx_download(downloads, entry)),
    };
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

/// Files in a repo snapshot that are never worth downloading.
///
/// The `original/` and `onnx/` filter is the one that matters: some Hugging
/// Face repos carry the unquantized source weights or an unrelated export
/// alongside the MLX ones, and fetching those would silently double or treble a
/// multi-gigabyte download for files the loader never opens.
fn is_skippable_repo_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("original/")
        || lower.starts_with("onnx/")
        || lower.starts_with(".git")
        || lower.ends_with(".md")
}

/// List a repo's files via the Hugging Face tree API.
async fn repo_files(
    client: &reqwest::Client,
    repo: &str,
    revision: &str,
) -> Result<Vec<(String, u64)>, String> {
    let url = format!("https://huggingface.co/api/models/{repo}/tree/{revision}?recursive=true");
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("could not list {repo}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("could not list {repo}: HTTP {}", response.status()));
    }
    let entries: Vec<Value> = response
        .json()
        .await
        .map_err(|e| format!("could not read the file list for {repo}: {e}"))?;

    let mut files: Vec<(String, u64)> = entries
        .iter()
        .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("file"))
        .filter_map(|entry| {
            let path = entry.get("path").and_then(Value::as_str)?.to_string();
            let size = entry.get("size").and_then(Value::as_u64).unwrap_or(0);
            (!is_skippable_repo_file(&path)).then_some((path, size))
        })
        .collect();

    // Some repos carry a stale `model.safetensors.index.json` left over from
    // before the weights were consolidated into a single `model.safetensors`
    // (observed on lmstudio-community/Qwen3-VL-4B-Instruct-MLX-4bit — the
    // index's weight_map still points at `model-0000N-of-0000N.safetensors`
    // shards that no longer exist in the repo). mlx-swift-lm trusts the index
    // over the consolidated file when both are present, so downloading it
    // produces a directory that fails to load with a confusing "not
    // supported" error. A monolithic `model.safetensors` needs no index, so
    // drop the index whenever both live in the same directory.
    let monolithic_dirs: std::collections::HashSet<String> = files
        .iter()
        .filter_map(|(path, _)| {
            let (dir, name) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
            (name == "model.safetensors").then(|| dir.to_string())
        })
        .collect();
    files.retain(|(path, _)| {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
        name != "model.safetensors.index.json" || !monolithic_dirs.contains(dir)
    });

    if files.is_empty() {
        return Err(format!("{repo} contains no downloadable model files"));
    }
    Ok(files)
}

/// Download a whole MLX model snapshot.
///
/// Unlike the GGUF path this fetches a directory, so it stages into a sibling
/// `.part` directory and renames at the end. Discovery only accepts a directory
/// holding both a `config.json` and safetensors shards, and a partial download
/// can satisfy that check partway through — staging elsewhere means a
/// half-finished model is never offered as loadable.
async fn run_mlx_download(downloads: Downloads, entry: &'static mlx_models::CatalogEntry) {
    let destination = mlx_models::destination_for(entry);
    if destination.is_dir() {
        let mut guard = downloads.lock().unwrap();
        guard
            .entry(DOWNLOAD_KEY.to_string())
            .or_default()
            .set_done();
        log::info!("MLX model {} is already installed", entry.id);
        return;
    }
    let staging = destination.with_extension("part");
    // A previous interrupted attempt leaves this behind; start clean rather
    // than resuming, since we cannot tell which files are complete.
    let _ = tokio::fs::remove_dir_all(&staging).await;
    if let Err(e) = tokio::fs::create_dir_all(&staging).await {
        return set_error(
            &downloads,
            format!("failed to create {}: {e}", staging.display()),
        );
    }

    let client = reqwest::Client::new();
    let files = match repo_files(&client, entry.repo, entry.revision).await {
        Ok(files) => files,
        Err(e) => return set_error(&downloads, e),
    };

    let total: u64 = files.iter().map(|(_, size)| *size).sum();
    downloads
        .lock()
        .unwrap()
        .entry(DOWNLOAD_KEY.to_string())
        .or_default()
        .total = total;

    let mut downloaded: u64 = 0;
    for (path, expected) in &files {
        let target = staging.join(path);
        if let Some(parent) = target.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return set_error(
                    &downloads,
                    format!("failed to create {}: {e}", parent.display()),
                );
            }
        }
        downloads
            .lock()
            .unwrap()
            .entry(DOWNLOAD_KEY.to_string())
            .or_default()
            .current_file = Some(path.clone());

        let url = llm_models::hf_url(entry.repo, entry.revision, path);
        log::info!("Downloading MLX model asset {path} from {url}");
        let response = match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                return set_error(
                    &downloads,
                    format!("download failed for {path}: HTTP {}", r.status()),
                )
            }
            Err(e) => return set_error(&downloads, format!("download failed for {path}: {e}")),
        };

        let handle = match tokio::fs::File::create(&target).await {
            Ok(f) => f,
            Err(e) => {
                return set_error(
                    &downloads,
                    format!("failed to create {}: {e}", target.display()),
                )
            }
        };
        let mut writer = tokio::io::BufWriter::new(handle);
        let mut stream = response.bytes_stream();
        let mut written: u64 = 0;

        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    let _ = tokio::fs::remove_dir_all(&staging).await;
                    return set_error(&downloads, format!("download interrupted for {path}: {e}"));
                }
            };
            if let Err(e) = writer.write_all(&chunk).await {
                let _ = tokio::fs::remove_dir_all(&staging).await;
                return set_error(&downloads, format!("failed writing {path}: {e}"));
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
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return set_error(&downloads, format!("failed writing {path}: {e}"));
        }
        drop(writer);

        if *expected > 0 && written != *expected {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return set_error(
                &downloads,
                format!(
                    "{path} was truncated: expected {expected} bytes but received {written}. \
                     Check your connection and try again."
                ),
            );
        }
    }

    if let Err(e) = tokio::fs::rename(&staging, &destination).await {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return set_error(
            &downloads,
            format!(
                "failed to place the model at {}: {e}",
                destination.display()
            ),
        );
    }

    let mut guard = downloads.lock().unwrap();
    guard
        .entry(DOWNLOAD_KEY.to_string())
        .or_default()
        .set_done();
    log::info!(
        "MLX model {} downloaded to {}",
        entry.id,
        destination.display()
    );
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

    #[test]
    fn engine_overrides_read_the_advanced_fields() {
        let fields: std::collections::HashMap<String, String> = [
            ("llm_n_ctx", "16384"),
            ("llm_n_parallel", "4"),
            ("llm_gpu_layers", "20"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
        let o = engine_overrides_from_fields(&fields);
        assert_eq!(o.n_ctx, Some(16384));
        assert_eq!(o.n_parallel, Some(4));
        assert_eq!(o.n_gpu_layers, Some(20));
    }

    /// A blank or nonsense field must mean "unspecified" so the environment and
    /// then the built-in default still apply — never a zero-sized context.
    #[test]
    fn engine_overrides_ignore_blank_and_zero() {
        let fields: std::collections::HashMap<String, String> = [
            ("llm_n_ctx", ""),
            ("llm_n_parallel", "0"),
            ("llm_gpu_layers", "lots"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
        let o = engine_overrides_from_fields(&fields);
        assert_eq!(o.n_ctx, None);
        assert_eq!(o.n_parallel, None);
        assert_eq!(o.n_gpu_layers, None);
    }

    /// Zero GPU layers is a real request — run on the CPU — so unlike the other
    /// two knobs it must survive rather than being read as "unspecified".
    #[test]
    fn zero_gpu_layers_means_cpu_only() {
        let fields: std::collections::HashMap<String, String> =
            [("llm_gpu_layers".to_string(), "0".to_string())].into();
        assert_eq!(engine_overrides_from_fields(&fields).n_gpu_layers, Some(0));
        assert_eq!(
            engine_overrides_from_json(&json!({"llm_gpu_layers": 0})).n_gpu_layers,
            Some(0)
        );
    }

    /// The JSON routes send numbers where the multipart routes send strings.
    #[test]
    fn engine_overrides_accept_json_numbers_and_strings() {
        let o = engine_overrides_from_json(&json!({"llm_n_ctx": 8192, "llm_n_parallel": "3"}));
        assert_eq!(o.n_ctx, Some(8192));
        assert_eq!(o.n_parallel, Some(3));
        assert_eq!(o.n_gpu_layers, None);
        assert_eq!(
            engine_overrides_from_json(&json!({"llm_n_ctx": 0})).n_ctx,
            None
        );
    }

    #[tokio::test]
    async fn engine_for_request_ignores_other_providers() {
        let state = AppState::new(None, false);
        for provider in ["ollama", "chatgpt", "gemini", "lmstudio"] {
            let resolved =
                engine_for_request(&state, provider, "whatever", EngineOverrides::default()).await;
            assert!(
                matches!(resolved, Ok(None)),
                "{provider} must not touch the local engine"
            );
        }
    }

    #[tokio::test]
    async fn engine_for_request_explains_a_missing_model() {
        let state = AppState::new(None, false);
        let err = engine_for_request(
            &state,
            "llamacpp",
            "no-such-model.gguf",
            EngineOverrides::default(),
        )
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
