//! Runtime configuration: CLI arguments, host/port env vars, and the
//! platform-specific log path rules ported from `server/src/config.py`.

use std::env;
use std::path::{Path, PathBuf};

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 19819;

pub const PID_FILE_NAME: &str = "lrgenius-server.pid";
pub const OK_FILE_NAME: &str = "lrgenius-server.OK";
pub const LOG_FILE_NAME: &str = "lrgenius-server.log";

pub fn host() -> String {
    env::var("GENIUSAI_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string())
}

pub fn port() -> u16 {
    env::var("GENIUSAI_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// Number of rotated log backups kept on startup (GENIUSAI_LOG_ROTATE_BACKUPS,
/// default 3, clamped to 1..=20).
pub fn log_rotate_backups() -> u32 {
    let n = env::var("GENIUSAI_LOG_ROTATE_BACKUPS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(3);
    n.clamp(1, 20)
}

pub fn is_running_in_docker() -> bool {
    Path::new("/.dockerenv").exists() || env::var("container").as_deref() == Ok("docker")
}

/// Log path for a bound catalog: `lrgenius-server.log` next to the db dir
/// (i.e. in the catalog folder, since db_path is `<catalog dir>/lrgenius.db`).
pub fn log_path_for_db(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(LOG_FILE_NAME)
}

/// Fallback log path when no db_path is bound yet.
pub fn default_log_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Logs/LrGeniusAI/service.log")
    }
    #[cfg(target_os = "windows")]
    {
        let base = env::var("LOCALAPPDATA").unwrap_or_default();
        PathBuf::from(base)
            .join("LrGeniusAI")
            .join("logs")
            .join(LOG_FILE_NAME)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(LOG_FILE_NAME)
    }
}
