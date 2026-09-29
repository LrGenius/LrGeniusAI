//! The develop model: values, settings and corrections.
//!
//! [`DevelopSettings`] holds one photo's (or preset's) develop settings in the
//! **stored** unit, keyed by registry [`KeyId`] so the order is the same in
//! every build. Only what needs logic has a typed view: corrections and their
//! masks ([`correction`]), the profile ([`Look`]), white balance
//! ([`whitebalance`]) and the process version. Everything the registry does
//! not know is kept verbatim ([`OpaqueEntry`]), never dropped.

pub mod correction;
pub mod policy;
pub mod value;
pub mod whitebalance;
pub mod xmp_node;

use std::collections::BTreeMap;

pub use correction::{
    Combine, Correction, LinearGradient, LuminanceRange, MaskComponent, MaskTool, RadialGradient,
    Semantic, SensorPoint,
};
pub use policy::{FrameSplit, SkipReason, Skipped, Target, PORTABLE_PROFILES};
pub use value::{Fields, Finite, Hex32, Opaque, OpaqueEntry, Struct, Value};
pub use whitebalance::{NamedWb, WbFamily, WbMode, WbSetting};
pub use xmp_node::{XmpArrayKind, XmpField, XmpNode, XmpValue};

use crate::registry::{self, KeyId, Level, ProcessVersion, StructKind};

/// Raw or not: decides the white-balance family and several defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// Raw files and raw DNGs.
    Raw,
    /// JPEG, TIFF, PSD, HEIC and DNGs converted from them.
    NonRaw,
}

/// What the caller believes the file kind is (for example the plugin's
/// `is_raw`). The readers trust the settings' own white-balance keys over it
/// and warn when they disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum FileKindHint {
    /// Nothing known.
    #[default]
    Unknown,
    /// The caller says raw.
    Raw,
    /// The caller says non-raw.
    NonRaw,
}

impl FileKindHint {
    /// The hinted kind, if any.
    pub fn kind(self) -> Option<FileKind> {
        match self {
            FileKindHint::Unknown => None,
            FileKindHint::Raw => Some(FileKind::Raw),
            FileKindHint::NonRaw => Some(FileKind::NonRaw),
        }
    }
}

impl From<Option<bool>> for FileKindHint {
    /// From an `is_raw` flag.
    fn from(is_raw: Option<bool>) -> FileKindHint {
        match is_raw {
            None => FileKindHint::Unknown,
            Some(true) => FileKindHint::Raw,
            Some(false) => FileKindHint::NonRaw,
        }
    }
}

/// The lowest process version whose settings are learned (PV2012, `"6.7"`):
/// it introduced the `*2012` keys every later version shares.
pub const MIN_SUPPORTED_PV: ProcessVersion = ProcessVersion::PV2012;

/// The creative profile, taken whole from its source: never rebuilt from a
/// name, and `Parameters` (the profile definition) stays opaque in
/// [`Look::rest`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    /// `Name`.
    pub name: Option<String>,
    /// `UUID`.
    pub uuid: Option<Hex32>,
    /// `Amount` (1 = 100 %).
    pub amount: Option<Finite>,
    /// `CameraModelRestriction`: the profile only exists for this camera.
    pub camera_restriction: Option<String>,
    /// Every other field, `Parameters` included, verbatim.
    pub rest: Struct,
}

impl Look {
    /// Splits the typed fields off a `Look` structure (`s.kind` must be
    /// [`StructKind::Look`]). A field whose value does not fit its typed
    /// field, an empty `CameraModelRestriction` included, stays in
    /// [`Look::rest`] as it was.
    pub(crate) fn from_struct(mut s: Struct) -> Look {
        debug_assert_eq!(s.kind, StructKind::Look, "not a Look structure");
        let level = Level::Struct(StructKind::Look);
        let string = |v: &Value| v.as_str().map(str::to_owned);
        Look {
            name: s.fields.take_if(level, "Name", string),
            uuid: s
                .fields
                .take_if(level, "UUID", |v| v.as_str().and_then(Hex32::parse)),
            amount: s.fields.take_if(level, "Amount", Value::as_finite),
            // "" (the form Adobe's own preset headers use) means "no
            // restriction"; it stays in `rest` so a round trip keeps it.
            camera_restriction: s.fields.take_if(level, "CameraModelRestriction", |v| {
                v.as_str().filter(|r| !r.is_empty()).map(str::to_owned)
            }),
            rest: s,
        }
    }
}

/// A key cannot be stored in [`DevelopSettings`]' value map.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InsertError {
    /// Only global keys live in the map; the others belong to corrections,
    /// mask components or structures.
    #[error("{key} is a {level} key, not a global one")]
    NotGlobal {
        /// Key name.
        key: &'static str,
        /// Its level.
        level: Level,
    },
    /// The key has a typed field of its own.
    #[error("{key} is held in its typed field, not in the value map")]
    TypedField {
        /// Key name.
        key: &'static str,
    },
}

/// Global keys modelled as typed fields of [`DevelopSettings`].
const TYPED_GLOBALS: &[&str] = &["MaskGroupBasedCorrections", "Look"];

/// One set of develop settings.
///
/// Invariant (enforced by [`DevelopSettings::insert`], the value map is
/// private): the map holds only [`Level::Global`] keys, and never
/// `MaskGroupBasedCorrections` or `Look`, which are [`Self::corrections`] and
/// [`Self::look`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DevelopSettings {
    values: BTreeMap<KeyId, Value>,
    /// `MaskGroupBasedCorrections`, in panel order (empty = none).
    pub corrections: Vec<Correction>,
    /// The creative profile.
    pub look: Option<Look>,
    /// Keys the registry does not know, pattern-family keys (`Table_…`,
    /// `UprightTransform_N`) and wrongly typed values. The Lua reader emits
    /// them sorted by key name (a Lua table has no order, and `serde_json`'s
    /// map order depends on its features); an XMP reader keeps document
    /// order.
    pub opaque: Vec<OpaqueEntry>,
    /// Raw or non-raw; `None` when unknown.
    pub file_kind: Option<FileKind>,
}

impl DevelopSettings {
    /// Empty settings.
    pub fn new() -> DevelopSettings {
        DevelopSettings::default()
    }

    /// Stores a global value, returning the previous one.
    pub fn insert(&mut self, id: KeyId, value: Value) -> Result<Option<Value>, InsertError> {
        let spec = id.spec();
        if spec.level != Level::Global {
            return Err(InsertError::NotGlobal {
                key: spec.name,
                level: spec.level,
            });
        }
        if TYPED_GLOBALS.contains(&spec.name) {
            return Err(InsertError::TypedField { key: spec.name });
        }
        Ok(self.values.insert(id, value))
    }

    /// The value of a global key.
    pub fn get(&self, id: KeyId) -> Option<&Value> {
        self.values.get(&id)
    }

    /// The value of the global key `name`.
    pub fn get_by_name(&self, name: &str) -> Option<&Value> {
        self.get(registry::lookup(Level::Global, name)?)
    }

    /// Removes a global value.
    pub fn remove(&mut self, id: KeyId) -> Option<Value> {
        self.values.remove(&id)
    }

    /// Global values in registry order.
    pub fn values(&self) -> impl Iterator<Item = (KeyId, &Value)> {
        self.values.iter().map(|(id, v)| (*id, v))
    }

    /// Number of global values (corrections, the Look and opaque entries
    /// not counted; see [`Self::is_empty`] for "nothing at all").
    pub fn value_count(&self) -> usize {
        self.values.len()
    }

    /// True when nothing at all is set (values, corrections, look, opaque).
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
            && self.corrections.is_empty()
            && self.look.is_none()
            && self.opaque.is_empty()
    }

    /// Typed view of `ProcessVersion`; `None` when absent or malformed.
    pub fn process_version(&self) -> Option<ProcessVersion> {
        ProcessVersion::parse(self.get_by_name("ProcessVersion")?.as_str()?)
    }

    /// Typed view of the white balance ([`WbSetting::from_settings`]).
    pub fn wb(&self) -> Option<WbSetting> {
        WbSetting::from_settings(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(level: Level, name: &str) -> KeyId {
        registry::lookup(level, name).unwrap()
    }

    #[test]
    fn insert_takes_global_keys() {
        let mut s = DevelopSettings::new();
        let exposure = id(Level::Global, "Exposure2012");
        assert_eq!(s.insert(exposure, Value::Real(Finite::ZERO)), Ok(None));
        assert_eq!(
            s.get_by_name("Exposure2012"),
            Some(&Value::Real(Finite::ZERO))
        );
        assert_eq!(s.value_count(), 1);
    }

    #[test]
    fn insert_refuses_non_global_keys() {
        let mut s = DevelopSettings::new();
        for (level, name) in [
            (Level::Correction, "LocalExposure2012"),
            (Level::MaskTool, "MaskName"),
            (Level::Struct(StructKind::Look), "Name"),
        ] {
            assert!(matches!(
                s.insert(id(level, name), Value::Int(0)),
                Err(InsertError::NotGlobal { .. })
            ));
        }
        assert!(s.is_empty());
    }

    #[test]
    fn insert_refuses_the_typed_globals() {
        let mut s = DevelopSettings::new();
        for name in TYPED_GLOBALS {
            assert_eq!(
                s.insert(id(Level::Global, name), Value::Int(0)),
                Err(InsertError::TypedField { key: name })
            );
        }
    }

    #[test]
    fn process_version_view() {
        let mut s = DevelopSettings::new();
        assert_eq!(s.process_version(), None);
        s.insert(
            id(Level::Global, "ProcessVersion"),
            Value::Str("15.4".into()),
        )
        .unwrap();
        assert_eq!(s.process_version(), Some(ProcessVersion::V6));
        assert!(s.process_version().unwrap() >= MIN_SUPPORTED_PV);
    }

    #[test]
    fn hint_from_is_raw() {
        assert_eq!(FileKindHint::from(Some(true)).kind(), Some(FileKind::Raw));
        assert_eq!(
            FileKindHint::from(Some(false)).kind(),
            Some(FileKind::NonRaw)
        );
        assert_eq!(FileKindHint::from(None).kind(), None);
    }
}
