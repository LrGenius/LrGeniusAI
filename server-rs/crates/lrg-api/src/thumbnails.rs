//! The small preview stored per photo for the browser search page
//! (`/v1/ui/search`), served by `GET /v1/photos/thumbnail`.
//!
//! Files, not a LanceDB column: a preview is ~10 KB of JPEG that only the page
//! ever reads, one at a time. In the metadata blob it would be decoded by every
//! `scan_meta` (stats, search, cleanup) — the same reason face thumbnails made
//! full scans of `FACE_TABLE` expensive — and a vector table has no business
//! holding images.
//!
//! Layout: `<lance root>/thumbnails/<aa>/<sha256 of the photo_id>.jpg`, with
//! `<aa>` the first two hex digits. Hashing keeps photo ids (which contain `:`
//! and can be long) out of file names on every platform, and the two-digit
//! fan-out keeps any one directory to a few hundred entries in a big catalog.
//!
//! Written by indexing (`routes::index_upload`) after the photo's row is
//! stored, and on every pass, so a re-index refreshes the preview of an edited
//! photo. Deleted with the photo by `/v1/photos/remove` and by the index
//! cleanup's purge.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Directory under the lance root.
pub const DIR: &str = "thumbnails";

/// Where `photo_id`'s preview lives under `lance_root`.
#[must_use]
pub fn path_for(lance_root: &Path, photo_id: &str) -> PathBuf {
    let digest = Sha256::digest(photo_id.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    lance_root
        .join(DIR)
        .join(&hex[..2])
        .join(format!("{hex}.jpg"))
}

/// Stores a preview, replacing any earlier one. Written to a temporary file
/// first and renamed, so a reader never sees half a JPEG.
pub fn write(lance_root: &Path, photo_id: &str, jpeg: &[u8]) -> std::io::Result<()> {
    let path = path_for(lance_root, photo_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("jpg.part");
    std::fs::write(&tmp, jpeg)?;
    std::fs::rename(&tmp, &path)
}

/// The stored preview, `None` when the photo has none.
#[must_use]
pub fn read(lance_root: &Path, photo_id: &str) -> Option<Vec<u8>> {
    std::fs::read(path_for(lance_root, photo_id)).ok()
}

/// Whether `photo_id` has a stored preview.
#[must_use]
pub fn exists(lance_root: &Path, photo_id: &str) -> bool {
    path_for(lance_root, photo_id).is_file()
}

/// Deletes the previews of `photo_ids`. A missing file is not an error; any
/// other failure is returned with the photo it concerns, for the caller to
/// log — a stray preview costs 10 KB of disk and nothing else.
pub fn delete(lance_root: &Path, photo_ids: &[String]) -> Vec<(String, std::io::Error)> {
    photo_ids
        .iter()
        .filter_map(|id| match std::fs::remove_file(path_for(lance_root, id)) {
            Ok(()) => None,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => Some((id.clone(), e)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_round_trips_and_is_replaced_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let id = "md5p:123456:1700000000:0123456789abcdef0123456789abcdef";
        assert!(!exists(root, id));
        assert_eq!(read(root, id), None);

        write(root, id, b"first").unwrap();
        assert!(exists(root, id));
        write(root, id, b"second").unwrap();
        assert_eq!(read(root, id).as_deref(), Some(&b"second"[..]));

        assert!(delete(root, &[id.to_string(), "never-stored".to_string()]).is_empty());
        assert!(!exists(root, id));
    }

    #[test]
    fn paths_are_safe_file_names_fanned_out_by_hash() {
        let root = Path::new("/lance");
        let path = path_for(root, "meta1:C:\\Photos\\a.CR3");
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), 64 + ".jpg".len());
        assert!(name
            .trim_end_matches(".jpg")
            .bytes()
            .all(|b| b.is_ascii_hexdigit()));
        let shard = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(shard, &name[..2]);
        assert_eq!(path.parent().unwrap().parent().unwrap(), root.join(DIR));
        // Different photos, different files.
        assert_ne!(path_for(root, "a"), path_for(root, "b"));
    }
}
