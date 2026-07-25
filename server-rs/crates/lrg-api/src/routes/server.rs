//! Server blueprint — port of `routes/server.py`. `/update/apply` gains
//! real behavior in M9 (model distribution / self-update pipeline).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path as UrlPath, State},
    http::{header::CONTENT_TYPE, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

use lrg_common::{lifecycle, logging, version};

use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ping", get(ping))
        .route("/shutdown", post(shutdown))
        .route("/unload", post(unload))
        .route("/restart", post(restart))
        .route("/initialize", post(initialize))
        .route("/version", get(get_version))
        .route("/version/check", post(version_check))
        .route("/health", get(health))
        .route("/logs", get(get_logs))
        .route("/logs/raw/{log_type}", get(get_raw_log))
        .route("/models", get(list_models).post(list_models))
}

#[derive(serde::Deserialize, Default)]
struct ModelsQuery {
    openai_apikey: Option<String>,
    gemini_apikey: Option<String>,
    ollama_base_url: Option<String>,
    #[allow(dead_code)]
    lmstudio_base_url: Option<String>,
}

/// Port of `list_models`. Scope for M7: Ollama (local REST probe) and
/// OpenAI (vision-model allowlist via `/v1/models`, needs an API key).
/// Gemini/LM Studio/Qwen return empty lists until their providers land.
async fn list_models(
    axum::extract::Query(q): axum::extract::Query<ModelsQuery>,
    body: Option<Json<Value>>,
) -> Json<Value> {
    log::info!("Models request received - checking all providers");
    let data = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));

    let openai_key = data
        .get("openai_apikey")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(q.openai_apikey);
    let ollama_base_url = data
        .get("ollama_base_url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(q.ollama_base_url);
    let _gemini_key = data
        .get("gemini_apikey")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(q.gemini_apikey);

    let ollama = lrg_providers::ollama::OllamaProvider::new(ollama_base_url);
    let ollama_models = if ollama.is_available().await {
        ollama.list_available_models().await
    } else {
        Vec::new()
    };

    let openai_models = match openai_key {
        Some(key) if !key.is_empty() => {
            lrg_providers::openai::OpenAiProvider::new(key)
                .list_available_models()
                .await
        }
        _ => Vec::new(),
    };

    Json(json!({
        "models": {
            "qwen": [],
            "ollama": ollama_models,
            "lmstudio": [],
            "chatgpt": openai_models,
            "gemini": [],
        }
    }))
}

async fn ping() -> &'static str {
    "pong"
}

/// Give the response time to flush, then trigger graceful shutdown —
/// equivalent of the Python `request_shutdown` (sleep 1s + SIGINT to self).
fn schedule_shutdown(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        state.shutdown.notify_one();
    });
}

async fn shutdown(State(state): State<Arc<AppState>>) -> Json<Value> {
    log::info!("Shutdown request received");
    schedule_shutdown(state);
    Json(json!({"status": "Server is shutting down..."}))
}

async fn restart(State(state): State<Arc<AppState>>) -> Json<Value> {
    log::info!("Restart request received via API");
    schedule_shutdown(state);
    Json(json!({"status": "Restarting..."}))
}

async fn unload(State(state): State<Arc<AppState>>) -> Json<Value> {
    log::info!("Unload request received via API");
    state.siglip.unload();
    state.face.unload();
    // Store handles stay open (LanceDB keeps its own lazy IO, nothing to
    // proactively drop).
    Json(json!({"status": "Resources unloaded successfully."}))
}

async fn initialize(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let db_path = body
        .as_ref()
        .and_then(|Json(v)| v.get("db_path"))
        .and_then(Value::as_str)
        .unwrap_or("");

    if db_path.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "db_path is required"})),
        )
            .into_response();
    }

    let switched = match state.ensure_db_path(db_path).await {
        Ok(switched) => switched,
        Err(e) => {
            log::error!("Failed to initialize database at {db_path}: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response();
        }
    };
    if !switched {
        return Json(json!({"status": "already_initialized", "db_path": db_path})).into_response();
    }

    let path = PathBuf::from(db_path);
    if let Err(e) = lifecycle::write_ok_file(&path).and_then(|_| lifecycle::write_pid_file(&path)) {
        log::error!("Failed to initialize database at {db_path}: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response();
    }
    Json(json!({"status": "success", "db_path": db_path})).into_response()
}

async fn get_version() -> Json<Value> {
    Json(serde_json::to_value(version::get_backend_version_info()).unwrap())
}

async fn version_check(body: Option<Json<Value>>) -> Json<Value> {
    let data = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let plugin_version = data.get("plugin_version").and_then(Value::as_str);
    let plugin_release_tag = data.get("plugin_release_tag").and_then(Value::as_str);
    let plugin_build = data.get("plugin_build").and_then(Value::as_i64);

    Json(version::check_plugin_backend_version(
        plugin_version,
        plugin_build,
        plugin_release_tag,
    ))
}

async fn health(State(state): State<Arc<AppState>>) -> Json<Value> {
    let (clip_status, clip_error) = state.siglip.status();
    let (face_status, face_error) = state.face.status();
    // M7 adds real LLM provider health.
    Json(json!({
        "clip_model": clip_status,
        "clip_error": clip_error,
        "face_model": face_status,
        "face_error": face_error,
    }))
}

fn read_tail(path: &Path, max_bytes: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    f.seek(SeekFrom::Start(size.saturating_sub(max_bytes)))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn expand_env_vars(path: &str) -> String {
    let mut out = path.to_string();
    if let Some(home) = std::env::var_os("HOME") {
        if let Some(rest) = out.strip_prefix("~/") {
            out = format!("{}/{}", home.to_string_lossy(), rest);
        }
    }
    for var in ["USERNAME", "APPDATA", "LOCALAPPDATA", "USERPROFILE"] {
        if let Ok(val) = std::env::var(var) {
            out = out.replace(&format!("%{var}%"), &val);
        }
    }
    out
}

fn ollama_log_path() -> Option<PathBuf> {
    [
        "~/.ollama/logs/server.log",
        "/root/.ollama/logs/server.log",
        r"C:\Users\%USERNAME%\AppData\Local\ollama\server.log",
    ]
    .iter()
    .map(|p| PathBuf::from(expand_env_vars(p)))
    .find(|p| p.is_file())
}

fn lmstudio_log_path() -> Option<PathBuf> {
    [
        "~/Library/Logs/LM Studio/main.log",
        r"%APPDATA%\LM Studio\logs\main.log",
    ]
    .iter()
    .map(|p| PathBuf::from(expand_env_vars(p)))
    .find(|p| p.is_file())
}

async fn get_logs() -> Json<Value> {
    let mut logs = serde_json::Map::new();

    let backend_log = logging::current_log_path();
    if backend_log.is_file() {
        match read_tail(&backend_log, 1024 * 1024) {
            Ok(content) => {
                logs.insert("backend".into(), json!(content));
            }
            Err(e) => {
                log::error!("Failed to read backend logs: {e}");
                logs.insert("backend_error".into(), json!(e.to_string()));
            }
        }
    }

    if let Some(p) = ollama_log_path() {
        if let Ok(content) = read_tail(&p, 512 * 1024) {
            logs.insert("ollama".into(), json!(content));
        }
    }
    if let Some(p) = lmstudio_log_path() {
        if let Ok(content) = read_tail(&p, 512 * 1024) {
            logs.insert("lmstudio".into(), json!(content));
        }
    }

    Json(Value::Object(logs))
}

async fn get_raw_log(UrlPath(log_type): UrlPath<String>) -> Response {
    let path = match log_type.as_str() {
        "backend" => Some(logging::current_log_path()),
        "ollama" => ollama_log_path(),
        "lmstudio" => lmstudio_log_path(),
        _ => None,
    };

    if let Some(path) = path.filter(|p| p.is_file()) {
        if let Ok(content) = tokio::fs::read(&path).await {
            return ([(CONTENT_TYPE, "text/plain; charset=utf-8")], content).into_response();
        }
    }

    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": format!("Log file for {log_type} not found")})),
    )
        .into_response()
}
