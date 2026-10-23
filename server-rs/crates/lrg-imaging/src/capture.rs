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

/// Capture time as seconds since the Unix epoch, from `DateTimeOriginal`
/// (falling back to `DateTime`), or `None` when the frame carries neither.
///
/// The EXIF clock is local time with no zone, so this reads it as if it were
/// UTC. That is wrong by the camera's offset, but *consistently* wrong across
/// one shoot, and the only consumer — burst grouping — compares frames to each
/// other. A frame whose date was never set (1970, or a camera reset to its
/// factory 2000-01-01) is still returned: it is wrong, but wrong for every
/// frame of that shoot alike, which still groups them correctly.
pub fn read_capture_time(image_bytes: &[u8]) -> Option<f64> {
    let exif = exif::Reader::new()
        .read_from_container(&mut Cursor::new(image_bytes))
        .ok()?;
    let ascii = |tag: exif::Tag| -> Option<String> {
        let field = exif.get_field(tag, exif::In::PRIMARY)?;
        let exif::Value::Ascii(parts) = &field.value else {
            return None;
        };
        parts
            .first()
            .and_then(|b| std::str::from_utf8(b).ok())
            .map(|t| t.trim().to_string())
    };
    for (tag, subsec_tag, offset_tag) in [
        (
            exif::Tag::DateTimeOriginal,
            exif::Tag::SubSecTimeOriginal,
            exif::Tag::OffsetTimeOriginal,
        ),
        (
            exif::Tag::DateTime,
            exif::Tag::SubSecTime,
            exif::Tag::OffsetTime,
        ),
    ] {
        let Some(text) = ascii(tag) else {
            continue;
        };
        let Some(mut ts) = parse_exif_datetime(&text) else {
            continue;
        };
        // Fraction of a second: at 20–30 fps the frames of one burst share a
        // whole second, and without this their order falls back to the
        // filename and the burst window blurs by a second each way.
        if let Some(fraction) = ascii(subsec_tag).and_then(|t| parse_subsec(&t)) {
            ts += fraction;
        }
        // "+01:00": turns the camera's local clock into a real epoch, which
        // is what `local_hour` downstream assumes it already is.
        if let Some(offset) = ascii(offset_tag).and_then(|t| parse_utc_offset(&t)) {
            ts -= offset;
        }
        return Some(ts);
    }
    None
}

/// `SubSecTime*` is the fractional digits only ("57" is .57 s, "570" is
/// .570 s), so the number of digits sets the scale.
fn parse_subsec(text: &str) -> Option<f64> {
    let digits = text.trim();
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let value: f64 = digits.parse().ok()?;
    Some(value / 10f64.powi(digits.len() as i32))
}

/// `±HH:MM` to seconds east of UTC. `None` for anything else, including the
/// "Z" and blank values some bodies write.
fn parse_utc_offset(text: &str) -> Option<f64> {
    let text = text.trim();
    let sign = match text.chars().next()? {
        '+' => 1.0,
        '-' => -1.0,
        _ => return None,
    };
    let (h, m) = text[1..].split_once(':')?;
    let hours: f64 = h.parse().ok()?;
    let minutes: f64 = m.parse().ok()?;
    if hours > 14.0 || minutes >= 60.0 {
        return None;
    }
    Some(sign * (hours * 3600.0 + minutes * 60.0))
}

/// `YYYY:MM:DD HH:MM:SS` to Unix seconds, treating the value as UTC. Returns
/// `None` for the all-zero placeholder some bodies write, and for anything
/// that does not parse.
fn parse_exif_datetime(text: &str) -> Option<f64> {
    let mut nums = [0i64; 6];
    let mut parts = text.split([':', ' ']).filter(|p| !p.is_empty());
    for slot in nums.iter_mut() {
        *slot = parts.next()?.parse().ok()?;
    }
    let [year, month, day, hour, minute, second] = nums;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || year < 1900 {
        return None;
    }
    // Days from civil, Howard Hinnant's algorithm: exact for the proleptic
    // Gregorian calendar, no table of month lengths needed.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second) as f64)
}

#[cfg(test)]
mod capture_time_tests {
    use super::parse_exif_datetime;

    #[test]
    fn exif_datetime_converts_to_unix_seconds() {
        // 2026-10-21 11:30:00 UTC.
        assert_eq!(
            parse_exif_datetime("2026:10:21 11:30:00"),
            Some(1_792_582_200.0)
        );
        assert_eq!(parse_exif_datetime("1970:01:01 00:00:00"), Some(0.0));
    }

    #[test]
    fn subseconds_and_offsets_refine_the_whole_second() {
        use super::{parse_subsec, parse_utc_offset};
        assert_eq!(parse_subsec("57"), Some(0.57));
        assert_eq!(parse_subsec("570"), Some(0.57));
        assert_eq!(parse_subsec("007"), Some(0.007));
        assert_eq!(parse_subsec(""), None);
        assert_eq!(parse_subsec("5a"), None);
        assert_eq!(parse_utc_offset("+01:00"), Some(3600.0));
        assert_eq!(parse_utc_offset("-04:30"), Some(-16200.0));
        assert_eq!(parse_utc_offset("Z"), None);
        assert_eq!(parse_utc_offset("   "), None);
        assert_eq!(parse_utc_offset("+25:00"), None);
    }

    #[test]
    fn placeholder_and_garbage_read_as_absent() {
        assert_eq!(parse_exif_datetime("0000:00:00 00:00:00"), None);
        assert_eq!(parse_exif_datetime(""), None);
        assert_eq!(parse_exif_datetime("yesterday"), None);
    }
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
