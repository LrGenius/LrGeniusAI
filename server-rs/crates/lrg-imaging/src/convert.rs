//! Port of `utils/image_convert.py`: normalize incoming bytes to
//! JPEG-ready bytes for the indexing pipeline.
//!
//! The pass-through rule is the critical invariant: JPEG/PNG bytes whose
//! long edge is within the limit are returned untouched, so embeddings of
//! already-indexed photos never drift.
//!
//! RAW (LibRaw) and HEIC (libheif) decoding are wired in behind
//! `RawDecoder`/vendored builds in a later pass; until then those files
//! raise the same "not available in this backend build" error the Python
//! backend uses when rawpy/pillow-heif are missing — the plugin falls
//! back to its export-based flow by design.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, ImageReader};

use crate::pil_resample::{resize_plane, Filter};

pub const RAW_EXTENSIONS: [&str; 26] = [
    "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "gpr", "iiq", "kdc", "mef",
    "mos", "mrw", "nef", "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw",
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

    let rgb = image.to_rgb8();
    let (iw, ih) = (rgb.width() as usize, rgb.height() as usize);
    let pixels = rgb.as_raw();
    let n = iw * ih;
    let mut out = vec![0u8; new_w as usize * new_h as usize * 3];
    for c in 0..3 {
        let plane: Vec<u8> = (0..n).map(|i| pixels[i * 3 + c]).collect();
        let resized = resize_plane(
            &plane,
            iw,
            ih,
            new_w as usize,
            new_h as usize,
            Filter::Lanczos3,
        );
        for (i, v) in resized.into_iter().enumerate() {
            out[i * 3 + c] = v;
        }
    }
    DynamicImage::ImageRgb8(image::RgbImage::from_raw(new_w, new_h, out).expect("sized buffer"))
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
        return Err(UnsupportedImageError(format!(
            "Cannot decode raw file '{filename}': raw decoding (rawpy) is not \
             available in this backend build"
        )));
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

    let decoded = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map(|r| r.decode())
        .map_err(|e| UnsupportedImageError(format!("Failed to convert image '{filename}': {e}")))?
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

    #[test]
    fn filename_mapping() {
        assert_eq!(jpeg_filename("IMG_1234.HEIC"), "IMG_1234.jpg");
        assert!(is_raw_filename("photo.NEF"));
        assert!(!is_raw_filename("photo.jpeg"));
        assert!(is_video_filename("clip.MOV"));
    }
}
