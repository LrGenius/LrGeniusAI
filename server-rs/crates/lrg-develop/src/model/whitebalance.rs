//! Typed view of white balance.
//!
//! Raw and non-raw files use different key families: `Temperature`/`Tint`
//! (Kelvin and tint, raw only) against `IncrementalTemperature`/
//! `IncrementalTint` (offsets, non-raw only). Which family a file uses is
//! read from the keys it carries ([`WbFamily::of_key`] reads the registry's
//! [`FileScope`]), never from its extension: a DNG converted from a JPEG uses
//! the non-raw family.
//!
//! The learning policy (majority vote over the candidate pool, when Kelvin
//! transfers) lives in `lrg-analysis::style_engine`; this module only states
//! facts.

use std::fmt;

use super::value::Finite;
use super::{DevelopSettings, FileKind};
use crate::registry::{FileScope, KeySpec};

/// The named white-balance presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NamedWb {
    /// `Daylight`.
    Daylight,
    /// `Cloudy`.
    Cloudy,
    /// `Shade`.
    Shade,
    /// `Tungsten`.
    Tungsten,
    /// `Fluorescent`.
    Fluorescent,
    /// `Flash`.
    Flash,
}

/// `WhiteBalance`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WbMode {
    /// `As Shot`: the camera's value.
    AsShot,
    /// `Auto`: resolved per photo by Lightroom.
    Auto,
    /// A named preset (raw files; Lightroom offers fewer for non-raw).
    Named(NamedWb),
    /// `Custom`: explicit temperature and tint.
    Custom,
}

impl WbMode {
    /// Parses the `WhiteBalance` value.
    pub fn parse(s: &str) -> Option<WbMode> {
        Some(match s {
            "As Shot" => WbMode::AsShot,
            "Auto" => WbMode::Auto,
            "Custom" => WbMode::Custom,
            "Daylight" => WbMode::Named(NamedWb::Daylight),
            "Cloudy" => WbMode::Named(NamedWb::Cloudy),
            "Shade" => WbMode::Named(NamedWb::Shade),
            "Tungsten" => WbMode::Named(NamedWb::Tungsten),
            "Fluorescent" => WbMode::Named(NamedWb::Fluorescent),
            "Flash" => WbMode::Named(NamedWb::Flash),
            _ => return None,
        })
    }

    /// The `WhiteBalance` value.
    pub fn as_str(self) -> &'static str {
        match self {
            WbMode::AsShot => "As Shot",
            WbMode::Auto => "Auto",
            WbMode::Custom => "Custom",
            WbMode::Named(NamedWb::Daylight) => "Daylight",
            WbMode::Named(NamedWb::Cloudy) => "Cloudy",
            WbMode::Named(NamedWb::Shade) => "Shade",
            WbMode::Named(NamedWb::Tungsten) => "Tungsten",
            WbMode::Named(NamedWb::Fluorescent) => "Fluorescent",
            WbMode::Named(NamedWb::Flash) => "Flash",
        }
    }
}

impl fmt::Display for WbMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which white-balance key family a file uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WbFamily {
    /// `Temperature` (Kelvin) / `Tint`.
    Raw,
    /// `IncrementalTemperature` / `IncrementalTint` (offsets, -100..100).
    NonRaw,
}

impl WbFamily {
    /// The family a key belongs to, from the registry's file scope: raw-only
    /// keys are the raw family, non-raw-only keys the non-raw family.
    pub fn of_key(spec: &KeySpec) -> Option<WbFamily> {
        match spec.file_kind {
            FileScope::RawOnly => Some(WbFamily::Raw),
            FileScope::NonRawOnly => Some(WbFamily::NonRaw),
            FileScope::Both => None,
        }
    }

    /// The families present in `settings`, as (raw, non-raw).
    pub fn present_in(settings: &DevelopSettings) -> (bool, bool) {
        let keys = WbFamily::keys_in(settings);
        (
            keys.iter().any(|(_, f)| *f == WbFamily::Raw),
            keys.iter().any(|(_, f)| *f == WbFamily::NonRaw),
        )
    }

    /// The white-balance family keys `settings` carries, with their family,
    /// in registry order.
    pub fn keys_in(settings: &DevelopSettings) -> Vec<(&'static str, WbFamily)> {
        settings
            .values()
            .filter_map(|(id, _)| WbFamily::of_key(id.spec()).map(|f| (id.spec().name, f)))
            .collect()
    }

    /// The temperature key of this family.
    pub fn temperature_key(self) -> &'static str {
        match self {
            WbFamily::Raw => "Temperature",
            WbFamily::NonRaw => "IncrementalTemperature",
        }
    }

    /// The tint key of this family.
    pub fn tint_key(self) -> &'static str {
        match self {
            WbFamily::Raw => "Tint",
            WbFamily::NonRaw => "IncrementalTint",
        }
    }
}

impl From<FileKind> for WbFamily {
    fn from(k: FileKind) -> WbFamily {
        match k {
            FileKind::Raw => WbFamily::Raw,
            FileKind::NonRaw => WbFamily::NonRaw,
        }
    }
}

impl From<WbFamily> for FileKind {
    fn from(f: WbFamily) -> FileKind {
        match f {
            WbFamily::Raw => FileKind::Raw,
            WbFamily::NonRaw => FileKind::NonRaw,
        }
    }
}

/// A photo's white balance: mode, family and the family's two numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WbSetting {
    /// `WhiteBalance`.
    pub mode: WbMode,
    /// Key family.
    pub family: WbFamily,
    /// `Temperature` (Kelvin) or `IncrementalTemperature` (offset).
    pub temperature: Option<Finite>,
    /// `Tint` or `IncrementalTint`.
    pub tint: Option<Finite>,
}

impl WbSetting {
    /// Reads the white balance of `settings`: `None` without a (known)
    /// `WhiteBalance` mode, or when neither the keys nor
    /// [`DevelopSettings::file_kind`] tell the family.
    pub fn from_settings(settings: &DevelopSettings) -> Option<WbSetting> {
        let mode = WbMode::parse(settings.get_by_name("WhiteBalance")?.as_str()?)?;
        let family = match WbFamily::present_in(settings) {
            (true, false) => WbFamily::Raw,
            (false, true) => WbFamily::NonRaw,
            _ => settings.file_kind?.into(),
        };
        let number = |name| settings.get_by_name(name).and_then(|v| v.as_finite());
        Some(WbSetting {
            mode,
            family,
            temperature: number(family.temperature_key()),
            tint: number(family.tint_key()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::value::Value;
    use crate::registry::{lookup, Level};

    fn set(s: &mut DevelopSettings, name: &str, v: Value) {
        s.insert(lookup(Level::Global, name).unwrap(), v).unwrap();
    }

    #[test]
    fn modes_round_trip_through_their_names() {
        for name in [
            "As Shot",
            "Auto",
            "Custom",
            "Daylight",
            "Cloudy",
            "Shade",
            "Tungsten",
            "Fluorescent",
            "Flash",
        ] {
            assert_eq!(WbMode::parse(name).unwrap().as_str(), name);
        }
        assert_eq!(WbMode::parse("Candlelight"), None);
        assert_eq!(WbMode::parse("auto"), None, "names are case-sensitive");
    }

    #[test]
    fn family_follows_the_registry_file_scope() {
        let fam = |n| WbFamily::of_key(lookup(Level::Global, n).unwrap().spec());
        assert_eq!(fam("Temperature"), Some(WbFamily::Raw));
        assert_eq!(fam("Tint"), Some(WbFamily::Raw));
        assert_eq!(fam("IncrementalTemperature"), Some(WbFamily::NonRaw));
        assert_eq!(fam("IncrementalTint"), Some(WbFamily::NonRaw));
        assert_eq!(fam("WhiteBalance"), None);
        assert_eq!(fam("Exposure2012"), None);
    }

    #[test]
    fn only_the_four_white_balance_keys_carry_a_family() {
        let with_family: Vec<&str> = crate::registry::iter()
            .filter(|(_, s)| WbFamily::of_key(s).is_some())
            .map(|(_, s)| s.name)
            .collect();
        assert_eq!(
            with_family,
            [
                "Temperature",
                "Tint",
                "IncrementalTemperature",
                "IncrementalTint"
            ]
        );
    }

    #[test]
    fn reads_the_family_from_the_keys() {
        let mut s = DevelopSettings::default();
        set(&mut s, "WhiteBalance", Value::Str("Custom".into()));
        set(&mut s, "IncrementalTemperature", Value::Int(12));
        set(&mut s, "IncrementalTint", Value::Int(-3));
        s.file_kind = Some(FileKind::Raw); // the keys win
        let wb = s.wb().unwrap();
        assert_eq!(wb.mode, WbMode::Custom);
        assert_eq!(wb.family, WbFamily::NonRaw);
        assert_eq!(wb.temperature, Some(Finite::new_const(12.0)));
        assert_eq!(wb.tint, Some(Finite::new_const(-3.0)));
    }

    #[test]
    fn falls_back_to_the_file_kind_without_numbers() {
        let mut s = DevelopSettings::default();
        set(&mut s, "WhiteBalance", Value::Str("Auto".into()));
        assert_eq!(s.wb(), None);
        s.file_kind = Some(FileKind::Raw);
        let wb = s.wb().unwrap();
        assert_eq!((wb.mode, wb.family), (WbMode::Auto, WbFamily::Raw));
        assert_eq!((wb.temperature, wb.tint), (None, None));
    }
}
