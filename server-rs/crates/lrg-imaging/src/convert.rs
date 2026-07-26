//! Port of `utils/image_convert.py`: normalize incoming bytes to
//! JPEG-ready bytes for the indexing pipeline.
//!
//! The pass-through rule is the critical invariant: JPEG/PNG bytes whose
//! long edge is within the limit are returned untouched, so embeddings of
//! already-indexed photos never drift.
//!
//! RAW decoding goes through `rawler` (see `crate::raw`) — the Rust
//! equivalent of the Python backend's rawpy/LibRaw dependency. HEIC
//! (libheif) decoding is wired in behind vendored builds in a later
//! pass; until then those files raise the same "not available in this
//! backend build" error the Python backend uses when pillow-heif is
//! missing — the plugin falls back to its export-based flow by design.

use std::io::Cursor;

use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, ImageFormat, ImageReader, RgbImage};

use crate::raw::decode_raw;

pub const RAW_EXTENSIONS: [&str; 27] = [
    "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "gpr", "iiq", "kdc", "mef",
    "mos", "mrw", "nef", "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw",
    "x3f",
];

pub const VIDEO_EXTENSIONS: [&str; 11] = [
    "3gp", "avi", "m2ts", "m4v", "mov", "mp4", "mpeg", "mpg", "mts", "webm", "wmv",
];

pub fn default_max_long_edge() -> u32 {
    std::env::var("GENIUSAI_CONVERT_MAX_EDGE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2048)
}

pub fn default_jpeg_quality() -> u8 {
    std::env::var("GENIUSAI_CONVERT_QUALITY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(85)
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct UnsupportedImageError(pub String);

fn extension(filename: &str) -> String {
    std::path::Path::new(filename)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn is_raw_filename(filename: &str) -> bool {
    RAW_EXTENSIONS.contains(&extension(filename).as_str())
}

pub fn is_video_filename(filename: &str) -> bool {
    VIDEO_EXTENSIONS.contains(&extension(filename).as_str())
}

fn jpeg_filename(filename: &str) -> String {
    let base = std::path::Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "photo".to_string());
    format!("{base}.jpg")
}

/// EXIF orientation (tag 0x0112) from raw bytes, if present.
fn exif_orientation(data: &[u8]) -> Option<u16> {
    let exif = exif::Reader::new()
        .read_from_container(&mut Cursor::new(data))
        .ok()?;
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .map(|v| v as u16)
}

/// Apply an EXIF orientation to an RGB8 image (ImageOps.exif_transpose).
fn apply_orientation(image: DynamicImage, orientation: u16) -> DynamicImage {
    match orientation {
        2 => image.fliph(),
        3 => image.rotate180(),
        4 => image.flipv(),
        5 => image.rotate90().fliph(),
        6 => image.rotate90(),
        7 => image.rotate270().fliph(),
        8 => image.rotate270(),
        _ => image,
    }
}

/// Pillow-exact LANCZOS downscale to `max_long_edge` (no-op if within).
/// Matches `_downscale`: `int(size * scale)` truncation, min 1.
fn downscale(image: DynamicImage, max_long_edge: u32) -> DynamicImage {
    let (w, h) = (image.width(), image.height());
    let long_edge = w.max(h);
    if long_edge <= max_long_edge {
        return image;
    }
    let scale = max_long_edge as f64 / long_edge as f64;
    let new_w = ((w as f64 * scale) as u32).max(1);
    let new_h = ((h as f64 * scale) as u32).max(1);

    let src = image.to_rgb8();
    let mut dst = RgbImage::new(new_w, new_h);
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    Resizer::new()
        .resize(&src, &mut dst, &options)
        .expect("RGB8 -> RGB8 resize");
    DynamicImage::ImageRgb8(dst)
}

/// Decodes with the `image` crate's default 512 MiB decode-allocation cap
/// disabled. That cap is meant for untrusted uploads; `data` here is
/// always the user's own trusted Lightroom catalog file (read from a
/// server-side path or an already-open multipart body), so the cap just
/// silently rejects legitimate large scans/panoramas — e.g. a 16-bit
/// medium-format TIFF comfortably exceeds it. RAW decoding via `rawler`
/// already has no such cap, so this matches that trust level rather than
/// introducing a new one.
fn decode_without_alloc_cap<R: std::io::BufRead + std::io::Seek>(
    mut reader: ImageReader<R>,
) -> image::ImageResult<DynamicImage> {
    reader.no_limits();
    reader.decode()
}

fn encode_jpeg(image: &DynamicImage, quality: u8) -> Result<Vec<u8>, UnsupportedImageError> {
    let rgb = image.to_rgb8();
    let mut out = Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    rgb.write_with_encoder(encoder)
        .map_err(|e| UnsupportedImageError(format!("JPEG encode failed: {e}")))?;
    Ok(out.into_inner())
}

/// Port of `normalize_image_bytes`. Returns (jpeg_ready_bytes, filename).
pub fn normalize_image_bytes(
    data: &[u8],
    filename: Option<&str>,
    max_long_edge: u32,
    quality: u8,
) -> Result<(Vec<u8>, String), UnsupportedImageError> {
    let filename = filename.unwrap_or("photo");

    if data.is_empty() {
        return Err(UnsupportedImageError(format!(
            "Empty image data for '{filename}'"
        )));
    }
    if is_video_filename(filename) {
        return Err(UnsupportedImageError(format!(
            "'{filename}' is a video file and cannot be indexed"
        )));
    }
    if is_raw_filename(filename) {
        // Never let the generic decoder open TIFF-based raws (NEF/CR2/DNG) —
        // it would return thumbnails or unprocessed sensor data.
        let t_decode = std::time::Instant::now();
        let decoded = decode_raw(data).map_err(|e| {
            UnsupportedImageError(format!("Failed to decode raw file '{filename}': {e}"))
        })?;
        log::debug!(
            "'{filename}': raw decode ({}x{}) took {:?}",
            decoded.width(),
            decoded.height(),
            t_decode.elapsed()
        );
        // Most RAW formats are TIFF-based containers, so the same
        // container-level EXIF reader used for JPEG/TIFF picks up the
        // root IFD's Orientation tag directly from the raw bytes here
        // too; formats with a non-TIFF container (CR3, RAF, X3F) simply
        // won't match and are left unrotated rather than failing.
        let oriented = match exif_orientation(data) {
            Some(o) => apply_orientation(decoded, o),
            None => decoded,
        };
        let t_resize = std::time::Instant::now();
        let resized = downscale(oriented, max_long_edge);
        log::debug!(
            "'{filename}': resize to {}x{} took {:?}",
            resized.width(),
            resized.height(),
            t_resize.elapsed()
        );
        let t_encode = std::time::Instant::now();
        let jpeg = encode_jpeg(&resized, quality)?;
        log::debug!("'{filename}': JPEG re-encode took {:?}", t_encode.elapsed());
        return Ok((jpeg, jpeg_filename(filename)));
    }

    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| UnsupportedImageError(format!("probe failed for '{filename}': {e}")))?;
    let format = reader.format();
    let Some(format) = format else {
        return Err(UnsupportedImageError(format!(
            "Unsupported or corrupt image data for '{filename}'"
        )));
    };
    let (w, h) = reader.into_dimensions().map_err(|_| {
        UnsupportedImageError(format!(
            "Unsupported or corrupt image data for '{filename}'"
        ))
    })?;

    // Byte-identical pass-through for in-limit JPEG/PNG (embedding stability).
    if matches!(format, ImageFormat::Jpeg | ImageFormat::Png) && w.max(h) <= max_long_edge {
        return Ok((data.to_vec(), filename.to_string()));
    }

    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| UnsupportedImageError(format!("Failed to convert image '{filename}': {e}")))?;
    let decoded = decode_without_alloc_cap(reader)
        .map_err(|e| UnsupportedImageError(format!("Failed to convert image '{filename}': {e}")))?;

    let oriented = match exif_orientation(data) {
        Some(o) => apply_orientation(decoded, o),
        None => decoded,
    };
    let resized = downscale(oriented, max_long_edge);
    Ok((encode_jpeg(&resized, quality)?, jpeg_filename(filename)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        });
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(img)
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn small_png_passes_through_byte_identical() {
        let data = png_bytes(640, 480);
        let (out, name) = normalize_image_bytes(&data, Some("photo.png"), 2048, 85).unwrap();
        assert_eq!(out, data);
        assert_eq!(name, "photo.png");
    }

    #[test]
    fn oversized_png_is_downscaled_and_reencoded() {
        let data = png_bytes(3000, 2000);
        let (out, name) = normalize_image_bytes(&data, Some("big.png"), 2048, 85).unwrap();
        assert_eq!(name, "big.jpg");
        let img = image::load_from_memory(&out).unwrap();
        // int(3000 * 2048/3000) = 2048, int(2000 * 0.6826...) = 1365
        assert_eq!((img.width(), img.height()), (2048, 1365));
        assert_ne!(out, data);
    }

    #[test]
    fn raw_and_video_and_garbage_rejected() {
        assert!(normalize_image_bytes(b"x", Some("a.nef"), 2048, 85).is_err());
        assert!(normalize_image_bytes(b"x", Some("a.mp4"), 2048, 85).is_err());
        assert!(normalize_image_bytes(b"not an image", Some("a.jpg"), 2048, 85).is_err());
        assert!(normalize_image_bytes(b"", Some("a.jpg"), 2048, 85).is_err());
    }

    /// A real-world TIFF (16-bit medium-format scan) exceeded the `image`
    /// crate's default 512 MiB decode-allocation cap and was silently
    /// skipped during indexing — see `decode_without_alloc_cap`, which
    /// `normalize_image_bytes` now decodes through instead of a bare
    /// `reader.decode()`.
    ///
    /// Reproducing the real trigger needs a 512+ MiB pixel buffer, which
    /// is far too slow for a debug-mode unit test (~100s). Instead this
    /// pins the mechanism on a tiny image: pre-set an artificially low
    /// cap on the reader (standing in for the default 512 MiB one a real
    /// oversized file would hit) and confirm `decode_without_alloc_cap` —
    /// the exact function production code calls — ignores it. Regresses
    /// (fails) if that function's `no_limits()` call is ever dropped.
    #[test]
    fn decode_without_alloc_cap_ignores_a_preset_cap() {
        let data = png_bytes(64, 64);

        let mut reader = ImageReader::new(Cursor::new(data.clone()))
            .with_guessed_format()
            .unwrap();
        let mut tiny_cap = image::Limits::default();
        tiny_cap.max_alloc = Some(1);
        reader.limits(tiny_cap.clone());
        // Sanity check: pins that this cap really does trigger the same
        // "Memory limit exceeded" error seen in the field, i.e. that this
        // test would actually catch a regression rather than passing
        // vacuously because `tiny_cap` stopped doing anything.
        let err = reader.decode().unwrap_err().to_string();
        assert!(
            err.contains("Memory limit exceeded"),
            "expected the allocation-cap error, got: {err}"
        );

        let mut reader = ImageReader::new(Cursor::new(data))
            .with_guessed_format()
            .unwrap();
        reader.limits(tiny_cap);
        assert!(decode_without_alloc_cap(reader).is_ok());
    }

    #[test]
    fn filename_mapping() {
        assert_eq!(jpeg_filename("IMG_1234.HEIC"), "IMG_1234.jpg");
        assert!(is_raw_filename("photo.NEF"));
        assert!(!is_raw_filename("photo.jpeg"));
        assert!(is_video_filename("clip.MOV"));
    }
}
