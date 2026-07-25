//! Camera RAW decoding via `rawler` (the dnglab project) — replaces the
//! Python backend's `rawpy`/LibRaw dependency.
//!
//! Mirrors `utils/image_convert.py::_convert_raw`'s two-tier strategy:
//! prefer the embedded JPEG/bitmap preview (milliseconds) and only fall
//! back to a full per-pixel demosaic when a file has no usable preview.
//! For the common TIFF-based formats (NEF, ARW, ...) as well as CR3,
//! `rawler`'s `full_image()` *is* that embedded-preview extraction (it
//! reads the `JPEGInterchangeFormat`/`Length` EXIF tags, or for CR3 the
//! embedded-JPEG track in the ISO-BMFF container), not a demosaic — it
//! just isn't named that way in its own API. `thumbnail_image` is only
//! worth trying for DNG (the only format whose decoder overrides it);
//! every other format goes straight to `full_image()`.

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

    // In rawler 0.7.2, `preview_image` is never overridden by any concrete
    // decoder (only the trait default, which logs a warning and returns
    // `Ok(None)`) and `thumbnail_image` is only overridden by DNG — every
    // other format (CR2/CR3/NEF/ARW/RAF/...) hits the same dead-code stub.
    // Skip straight to `full_image()` for those instead of paying two
    // guaranteed-useless calls (and log spam) on every single decode.
    if decoder.format_hint() == rawler::decoders::FormatHint::DNG {
        if let Some(img) = decoder
            .thumbnail_image(&source, &params)
            .map_err(|e| RawDecodeError(e.to_string()))?
        {
            return Ok(img);
        }
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
