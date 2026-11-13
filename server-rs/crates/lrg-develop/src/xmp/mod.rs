//! The XMP form: sidecars (`<photo>.xmp`), develop presets and profiles as
//! Lightroom Classic and Camera Raw write them. [`read`] parses all three;
//! [`write`](mod@write) writes develop presets ([`format`](mod@format) spells the values).

pub mod format;
pub mod read;
pub mod write;

pub use format::EngineVersion;
pub use read::{
    parse, PresetHeader, SkippedCounts, XmpDocument, XmpKind, CRSS_NS, CRS_NS, MAX_XMP_BYTES,
    MAX_XMP_DEPTH,
};
pub use write::{
    compatible_version, write, PresetSpec, WriteError, WriteMode, Written, COMPATIBLE_VERSIONS,
    TARGET_ENGINE,
};
