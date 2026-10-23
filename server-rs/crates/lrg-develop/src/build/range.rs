//! Luminance range masks (`Mask/RangeMask`, `CorrectionRangeMask.Type` 2).
//!
//! `LumRange` is four luminance levels in 0..1: low feather, low, high, high
//! feather. Tones between low and high are fully selected, the selection
//! fades out between each feather point and its edge. The builder checks
//! `0 ≤ low feather ≤ low ≤ high ≤ high feather ≤ 1`. `Version` 3 and
//! `SampleType` 0 are added by the preset writer; the eyedropper sample
//! (`LuminanceDepthSampleInfo`) is photo-specific and never built, although
//! every luminance range in Lightroom's own sidecars carries one (no preset
//! of Adobe's has a range mask, so the preset form without it is
//! unverified). `CorrectionRangeMask.Invert` is not the caller's: the builder
//! sets it to the component's `MaskInverted`, as Lightroom does.

use super::BuildError;
use crate::model::correction::LuminanceRange;
use crate::model::value::{Finite, Struct};
use crate::registry::StructKind;

impl LuminanceRange {
    /// A luminance range from `[low feather, low, high, high feather]`.
    pub fn new(range: [f64; 4]) -> Result<LuminanceRange, BuildError> {
        let err = || BuildError::LumRangeOrder(range);
        let finite = range
            .iter()
            .map(|&x| Finite::new(x).map_err(|_| err()))
            .collect::<Result<Vec<_>, _>>()?;
        let r = LuminanceRange {
            range: finite.try_into().expect("four values"),
            invert: false,
            rest: Struct::new(StructKind::CorrectionRangeMask),
        };
        check(&r)?;
        Ok(r)
    }

    /// The shadows: black up to `upto`, fading out over `feather` above it.
    pub fn lows(upto: f64, feather: f64) -> Result<LuminanceRange, BuildError> {
        LuminanceRange::new([0.0, 0.0, upto, upto + feather])
    }

    /// The highlights: `from` up to white, fading in over `feather` below
    /// it.
    pub fn highs(from: f64, feather: f64) -> Result<LuminanceRange, BuildError> {
        LuminanceRange::new([from - feather, from, 1.0, 1.0])
    }
}

pub(super) fn check(r: &LuminanceRange) -> Result<(), BuildError> {
    let [fl, l, h, fh] = r.range.map(Finite::get);
    if !(0.0 <= fl && fl <= l && l <= h && h <= fh && fh <= 1.0) {
        return Err(BuildError::LumRangeOrder([fl, l, h, fh]));
    }
    if !r.rest.fields.is_empty() {
        return Err(BuildError::NotBuildable(
            "a luminance range with further CorrectionRangeMask fields",
        ));
    }
    Ok(())
}
