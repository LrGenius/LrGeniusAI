//! File + stdout logger matching the Python backend's behavior:
//! `%Y-%m-%d %H:%M:%S,%3f LEVEL message` format, startup rotation
//! (`.log` -> `.log.1` -> ... -> `.log.N`), and a swappable log file so the
//! target can move next to the catalog when a `db_path` binds mid-run
//! (port of `config.update_log_path`).
//!
//! The file is opened for each line and closed again, never held. The log sits
//! next to the catalog and the backend outlives Lightroom (a LaunchAgent on
//! macOS, started at login on Windows), so a handle kept open stops the drive
//! holding the catalog from being ejected for as long as the backend runs
//! (#469). Holding nothing covers every way Lightroom can go away, a crash
//! included, which a release sent from the plug-in's shutdown function would
//! not. A line costs about 20 µs instead of 2 on macOS.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::config;

struct SwapLogger {
    path: RwLock<PathBuf>,
    stdout: bool,
    /// The file could not be written last time, so that going out of reach is
    /// said once rather than for every line.
    file_unreachable: AtomicBool,
}

static LOGGER: OnceLock<SwapLogger> = OnceLock::new();

impl SwapLogger {
    /// Append mode keeps lines from different threads whole, so no lock is
    /// held around the I/O.
    fn append_line(&self, line: &str) {
        let Ok(path) = self.path.read() else {
            return;
        };
        match open_for_append(&path).and_then(|mut f| f.write_all(line.as_bytes())) {
            Ok(()) => self.file_unreachable.store(false, Ordering::Relaxed),
            // Ejecting the drive leads here while the backend runs on, which
            // is fine: nothing else needs the file. Stderr, because stdout may
            // be carrying a CLI subcommand's machine-readable output.
            Err(e) => {
                if !self.file_unreachable.swap(true, Ordering::Relaxed) {
                    let _ = writeln!(
                        std::io::stderr(),
                        "The log file at {} is out of reach ({e}), perhaps because its drive was \
                         ejected - not logging there until it is back.",
                        path.display()
                    );
                }
            }
        }
    }
}

impl Log for SwapLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // Foreign crates (lance, datafusion, ort, ...) log through the same
        // facade; keep only their warnings/errors so our log mirrors the
        // Python backend's own-logger-only output.
        let target = record.target();
        let ours = target.starts_with("lrg") || target.starts_with("geniusai");
        if !ours && record.level() > Level::Warn {
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
        self.append_line(&line);
        if self.stdout {
            print!("{line}");
        }
    }

    /// Nothing to do: every line has already been written and closed.
    fn flush(&self) {}
}

/// Never creates the directory: on a drive that is not mounted that would put
/// the log on the system disk, under what becomes the drive's mount point.
fn open_for_append(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// Readies `path` for logging, directory included, and says whether it can be
/// written. Only for when the path is chosen; a line uses [`open_for_append`].
fn prepare_log_file(path: &Path) -> bool {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    open_for_append(path).is_ok()
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
    init_with_options(log_path, debug, true);
}

/// `stdout_echo = false` keeps stdout clean for CLI subcommands that print
/// machine-readable output (logs still go to the file).
pub fn init_with_options(log_path: &Path, debug: bool, stdout_echo: bool) {
    if !config::is_running_in_docker() {
        rotate_log_on_startup(log_path, config::log_rotate_backups());
    }
    let opened = prepare_log_file(log_path);
    let logger = SwapLogger {
        path: RwLock::new(log_path.to_path_buf()),
        stdout: stdout_echo,
        file_unreachable: AtomicBool::new(!opened),
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
        // Losing the file is the one failure this module cannot report through
        // itself, so say it on the channel that is still working. Whoever reads
        // the log later would otherwise just find nothing and have no idea why.
        if !opened {
            log::warn!(
                "Could not open the log file at {} - logging to stdout only for this run. \
                 Check that the directory exists and is writable.",
                log_path.display()
            );
        }
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
    let opened = prepare_log_file(new_path);
    logger.file_unreachable.store(!opened, Ordering::Relaxed);
    {
        let mut current = logger.path.write().unwrap();
        *current = new_path.to_path_buf();
    }
    if opened {
        log::info!("Log path context updated to: {}", new_path.display());
    } else {
        // Don't claim a move that didn't happen: from here on the only output
        // is stdout, and the reason has to travel with that.
        log::warn!(
            "Could not open the log file at {} - logging to stdout only from here. \
             Check that the directory exists and is writable.",
            new_path.display()
        );
    }
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

    fn logger_at(path: &Path) -> SwapLogger {
        SwapLogger {
            path: RwLock::new(path.to_path_buf()),
            stdout: false,
            file_unreachable: AtomicBool::new(false),
        }
    }

    #[test]
    fn no_handle_stays_open_between_lines() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("lrgenius-server.log");
        let logger = logger_at(&log);

        logger.append_line("first\n");
        assert_eq!(fs::read_to_string(&log).unwrap(), "first\n");

        // A logger still holding the file would go on writing to the removed
        // one, so the second line would never show up at this path.
        fs::remove_file(&log).unwrap();
        logger.append_line("second\n");
        assert_eq!(fs::read_to_string(&log).unwrap(), "second\n");
    }

    #[test]
    fn lines_append_to_what_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("lrgenius-server.log");
        fs::write(&log, "from an earlier run\n").unwrap();

        logger_at(&log).append_line("new\n");

        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            "from an earlier run\nnew\n"
        );
    }

    #[test]
    fn a_missing_directory_is_left_missing_and_logging_resumes_when_it_is_back() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir.path().join("catalog");
        let log = catalog.join("lrgenius-server.log");
        let logger = logger_at(&log);

        logger.append_line("drive is ejected\n");
        assert!(!catalog.exists(), "a line must not recreate the directory");
        assert!(logger.file_unreachable.load(Ordering::Relaxed));

        fs::create_dir(&catalog).unwrap();
        logger.append_line("drive is back\n");
        assert_eq!(fs::read_to_string(&log).unwrap(), "drive is back\n");
        assert!(!logger.file_unreachable.load(Ordering::Relaxed));
    }

    #[test]
    fn lines_from_many_threads_stay_whole() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("lrgenius-server.log");
        let logger = logger_at(&log);
        let line = format!("{}\n", "x".repeat(200));

        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..200 {
                        logger.append_line(&line);
                    }
                });
            }
        });

        let content = fs::read_to_string(&log).unwrap();
        assert_eq!(content.lines().count(), 8 * 200);
        assert!(content.lines().all(|l| l.len() == 200));
    }

    #[test]
    fn preparing_a_log_file_creates_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("a/b/lrgenius-server.log");
        assert!(prepare_log_file(&log));
        assert!(log.is_file());

        // A regular file where the directory would have to go.
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, "not a directory").unwrap();
        assert!(!prepare_log_file(&blocker.join("lrgenius-server.log")));
    }

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
