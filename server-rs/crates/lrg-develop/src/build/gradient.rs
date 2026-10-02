//! Linear and radial gradients.
//!
//! Coordinates are normalised per axis in the **uncropped image in sensor
//! orientation** (0 = left/top edge, 1 = right/bottom edge; values may leave
//! 0..1), the frame Lightroom stores gradients in. Builders take only
//! [`SensorPoint`]s and normalised numbers, never raw pixel tuples;
//! converting export or display pixels (`FrameGeometry`) comes with its first
//! consumer in step 2. Gradients are PHOTO: they belong to one frame and
//! never reach a shared preset.

use super::{BuildError, RADIAL_MIDPOINT, RADIAL_ROUNDNESS};
use crate::model::correction::{LinearGradient, RadialGradient, SensorPoint};
use crate::model::value::Finite;

fn finite(x: f64, what: &'static str) -> Result<Finite, BuildError> {
    Finite::new(x).map_err(|_| BuildError::NotFinite { what })
}

impl SensorPoint {
    /// A point from normalised sensor coordinates.
    pub fn new(x: f64, y: f64) -> Result<SensorPoint, BuildError> {
        Ok(SensorPoint {
            x: finite(x, "point x")?,
            y: finite(y, "point y")?,
        })
    }
}

impl LinearGradient {
    /// A linear gradient from `zero` to `full`; the two points must differ.
    /// By the key names (`ZeroX`/`FullX`) the effect is none at `zero` and
    /// full at `full`; that direction is unverified until experiment E6.
    pub fn new(zero: SensorPoint, full: SensorPoint) -> Result<LinearGradient, BuildError> {
        let g = LinearGradient { zero, full };
        check_linear(&g)?;
        Ok(g)
    }
}

impl RadialGradient {
    /// An unrotated ellipse in its bounding box (`top < bottom`,
    /// `left < right`) with `feather` 0..=100. `Angle` is 0, `Midpoint` 50
    /// and `Roundness` 0, as Lightroom writes them; rotation stays out until
    /// experiment E6 settles its convention.
    ///
    /// `flipped` starts true (effect inside); [`CorrectionBuilder::build`](super::CorrectionBuilder::build)
    /// sets it to `!MaskInverted` of the component's combination, the
    /// relation every radial gradient Lightroom writes holds.
    pub fn new(
        top: f64,
        left: f64,
        bottom: f64,
        right: f64,
        feather: i64,
    ) -> Result<RadialGradient, BuildError> {
        let g = RadialGradient {
            top: finite(top, "top")?,
            left: finite(left, "left")?,
            bottom: finite(bottom, "bottom")?,
            right: finite(right, "right")?,
            feather,
            midpoint: RADIAL_MIDPOINT,
            roundness: RADIAL_ROUNDNESS,
            flipped: true,
        };
        check_radial(&g)?;
        Ok(g)
    }
}

pub(super) fn check_linear(g: &LinearGradient) -> Result<(), BuildError> {
    if g.zero == g.full {
        return Err(BuildError::Geometry(
            "a linear gradient needs two different points",
        ));
    }
    Ok(())
}

pub(super) fn check_radial(g: &RadialGradient) -> Result<(), BuildError> {
    if g.top >= g.bottom || g.left >= g.right {
        return Err(BuildError::Geometry(
            "a radial gradient needs top < bottom and left < right",
        ));
    }
    if !(0..=100).contains(&g.feather) {
        return Err(BuildError::Geometry(
            "a radial gradient's feather is 0..=100",
        ));
    }
    if g.midpoint != RADIAL_MIDPOINT || g.roundness != RADIAL_ROUNDNESS {
        return Err(BuildError::NotBuildable(
            "a radial gradient with a midpoint other than 50 or a roundness other than 0",
        ));
    }
    Ok(())
}
