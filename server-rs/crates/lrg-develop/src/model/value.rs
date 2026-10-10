//! Values of the develop model.
//!
//! [`Finite`] is the float type for everything that can reach a writer:
//! setting values, amounts, curve points and the registry's ranges. JSON.lua
//! and `serde_json` both turn NaN and infinities into `null`, so a non-finite
//! number must be impossible to construct rather than checked at the end.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::correction::Correction;
use crate::registry::{KeyId, KeySpec, Level, Lit, NumFmt, Scalar, StructKind};

/// A finite `f64`: never NaN, never ±∞, and `-0.0` is stored as `0.0`.
///
/// Equality, ordering and hashing go through the bit pattern (after the
/// `-0.0` normalisation), so `Finite` can be `Eq`, `Ord` and a map key, and a
/// value that compares equal also serializes identically.
#[derive(Clone, Copy)]
pub struct Finite(f64);

/// The value handed to [`Finite::new`] was NaN or infinite.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("not a finite number: {0}")]
pub struct NotFinite(pub f64);

impl Finite {
    /// `0.0`.
    pub const ZERO: Finite = Finite(0.0);

    /// Checks `x`; `-0.0` becomes `0.0`.
    pub fn new(x: f64) -> Result<Finite, NotFinite> {
        if x.is_finite() {
            Ok(Finite(if x == 0.0 { 0.0 } else { x }))
        } else {
            Err(NotFinite(x))
        }
    }

    /// Compile-time constructor for tables such as the key registry.
    ///
    /// # Panics
    ///
    /// Panics when `x` is not finite. In a `const`/`static` initializer that
    /// panic is a compile error, which is the point.
    pub const fn new_const(x: f64) -> Finite {
        assert!(x.is_finite(), "Finite::new_const: not a finite number");
        Finite(if x == 0.0 { 0.0 } else { x })
    }

    /// Builds a `Finite` from an integer (exact for |i| < 2^53).
    pub fn from_i64(i: i64) -> Finite {
        // Every i64 is finite as an f64; `new_const` only normalises -0.0.
        Finite::new_const(i as f64)
    }

    /// The wrapped value.
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for Finite {
    type Error = NotFinite;
    fn try_from(x: f64) -> Result<Self, Self::Error> {
        Finite::new(x)
    }
}

impl From<Finite> for f64 {
    fn from(f: Finite) -> f64 {
        f.0
    }
}

impl PartialEq for Finite {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Finite {}

impl Hash for Finite {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl PartialOrd for Finite {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Finite {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl fmt::Debug for Finite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for Finite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Serialize for Finite {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for Finite {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let x = f64::deserialize(deserializer)?;
        Finite::new(x).map_err(serde::de::Error::custom)
    }
}

/// 32 hex characters: a sync id, UUID or digest.
///
/// Kept exactly as read (case included); the XMP writer (PR 1b) upper-cases.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Hex32(String);

impl Hex32 {
    /// `Some` for exactly 32 ASCII hex digits.
    pub fn parse(s: &str) -> Option<Hex32> {
        (s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| Hex32(s.to_owned()))
    }

    /// The digits as read.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Hex32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Placeholder for an XMP subtree kept verbatim (namespace, `rdf:Bag`,
/// further languages, qualifiers). The XMP parser (PR 1c) fills it in; until
/// then it cannot be constructed outside this crate.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct XmpNode {}

/// Lossless pass-through of something the model does not interpret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opaque {
    /// From a Lua/JSON table: `null` in an array, mixed arrays, unknown
    /// objects, and the keys the registry keeps opaque on purpose
    /// (`ValueKind::Any`, `ValueKind::Settings`).
    Json(serde_json::Value),
    /// From an XMP file (PR 1c).
    Xmp(XmpNode),
}

/// A key the model keeps without a registry row, or a registry key whose
/// value did not have the registry's type: its name, and the value verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpaqueEntry {
    /// XMP namespace URI; `None` for Lua tables and the `crs:` namespace.
    pub ns: Option<String>,
    /// Key name as it appeared.
    pub name: String,
    /// The value, verbatim.
    pub value: Opaque,
}

/// The fields of one level: registry keys by [`KeyId`] (registry order, the
/// same in every build) plus what the registry does not know (sorted by name
/// from a Lua table, in document order from XMP).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fields {
    /// Registry keys and their values.
    pub values: BTreeMap<KeyId, Value>,
    /// Unknown keys, pattern-family keys and wrongly typed values (see
    /// [`DevelopSettings::opaque`](super::DevelopSettings::opaque) for the
    /// order).
    pub opaque: Vec<OpaqueEntry>,
}

impl Fields {
    /// True when there are no values and no opaque entries.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty() && self.opaque.is_empty()
    }

    /// The value of `name` at `level`, if the registry knows the key and it is set.
    pub fn get(&self, level: Level, name: &str) -> Option<&Value> {
        self.values.get(&crate::registry::lookup(level, name)?)
    }

    /// Removes and returns the value of `name` at `level`.
    pub fn take(&mut self, level: Level, name: &str) -> Option<Value> {
        self.values.remove(&crate::registry::lookup(level, name)?)
    }

    /// Removes `name` at `level` when `convert` accepts its value; a value
    /// `convert` rejects stays where it was.
    pub fn take_if<T>(
        &mut self,
        level: Level,
        name: &str,
        convert: impl FnOnce(&Value) -> Option<T>,
    ) -> Option<T> {
        let id = crate::registry::lookup(level, name)?;
        let t = convert(self.values.get(&id)?)?;
        self.values.remove(&id);
        Some(t)
    }
}

/// A nested structure (`LensBlur`, `CorrectionRangeMask`, a `PointColors`
/// item, ...): its kind and its fields at [`Level::Struct`] of that kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Struct {
    /// Which structure.
    pub kind: StructKind,
    /// Its fields.
    pub fields: Fields,
}

impl Struct {
    /// An empty structure of `kind`.
    pub fn new(kind: StructKind) -> Struct {
        Struct {
            kind,
            fields: Fields::default(),
        }
    }

    /// The value of field `name`.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.get(Level::Struct(self.kind), name)
    }
}

/// One value of the model, in the **stored** unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// `Int`, `IntFlag` (0/1), `EnumInt` and `VersionU32` keys.
    Int(i64),
    /// `Real` keys.
    Real(Finite),
    /// `Bool` keys.
    Bool(bool),
    /// `Str`, `Enum`, `Hex32` and `VersionStr` keys.
    Str(String),
    /// A tone curve as (x, y) points, global or local form alike.
    Curve(Vec<(Finite, Finite)>),
    /// A sequence of strings.
    StrList(Vec<String>),
    /// A language alternative with only an `x-default` item.
    Alt(String),
    /// One nested structure.
    Struct(Struct),
    /// A sequence of nested structures.
    StructList(Vec<Struct>),
    /// Nested mask items kept uninterpreted at [`Level::MaskTool`]: the brush
    /// strokes of a `Mask/Aggregate`, the heal shapes of a retouch spot.
    Tools(Vec<Fields>),
    /// Corrections of a legacy `*BasedCorrections` key
    /// (`MaskGroupBasedCorrections` itself is [`DevelopSettings::corrections`](super::DevelopSettings::corrections)).
    Corrections(Vec<Correction>),
    /// A registry key the model keeps verbatim on purpose (`ValueKind::Any`,
    /// `Look.Parameters`, a language alternative with several languages).
    Opaque(Opaque),
}

impl From<Scalar> for Value {
    /// A coerced number ([`KeySpec::coerce`]) as the value of its key's kind.
    fn from(s: Scalar) -> Value {
        match s {
            Scalar::Int(i) => Value::Int(i),
            Scalar::Real(r) => Value::Real(r),
            Scalar::Bool(b) => Value::Bool(b),
        }
    }
}

/// Rounds to `decimals` places the way a `%.Nf` writer would.
fn round_to(x: Finite, decimals: u8) -> Finite {
    format!("{:.*}", usize::from(decimals), x.get())
        .parse::<f64>()
        .ok()
        .and_then(|y| Finite::new(y).ok())
        .unwrap_or(x)
}

impl Value {
    /// The value rounded to the precision the XMP writer uses for this key
    /// ([`NumFmt`]), for comparing a Lua value with one that went through
    /// XMP. Non-numeric values are returned unchanged.
    pub fn quantized(&self, spec: &KeySpec) -> Value {
        let decimals = match spec.fmt {
            NumFmt::Int => 0,
            NumFmt::Fixed(n) => n,
            NumFmt::Trim6 | NumFmt::CompoundFixed6 => 6,
            NumFmt::Text => return self.clone(),
        };
        match self {
            Value::Real(x) => Value::Real(round_to(*x, decimals)),
            Value::Curve(points) => Value::Curve(
                points
                    .iter()
                    .map(|&(x, y)| (round_to(x, decimals), round_to(y, decimals)))
                    .collect(),
            ),
            v => v.clone(),
        }
    }

    /// Whether this value equals a registry default literal. Integers and
    /// reals compare numerically; a curve compares with a flat integer list.
    pub fn equals_lit(&self, lit: &Lit) -> bool {
        match (self, lit) {
            (Value::Int(a), Lit::Int(b)) => a == b,
            (Value::Int(a), Lit::Real(b)) => (*a as f64) == b.get(),
            (Value::Real(a), Lit::Real(b)) => a == b,
            (Value::Real(a), Lit::Int(b)) => a.get() == *b as f64,
            (Value::Bool(a), Lit::Bool(b)) => a == b,
            (Value::Str(a) | Value::Alt(a), Lit::Str(b)) => a == b,
            (Value::Curve(points), Lit::IntList(flat)) => {
                points.len() * 2 == flat.len()
                    && points
                        .iter()
                        .zip(flat.as_chunks::<2>().0)
                        .all(|(&(x, y), xy)| x.get() == xy[0] as f64 && y.get() == xy[1] as f64)
            }
            _ => false,
        }
    }

    /// The number of an `Int` or `Real` value.
    pub fn as_finite(&self) -> Option<Finite> {
        match *self {
            Value::Int(i) => Some(Finite::from_i64(i)),
            Value::Real(r) => Some(r),
            _ => None,
        }
    }

    /// The integer of an `Int` value.
    pub fn as_int(&self) -> Option<i64> {
        match *self {
            Value::Int(i) => Some(i),
            _ => None,
        }
    }

    /// The flag of a `Bool` value.
    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }

    /// The text of a `Str` value.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn rejects_nan_and_infinities() {
        assert!(Finite::new(f64::NAN).is_err());
        assert!(Finite::new(f64::INFINITY).is_err());
        assert!(Finite::new(f64::NEG_INFINITY).is_err());
        assert_eq!(Finite::new(1.5).unwrap().get(), 1.5);
    }

    #[test]
    fn negative_zero_equals_zero_and_hashes_the_same() {
        let neg = Finite::new(-0.0).unwrap();
        assert_eq!(neg, Finite::ZERO);
        assert!(neg.get().is_sign_positive());
        let set: HashSet<Finite> = [neg, Finite::ZERO].into_iter().collect();
        assert_eq!(set.len(), 1);
        assert_eq!(Finite::new_const(-0.0), Finite::ZERO);
    }

    #[test]
    fn orders_totally() {
        let mut v = [2.0, -1.0, 0.5].map(Finite::new_const);
        v.sort();
        assert_eq!(v.map(Finite::get), [-1.0, 0.5, 2.0]);
    }

    #[test]
    fn serde_round_trips_and_never_yields_null() {
        let f = Finite::new(0.0625).unwrap();
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(json, "0.0625");
        assert_eq!(serde_json::from_str::<Finite>(&json).unwrap(), f);
        assert!(serde_json::from_str::<Finite>("null").is_err());
    }

    #[test]
    fn hex32_needs_exactly_32_hex_digits() {
        assert!(Hex32::parse("0000000000000000000000000000000A").is_some());
        assert!(Hex32::parse("0000000000000000000000000000000a").is_some());
        assert!(Hex32::parse("000000000000000000000000000000A").is_none());
        assert!(Hex32::parse("0000000000000000000000000000000G").is_none());
    }

    fn spec(level: Level, name: &str) -> &'static KeySpec {
        crate::registry::lookup(level, name).unwrap().spec()
    }

    #[test]
    fn quantized_rounds_to_the_writer_precision() {
        let exposure = spec(Level::Global, "Exposure2012");
        assert_eq!(
            Value::Real(Finite::new_const(0.456)).quantized(exposure),
            Value::Real(Finite::new_const(0.46))
        );
        let local = spec(Level::Correction, "LocalExposure2012");
        assert_eq!(
            Value::Real(Finite::new_const(0.30000000000000004)).quantized(local),
            Value::Real(Finite::new_const(0.3))
        );
        assert_eq!(
            Value::Str("x".into()).quantized(local),
            Value::Str("x".into())
        );
    }

    #[test]
    fn equals_lit_compares_numbers_across_int_and_real() {
        assert!(Value::Int(0).equals_lit(&Lit::Real(Finite::ZERO)));
        assert!(Value::Real(Finite::new_const(2.0)).equals_lit(&Lit::Int(2)));
        assert!(!Value::Bool(true).equals_lit(&Lit::Int(1)));
        let curve = Value::Curve(vec![
            (Finite::ZERO, Finite::ZERO),
            (Finite::new_const(255.0), Finite::new_const(255.0)),
        ]);
        assert!(curve.equals_lit(&Lit::IntList(&[0, 0, 255, 255])));
        assert!(!curve.equals_lit(&Lit::IntList(&[0, 0])));
    }

    #[test]
    fn take_if_leaves_a_rejected_value_in_place() {
        let mut fields = Fields::default();
        let id = crate::registry::lookup(Level::Correction, "CorrectionName").unwrap();
        fields.values.insert(id, Value::Int(3));
        assert_eq!(
            fields.take_if(Level::Correction, "CorrectionName", |v| v
                .as_str()
                .map(str::to_owned)),
            None
        );
        assert!(fields.values.contains_key(&id));
        assert_eq!(
            fields.take(Level::Correction, "CorrectionName"),
            Some(Value::Int(3))
        );
        assert!(fields.is_empty());
    }
}
