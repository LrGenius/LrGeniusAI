//! Image pipeline: Pillow-exact resampling, pHash, culling metrics,
//! source-state measurement for the edit path (`scene`),
//! RAW decoding (`raw`, via `rawler`), JPEG/PNG conversion (`convert`).
//! HEIC (libheif) decoding is still not wired in.

pub mod capture;
pub mod convert;
pub mod cull_config;
pub mod location;
pub mod metrics;
pub mod pil_resample;
pub mod raw;
pub mod scene;
