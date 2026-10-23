//! The backend's access token: what every request to its API has to carry
//! (`Authorization: Bearer <token>`), so that only the user's own plug-in gets
//! in — not a web page in their browser, not another account on the same
//! computer, not another machine on the network when the backend listens there.
//!
//! The token is made once and kept in a file only the user can read
//! ([`token_file`]). On the computer the backend runs on, the plug-in reads
//! that file itself, so nobody has to copy anything; being able to read it is
//! the proof of being the same user. A plug-in on another computer cannot, so
//! the user copies the token over once: `geniusai-server token` prints it.
//!
//! [`ENV_TOKEN`] replaces the file with a fixed token, for a backend whose
//! setup (a container, a service) already has a place for secrets.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A fixed token instead of the file.
pub const ENV_TOKEN: &str = "LRG_API_TOKEN";

/// The file's name, inside [`token_dir`]. The plug-in builds the same path
/// (`APISearchIndex.lua`), so this must not change on its own.
pub const FILE_NAME: &str = "api-token";

/// Where the token came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// [`ENV_TOKEN`].
    Env,
    /// The file at this path.
    File(PathBuf),
}

#[derive(Debug, Clone)]
pub struct ApiToken {
    pub token: String,
    pub source: Source,
}

/// The per-user folder the token file lives in. The plug-in builds the same
/// path from the user's home folder, so both use the home folder rather than
/// `LOCALAPPDATA`/`XDG_*`, which can point elsewhere.
pub fn token_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library/Application Support/LrGeniusAI"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("USERPROFILE")
            .map(|home| PathBuf::from(home).join("AppData\\Local\\LrGeniusAI"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // No Lightroom here, so no plug-in that reads the file: this is a
        // backend for other computers, whose user runs `geniusai-server token`.
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|base| base.join("LrGeniusAI"))
    }
}

pub fn token_file() -> Option<PathBuf> {
    token_dir().map(|dir| dir.join(FILE_NAME))
}

/// What is accepted as a token: long enough to be a real secret, and nothing
/// that cannot travel in an HTTP header.
pub fn valid(token: &str) -> bool {
    (32..=256).contains(&token.len())
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A fresh random token: two v4 UUIDs, 244 random bits from the operating
/// system's random source.
pub fn mint() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Compares tokens without leaking through timing how much of a guess matched.
pub fn matches(expected: &str, presented: &str) -> bool {
    let (a, b) = (expected.as_bytes(), presented.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn env_token() -> Result<Option<String>, String> {
    match std::env::var(ENV_TOKEN) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) if valid(value.trim()) => Ok(Some(value.trim().to_string())),
        Ok(_) => Err(format!(
            "{ENV_TOKEN} is set but is not a usable token: it needs 32 to 256 characters, \
             only letters, digits, '-' and '_'."
        )),
        Err(_) => Ok(None),
    }
}

fn no_folder() -> String {
    "There is no home folder to keep the backend's access token in (HOME, or USERPROFILE on \
     Windows, is not set). Set one, or give the token in LRG_API_TOKEN."
        .to_string()
}

/// The token the backend uses: [`ENV_TOKEN`] when set, else the one in the
/// file, which is made the first time.
pub fn load_or_create() -> Result<ApiToken, String> {
    if let Some(token) = env_token()? {
        return Ok(ApiToken {
            token,
            source: Source::Env,
        });
    }
    let path = token_file().ok_or_else(no_folder)?;
    load_or_create_at(&path)
}

/// Replaces the token in the file with a new one. A running backend keeps the
/// old one until it restarts.
pub fn renew() -> Result<ApiToken, String> {
    if env_token()?.is_some() {
        return Err(format!(
            "The token comes from {ENV_TOKEN}, so it is changed there, not here."
        ));
    }
    let path = token_file().ok_or_else(no_folder)?;
    renew_at(&path)
}

pub fn load_or_create_at(path: &Path) -> Result<ApiToken, String> {
    if let Ok(text) = fs::read_to_string(path) {
        let token = text.trim();
        if valid(token) {
            // A copy restored from a backup, or written by hand, may be
            // readable by everyone; the token is only worth what the file's
            // permissions are.
            restrict(path);
            return Ok(ApiToken {
                token: token.to_string(),
                source: Source::File(path.to_path_buf()),
            });
        }
        log::warn!(
            "The access token in {} is not usable; making a new one.",
            path.display()
        );
    }
    renew_at(path)
}

pub fn renew_at(path: &Path) -> Result<ApiToken, String> {
    let token = mint();
    write_private(path, &token).map_err(|e| {
        format!(
            "Could not save the backend's access token to {}: {e}",
            path.display()
        )
    })?;
    Ok(ApiToken {
        token,
        source: Source::File(path.to_path_buf()),
    })
}

/// Writes the file so that only this user can read it, replacing it whole:
/// a reader never sees half a token.
fn write_private(path: &Path, token: &str) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("the token file has no folder"))?;
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // Windows: the folder is under the user's profile, which only that
        // user (and administrators) can read.
        let mut file = options.open(&tmp)?;
        io::Write::write_all(&mut file, format!("{token}\n").as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        if meta.permissions().mode() & 0o077 != 0 {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn made_once_then_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("LrGeniusAI").join(FILE_NAME);

        let first = load_or_create_at(&path).unwrap();
        assert!(valid(&first.token));
        assert_eq!(first.source, Source::File(path.clone()));

        let again = load_or_create_at(&path).unwrap();
        assert_eq!(again.token, first.token);
        assert!(!path.with_extension("tmp").exists());
    }

    #[test]
    fn renew_replaces_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let first = load_or_create_at(&path).unwrap();
        let second = renew_at(&path).unwrap();
        assert_ne!(first.token, second.token);
        assert_eq!(load_or_create_at(&path).unwrap().token, second.token);
    }

    #[test]
    fn an_unusable_file_gets_a_new_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        fs::write(&path, "short\n").unwrap();
        let token = load_or_create_at(&path).unwrap().token;
        assert!(valid(&token));
        assert_eq!(fs::read_to_string(&path).unwrap().trim(), token);
    }

    #[cfg(unix)]
    #[test]
    fn only_the_user_can_read_the_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        load_or_create_at(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // A copy anyone can read is tightened when it is next loaded.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        load_or_create_at(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn matches_compares_whole_tokens() {
        let token = mint();
        assert!(matches(&token, &token));
        assert!(!matches(&token, &token[..token.len() - 1]));
        assert!(!matches(&token, ""));
        assert!(!matches(&token, &mint()));
    }
}
