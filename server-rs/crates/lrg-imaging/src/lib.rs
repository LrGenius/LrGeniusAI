//! Image pipeline: Pillow-exact resampling, pHash, culling metrics,
//! region focus at full resolution (`focus`),
//! source-state measurement for the edit path (`scene`), a render's tones in
//! numbers for AI Edit's retoucher loop (`tone`),
//! RAW decoding (`raw`, via `rawler`), JPEG/PNG conversion (`convert`),
//! location metadata (`location`) and offline reverse geocoding (`geocode`).
//! HEIC (libheif) decoding is still not wired in.

pub mod capture;
pub mod convert;
pub mod cull_config;
pub mod focus;
pub mod geocode;
pub mod location;
pub mod metrics;
pub mod pil_resample;
pub mod raw;
pub mod scene;
pub mod subject_focus;
pub mod tone;
