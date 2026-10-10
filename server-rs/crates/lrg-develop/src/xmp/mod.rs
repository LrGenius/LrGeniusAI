//! The XMP form: sidecars (`<photo>.xmp`), develop presets and profiles as
//! Lightroom Classic and Camera Raw write them. Reader now; the writer
//! follows in PR 1b.

pub mod read;

pub use read::{
    parse, PresetHeader, SkippedCounts, XmpDocument, XmpKind, CRSS_NS, CRS_NS, MAX_XMP_BYTES,
    MAX_XMP_DEPTH,
};
