//! Image pipeline: Pillow-exact resampling, pHash, culling metrics.
//! Conversion (RAW/HEIC/JPEG) and EXIF/IPTC/XMP parsing follow in M3.

pub mod convert;
pub mod location;
pub mod metrics;
pub mod pil_resample;
