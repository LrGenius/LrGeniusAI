//! Capture settings read back out of a frame's EXIF.
//!
//! Separate from [`crate::location`], which reads the same containers for a
//! different purpose: location tags are prompt context for the LLM, while these
//! are inputs to decisions the backend makes itself — today only the edit
//! guardrails, which need to know how much noise a shadow lift would raise.
//!
//! Deliberately narrow. Everything here has a caller; there is no value in
//! extracting the whole EXIF block on the chance that something later wants it,
//! because an unread field cannot be wrong in a way anyone notices.

use std::io::Cursor;

/// ISO speed at capture, or `None` when the frame carries no usable value.
///
/// `None` is the honest answer for a JPEG that has been through an editor that
/// dropped EXIF, and callers must treat it as "unknown" rather than as a low
/// ISO: `lrg_analysis::edit_guardrails::derive_budget` leaves the shadow-lift
/// ceiling at its maximum when the ISO is unknown, which is the right failure
/// direction for a rule that exists to prevent damage.
///
/// Reads `ISOSpeedRatings` (EXIF tag 0x8827) and falls back to
/// `PhotographicSensitivity`, which is what the 2.3 spec renamed it to and what
/// some bodies write instead. A zero or negative reading is treated as absent —
/// several cameras write `0` when the sensitivity is set automatically and the
/// value was never resolved.
pub fn read_iso(image_bytes: &[u8]) -> Option<f64> {
    let exif = exif::Reader::new()
        .read_from_container(&mut Cursor::new(image_bytes))
        .ok()?;
    for tag in [exif::Tag::ISOSpeed, exif::Tag::PhotographicSensitivity] {
        let Some(field) = exif.get_field(tag, exif::In::PRIMARY) else {
            continue;
        };
        // Bodies disagree on the encoding: SHORT is the common case, LONG shows
        // up on high-ISO bodies whose values exceed 65535.
        let value = match &field.value {
            exif::Value::Short(v) => v.first().map(|n| *n as f64),
            exif::Value::Long(v) => v.first().map(|n| *n as f64),
            _ => None,
        };
        if let Some(iso) = value.filter(|v| *v > 0.0) {
            return Some(iso);
        }
    }
    None
}

/// Whether a filename names a raw file.
///
/// Decides whether clipped highlights are recoverable, which is the one place
/// the guardrails treat raw and rendered sources differently.
///
/// It has to work off the *name* rather than the bytes, because by the time the
/// backend sees an image it has been normalised to JPEG — the original encoding
/// is no longer in the data. That makes this a heuristic, and the failure
/// direction is chosen accordingly: an unknown extension reads as not-raw, so a
/// frame whose origin cannot be established gets the conservative budget rather
/// than a headroom allowance it may not have.
///
/// DNG is included because Lightroom treats it as raw, and the plugin's own
/// `fileFormat` metadata reports `DNG` alongside `RAW`.
pub fn is_raw_filename(filename: &str) -> bool {
    const RAW_EXTENSIONS: &[&str] = &[
        "3fr", "arw", "cr2", "cr3", "crw", "dng", "erf", "fff", "iiq", "kdc", "mef", "mos", "mrw",
        "nef", "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
    ];
    let Some((_, extension)) = filename.rsplit_once('.') else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    RAW_EXTENSIONS.contains(&extension.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_extensions_are_recognised_case_insensitively() {
        assert!(is_raw_filename("DSC_0001.NEF"));
        assert!(is_raw_filename("shot.cr3"));
        assert!(is_raw_filename("scan.dng"));
    }

    #[test]
    fn rendered_and_unknown_sources_read_as_not_raw() {
        // The conservative direction: no headroom is assumed for anything that
        // cannot be shown to be raw.
        assert!(!is_raw_filename("export.jpg"));
        assert!(!is_raw_filename("layered.psd"));
        assert!(!is_raw_filename("noextension"));
        assert!(!is_raw_filename(""));
    }

    #[test]
    fn a_dot_in_the_directory_does_not_confuse_the_extension() {
        assert!(is_raw_filename("my.photos/DSC_0001.arw"));
        assert!(!is_raw_filename("my.raw.photos/export.jpg"));
    }

    #[test]
    fn iso_is_absent_for_data_that_is_not_an_image() {
        assert_eq!(read_iso(b"not an image"), None);
        assert_eq!(read_iso(&[]), None);
    }
}
