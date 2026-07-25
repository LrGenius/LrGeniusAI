//! Camera RAW decoding via `rawler` (the dnglab project) — replaces the
//! Python backend's `rawpy`/LibRaw dependency.
//!
//! Mirrors `utils/image_convert.py::_convert_raw`'s two-tier strategy:
//! prefer the embedded JPEG/bitmap preview (milliseconds) and only fall
//! back to a full per-pixel demosaic when a file has no usable preview.
//! For the common TIFF-based formats (NEF, ARW, ...) `rawler`'s
//! `full_image()` *is* that embedded-preview extraction (it reads the
//! `JPEGInterchangeFormat`/`Length` EXIF tags), not a demosaic — it just
//! isn't named that way in its own API, so this module tries
//! `thumbnail_image` -> `preview_image` -> `full_image` in that order
//! and treats the first `Some(_)` as the fast path regardless of which
//! method produced it.

use image::DynamicImage;
use rawler::decoders::RawDecodeParams;
use rawler::imgop::develop::RawDevelop;
use rawler::rawsource::RawSource;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RawDecodeError(pub String);

/// Decode RAW `data` to a `DynamicImage`, preferring an embedded preview
/// and falling back to a full demosaic. Does not apply orientation —
/// callers combine this with EXIF-based orientation the same way they
/// already do for JPEG/HEIC (`rawler`'s embedded-preview path returns a
/// plain decoded image with no EXIF of its own attached).
pub fn decode_raw(data: &[u8]) -> Result<DynamicImage, RawDecodeError> {
    let source = RawSource::new_from_slice(data);
    let decoder = rawler::get_decoder(&source).map_err(|e| RawDecodeError(e.to_string()))?;
    let params = RawDecodeParams::default();

    if let Some(img) = decoder
        .thumbnail_image(&source, &params)
        .map_err(|e| RawDecodeError(e.to_string()))?
    {
        return Ok(img);
    }
    if let Some(img) = decoder
        .preview_image(&source, &params)
        .map_err(|e| RawDecodeError(e.to_string()))?
    {
        return Ok(img);
    }
    if let Some(img) = decoder
        .full_image(&source, &params)
        .map_err(|e| RawDecodeError(e.to_string()))?
    {
        return Ok(img);
    }

    // No usable embedded preview at any tier — full demosaic as a last
    // resort, matching Python's `raw.postprocess(...)` fallback.
    let raw_image = rawler::decode(&source, &params).map_err(|e| RawDecodeError(e.to_string()))?;
    RawDevelop::default()
        .develop_intermediate(&raw_image)
        .map_err(|e| RawDecodeError(e.to_string()))?
        .to_dynamic_image()
        .ok_or_else(|| RawDecodeError("demosaic produced no usable image data".to_string()))
}
