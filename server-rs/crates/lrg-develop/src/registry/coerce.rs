//! The one place where numbers become the registry's value kinds.
//!
//! Builders, [`from_ui`](super::from_ui) and blend results round integer keys
//! half-to-even here, and the readers report a rounding as a warning (the
//! value stays in the model, rounded). Nothing else may cast a float to an
//! integer key.

use super::{Finite, KeySpec, ValueKind};

/// A number (or 0/1 flag) converted to the kind its key expects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scalar {
    /// For `Int`, `IntFlag`, `EnumInt` and `VersionU32` keys.
    Int(i64),
    /// For `Real` keys.
    Real(Finite),
    /// For `Bool` keys.
    Bool(bool),
}

/// Result of [`KeySpec::coerce`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Coerced {
    /// The converted value.
    pub value: Scalar,
    /// True when a non-integer was rounded to fit an integer key. A reader
    /// turns this into a warning; the rounded value is kept.
    pub rounded: bool,
}

/// A value that cannot take the key's kind at all.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum CoerceError {
    /// The key does not hold a number or a flag.
    #[error("{key}: expects {kind:?}, not a number")]
    NotNumeric {
        /// Key name.
        key: &'static str,
        /// The key's kind.
        kind: ValueKind,
    },
    /// Rounded value outside `i64`.
    #[error("{key}: {value} does not fit an integer")]
    OutOfIntRange {
        /// Key name.
        key: &'static str,
        /// The value given.
        value: Finite,
    },
    /// A 0/1 flag got another value.
    #[error("{key}: {value} is not 0 or 1")]
    NotAFlag {
        /// Key name.
        key: &'static str,
        /// The value given.
        value: Finite,
    },
    /// An enum integer outside its set.
    #[error("{key}: {value} is not one of {allowed:?}")]
    NotInEnum {
        /// Key name.
        key: &'static str,
        /// The value given.
        value: i64,
        /// The allowed values.
        allowed: &'static [i64],
    },
}

/// Rounds half to even (`0.5 → 0`, `1.5 → 2`, `-2.5 → -2`); `None` outside
/// the `i64` range.
pub fn round_half_even(x: Finite) -> Option<i64> {
    let r = x.get().round_ties_even();
    // i64::MIN is exactly representable; i64::MAX is not (it rounds up to 2^63).
    if r >= -(2f64.powi(63)) && r < 2f64.powi(63) {
        Some(r as i64)
    } else {
        None
    }
}

impl KeySpec {
    /// Converts a number to this key's kind.
    ///
    /// Integer kinds round half-to-even and flag it in [`Coerced::rounded`];
    /// `IntFlag` then must be 0/1 and `EnumInt` one of its values. `Bool`
    /// keys take exactly 0 and 1 (the 0/1 some Lua tables use). Real keys
    /// take any finite number, integers included. Ranges are not checked
    /// here; see [`from_ui`](super::from_ui).
    pub fn coerce(&self, x: Finite) -> Result<Coerced, CoerceError> {
        let key = self.name;
        match self.kind {
            ValueKind::Real => Ok(Coerced {
                value: Scalar::Real(x),
                rounded: false,
            }),
            ValueKind::Int | ValueKind::VersionU32 | ValueKind::IntFlag | ValueKind::EnumInt(_) => {
                let i = round_half_even(x).ok_or(CoerceError::OutOfIntRange { key, value: x })?;
                let rounded = x.get() != i as f64;
                match self.kind {
                    ValueKind::IntFlag if !(0..=1).contains(&i) => {
                        return Err(CoerceError::NotAFlag { key, value: x })
                    }
                    ValueKind::EnumInt(allowed) if !allowed.contains(&i) => {
                        return Err(CoerceError::NotInEnum {
                            key,
                            value: i,
                            allowed,
                        })
                    }
                    _ => {}
                }
                Ok(Coerced {
                    value: Scalar::Int(i),
                    rounded,
                })
            }
            ValueKind::Bool(_) => {
                let b = if x == Finite::ZERO {
                    false
                } else if x == Finite::new_const(1.0) {
                    true
                } else {
                    return Err(CoerceError::NotAFlag { key, value: x });
                };
                Ok(Coerced {
                    value: Scalar::Bool(b),
                    rounded: false,
                })
            }
            kind => Err(CoerceError::NotNumeric { key, kind }),
        }
    }

    /// Converts a boolean to this key's kind: a `Bool` key keeps it, an
    /// `IntFlag` key gets 0/1.
    pub fn coerce_bool(&self, b: bool) -> Result<Coerced, CoerceError> {
        let value = match self.kind {
            ValueKind::Bool(_) => Scalar::Bool(b),
            ValueKind::IntFlag => Scalar::Int(i64::from(b)),
            kind => {
                return Err(CoerceError::NotNumeric {
                    key: self.name,
                    kind,
                })
            }
        };
        Ok(Coerced {
            value,
            rounded: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{lookup, Level};

    fn spec(name: &str) -> &'static KeySpec {
        lookup(Level::Global, name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .spec()
    }

    fn f(x: f64) -> Finite {
        Finite::new(x).unwrap()
    }

    #[test]
    fn rounds_half_to_even() {
        let cases = [
            (0.5, 0),
            (1.5, 2),
            (2.5, 2),
            (-0.5, 0),
            (-1.5, -2),
            (-2.5, -2),
            (2.4, 2),
            (2.6, 3),
        ];
        for (x, want) in cases {
            assert_eq!(round_half_even(f(x)), Some(want), "{x}");
        }
        assert_eq!(round_half_even(f(1e300)), None);
        assert_eq!(round_half_even(f(-9.3e18)), None);
    }

    #[test]
    fn int_key_rounds_and_flags_the_rounding() {
        let contrast = spec("Contrast2012");
        assert_eq!(
            contrast.coerce(f(12.5)).unwrap(),
            Coerced {
                value: Scalar::Int(12),
                rounded: true
            }
        );
        assert_eq!(
            contrast.coerce(f(-7.0)).unwrap(),
            Coerced {
                value: Scalar::Int(-7),
                rounded: false
            }
        );
    }

    #[test]
    fn real_key_takes_integers_silently() {
        let exposure = spec("Exposure2012");
        assert_eq!(
            exposure.coerce(f(1.0)).unwrap(),
            Coerced {
                value: Scalar::Real(f(1.0)),
                rounded: false
            }
        );
    }

    #[test]
    fn flags_take_zero_and_one_only() {
        let lens = spec("LensProfileEnable");
        assert_eq!(lens.coerce(f(1.0)).unwrap().value, Scalar::Int(1));
        assert!(matches!(
            lens.coerce(f(2.0)),
            Err(CoerceError::NotAFlag { .. })
        ));
        assert_eq!(lens.coerce_bool(true).unwrap().value, Scalar::Int(1));
    }

    #[test]
    fn bool_keys_accept_zero_and_one() {
        let gray = spec("ConvertToGrayscale");
        assert_eq!(gray.coerce(f(0.0)).unwrap().value, Scalar::Bool(false));
        assert_eq!(gray.coerce(f(1.0)).unwrap().value, Scalar::Bool(true));
        assert!(gray.coerce(f(0.5)).is_err());
        assert_eq!(gray.coerce_bool(true).unwrap().value, Scalar::Bool(true));
    }

    #[test]
    fn enum_ints_stay_in_their_set() {
        let style = spec("PostCropVignetteStyle");
        assert_eq!(style.coerce(f(2.0)).unwrap().value, Scalar::Int(2));
        assert!(matches!(
            style.coerce(f(7.0)),
            Err(CoerceError::NotInEnum { value: 7, .. })
        ));
    }

    #[test]
    fn non_numeric_keys_refuse_numbers() {
        assert!(matches!(
            spec("WhiteBalance").coerce(f(1.0)),
            Err(CoerceError::NotNumeric { .. })
        ));
        assert!(spec("WhiteBalance").coerce_bool(true).is_err());
    }
}
