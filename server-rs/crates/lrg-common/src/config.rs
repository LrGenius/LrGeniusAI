//! Runtime configuration: CLI arguments, host/port env vars, and the
//! platform-specific log path rules ported from `server/src/config.py`.

use std::env;
#[cfg(target_os = "macos")]
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 19819;

pub const PID_FILE_NAME: &str = "lrgenius-server.pid";
pub const OK_FILE_NAME: &str = "lrgenius-server.OK";
pub const LOG_FILE_NAME: &str = "lrgenius-server.log";

pub fn host() -> String {
    env::var("GENIUSAI_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string())
}

/// The listen port, from `GENIUSAI_PORT`.
///
/// A value that will not parse falls back to the default rather than aborting
/// startup — but it says so. Silently listening on 19819 after being asked for
/// something else surfaces to the user as "the backend is not responding",
/// with nothing anywhere connecting that to a typo in an env var.
pub fn port() -> u16 {
    let Ok(raw) = env::var("GENIUSAI_PORT") else {
        return DEFAULT_PORT;
    };
    match raw.parse() {
        Ok(port) => port,
        Err(_) => {
            log::warn!(
                "GENIUSAI_PORT={raw:?} is not a valid port number; falling back to {DEFAULT_PORT}"
            );
            DEFAULT_PORT
        }
    }
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

/// True when `dir` exists (or can be created) and accepts a new file.
///
/// Writing a probe file is the only answer we can trust: the mode bits alone
/// miss ACLs, a read-only volume, and the case where the directory belongs to
/// another user. The probe is a separate file rather than the log itself so
/// that startup rotation still sees the real history instead of rotating an
/// empty file we just created.
#[cfg(target_os = "macos")]
fn log_dir_is_usable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".lrgenius-write-probe");
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Fallback log path when no db_path is bound yet.
///
/// Touches the filesystem: it creates the directory it settles on, so the
/// caller can open the file without a second thought. Called once at startup.
///
/// The macOS branch has two candidates on purpose. The installer puts logs in
/// `/Library/Logs/LrGeniusAI`, but that directory is not ours to recreate --
/// `/Library/Logs` is `root:wheel` and the server runs as a LaunchAgent under
/// the logged-in user, who can write inside a directory the installer made for
/// them but cannot make a new one. Disk cleaners do delete it (BuhoCleaner
/// among them), and logging nowhere at all is a poor answer to that, so fall
/// back to the per-user location we can always recreate.
pub fn default_log_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let system = PathBuf::from("/Library/Logs/LrGeniusAI");
        if log_dir_is_usable(&system) {
            return system.join("service.log");
        }
        if let Some(home) = env::var_os("HOME") {
            let user = PathBuf::from(home).join("Library/Logs/LrGeniusAI");
            if log_dir_is_usable(&user) {
                return user.join("service.log");
            }
        }
        // Nothing is writable. Return the documented path anyway so the
        // reported location matches what the installer set up; the logger
        // degrades to stdout-only from here.
        system.join("service.log")
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn usable_dir_is_created_on_demand_and_leaves_nothing_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Logs/LrGeniusAI");
        assert!(log_dir_is_usable(&dir));
        assert!(dir.is_dir());
        // The probe must not survive, or rotation would inherit it.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn unusable_when_the_directory_cannot_be_created() {
        let tmp = tempfile::tempdir().unwrap();
        // A regular file where a parent directory would have to go: this is
        // the "cannot create" case without depending on running as non-root,
        // which a permission-based test would.
        let blocker = tmp.path().join("Logs");
        fs::write(&blocker, b"not a directory").unwrap();
        assert!(!log_dir_is_usable(&blocker.join("LrGeniusAI")));
    }

    #[test]
    fn default_log_path_points_somewhere_writable() {
        let path = default_log_path();
        let dir = path.parent().unwrap();
        assert_eq!(path.file_name().unwrap(), "service.log");
        // Whichever candidate won, the directory exists by the time we return
        // -- unless the machine has neither, which is not the case in CI.
        assert!(dir.is_dir(), "{} should exist", dir.display());
    }
}
