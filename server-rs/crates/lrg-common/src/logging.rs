//! File + stdout logger matching the Python backend's behavior:
//! `%Y-%m-%d %H:%M:%S,%3f LEVEL message` format, startup rotation
//! (`.log` -> `.log.1` -> ... -> `.log.N`), and a swappable log file so the
//! target can move next to the catalog when a `db_path` binds mid-run
//! (port of `config.update_log_path`).

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::config;

struct SwapLogger {
    file: Mutex<Option<File>>,
    path: RwLock<PathBuf>,
    stdout: bool,
}

static LOGGER: OnceLock<SwapLogger> = OnceLock::new();

impl Log for SwapLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S,%3f");
        let level = match record.level() {
            Level::Warn => "WARNING",
            Level::Error => "ERROR",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        };
        let line = format!("{ts} {level} {}\n", record.args());
        if let Ok(mut guard) = self.file.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.write_all(line.as_bytes());
            }
        }
        if self.stdout {
            print!("{line}");
        }
    }

    fn flush(&self) {
        if let Ok(mut guard) = self.file.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

fn open_log_file(path: &Path) -> Option<File> {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Shift existing log files: `.log` -> `.log.1`, `.log.1` -> `.log.2`, ...;
/// remove `.log.<backup_count>`. Direct port of `_rotate_log_on_startup`.
pub fn rotate_log_on_startup(log_path: &Path, backup_count: u32) {
    if backup_count == 0 || !log_path.is_file() {
        return;
    }
    let base = log_path.to_string_lossy().to_string();
    let oldest = format!("{base}.{backup_count}");
    let _ = fs::remove_file(&oldest);
    for i in (1..backup_count).rev() {
        let src = format!("{base}.{i}");
        let dst = format!("{base}.{}", i + 1);
        if Path::new(&src).is_file() {
            let _ = fs::rename(&src, &dst);
        }
    }
    let _ = fs::rename(&base, format!("{base}.1"));
}

/// Initialize global logging. Rotates the startup log unless running in
/// Docker (where a single file keeps container logs simple).
pub fn init(log_path: &Path, debug: bool) {
    if !config::is_running_in_docker() {
        rotate_log_on_startup(log_path, config::log_rotate_backups());
    }
    let logger = SwapLogger {
        file: Mutex::new(open_log_file(log_path)),
        path: RwLock::new(log_path.to_path_buf()),
        stdout: true,
    };
    if LOGGER.set(logger).is_ok() {
        // OnceLock guarantees a stable address for the lifetime of the process.
        let r: &'static SwapLogger = LOGGER.get().unwrap();
        let _ = log::set_logger(r);
        log::set_max_level(if debug {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        });
    }
}

/// Swap the active log file (no rotation), used when a db_path binds and the
/// log should move next to the catalog. No-op when the path is unchanged.
pub fn swap_log_file(new_path: &Path) {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    {
        let current = logger.path.read().unwrap();
        if *current == new_path {
            return;
        }
    }
    let new_file = open_log_file(new_path);
    {
        let mut guard = logger.file.lock().unwrap();
        *guard = new_file;
    }
    {
        let mut current = logger.path.write().unwrap();
        *current = new_path.to_path_buf();
    }
    log::info!("Log path context updated to: {}", new_path.display());
}

/// The currently active log file path (config.LOG_PATH equivalent).
pub fn current_log_path() -> PathBuf {
    LOGGER
        .get()
        .map(|l| l.path.read().unwrap().clone())
        .unwrap_or_else(config::default_log_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_shifts_backups_and_drops_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("lrgenius-server.log");
        fs::write(&log, "current").unwrap();
        fs::write(dir.path().join("lrgenius-server.log.1"), "one").unwrap();
        fs::write(dir.path().join("lrgenius-server.log.2"), "two").unwrap();
        fs::write(dir.path().join("lrgenius-server.log.3"), "three").unwrap();

        rotate_log_on_startup(&log, 3);

        assert!(!log.exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("lrgenius-server.log.1")).unwrap(),
            "current"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("lrgenius-server.log.2")).unwrap(),
            "one"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("lrgenius-server.log.3")).unwrap(),
            "two"
        );
    }

    #[test]
    fn rotation_noop_when_no_log_exists() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("lrgenius-server.log");
        rotate_log_on_startup(&log, 3);
        assert!(!log.exists());
    }
}
