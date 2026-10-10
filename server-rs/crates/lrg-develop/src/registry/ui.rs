//! The one place where stored values and UI values are converted.
//!
//! Lua tables and XMP files both carry the **stored** unit
//! (`LocalExposure2012 = EV / 4`, other signed `Local*` = UI / 100). UI units
//! exist only at the recipe/LLM/display boundary, and they go through here.
//!
//! [`from_ui_value`] is what a builder or blend uses: it returns the model
//! [`Value`] of the key's kind (`Value::Int` for `Contrast2012`), so the
//! integer rounding happens here and in [`KeySpec::coerce`] only.
//! [`from_ui`]/[`from_ui_clamped`] give the same number as a [`Finite`], for
//! display and comparison.

use super::{CoerceError, Finite, KeySpec, Scalar, UiScale};
use crate::model::Value;

/// Why a UI value could not be converted.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum UiError {
    /// The key is not a number.
    #[error("{key}: not a numeric key")]
    NotNumeric {
        /// Key name.
        key: &'static str,
    },
    /// The key's UI scale is unverified (`LocalHue`, `LocalGrain`), so no
    /// value may be converted for it.
    #[error("{key}: the UI scale is not verified; the value cannot be converted")]
    UnknownScale {
        /// Key name.
        key: &'static str,
    },
    /// The stored value falls outside the key's range.
    #[error("{key}: {value} (stored unit) is outside {min}..{max}")]
    OutOfRange {
        /// Key name.
        key: &'static str,
        /// The stored value before clamping.
        value: Finite,
        /// Lower bound (stored unit).
        min: Finite,
        /// Upper bound (stored unit).
        max: Finite,
    },
    /// The value does not fit the key's kind (a flag or enum value).
    #[error(transparent)]
    Coerce(#[from] CoerceError),
    /// The conversion left the range of `f64` (a huge stored value times the
    /// key's divisor).
    #[error("{key}: the converted value is not a finite number")]
    NotFinite {
        /// Key name.
        key: &'static str,
    },
}

fn divisor(spec: &KeySpec) -> Result<f64, UiError> {
    if !spec.kind.is_numeric() {
        return Err(UiError::NotNumeric { key: spec.name });
    }
    match spec.ui {
        UiScale::Identity => Ok(1.0),
        UiScale::Div(d) => Ok(d),
        UiScale::Unknown => Err(UiError::UnknownScale { key: spec.name }),
    }
}

fn finite(spec: &KeySpec, x: f64) -> Result<Finite, UiError> {
    // A finite stored value times the divisor can still overflow (1e307 ×
    // 100); dividing by a divisor ≥ 1 cannot.
    Finite::new(x).map_err(|_| UiError::NotFinite { key: spec.name })
}

fn as_finite(spec: &KeySpec, s: Scalar) -> Result<Finite, UiError> {
    match s {
        Scalar::Int(i) => Ok(Finite::from_i64(i)),
        Scalar::Real(r) => Ok(r),
        Scalar::Bool(_) => Err(UiError::NotNumeric { key: spec.name }),
    }
}

/// Stored value → UI value (`LocalExposure2012` 0.0625 → 0.25 EV).
pub fn to_ui(spec: &KeySpec, stored: Finite) -> Result<Finite, UiError> {
    let d = divisor(spec)?;
    finite(spec, stored.get() * d)
}

/// UI value → stored value, rounded for integer keys ([`KeySpec::coerce`])
/// and checked against the key's range. Out of range is an error; use
/// [`from_ui_clamped`] to clamp instead.
pub fn from_ui(spec: &KeySpec, ui: Finite) -> Result<Finite, UiError> {
    match from_ui_clamped(spec, ui)? {
        (v, None) => Ok(v),
        (_, Some(err)) => Err(err),
    }
}

/// Like [`from_ui`], but clamps an out-of-range value to the key's range and
/// returns the [`UiError::OutOfRange`] alongside it, so the caller can report
/// the clamping (a builder as `Skipped`, a blend as a warning).
pub fn from_ui_clamped(spec: &KeySpec, ui: Finite) -> Result<(Finite, Option<UiError>), UiError> {
    let (s, err) = stored_scalar(spec, ui)?;
    Ok((as_finite(spec, s)?, err))
}

/// [`from_ui_clamped`] as the model value of the key's kind: `Value::Int`
/// for integer keys (equal to what the readers produce), `Value::Real` for
/// real keys. This is the form a builder or blend stores.
pub fn from_ui_value(spec: &KeySpec, ui: Finite) -> Result<(Value, Option<UiError>), UiError> {
    let (s, err) = stored_scalar(spec, ui)?;
    Ok((Value::from(s), err))
}

/// UI → stored, clamped to the range (with the error alongside), coerced to
/// the key's kind.
fn stored_scalar(spec: &KeySpec, ui: Finite) -> Result<(Scalar, Option<UiError>), UiError> {
    let d = divisor(spec)?;
    let stored = finite(spec, ui.get() / d)?;
    let (min, max) = match spec.range {
        Some(r) => r,
        None => return Ok((spec.coerce(stored)?.value, None)),
    };
    if stored < min || stored > max {
        let clamped = stored.clamp(min, max);
        let err = UiError::OutOfRange {
            key: spec.name,
            value: stored,
            min,
            max,
        };
        return Ok((spec.coerce(clamped)?.value, Some(err)));
    }
    Ok((spec.coerce(stored)?.value, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{iter, lookup, Level, Policy};

    fn spec(level: Level, name: &str) -> &'static KeySpec {
        lookup(level, name)
            .unwrap_or_else(|| panic!("{level}/{name} missing"))
            .spec()
    }

    fn f(x: f64) -> Finite {
        Finite::new(x).unwrap()
    }

    #[test]
    fn local_exposure_is_stored_as_a_quarter() {
        let s = spec(Level::Correction, "LocalExposure2012");
        assert_eq!(from_ui(s, f(0.25)).unwrap(), f(0.0625));
        assert_eq!(to_ui(s, f(0.0625)).unwrap(), f(0.25));
        assert_eq!(from_ui(s, f(4.0)).unwrap(), f(1.0));
        assert!(matches!(
            from_ui(s, f(5.0)),
            Err(UiError::OutOfRange { .. })
        ));
    }

    #[test]
    fn signed_locals_and_amount_are_stored_per_hundred() {
        let shadows = spec(Level::Correction, "LocalShadows2012");
        assert_eq!(from_ui(shadows, f(12.0)).unwrap(), f(0.12));
        let amount = spec(Level::Correction, "CorrectionAmount");
        assert_eq!(from_ui(amount, f(100.0)).unwrap(), f(1.0));
        assert_eq!(from_ui(amount, f(200.0)).unwrap(), f(2.0));
    }

    #[test]
    fn identity_keys_stay_in_ui_units() {
        let hue = spec(Level::Correction, "LocalToningHue");
        assert_eq!(from_ui(hue, f(240.0)).unwrap(), f(240.0));
        let refine = spec(Level::Correction, "LocalCurveRefineSaturation");
        assert_eq!(from_ui(refine, f(30.0)).unwrap(), f(30.0));
        let exposure = spec(Level::Global, "Exposure2012");
        assert_eq!(from_ui(exposure, f(0.5)).unwrap(), f(0.5));
    }

    #[test]
    fn unverified_scales_cannot_be_converted() {
        for name in ["LocalHue", "LocalGrain"] {
            let s = spec(Level::Correction, name);
            assert!(matches!(
                from_ui(s, f(1.0)),
                Err(UiError::UnknownScale { .. })
            ));
            assert!(matches!(
                to_ui(s, f(0.1)),
                Err(UiError::UnknownScale { .. })
            ));
        }
    }

    #[test]
    fn integer_keys_round_half_even_on_the_way_in() {
        let contrast = spec(Level::Global, "Contrast2012");
        assert_eq!(from_ui(contrast, f(12.5)).unwrap(), f(12.0));
        assert_eq!(from_ui(contrast, f(13.5)).unwrap(), f(14.0));
    }

    #[test]
    fn clamping_reports_the_original_value() {
        let contrast = spec(Level::Global, "Contrast2012");
        let (v, err) = from_ui_clamped(contrast, f(250.0)).unwrap();
        assert_eq!(v, f(100.0));
        assert!(matches!(
            err,
            Some(UiError::OutOfRange { value, .. }) if value == f(250.0)
        ));
        let (v, err) = from_ui_clamped(contrast, f(-20.0)).unwrap();
        assert_eq!((v, err), (f(-20.0), None));
    }

    #[test]
    fn from_ui_value_keeps_the_key_kind() {
        let contrast = spec(Level::Global, "Contrast2012");
        assert_eq!(
            from_ui_value(contrast, f(12.5)).unwrap(),
            (Value::Int(12), None)
        );
        let exposure = spec(Level::Global, "Exposure2012");
        assert_eq!(
            from_ui_value(exposure, f(0.5)).unwrap(),
            (Value::Real(f(0.5)), None)
        );
        let local = spec(Level::Correction, "LocalExposure2012");
        let (v, err) = from_ui_value(local, f(5.0)).unwrap();
        assert_eq!(v, Value::Real(f(1.0)));
        assert!(matches!(err, Some(UiError::OutOfRange { .. })));
    }

    #[test]
    fn an_overflowing_conversion_is_not_finite() {
        let shadows = spec(Level::Correction, "LocalShadows2012");
        assert_eq!(
            to_ui(shadows, f(1e307)),
            Err(UiError::NotFinite {
                key: "LocalShadows2012"
            })
        );
    }

    #[test]
    fn non_numeric_keys_are_refused() {
        let wb = spec(Level::Global, "WhiteBalance");
        assert!(matches!(
            from_ui(wb, f(1.0)),
            Err(UiError::NotNumeric { .. })
        ));
    }

    /// Every learnable correction-level number converts both ways, and the
    /// ends of its UI range land on the ends of its stored range.
    #[test]
    fn every_learnable_correction_key_round_trips_through_ui_units() {
        let mut checked = 0;
        for (_, s) in iter() {
            if s.level != Level::Correction || !s.policy.is_learnable() || !s.kind.is_numeric() {
                continue;
            }
            let (min, max) = s.range.unwrap_or_else(|| panic!("{} has no range", s.name));
            for stored in [min, max, Finite::ZERO.clamp(min, max)] {
                let ui = to_ui(s, stored).unwrap();
                let back = from_ui(s, ui).unwrap();
                assert!(
                    (back.get() - stored.get()).abs() < 1e-12,
                    "{}: {stored} -> {ui} -> {back}",
                    s.name
                );
            }
            checked += 1;
        }
        assert!(checked >= 20, "only {checked} correction keys checked");
        // The policy filter above must not silently skip the scaled keys.
        assert!(spec(Level::Correction, "LocalExposure2012").policy == Policy::Learn);
    }
}
