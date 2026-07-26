//! PID/OK handshake files the Lightroom plugin polls, written next to the
//! catalog (the parent directory of db_path). Port of `server_lifecycle.py`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{OK_FILE_NAME, PID_FILE_NAME};

/// db_path is `<catalog dir>/lrgenius.db`; handshake files live in the
/// catalog dir itself.
fn db_dir(db_path: &Path) -> Option<&Path> {
    db_path.parent()
}

fn pid_file(db_path: &Path) -> Option<PathBuf> {
    db_dir(db_path).map(|d| d.join(PID_FILE_NAME))
}

fn ok_file(db_path: &Path) -> Option<PathBuf> {
    db_dir(db_path).map(|d| d.join(OK_FILE_NAME))
}

/// Atomic write via tmp + rename (works on POSIX and Windows), matching
/// the Python `write_pid_file`.
pub fn write_pid_file(db_path: &Path) -> std::io::Result<()> {
    let Some(pid_path) = pid_file(db_path) else {
        return Ok(());
    };
    let tmp = pid_path.with_extension("pid.tmp");
    fs::write(&tmp, std::process::id().to_string())?;
    fs::rename(&tmp, &pid_path)
}

pub fn remove_pid_file(db_path: &Path) {
    if let Some(p) = pid_file(db_path) {
        let _ = fs::remove_file(p);
    }
}

pub fn write_ok_file(db_path: &Path) -> std::io::Result<()> {
    let Some(p) = ok_file(db_path) else {
        return Ok(());
    };
    fs::write(p, "OK\n")
}

pub fn remove_ok_file(db_path: &Path) {
    if let Some(p) = ok_file(db_path) {
        let _ = fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_and_ok_files_land_in_catalog_dir() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("lrgenius.db");

        write_pid_file(&db_path).unwrap();
        write_ok_file(&db_path).unwrap();

        let pid = fs::read_to_string(dir.path().join(PID_FILE_NAME)).unwrap();
        assert_eq!(pid, std::process::id().to_string());
        assert_eq!(
            fs::read_to_string(dir.path().join(OK_FILE_NAME)).unwrap(),
            "OK\n"
        );
        assert!(!dir.path().join("lrgenius-server.pid.tmp").exists());

        remove_pid_file(&db_path);
        remove_ok_file(&db_path);
        assert!(!dir.path().join(PID_FILE_NAME).exists());
        assert!(!dir.path().join(OK_FILE_NAME).exists());
    }
}
