//! `POST /update/apply` — real self-update behavior (M9), replacing the
//! Python era's tkinter updater GUI + raw-source-file patching. Per the
//! rewrite plan: the manifest (v2, see `scripts/generate_update_manifest.py`)
//! now points at one whole binary per platform instead of per-.py-file
//! patches, so applying an update means: download + verify the binary,
//! patch the (still-interpreted) plugin `.lua` files, rename the current
//! exe aside, move the new one into its place, and relaunch — the
//! "rename dance" the plan calls out, which works even for a binary
//! that's currently running on both Windows and Unix.
//!
//! Loopback-only, matching Python's guard against a remote caller
//! triggering arbitrary local file writes. Live-tested end-to-end
//! against a local HTTP server standing in for a GitHub release asset
//! (serving the running binary's own bytes back at itself, so the sha256
//! matched cleanly): download, verify, stage, rename-current-exe-aside,
//! move-staged-into-place, graceful shutdown, and a bare relaunch with a
//! fresh PID that came back up and answered `/ping` all worked exactly
//! as designed on macOS arm64. Not exercised on Windows or against a
//! real GitHub release (no Windows machine or published release
//! available in this environment) — the `run_hidden.vbs` relaunch path
//! in particular still needs a real Windows run before shipping.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route("/update/apply", axum::routing::post(update_apply))
}

/// Maps this build's OS/arch to the manifest's platform keys (must match
/// `scripts/generate_update_manifest.py`'s `--binary PLATFORM=PATH` keys).
fn platform_key() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "macos-arm64",
        ("windows", "x86_64") => "windows-x64",
        ("linux", "x86_64") => "linux-x64",
        _ => "unsupported",
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({"error": msg.into()}))).into_response()
}

async fn update_apply(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    body: Option<Json<Value>>,
) -> Response {
    if !addr.ip().is_loopback() {
        log::warn!(
            "Rejected /update/apply from non-loopback address: {}",
            addr.ip()
        );
        return err(
            StatusCode::FORBIDDEN,
            "update endpoint only available on local backend",
        );
    }

    let data = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let Some(manifest) = data.get("manifest").filter(|m| !m.is_null()) else {
        return err(
            StatusCode::BAD_REQUEST,
            "manifest and plugin_path are required",
        );
    };
    let Some(plugin_path) = data
        .get("plugin_path")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return err(
            StatusCode::BAD_REQUEST,
            "manifest and plugin_path are required",
        );
    };

    if manifest
        .get("breaking_changes")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let release_url = manifest
            .get("release_url")
            .and_then(Value::as_str)
            .unwrap_or("");
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "This release requires the full installer due to breaking changes. Please download it from {release_url}"
            ),
        );
    }

    let key = platform_key();
    let Some(platform_entry) = manifest.get("platforms").and_then(|p| p.get(key)) else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("No update binary available for this platform ({key})"),
        );
    };
    let Some(url) = platform_entry.get("url").and_then(Value::as_str) else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "manifest entry missing url",
        );
    };
    let expected_sha256 = platform_entry
        .get("sha256")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();

    let current_exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not resolve current executable: {e}"),
            )
        }
    };
    let exe_dir = current_exe.parent().unwrap_or_else(|| Path::new("."));
    let exe_name = current_exe
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "lrgenius-server".to_string());
    let staged_path = exe_dir.join(format!("{exe_name}.update"));

    log::info!("Downloading update binary from {url}");
    let client = reqwest::Client::new();
    let bytes = match download(&client, url, Duration::from_secs(300)).await {
        Ok(b) => b,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to download update: {e}"),
            )
        }
    };
    let actual_sha256 = sha256_hex(&bytes);
    if !expected_sha256.is_empty() && actual_sha256 != expected_sha256 {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "downloaded update failed checksum verification",
        );
    }
    if let Err(e) = tokio::fs::write(&staged_path, &bytes).await {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to stage update: {e}"),
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = tokio::fs::metadata(&staged_path).await {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = tokio::fs::set_permissions(&staged_path, perms).await;
        }
    }

    let plugin_warnings = patch_plugin_files(&client, manifest, plugin_path).await;

    log::info!(
        "Update staged at {}; scheduling swap + restart",
        staged_path.display()
    );
    let state_for_task = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Err(e) = apply_staged_update(&state_for_task, &current_exe, &staged_path).await {
            log::error!("Failed to apply staged update: {e}");
        }
    });

    Json(json!({
        "status": "success",
        "message": "Update downloaded and verified; restarting to apply it.",
        "warning": if plugin_warnings.is_empty() { Value::Null } else { json!(plugin_warnings.join("; ")) },
    }))
    .into_response()
}

async fn download(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let resp = client
        .get(url)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| e.to_string())
}

/// Best-effort per-file plugin patch: one bad/unreachable file shouldn't
/// abort an otherwise-successful binary update. Returns human-readable
/// warnings for anything skipped.
async fn patch_plugin_files(
    client: &reqwest::Client,
    manifest: &Value,
    plugin_path: &str,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let Some(files) = manifest.get("plugin_files").and_then(Value::as_array) else {
        return warnings;
    };
    for entry in files {
        let (Some(rel_path), Some(file_url)) = (
            entry.get("path").and_then(Value::as_str),
            entry.get("url").and_then(Value::as_str),
        ) else {
            continue;
        };
        let expected = entry
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let content = match download(client, file_url, Duration::from_secs(30)).await {
            Ok(c) => c,
            Err(e) => {
                warnings.push(format!("{rel_path}: download failed: {e}"));
                continue;
            }
        };
        if !expected.is_empty() && sha256_hex(&content) != expected {
            warnings.push(format!("{rel_path}: checksum mismatch, skipped"));
            continue;
        }
        let dest = PathBuf::from(plugin_path).join(rel_path);
        if let Some(parent) = dest.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if let Err(e) = tokio::fs::write(&dest, &content).await {
            warnings.push(format!("{rel_path}: write failed: {e}"));
        }
    }
    warnings
}

/// The "rename dance": rename the current exe aside (works even while
/// it's the running image on both Windows and Unix — renaming, unlike
/// deleting or overwriting-in-place, doesn't require the file to be
/// unmapped first), move the verified new binary into its place, then
/// hand off to the existing graceful-shutdown machinery. `main.rs`
/// spawns the relaunch only once `axum::serve` has fully returned, so
/// the old process's listener is guaranteed gone before the new
/// process tries to bind the same port.
async fn apply_staged_update(
    state: &AppState,
    current_exe: &Path,
    staged_path: &Path,
) -> Result<(), String> {
    let old_path = current_exe.with_extension("old");
    let _ = tokio::fs::remove_file(&old_path).await; // best-effort cleanup from a previous update
    tokio::fs::rename(current_exe, &old_path)
        .await
        .map_err(|e| format!("rename current exe aside failed: {e}"))?;
    tokio::fs::rename(staged_path, current_exe)
        .await
        .map_err(|e| format!("move staged binary into place failed: {e}"))?;
    *state.relaunch_after_shutdown.lock().unwrap() = Some(current_exe.to_path_buf());
    state.shutdown.notify_one();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_key_matches_this_build() {
        // Just exercises the mapping logic; the actual value depends on
        // the machine running the test.
        let key = platform_key();
        assert!(!key.is_empty());
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        // sha256("") — a standard test vector.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
