//! The registry of Lightroom develop keys (`crs:` in XMP, the plain key names
//! in `getDevelopSettings()` tables).
//!
//! Every key the backend knows is one [`KeySpec`] row in the static table
//! ([`table()`]): its level, value kind, range in the **stored**
//! unit, UI scaling, number format, defaults, policy class, frame scope, file
//! kind, where it occurs (Lua, XMP or both), minimum process version and
//! recipe alias. Families that are never modelled one key at a time
//! (`Table_<md5>` blobs, `pm_*` patch state, the FilterList payload, the
//! numbered Upright matrices) are [`patterns`](PatternSpec) instead.
//!
//! The table is the single source of truth. It was seeded once from
//! `server-rs/scripts/develop_registry/spec.py` by `gen_table_rs.py`; edit
//! `table.rs` by hand from now on and regenerate the review snapshot with
//! `LRG_BLESS=1 cargo test -p lrg-develop --test registry_snapshot`.
//!
//! Two rules keep conversions in one place each: UI ↔ stored scaling lives
//! only in [`to_ui`]/[`from_ui`], and integer rounding only in
//! [`KeySpec::coerce`].

mod coerce;
mod patterns;
mod table;
mod ui;

use std::collections::HashMap;
use std::fmt;
use std::sync::LazyLock;

pub use crate::model::value::Finite;
pub use coerce::{round_half_even, CoerceError, Coerced, Scalar};
pub use patterns::{match_pattern, Matcher, PatternSpec, PATTERNS};
pub use ui::{from_ui, from_ui_clamped, from_ui_value, to_ui, UiError};

/// Index of a row in the registry table. Ordering follows the table, which is
/// the natural registry order (panel order, then levels).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct KeyId(u16);

impl KeyId {
    /// Position in [`table()`].
    pub fn index(self) -> usize {
        usize::from(self.0)
    }

    /// The row this id stands for.
    pub fn spec(self) -> &'static KeySpec {
        &table::TABLE[self.index()]
    }
}

/// Where a key lives in the develop description.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Level {
    /// Top level of the develop settings (also the keys of `Look.Parameters`
    /// and `Preset.Parameters`, see [`ValueKind::Settings`]).
    Global,
    /// Preset/profile file header (XMP only).
    Header,
    /// One `MaskGroupBasedCorrections` item.
    Correction,
    /// One mask component: a `CorrectionMasks` item, and every nested `Masks`
    /// item (brush strokes of a `Mask/Aggregate`, heal shapes of a retouch
    /// spot).
    MaskTool,
    /// A field of a nested structure.
    Struct(StructKind),
}

impl Level {
    /// True for every level whose fields are written inside an XMP struct
    /// (lower-case `true`/`false`, never a `+` sign).
    pub fn is_inside_struct(self) -> bool {
        !matches!(self, Level::Global | Level::Header)
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Level::Global => f.write_str("global"),
            Level::Header => f.write_str("header"),
            Level::Correction => f.write_str("correction"),
            Level::MaskTool => f.write_str("mask-tool"),
            Level::Struct(k) => write!(f, "struct:{}", k.name()),
        }
    }
}

/// The nested structures the registry knows fields of.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StructKind {
    /// `Look`: the creative profile reference.
    Look,
    /// `LensBlur`.
    LensBlur,
    /// `AILook`: computed state of an Adaptive profile.
    AiLook,
    /// `CorrectionRangeMask` inside a `Mask/RangeMask` component.
    CorrectionRangeMask,
    /// `CorrectionRangeMask.AreaModels` items (colour range, area form).
    AreaModel,
    /// `Gesture` items of a "Select Object" mask.
    Gesture,
    /// `Gesture.Points` items.
    GesturePoint,
    /// `PointColors` items in the Lua table form.
    PointColor,
    /// `HueRange`/`SatRange`/`LumRange` inside a `PointColors` item.
    PointColorRange,
    /// `RetouchAreas` and `RemoveAreas` items.
    RetouchArea,
    /// `RetouchInfo` items in the Lua table form.
    RetouchInfo,
    /// `RangeMaskMapInfo` (and its inner wrapper of the same name).
    RangeMaskMapInfo,
    /// `DepthMapInfo`.
    DepthMapInfo,
    /// `ISODependent` items of Adobe's ISO-adaptive presets.
    IsoDependent,
    /// `Preset`: the record of the last applied preset in a sidecar.
    Preset,
    /// `FilterList`: Denoise/Raw Details/Super Resolution/generative payloads.
    /// Its fields are covered by a pattern and kept opaque.
    FilterList,
    /// An object the registry has no fields for.
    Unknown,
}

impl StructKind {
    /// Every kind, in declaration order.
    pub const ALL: &'static [StructKind] = &[
        StructKind::Look,
        StructKind::LensBlur,
        StructKind::AiLook,
        StructKind::CorrectionRangeMask,
        StructKind::AreaModel,
        StructKind::Gesture,
        StructKind::GesturePoint,
        StructKind::PointColor,
        StructKind::PointColorRange,
        StructKind::RetouchArea,
        StructKind::RetouchInfo,
        StructKind::RangeMaskMapInfo,
        StructKind::DepthMapInfo,
        StructKind::IsoDependent,
        StructKind::Preset,
        StructKind::FilterList,
        StructKind::Unknown,
    ];

    /// Stable name used in [`Level`]'s `Display` and the registry snapshot.
    pub fn name(self) -> &'static str {
        match self {
            StructKind::Look => "Look",
            StructKind::LensBlur => "LensBlur",
            StructKind::AiLook => "AILook",
            StructKind::CorrectionRangeMask => "CorrectionRangeMask",
            StructKind::AreaModel => "AreaModel",
            StructKind::Gesture => "Gesture",
            StructKind::GesturePoint => "GesturePoint",
            StructKind::PointColor => "PointColor",
            StructKind::PointColorRange => "PointColorRange",
            StructKind::RetouchArea => "RetouchArea",
            StructKind::RetouchInfo => "RetouchInfo",
            StructKind::RangeMaskMapInfo => "RangeMaskMapInfo",
            StructKind::DepthMapInfo => "DepthMapInfo",
            StructKind::IsoDependent => "ISODependent",
            StructKind::Preset => "Preset",
            StructKind::FilterList => "FilterList",
            StructKind::Unknown => "Unknown",
        }
    }
}

/// How a boolean is spelled in XMP (Lua always uses real booleans).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BoolStyle {
    /// `True`/`False`: global develop keys and the preset header.
    TitleCase,
    /// `true`/`false`: every field inside a struct.
    Lower,
}

/// The two tone-curve shapes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CurveKind {
    /// `ToneCurvePV2012*`: XMP items `"x, y"`, Lua a flat number array.
    Global,
    /// `MainCurve`/`RedCurve`/...: XMP items `"x,y"`, Lua a list of `"x,y"`
    /// strings.
    Local,
}

/// The type of a key's value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ValueKind {
    /// Integer slider or count.
    Int,
    /// Real number.
    Real,
    /// Boolean.
    Bool(BoolStyle),
    /// A 0/1 integer that means on/off (`LensProfileEnable`, `AutoLateralCA`,
    /// `CropConstrainToWarp`, `HDREditMode`).
    IntFlag,
    /// A string from a closed set.
    Enum(&'static [&'static str]),
    /// An integer from a closed set.
    EnumInt(&'static [i64]),
    /// Free text.
    Str,
    /// 32 upper-case hex characters (ids and digests).
    Hex32,
    /// Packed engine version `major<<24 | minor<<16 | ...`.
    VersionU32,
    /// Version string such as `"15.4"` or `"18.5.1"`.
    VersionStr,
    /// Tone curve.
    Curve(CurveKind),
    /// Sequence of strings.
    StrSeq,
    /// XMP `rdf:Alt` with an `x-default` item (Lua: `{ ["x-default"] = ... }`).
    LangAlt,
    /// One nested structure.
    Struct(StructKind),
    /// Sequence of nested structures.
    StructSeq(StructKind),
    /// Sequence of corrections ([`Level::Correction`] items).
    CorrectionSeq,
    /// Sequence of mask components ([`Level::MaskTool`] items).
    ComponentSeq,
    /// A nested develop-settings table whose keys are [`Level::Global`] keys
    /// (`Look.Parameters`, `Preset.Parameters`). Kept opaque by the readers.
    Settings,
    /// The Lua table and the XMP file use different shapes for this key.
    PerFormat {
        /// Shape in a `getDevelopSettings()` table.
        lua: &'static ValueKind,
        /// Shape in XMP.
        xmp: &'static ValueKind,
    },
    /// Type not established (key known by name only); kept as it comes.
    Any,
}

impl ValueKind {
    /// The shape in a `getDevelopSettings()` table.
    pub fn lua_form(self) -> ValueKind {
        match self {
            ValueKind::PerFormat { lua, .. } => *lua,
            k => k,
        }
    }

    /// The shape in an XMP file.
    pub fn xmp_form(self) -> ValueKind {
        match self {
            ValueKind::PerFormat { xmp, .. } => *xmp,
            k => k,
        }
    }

    /// True for the kinds whose value is a single number.
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            ValueKind::Int
                | ValueKind::Real
                | ValueKind::IntFlag
                | ValueKind::EnumInt(_)
                | ValueKind::VersionU32
        )
    }

    /// True for the numeric kinds that only hold integers.
    pub fn is_integer(self) -> bool {
        self.is_numeric() && self != ValueKind::Real
    }
}

/// Stored ↔ UI scaling (see [`to_ui`]/[`from_ui`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum UiScale {
    /// Stored value = UI value.
    Identity,
    /// Stored value = UI value / divisor (`LocalExposure2012`: 4, other
    /// signed `Local*` and `CorrectionAmount`: 100).
    Div(f64),
    /// The scale is not verified; such keys cannot be converted, so no
    /// builder can write them.
    Unknown,
}

/// XMP number format (rule 9 of the registry's serialization notes).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum NumFmt {
    /// Integer, no decimal point.
    Int,
    /// Fixed number of decimals (`Exposure2012`: 2, `SharpenRadius`: 1).
    Fixed(u8),
    /// Up to 6 decimals, trailing zeros trimmed.
    Trim6,
    /// A string of `%.6f` numbers (`ReferencePoint`, `LumRange`, ...).
    CompoundFixed6,
    /// Not a number in XMP, or not written to XMP at all (Lua-only keys).
    Text,
}

/// A literal default value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lit {
    /// Integer.
    Int(i64),
    /// Real.
    Real(Finite),
    /// Boolean.
    Bool(bool),
    /// String or enum label.
    Str(&'static str),
    /// Flat integer list (a global tone curve).
    IntList(&'static [i64]),
}

/// A default for one file kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Def {
    /// The value Lightroom uses when nothing was changed.
    Value(Lit),
    /// The key does not exist for this file kind (`Temperature` on a JPEG).
    Absent,
    /// Not established yet (every non-raw default until the E1 JPEG readback).
    Unverified,
    /// There is no fixed default: Lightroom fills the value per photo
    /// (as-shot white balance, lens identity) or only writes the key when a
    /// feature is used (digests, retouch, version stamps).
    NoDefault,
}

/// Defaults per file kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DefaultSpec {
    /// Raw files (and raw DNGs).
    pub raw: Def,
    /// JPEG/TIFF/PSD and DNGs converted from them.
    pub non_raw: Def,
}

/// Why a learnable key needs extra care (the `†` in the registry).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Gate {
    /// Exists for raw files only.
    RawOnly,
    /// Exists for non-raw files only.
    NonRawOnly,
    /// Categorical: weighted majority vote, never a mean.
    Categorical,
    /// An angle: circular mean, weighted by the matching saturation.
    CircularHue,
    /// Only to the same camera (or make) as the example.
    CameraRestricted,
    /// Per mask component: the mask type decides.
    ByMaskType,
    /// Needs an AI update in Lightroom after applying.
    NeedsAiUpdate,
    /// Only meaningful while the named key has its enabling value.
    DependsOn(&'static str),
    /// The default differs between raw and non-raw files: learn per file kind.
    FileKindDefault,
    /// Only these values transfer; any other value is photo-specific.
    OnlyValues(&'static [&'static str]),
}

/// Policy class (see the registry's policy table).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Policy {
    /// Learn, blend, share in presets, apply.
    Learn,
    /// [`Policy::Learn`] behind a [`Gate`].
    LearnGated(Gate),
    /// Apply to one photo with its own context only; never averaged or shared.
    Photo,
    /// Owned by Lightroom (digests, caches, runtime ids); never written.
    Computed,
    /// User-authored per-photo content (retouch, brush strokes); never written.
    Never,
    /// Set by our serializer from context (version stamps, header, names).
    Meta,
    /// Not enough evidence; round-trip only.
    Unknown,
}

impl Policy {
    /// True for [`Policy::Learn`] and [`Policy::LearnGated`].
    pub fn is_learnable(self) -> bool {
        matches!(self, Policy::Learn | Policy::LearnGated(_))
    }

    /// The gate of a [`Policy::LearnGated`] key.
    pub fn gate(self) -> Option<Gate> {
        match self {
            Policy::LearnGated(g) => Some(g),
            _ => None,
        }
    }

    /// Stable name (`LEARN`, `LEARN†`, `PHOTO`, ...), as in the registry.
    pub fn class_name(self) -> &'static str {
        match self {
            Policy::Learn => "LEARN",
            Policy::LearnGated(_) => "LEARN†",
            Policy::Photo => "PHOTO",
            Policy::Computed => "COMPUTED",
            Policy::Never => "NEVER",
            Policy::Meta => "META",
            Policy::Unknown => "UNKNOWN",
        }
    }
}

/// Whether a value can be shared across the frames of a series.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FrameScope {
    /// Frame-independent (tone, HSL, colour grading, detail, profile,
    /// semantic masks).
    Shareable,
    /// Shared, but adjusted per frame (exposure, white balance, the
    /// guardrail keys).
    PerFrameAdjusted,
    /// Belongs to one frame (crop, geometry, lens identity, retouch,
    /// geometric masks).
    PerFrameOnly,
    /// Not a transferable setting (computed, meta, unknown).
    NotApplicable,
}

/// File kinds a key exists for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FileScope {
    /// Raw and non-raw.
    Both,
    /// Raw only (`Temperature`, `Tint`).
    RawOnly,
    /// Non-raw only (`IncrementalTemperature`, `IncrementalTint`).
    NonRawOnly,
}

/// Where a key occurs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Presence {
    /// `getDevelopSettings()` tables and XMP.
    Both,
    /// Lua tables only (panel switches, runtime ids).
    LuaOnly,
    /// XMP only (preset header, `HasCrop`, `HasSettings`, ...).
    XmpOnly,
    /// A name from Camera Raw's key tables that Lightroom Classic was never
    /// observed writing; no value semantics.
    Unobserved,
}

/// A Lightroom process version (`crs:ProcessVersion`, `"15.4"`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ProcessVersion {
    major: u16,
    minor: u16,
}

impl ProcessVersion {
    /// PV2003, `"5.0"`.
    pub const PV2003: ProcessVersion = ProcessVersion::new(5, 0);
    /// PV2010, `"5.7"`.
    pub const PV2010: ProcessVersion = ProcessVersion::new(5, 7);
    /// PV2012, `"6.7"` (the `*2012` keys).
    pub const PV2012: ProcessVersion = ProcessVersion::new(6, 7);
    /// Version 4, `"10.0"`.
    pub const V4: ProcessVersion = ProcessVersion::new(10, 0);
    /// Version 5, `"11.0"`.
    pub const V5: ProcessVersion = ProcessVersion::new(11, 0);
    /// Version 6, `"15.4"`.
    pub const V6: ProcessVersion = ProcessVersion::new(15, 4);

    /// Builds a version from its two parts.
    pub const fn new(major: u16, minor: u16) -> ProcessVersion {
        ProcessVersion { major, minor }
    }

    /// Parses `"15.4"`; `None` for anything else.
    pub fn parse(s: &str) -> Option<ProcessVersion> {
        let (major, minor) = s.trim().split_once('.')?;
        let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
        if !digits(major) || !digits(minor) {
            return None;
        }
        Some(ProcessVersion::new(
            major.parse().ok()?,
            minor.parse().ok()?,
        ))
    }

    /// Major part.
    pub fn major(self) -> u16 {
        self.major
    }

    /// Minor part.
    pub fn minor(self) -> u16 {
        self.minor
    }

    /// Adobe's name for the known versions (`"PV2012"`, `"Version 6"`).
    pub fn label(self) -> Option<&'static str> {
        Some(match self {
            ProcessVersion::PV2003 => "PV2003",
            ProcessVersion::PV2010 => "PV2010",
            ProcessVersion::PV2012 => "PV2012",
            ProcessVersion::V4 => "Version 4",
            ProcessVersion::V5 => "Version 5",
            ProcessVersion::V6 => "Version 6",
            _ => return None,
        })
    }
}

impl fmt::Display for ProcessVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// One registry row.
#[derive(Clone, Copy, Debug)]
pub struct KeySpec {
    /// Key name without the `crs:` prefix (`"LocalExposure2012"`).
    pub name: &'static str,
    /// Where the key lives.
    pub level: Level,
    /// Value type.
    pub kind: ValueKind,
    /// Allowed range in the **stored** unit (`LocalExposure2012`: -1..1),
    /// for numeric keys whose range is established. `None` also for values
    /// that may legitimately leave any range (mask coordinates).
    pub range: Option<(Finite, Finite)>,
    /// Stored ↔ UI scaling.
    pub ui: UiScale,
    /// XMP number format.
    pub fmt: NumFmt,
    /// Positive values get a `+` in XMP (global level only).
    pub plus_sign: bool,
    /// Defaults per file kind.
    pub default: DefaultSpec,
    /// Policy class.
    pub policy: Policy,
    /// Frame scope.
    pub frame: FrameScope,
    /// File kinds the key exists for.
    pub file_kind: FileScope,
    /// Lua, XMP or both.
    pub presence: Presence,
    /// Lowest process version the key applies to.
    pub min_pv: Option<ProcessVersion>,
    /// Path of the matching field in the edit recipe (`"global.exposure"`).
    pub recipe_alias: Option<&'static str>,
    /// Maintainer's note (why a policy, what is unverified).
    pub note: &'static str,
}

/// How a `(level, name)` pair resolves.
#[derive(Clone, Copy, Debug)]
pub enum Resolved {
    /// A registry row.
    Key(KeyId),
    /// A key of a pattern family.
    Pattern(&'static PatternSpec),
    /// Not in the registry.
    Unknown,
}

static INDEX: LazyLock<HashMap<(Level, &'static str), KeyId>> = LazyLock::new(|| {
    let mut map = HashMap::with_capacity(table::TABLE.len());
    for (i, spec) in table::TABLE.iter().enumerate() {
        let id = KeyId(u16::try_from(i).expect("registry table exceeds u16::MAX rows"));
        let previous = map.insert((spec.level, spec.name), id);
        debug_assert!(
            previous.is_none(),
            "duplicate registry row {}/{}",
            spec.level,
            spec.name
        );
    }
    map
});

/// All registry rows, in registry order.
pub fn table() -> &'static [KeySpec] {
    table::TABLE
}

// Every row index fits a `KeyId`, so `iter` may cast.
const _: () = assert!(table::TABLE.len() <= u16::MAX as usize);

/// All registry rows with their ids.
pub fn iter() -> impl Iterator<Item = (KeyId, &'static KeySpec)> {
    table::TABLE
        .iter()
        .enumerate()
        .map(|(i, spec)| (KeyId(i as u16), spec))
}

/// The row for `name` at `level`, if the registry has one.
pub fn lookup(level: Level, name: &str) -> Option<KeyId> {
    INDEX.get(&(level, name)).copied()
}

/// Resolves `name` at `level`: a registry row first, then a pattern family.
pub fn resolve(level: Level, name: &str) -> Resolved {
    if let Some(id) = lookup(level, name) {
        Resolved::Key(id)
    } else if let Some(p) = match_pattern(level, name) {
        Resolved::Pattern(p)
    } else {
        Resolved::Unknown
    }
}

#[cfg(test)]
mod tests;
