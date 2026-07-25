//! Image pipeline: Pillow-exact resampling, pHash, culling metrics,
//! RAW decoding (`raw`, via `rawler`), JPEG/PNG conversion (`convert`).
//! HEIC (libheif) decoding is still not wired in.

pub mod convert;
pub mod location;
pub mod metrics;
pub mod pil_resample;
pub mod raw;
