//! [`DevelopSettings`] → XMP: a Lightroom Classic develop preset, or (tests
//! only) a lossless rewrite of what [`parse`](super::parse) read.
//!
//! Hand-written and deterministic: the same input gives the same bytes in
//! every build. Keys come in registry order (never a `serde_json` map, whose
//! order depends on the features the workspace unifies), unknown content in
//! the order it was read, namespace prefixes in the order they are first
//! needed.
//!
//! # Modes
//!
//! - [`WriteMode::Preset`]: what production writes. The settings go through
//!   [`DevelopSettings::filtered`] with [`Target::Preset`] first, so only keys
//!   whose policy reaches a shared preset are written, and every value that
//!   did not make it is returned in [`Written::skipped`]. The writer then
//!   sets what describes the file rather than the settings: the preset
//!   header, `Version` ([`TARGET_ENGINE`]), `CompatibleVersion` (from
//!   [`COMPATIBLE_VERSIONS`]), `ProcessVersion` only when
//!   [`PresetSpec::process_version`] asks for it, `HasSettings`,
//!   `WhiteBalance="Custom"` next to white-balance numbers, and
//!   `ToneCurveName2012` next to a point curve. It also completes the
//!   fixed form Adobe's adaptive presets use for what the policy filter
//!   removes as per-photo (see [`write`](fn@write)). A last guard refuses the
//!   never-write list (digests, runtime ids, brush and retouch payloads,
//!   `Enable*` switches, anything Lua-only) whatever the policy says.
//! - `WriteMode::TestRoundTrip` (crate feature `test-roundtrip`, enabled
//!   only through this crate's own dev-dependency): everything the model
//!   holds, opaque, COMPUTED, NEVER and UNKNOWN content included, so
//!   XMP → model → XMP → model compares equal. **Never write a photo's
//!   sidecar with it**: the model drops every namespace but `crs:` (`dc:`,
//!   `xmp:`, `exif:`, `tiff:`, `aux:`, `photoshop:`, `xmpMM:`), so a sidecar
//!   rewritten this way loses the photo's metadata. Writing into an existing
//!   sidecar needs a merge into that document, not a rewrite.
//!
//! # Document shape
//!
//! One `rdf:Description` in the `crs:` namespace, `x:xmptk` set to
//! [`xmptk`] (never an Adobe toolkit string). Simple values are attributes
//! of their description; `rdf:Seq`, `rdf:Alt` and structures are child
//! elements. A structure of simple values only is written as attributes on
//! its property element (or `rdf:li`), one with a complex field as a nested
//! `rdf:Description`; `rdf:parseType="Resource"` and `rdf:Bag` are never
//! written (an `rdf:Bag` the reader kept whole goes back as one in a test
//! round trip). Numbers, booleans, curves and ids are spelled by
//! [`super::format`].

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use super::format::{
    escape_attr, escape_text, format_bool, format_compound, format_curve_point, format_int,
    format_real, hex32_upper, reformat_compound, EngineVersion, InvalidXmlChar,
};
use super::read::{PresetHeader, CRS_NS};
use crate::model::correction::{Correction, MaskComponent, MaskTool, Semantic};
use crate::model::policy::{SkipReason, Skipped, Target};
use crate::model::value::{Fields, Finite, Hex32, Opaque, OpaqueEntry, Struct, Value};
use crate::model::xmp_node::{XmpArrayKind, XmpField, XmpNode, XmpValue};
use crate::model::{DevelopSettings, Look};
use crate::registry::{
    self, KeyId, KeySpec, Level, NumFmt, Presence, ProcessVersion, StructKind, ValueKind,
};

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// The Camera Raw engine the writer targets: Lightroom Classic 15.5.1 ships
/// Camera Raw 18.5.1. Written as `crs:Version`, and the cap of
/// `crs:CompatibleVersion`.
pub const TARGET_ENGINE: EngineVersion = EngineVersion::new(18, 5);

/// The preset group ([`PresetHeader::group`]) of [`PresetHeader::lrgenius`].
pub const PRESET_GROUP: &str = "LrGeniusAI";

/// The `x:xmptk` toolkit string: `LrGeniusAI <backend version>` (the
/// release version baked into the build, `0.0.0-dev` otherwise).
pub fn xmptk() -> String {
    format!(
        "LrGeniusAI {}",
        option_env!("LRG_BACKEND_VERSION").unwrap_or("0.0.0-dev")
    )
}

/// A preset feature that needs a minimum Camera Raw engine to be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    /// Any `MaskGroupBasedCorrections` (the Masking panel).
    MaskGroups,
    /// A people part (`MaskSubType` 3, or a person part at a point).
    PeopleParts,
    /// "Select Background" (`MaskSubType` 0, category 22).
    Background,
    /// A landscape class (`MaskSubType` 0, categories 50001–50008).
    Landscape,
    /// A point curve inside a correction (`MainCurve`, `RedCurve`, ...).
    LocalCurves,
    /// `LensBlur`.
    LensBlur,
    /// The global `CurveRefineSaturation`.
    CurveRefineSaturation,
    /// `LocalPointColors` (point colour inside a mask).
    LocalPointColors,
}

/// The lowest `CompatibleVersion` Adobe's bundled presets (Lightroom Classic
/// 15.x) use for each feature; the writer takes the highest one a preset
/// needs, capped at [`TARGET_ENGINE`], and writes none when no row applies
/// (as most of Adobe's own presets without masks do).
///
/// | Feature | Version | Evidence |
/// |---|---|---|
/// | mask groups, subject, sky | 14.0 | adaptive Subject/Sky presets |
/// | people parts | 15.0 | adaptive Portrait presets |
/// | background | 15.0 | not in the bundle; Select Background shipped with the people masks. Unverified: the hand test only shows that a newer Lightroom imports it, and an import into 15.x cannot check a minimum |
/// | landscape classes | 15.3 | adaptive Landscape presets |
/// | point curves in a correction | 15.3 | adaptive presets with `MainCurve`/`BlueCurve` |
/// | `LensBlur` | 16.0 | Blur Background presets |
/// | global `CurveRefineSaturation` | 17.0 | presets with the refine-saturation curve |
/// | `LocalPointColors` | 17.4 | adaptive presets with point colour in a mask |
///
/// The preset writer computes this from the *filtered* settings, so two
/// rows never apply to a preset it writes: `LocalPointColors` (UNKNOWN) and
/// a person part at a point ([`Semantic::PersonPartAt`], PHOTO; the other
/// half of [`Feature::PeopleParts`]) never pass the policy filter. They stay
/// for [`compatible_version`] on unfiltered settings.
pub const COMPATIBLE_VERSIONS: &[(Feature, EngineVersion)] = &[
    (Feature::MaskGroups, EngineVersion::new(14, 0)),
    (Feature::PeopleParts, EngineVersion::new(15, 0)),
    (Feature::Background, EngineVersion::new(15, 0)),
    (Feature::Landscape, EngineVersion::new(15, 3)),
    (Feature::LocalCurves, EngineVersion::new(15, 3)),
    (Feature::LensBlur, EngineVersion::new(16, 0)),
    (Feature::CurveRefineSaturation, EngineVersion::new(17, 0)),
    (Feature::LocalPointColors, EngineVersion::new(17, 4)),
];

/// The features of [`COMPATIBLE_VERSIONS`] `settings` uses.
pub fn features(settings: &DevelopSettings) -> Vec<Feature> {
    let masks = || settings.corrections.iter().flat_map(|c| c.masks.iter());
    let semantic = |pred: fn(&Semantic) -> bool| {
        masks().any(|m| matches!(&m.tool, MaskTool::Semantic(s) if pred(s)))
    };
    let local = |names: &[&str]| {
        settings
            .corrections
            .iter()
            .any(|c| names.iter().any(|n| c.local_value(n).is_some()))
    };
    let mut out = Vec::new();
    for (feature, _) in COMPATIBLE_VERSIONS {
        let used = match feature {
            Feature::MaskGroups => !settings.corrections.is_empty(),
            Feature::PeopleParts => {
                semantic(|s| matches!(s, Semantic::PeoplePart(_) | Semantic::PersonPartAt { .. }))
            }
            Feature::Background => semantic(|s| matches!(s, Semantic::Background)),
            Feature::Landscape => semantic(|s| matches!(s, Semantic::Landscape(_))),
            Feature::LocalCurves => local(&["MainCurve", "RedCurve", "GreenCurve", "BlueCurve"]),
            Feature::LensBlur => settings.get_by_name("LensBlur").is_some(),
            Feature::CurveRefineSaturation => {
                settings.get_by_name("CurveRefineSaturation").is_some()
            }
            Feature::LocalPointColors => local(&["LocalPointColors"]),
        };
        if used {
            out.push(*feature);
        }
    }
    out
}

/// The `CompatibleVersion` a preset of `settings` needs: the highest
/// [`COMPATIBLE_VERSIONS`] row among its [`features`], capped at
/// [`TARGET_ENGINE`]; `None` when no row applies.
pub fn compatible_version(settings: &DevelopSettings) -> Option<EngineVersion> {
    let used = features(settings);
    COMPATIBLE_VERSIONS
        .iter()
        .filter(|(f, _)| used.contains(f))
        .map(|(_, v)| *v)
        .max()
        .map(|v| v.min(TARGET_ENGINE))
}

impl PresetHeader {
    /// A develop preset header in the [`PRESET_GROUP`] group, with Adobe's
    /// usual flags: amount slider (both generations), colour and
    /// monochrome photos.
    pub fn lrgenius(uuid: Hex32, name: impl Into<String>) -> PresetHeader {
        PresetHeader {
            preset_type: Some("Normal".into()),
            uuid: Some(uuid),
            name: Some(name.into()),
            group: Some(PRESET_GROUP.into()),
            supports_amount: Some(true),
            supports_amount2: Some(true),
            supports_color: Some(true),
            supports_monochrome: Some(true),
            ..PresetHeader::default()
        }
    }
}

/// What a develop preset is written with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetSpec {
    /// The header. `uuid` and `name` are required; every other field has a
    /// default (see [`write`](fn@write)). `rest` may carry further header flags
    /// (`ShowInPresets`, ...); anything in it that is not a META header key
    /// is skipped and reported.
    pub header: PresetHeader,
    /// The preset may reach raw and non-raw photos alike: no raw-only or
    /// non-raw-only key (`Temperature`, `IncrementalTint`, ...), see
    /// [`Target::Preset`].
    pub mixed_file_kinds: bool,
    /// `ProcessVersion` to stamp into the preset; `None` writes none, so the
    /// preset leaves each photo's process version as it is. The settings' own
    /// `ProcessVersion` never reaches a preset (it describes the example).
    pub process_version: Option<ProcessVersion>,
}

impl PresetSpec {
    /// A preset for raw and non-raw photos, without a process version.
    pub fn new(header: PresetHeader) -> PresetSpec {
        PresetSpec {
            header,
            mixed_file_kinds: true,
            process_version: None,
        }
    }
}

/// How [`write`](fn@write) writes.
///
/// `#[non_exhaustive]`: `TestRoundTrip` exists only with the crate feature
/// `test-roundtrip`, which any build of this crate's tests (`cargo test
/// --workspace`, `cargo clippy --all-targets`) unifies into the one
/// `lrg-develop` every crate links. A match outside this crate therefore
/// always needs a wildcard arm, and no other crate may name
/// `TestRoundTrip`: CI would not notice, only the release build would fail.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteMode {
    /// A shared develop preset: policy-filtered, header and stamps set by
    /// the writer.
    Preset(PresetSpec),
    /// Tests only: everything the model and this header hold, verbatim. See
    /// the module docs for why this must never write a sidecar.
    #[cfg(feature = "test-roundtrip")]
    TestRoundTrip(Option<PresetHeader>),
}

/// A written XMP document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    /// The document (UTF-8, ends with a line feed).
    pub xmp: String,
    /// Every value that was not written although it differs from its
    /// default: the policy filter's reports, then the writer's own (content
    /// read from a Lua table or kept whole, header keys that are not META,
    /// the never-write guard). What the preset form puts back without a loss
    /// is not reported: an AI mask's `ReferencePoint` and `ErrorReason`
    /// (Lightroom recomputes them per photo; a people part's point only at
    /// the centre), a range mask's `SampleType` 0, `WhiteBalance="Custom"`
    /// next to its numbers. Empty in a test round trip.
    pub skipped: Vec<Skipped>,
}

/// The settings cannot be written.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    /// A preset header without a required field.
    #[error("a develop preset needs a {0}")]
    MissingHeaderField(&'static str),
    /// Only `PresetType="Normal"` (develop presets) can be written.
    #[error("cannot write a preset of type {0:?}; only \"Normal\" develop presets")]
    UnsupportedPresetType(String),
    /// A value whose variant does not fit its registry key.
    #[error("{path}: expected {expected}, found {found}")]
    WrongValue {
        /// Where (`"MaskGroupBasedCorrections[0].LocalExposure2012"`).
        path: String,
        /// The registry's kind.
        expected: String,
        /// The model value's variant.
        found: &'static str,
    },
    /// An id that is not 32 hex digits, in a preset.
    #[error("{path}: {value:?} is not 32 hex digits")]
    NotHex32 {
        /// Where.
        path: String,
        /// The value.
        value: String,
    },
    /// A compound number string (`ReferencePoint`, `LumRange`) that is not
    /// a list of numbers, in a preset.
    #[error("{path}: {value:?} is not a list of numbers")]
    NotCompound {
        /// Where.
        path: String,
        /// The value.
        value: String,
    },
    /// Content kept from a Lua table (`Opaque::Json`) has no XMP form; a test
    /// round trip cannot write it (a preset skips and reports it).
    #[error("{path}: content read from a Lua table has no XMP form")]
    NotXmp {
        /// Where.
        path: String,
    },
    /// Text XML 1.0 cannot hold.
    #[error("{path}: {source}")]
    InvalidChar {
        /// Where.
        path: String,
        /// The character.
        source: InvalidXmlChar,
    },
}

/// Writes `settings` as an XMP document.
///
/// In [`WriteMode::Preset`] the header defaults to
/// `PresetType="Normal"`, an empty `Cluster`, `CameraModelRestriction`,
/// `Copyright` and `ContactInfo`, the four `Supports*` flags of
/// [`PresetHeader::lrgenius`] (all `True`), and the values Adobe's newer
/// (Camera Raw >= 14.4) presets carry: `SupportsHighDynamicRange`,
/// `SupportsNormalDynamicRange`, `SupportsSceneReferred`,
/// `SupportsOutputReferred` `True`, `RequiresRGBTables` `False`;
/// `ShortName`, `SortName`, `Group` and `Description` default to an empty
/// `x-default` item. `Name` must not be blank.
///
/// A preset also gets the fixed form of Adobe's adaptive presets for what the
/// policy filter removes or a builder may leave out: a correction's `What`,
/// `CorrectionAmount` 1 and `CorrectionActive`; a component's `MaskActive`;
/// an AI mask's `MaskVersion` 1, `ReferencePoint` `0.500000 0.500000` and
/// `ErrorReason` 0; a luminance range's `CorrectionRangeMask` `Version` 3
/// and `SampleType` 0. Values already present are kept.
pub fn write(settings: &DevelopSettings, mode: &WriteMode) -> Result<Written, WriteError> {
    match mode {
        WriteMode::Preset(spec) => write_preset(settings, spec),
        #[cfg(feature = "test-roundtrip")]
        WriteMode::TestRoundTrip(header) => write_round_trip(settings, header.as_ref()),
    }
}

fn id(level: Level, name: &str) -> KeyId {
    registry::lookup(level, name).unwrap_or_else(|| panic!("registry row {level}/{name}"))
}

fn child(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

fn write_preset(settings: &DevelopSettings, spec: &PresetSpec) -> Result<Written, WriteError> {
    let h = &spec.header;
    if let Some(t) = h.preset_type.as_deref().filter(|t| *t != "Normal") {
        return Err(WriteError::UnsupportedPresetType(t.to_owned()));
    }
    let uuid = h
        .uuid
        .as_ref()
        .ok_or(WriteError::MissingHeaderField("UUID"))?;
    let name = h
        .name
        .as_ref()
        .filter(|n| !n.trim().is_empty())
        .ok_or(WriteError::MissingHeaderField("Name"))?;

    let (filtered, skipped) = settings.filtered(&Target::Preset {
        mixed_file_kinds: spec.mixed_file_kinds,
    });
    let mut conv = Conv::new(true, skipped);

    // Header: defaults, then the META keys of `rest`, then the typed fields.
    const HD: Level = Level::Header;
    let mut header = Fields::default();
    let set = |f: &mut Fields, name: &str, v: Value| {
        f.values.insert(id(HD, name), v);
    };
    let text = |s: &str| Value::Str(s.to_owned());
    for (k, v) in [
        ("Cluster", text("")),
        ("SupportsHighDynamicRange", Value::Bool(true)),
        ("SupportsNormalDynamicRange", Value::Bool(true)),
        ("SupportsSceneReferred", Value::Bool(true)),
        ("SupportsOutputReferred", Value::Bool(true)),
        // `False` is right because a preset never carries an RGB table:
        // `RGBTables` is COMPUTED, the `Table_*` blobs are opaque, and the
        // Look goes by reference without its `Parameters`.
        ("RequiresRGBTables", Value::Bool(false)),
        ("CameraModelRestriction", text("")),
        ("Copyright", text("")),
        ("ContactInfo", text("")),
    ] {
        set(&mut header, k, v);
    }
    for (kid, v) in &h.rest.values {
        let s = kid.spec();
        if s.policy == registry::Policy::Meta {
            header.values.insert(*kid, v.clone());
        } else {
            conv.skip(s.name.to_owned(), SkipReason::Policy(s.policy));
        }
    }
    for o in &h.rest.opaque {
        conv.skip(o.name.clone(), SkipReason::Opaque);
    }
    set(&mut header, "PresetType", text("Normal"));
    set(&mut header, "UUID", Value::Str(uuid.as_str().to_owned()));
    set(&mut header, "Name", Value::Alt(name.clone()));
    let flags = [
        ("SupportsAmount", h.supports_amount),
        ("SupportsAmount2", h.supports_amount2),
        ("SupportsColor", h.supports_color),
        ("SupportsMonochrome", h.supports_monochrome),
    ];
    for (k, flag) in flags {
        set(&mut header, k, Value::Bool(flag.unwrap_or(true)));
    }
    if let Some(c) = &h.cluster {
        set(&mut header, "Cluster", text(c));
    }
    if let Some(r) = &h.camera_model_restriction {
        set(&mut header, "CameraModelRestriction", text(r));
    }
    for (k, v) in [
        ("ShortName", &h.short_name),
        ("SortName", &h.sort_name),
        ("Group", &h.group),
        ("Description", &h.description),
    ] {
        set(&mut header, k, Value::Alt(v.clone().unwrap_or_default()));
    }

    // Global values, then what the writer sets from context.
    let mut global = global_fields(&filtered);
    let g = |name: &str| id(Level::Global, name);
    let has = |f: &Fields, name: &str| f.values.contains_key(&g(name));
    let wb_numbers = [
        "Temperature",
        "Tint",
        "IncrementalTemperature",
        "IncrementalTint",
    ];
    if wb_numbers.iter().any(|k| has(&global, k)) && !has(&global, "WhiteBalance") {
        global.values.insert(g("WhiteBalance"), text("Custom"));
    }
    if let Some(curve @ Value::Curve(_)) = global.values.get(&g("ToneCurvePV2012")) {
        let linear = curve.equals_lit(&registry::Lit::IntList(&[0, 0, 255, 255]));
        global.values.insert(
            g("ToneCurveName2012"),
            text(if linear { "Linear" } else { "Custom" }),
        );
    }
    global
        .values
        .insert(g("Version"), Value::Str(TARGET_ENGINE.to_string()));
    if let Some(v) = compatible_version(&filtered) {
        global
            .values
            .insert(g("CompatibleVersion"), Value::Int(v.packed()));
    }
    if let Some(pv) = spec.process_version {
        global
            .values
            .insert(g("ProcessVersion"), Value::Str(pv.to_string()));
    }
    global.values.insert(g("HasSettings"), Value::Bool(true));

    // Not reported on purpose: the preset form puts these back as the source
    // had them or as Lightroom recomputes them per photo (see
    // `written_back`); nothing was lost that the user could act on. A value
    // the form really replaces stays reported.
    let wb_custom = global.values.get(&g("WhiteBalance")) == Some(&text("Custom"));
    let back = written_back(settings, wb_custom);
    conv.skipped.retain(|k| !back.contains(&k.path));

    let fields = conv.top_level(&header, &global)?;
    let xmp = Xml::document(&fields)?;
    Ok(Written {
        xmp,
        skipped: conv.skipped,
    })
}

/// The paths of values the policy filter removes and the preset form puts
/// back, where nothing is lost:
///
/// - an AI mask's `ReferencePoint` and `ErrorReason` (Subject, Sky,
///   Background, a landscape class): Lightroom recomputes both per photo
///   when it updates the mask, and Adobe's own presets carry the centre and
///   0 whatever photo they were made on;
/// - a people part's (`MaskSubType` 3) `ReferencePoint` only at the centre:
///   another point may pick the person (Lightroom's sidecars have many), so
///   replacing it is a change and stays reported until the hand test shows
///   what the preset form selects; its `ErrorReason` as above;
/// - a luminance range's `SampleType` 0 (another value is replaced, and
///   reported);
/// - `WhiteBalance="Custom"` when the writer stamps `Custom` next to the
///   numbers.
fn written_back(settings: &DevelopSettings, wb_custom: bool) -> HashSet<String> {
    let mut out = HashSet::new();
    let custom = settings.get_by_name("WhiteBalance").and_then(Value::as_str) == Some("Custom");
    if wb_custom && custom {
        out.insert("WhiteBalance".to_owned());
    }
    let centred = |v: Option<&Value>| {
        let nums: Option<Vec<f64>> = v
            .and_then(Value::as_str)
            .map(|s| s.split_whitespace().map(|x| x.parse().ok()).collect())
            .unwrap_or(None);
        nums.as_deref() == Some(&[0.5, 0.5])
    };
    for (i, c) in settings.corrections.iter().enumerate() {
        for (j, m) in c.masks.iter().enumerate() {
            let base = format!("MaskGroupBasedCorrections[{i}].CorrectionMasks[{j}]");
            match &m.tool {
                MaskTool::Semantic(Semantic::PersonPartAt { .. }) => {}
                MaskTool::Semantic(s) => {
                    let people = matches!(s, Semantic::PeoplePart(_));
                    let point = m.extra.get(Level::MaskTool, "ReferencePoint");
                    if !people || centred(point) {
                        out.insert(format!("{base}.ReferencePoint"));
                    }
                    out.insert(format!("{base}.ErrorReason"));
                }
                MaskTool::LuminanceRange(lr)
                    if lr.rest.get("SampleType").and_then(Value::as_int) == Some(0) =>
                {
                    out.insert(format!("{base}.CorrectionRangeMask.SampleType"));
                }
                _ => {}
            }
        }
    }
    out
}

#[cfg(feature = "test-roundtrip")]
fn write_round_trip(
    settings: &DevelopSettings,
    header: Option<&PresetHeader>,
) -> Result<Written, WriteError> {
    let mut conv = Conv::new(false, Vec::new());
    let header = header.map(header_fields).unwrap_or_default();
    let global = global_fields(settings);
    let fields = conv.top_level(&header, &global)?;
    let xmp = Xml::document(&fields)?;
    Ok(Written {
        xmp,
        skipped: conv.skipped,
    })
}

/// The header's typed fields back in their registry rows, next to `rest`.
#[cfg(feature = "test-roundtrip")]
fn header_fields(h: &PresetHeader) -> Fields {
    const HD: Level = Level::Header;
    let mut f = h.rest.clone();
    let mut put = |name: &str, v: Option<Value>| {
        if let Some(v) = v {
            f.values.insert(id(HD, name), v);
        }
    };
    let s = |v: &Option<String>| v.clone().map(Value::Str);
    let alt = |v: &Option<String>| v.clone().map(Value::Alt);
    let b = |v: Option<bool>| v.map(Value::Bool);
    put("PresetType", s(&h.preset_type));
    put(
        "UUID",
        h.uuid.as_ref().map(|u| Value::Str(u.as_str().to_owned())),
    );
    put("Name", alt(&h.name));
    put("ShortName", alt(&h.short_name));
    put("SortName", alt(&h.sort_name));
    put("Group", alt(&h.group));
    put("Description", alt(&h.description));
    put("Cluster", s(&h.cluster));
    put("CameraModelRestriction", s(&h.camera_model_restriction));
    put("SupportsAmount", b(h.supports_amount));
    put("SupportsAmount2", b(h.supports_amount2));
    put("SupportsColor", b(h.supports_color));
    put("SupportsMonochrome", b(h.supports_monochrome));
    f
}

/// The global values with the `Look` and the corrections back in their
/// registry rows, and the opaque entries.
fn global_fields(settings: &DevelopSettings) -> Fields {
    let mut f = Fields {
        values: settings.values().map(|(k, v)| (k, v.clone())).collect(),
        opaque: settings.opaque.clone(),
    };
    if let Some(look) = &settings.look {
        f.values
            .insert(id(Level::Global, "Look"), Value::Struct(look_struct(look)));
    }
    if !settings.corrections.is_empty() {
        f.values.insert(
            id(Level::Global, "MaskGroupBasedCorrections"),
            Value::Corrections(settings.corrections.clone()),
        );
    }
    f
}

/// The `Look`'s typed fields back in their registry rows, next to `rest`
/// (shared with the Lua writer).
pub(crate) fn look_struct(look: &Look) -> Struct {
    const L: Level = Level::Struct(StructKind::Look);
    let mut s = look.rest.clone();
    let mut put = |name: &str, v: Option<Value>| {
        if let Some(v) = v {
            s.fields.values.insert(id(L, name), v);
        }
    };
    put("Name", look.name.clone().map(Value::Str));
    put(
        "UUID",
        look.uuid
            .as_ref()
            .map(|u| Value::Str(u.as_str().to_owned())),
    );
    put("Amount", look.amount.map(Value::Real));
    put(
        "CameraModelRestriction",
        look.camera_restriction.clone().map(Value::Str),
    );
    s
}

/// Header keys in the order Adobe's presets write them; any other header key
/// follows in registry order.
const HEADER_ORDER: &[&str] = &[
    "PresetType",
    "Cluster",
    "UUID",
    "SupportsAmount2",
    "SupportsAmount",
    "SupportsColor",
    "SupportsMonochrome",
    "SupportsHighDynamicRange",
    "SupportsNormalDynamicRange",
    "SupportsSceneReferred",
    "SupportsOutputReferred",
    "RequiresRGBTables",
    "ShowInPresets",
    "ShowInQuickActions",
    "CameraModelRestriction",
    "Copyright",
    "ContactInfo",
    "Name",
    "ShortName",
    "SortName",
    "Group",
    "Description",
];

/// Version stamps, written right after the header (as Adobe does).
const STAMPS: &[&str] = &["Version", "CompatibleVersion", "ProcessVersion"];

/// Keys a preset never carries, whatever their policy says (checked after
/// the policy filter, as a last guard): digests (every key with `Digest` in
/// its name), runtime ids and reference points, mask payload bookkeeping,
/// `MaskBrushTable*`, brush dabs, retouch and red-eye content, filter
/// payloads, computed profile state, orientation, the auto-white-balance
/// stamp, sidecar bookkeeping, and — not listed — the `Enable*` panel
/// switches and every key Lightroom only has in its Lua tables or was never
/// seen writing.
pub const NEVER_WRITE: &[&str] = &[
    "CorrectionID",
    "MaskID",
    "CorrectionReferenceX",
    "CorrectionReferenceY",
    "FullMaskSize",
    "WholeImageArea",
    "Origin",
    "ModelVersion",
    "Dabs",
    "RetouchAreas",
    "RemoveAreas",
    "RetouchInfo",
    "RedEyeInfo",
    "FilterList",
    "AILook",
    "DepthMapInfo",
    "orientation",
    "AutoWhiteVersion",
    "RawFileName",
    "AlreadyApplied",
];

/// Whether `spec` is on the never-write list.
pub fn never_written(spec: &KeySpec) -> bool {
    NEVER_WRITE.contains(&spec.name)
        || spec.name.contains("Digest")
        || spec.name.starts_with("MaskBrushTable")
        || (spec.level == Level::Global && spec.name.starts_with("Enable"))
        || matches!(spec.presence, Presence::LuaOnly | Presence::Unobserved)
}

/// Model → XMP data model ([`XmpField`]s with formatted text).
struct Conv {
    /// Preset mode: guard, quantize, upper-case ids, add the fixed form.
    preset: bool,
    crs: Arc<str>,
    skipped: Vec<Skipped>,
}

fn text_node(s: String) -> XmpNode {
    XmpNode::text(s)
}

fn seq(items: Vec<XmpNode>) -> XmpNode {
    XmpNode {
        value: XmpValue::Array {
            kind: XmpArrayKind::Seq,
            items,
        },
        lang: None,
        qualifiers: Vec::new(),
    }
}

fn is_simple(n: &XmpNode) -> bool {
    n.is_plain() && matches!(n.value, XmpValue::Text(_))
}

fn variant(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "an integer",
        Value::Real(_) => "a real",
        Value::Bool(_) => "a boolean",
        Value::Str(_) => "text",
        Value::Curve(_) => "a curve",
        Value::StrList(_) => "a text list",
        Value::Alt(_) => "a language alternative",
        Value::Struct(_) => "a structure",
        Value::StructList(_) => "a structure list",
        Value::Tools(_) => "mask components",
        Value::Corrections(_) => "corrections",
        Value::Opaque(_) => "opaque content",
    }
}

impl Conv {
    fn new(preset: bool, skipped: Vec<Skipped>) -> Conv {
        Conv {
            preset,
            crs: Arc::from(CRS_NS),
            skipped,
        }
    }

    fn lossless(&self) -> bool {
        !self.preset
    }

    fn skip(&mut self, path: String, reason: SkipReason) {
        self.skipped.push(Skipped { path, reason });
    }

    /// Content read from a Lua table: skipped in a preset, an error in a
    /// test round trip.
    fn json(&mut self, path: &str) -> Result<(), WriteError> {
        if self.preset {
            self.skip(path.to_owned(), SkipReason::Opaque);
            Ok(())
        } else {
            Err(WriteError::NotXmp {
                path: path.to_owned(),
            })
        }
    }

    fn crs_field(&self, name: &str, node: XmpNode) -> XmpField {
        XmpField {
            ns: self.crs.clone(),
            name: name.to_owned(),
            node,
        }
    }

    /// The top-level description: header and stamps, global values,
    /// `HasSettings`, then the complex values, then the opaque entries.
    fn top_level(&mut self, header: &Fields, global: &Fields) -> Result<Vec<XmpField>, WriteError> {
        let mut ranked: Vec<((u8, usize), XmpField)> = Vec::new();
        for (kid, node) in self.typed(header, "")? {
            let name = kid.spec().name;
            let pos = HEADER_ORDER
                .iter()
                .position(|n| *n == name)
                .unwrap_or(HEADER_ORDER.len() + kid.index());
            let group = if is_simple(&node) { 0 } else { 4 };
            ranked.push(((group, pos), self.crs_field(name, node)));
        }
        for (kid, node) in self.typed(global, "")? {
            let name = kid.spec().name;
            let rank = if let Some(p) = STAMPS.iter().position(|n| *n == name) {
                (1, p)
            } else if name == "HasSettings" {
                (3, 0)
            } else if is_simple(&node) {
                (2, kid.index())
            } else {
                (5, kid.index())
            };
            ranked.push((rank, self.crs_field(name, node)));
        }
        ranked.sort_by_key(|(rank, _)| *rank);
        let (simple, complex): (Vec<_>, Vec<_>) =
            ranked.into_iter().partition(|((group, _), _)| *group < 4);
        let mut opaque = self.opaque(&header.opaque, "")?;
        opaque.extend(self.opaque(&global.opaque, "")?);
        Ok(arrange(
            simple.into_iter().map(|(_, f)| f).collect(),
            complex.into_iter().map(|(_, f)| f).collect(),
            opaque,
            true,
        ))
    }

    /// The registry values of `f` as nodes, in registry order; in a preset
    /// without the never-write keys.
    fn typed(&mut self, f: &Fields, path: &str) -> Result<Vec<(KeyId, XmpNode)>, WriteError> {
        let mut out = Vec::with_capacity(f.values.len());
        for (kid, v) in &f.values {
            let spec = kid.spec();
            let p = child(path, spec.name);
            if self.preset && never_written(spec) {
                self.skip(p, SkipReason::Policy(spec.policy));
                continue;
            }
            if let Some(node) = self.node(spec, v, &p)? {
                out.push((*kid, node));
            }
        }
        Ok(out)
    }

    /// Opaque entries as fields, in their order (a preset has none left
    /// after the policy filter; any that come are skipped and reported).
    fn opaque(&mut self, entries: &[OpaqueEntry], path: &str) -> Result<Vec<XmpField>, WriteError> {
        let mut out = Vec::new();
        for e in entries {
            let p = child(path, &e.name);
            if self.preset {
                self.skip(p, SkipReason::Opaque);
                continue;
            }
            match &e.value {
                Opaque::Xmp(node) => out.push(XmpField {
                    ns: e.ns.clone().unwrap_or_else(|| self.crs.clone()),
                    name: e.name.clone(),
                    node: node.clone(),
                }),
                Opaque::Json(_) => self.json(&p)?,
            }
        }
        Ok(out)
    }

    /// A structure: its registry values in registry order and its opaque
    /// entries in theirs, arranged by [`arrange`].
    fn structure(&mut self, f: &Fields, path: &str) -> Result<XmpNode, WriteError> {
        let (simple, complex): (Vec<_>, Vec<_>) = self
            .typed(f, path)?
            .into_iter()
            .partition(|(_, n)| is_simple(n));
        let named = |this: &Self, v: Vec<(KeyId, XmpNode)>| -> Vec<XmpField> {
            v.into_iter()
                .map(|(kid, n)| this.crs_field(kid.spec().name, n))
                .collect()
        };
        let simple = named(self, simple);
        let complex = named(self, complex);
        let opaque = self.opaque(&f.opaque, path)?;
        Ok(XmpNode {
            value: XmpValue::Struct(arrange(simple, complex, opaque, false)),
            lang: None,
            qualifiers: Vec::new(),
        })
    }

    /// One registry value as a node; `None` when it is skipped.
    fn node(
        &mut self,
        spec: &'static KeySpec,
        v: &Value,
        path: &str,
    ) -> Result<Option<XmpNode>, WriteError> {
        let lossless = self.lossless();
        let kind = spec.kind.xmp_form();
        let node = match (kind, v) {
            // Kept whole by the reader: its meaning is not established, so a
            // preset never carries it (the policy filter removes it first;
            // this is the guard).
            (_, Value::Opaque(Opaque::Xmp(_))) if self.preset => {
                self.skip(path.to_owned(), SkipReason::Opaque);
                return Ok(None);
            }
            (_, Value::Opaque(Opaque::Xmp(n))) => n.clone(),
            (_, Value::Opaque(Opaque::Json(_))) => {
                self.json(path)?;
                return Ok(None);
            }
            (
                ValueKind::Int | ValueKind::IntFlag | ValueKind::EnumInt(_) | ValueKind::VersionU32,
                Value::Int(i),
            ) => text_node(format_int(spec, *i)),
            (ValueKind::Real, Value::Real(x)) => text_node(format_real(spec, *x, lossless)),
            (ValueKind::Real, Value::Int(i)) => {
                text_node(format_real(spec, Finite::from_i64(*i), lossless))
            }
            (ValueKind::Bool(style), Value::Bool(b)) => text_node(format_bool(style, *b).into()),
            (ValueKind::Hex32, Value::Str(s)) => text_node(self.hex(s, path)?),
            (ValueKind::Enum(_) | ValueKind::Str | ValueKind::VersionStr, Value::Str(s)) => {
                text_node(self.compound(spec, s, path)?)
            }
            (ValueKind::Curve(curve), Value::Curve(points)) => seq(points
                .iter()
                .map(|p| text_node(format_curve_point(curve, spec.fmt, *p, lossless)))
                .collect()),
            (ValueKind::StrSeq, Value::StrList(items)) => {
                let mut nodes = Vec::with_capacity(items.len());
                for (i, s) in items.iter().enumerate() {
                    nodes.push(text_node(self.compound(
                        spec,
                        s,
                        &format!("{path}[{i}]"),
                    )?));
                }
                seq(nodes)
            }
            (ValueKind::LangAlt, Value::Alt(s)) => XmpNode {
                value: XmpValue::Array {
                    kind: XmpArrayKind::Alt,
                    items: vec![XmpNode {
                        lang: Some("x-default".into()),
                        ..XmpNode::text(s.clone())
                    }],
                },
                lang: None,
                qualifiers: Vec::new(),
            },
            (ValueKind::Struct(_), Value::Struct(s)) => self.structure(&s.fields, path)?,
            (ValueKind::StructSeq(_), Value::StructList(items)) => {
                let mut nodes = Vec::with_capacity(items.len());
                for (i, s) in items.iter().enumerate() {
                    nodes.push(self.structure(&s.fields, &format!("{path}[{i}]"))?);
                }
                seq(nodes)
            }
            (ValueKind::CorrectionSeq, Value::Corrections(items)) => {
                let mut nodes = Vec::with_capacity(items.len());
                for (i, c) in items.iter().enumerate() {
                    let f = self.correction_fields(c);
                    nodes.push(self.structure(&f, &format!("{path}[{i}]"))?);
                }
                seq(nodes)
            }
            (ValueKind::ComponentSeq, Value::Tools(items)) => {
                let mut nodes = Vec::with_capacity(items.len());
                for (i, f) in items.iter().enumerate() {
                    nodes.push(self.structure(f, &format!("{path}[{i}]"))?);
                }
                seq(nodes)
            }
            (kind, v) => {
                return Err(WriteError::WrongValue {
                    path: path.to_owned(),
                    expected: format!("{kind:?}"),
                    found: variant(v),
                })
            }
        };
        Ok(Some(node))
    }

    /// An id: upper-cased and checked in a preset, verbatim in a round trip
    /// (the reader keeps ids as read, case included).
    fn hex(&self, s: &str, path: &str) -> Result<String, WriteError> {
        if !self.preset {
            return Ok(s.to_owned());
        }
        hex32_upper(s).ok_or_else(|| WriteError::NotHex32 {
            path: path.to_owned(),
            value: s.to_owned(),
        })
    }

    /// Text; a compound number string re-spelled at `%.6f` in a preset.
    fn compound(&self, spec: &KeySpec, s: &str, path: &str) -> Result<String, WriteError> {
        if !self.preset || spec.fmt != NumFmt::CompoundFixed6 {
            return Ok(s.to_owned());
        }
        reformat_compound(s).ok_or_else(|| WriteError::NotCompound {
            path: path.to_owned(),
            value: s.to_owned(),
        })
    }

    /// A correction's typed parts back in their registry rows (the inverse
    /// of `Correction::from_fields`), plus the preset form.
    fn correction_fields(&self, c: &Correction) -> Fields {
        const C: Level = Level::Correction;
        let mut f = c.extra.clone();
        let v = &mut f.values;
        for (kid, value) in &c.local {
            v.insert(*kid, value.clone());
        }
        if let Some(n) = &c.name {
            v.insert(id(C, "CorrectionName"), Value::Str(n.clone()));
        }
        if let Some(s) = &c.sync_id {
            v.insert(id(C, "CorrectionSyncID"), Value::Str(s.as_str().to_owned()));
        }
        if let Some(a) = c.amount {
            v.insert(id(C, "CorrectionAmount"), Value::Real(a));
        }
        if let Some(a) = c.active {
            v.insert(id(C, "CorrectionActive"), Value::Bool(a));
        }
        if !c.masks.is_empty() {
            let tools = c.masks.iter().map(|m| self.component_fields(m)).collect();
            v.insert(id(C, "CorrectionMasks"), Value::Tools(tools));
        }
        if self.preset {
            let defaults = [
                ("What", Value::Str("Correction".into())),
                ("CorrectionAmount", Value::Real(Finite::new_const(1.0))),
                ("CorrectionActive", Value::Bool(true)),
            ];
            for (k, d) in defaults {
                v.entry(id(C, k)).or_insert(d);
            }
        }
        f
    }

    /// A mask component's typed parts back in their registry rows (the
    /// inverse of `MaskComponent::from_fields`), plus the preset form.
    fn component_fields(&self, m: &MaskComponent) -> Fields {
        const M: Level = Level::MaskTool;
        let mut f = m.extra.clone();
        let v = &mut f.values;
        let mut put = |name: &str, value: Value| {
            v.insert(id(M, name), value);
        };
        if let Some(n) = &m.name {
            put("MaskName", Value::Str(n.clone()));
        }
        if let Some(s) = &m.sync_id {
            put("MaskSyncID", Value::Str(s.as_str().to_owned()));
        }
        if let Some(a) = m.active {
            put("MaskActive", Value::Bool(a));
        }
        if let Some((mode, value, inverted)) = m.combine.encode() {
            put("MaskBlendMode", Value::Int(mode));
            put("MaskValue", Value::Real(value));
            put("MaskInverted", Value::Bool(inverted));
        }
        let lossless = self.lossless();
        let real = Value::Real;
        match &m.tool {
            MaskTool::Semantic(s) => {
                put("What", Value::Str("Mask/Image".into()));
                let (sub, category) = s.encoding();
                put("MaskSubType", Value::Int(sub));
                if let Some(c) = category {
                    put("MaskSubCategoryID", Value::Int(c));
                }
                if let Semantic::PersonPartAt { point, .. } = s {
                    put(
                        "ReferencePoint",
                        Value::Str(format_compound(&[point.x, point.y], lossless)),
                    );
                }
            }
            MaskTool::Linear(g) => {
                put("What", Value::Str("Mask/Gradient".into()));
                put("ZeroX", real(g.zero.x));
                put("ZeroY", real(g.zero.y));
                put("FullX", real(g.full.x));
                put("FullY", real(g.full.y));
            }
            MaskTool::Radial(g) => {
                put("What", Value::Str("Mask/CircularGradient".into()));
                put("Top", real(g.top));
                put("Left", real(g.left));
                put("Bottom", real(g.bottom));
                put("Right", real(g.right));
                put("Angle", real(Finite::ZERO));
                put("Midpoint", Value::Int(g.midpoint));
                put("Roundness", Value::Int(g.roundness));
                put("Feather", Value::Int(g.feather));
                put("Flipped", Value::Bool(g.flipped));
            }
            MaskTool::LuminanceRange(lr) => {
                put("What", Value::Str("Mask/RangeMask".into()));
                const R: Level = Level::Struct(StructKind::CorrectionRangeMask);
                let mut crm = lr.rest.clone();
                let rv = &mut crm.fields.values;
                rv.insert(id(R, "Type"), Value::Int(2));
                rv.insert(id(R, "Invert"), Value::Bool(lr.invert));
                rv.insert(
                    id(R, "LumRange"),
                    Value::Str(format_compound(&lr.range, lossless)),
                );
                if self.preset {
                    rv.entry(id(R, "Version")).or_insert(Value::Int(3));
                    rv.entry(id(R, "SampleType")).or_insert(Value::Int(0));
                }
                put("CorrectionRangeMask", Value::Struct(crm));
            }
            MaskTool::Opaque { what } => {
                if let Some(w) = what {
                    put("What", Value::Str(w.clone()));
                }
            }
        }
        if self.preset {
            let mut defaults = vec![("MaskActive", Value::Bool(true))];
            if matches!(&m.tool, MaskTool::Semantic(s) if !matches!(s, Semantic::PersonPartAt { .. }))
            {
                defaults.extend([
                    ("MaskVersion", Value::Int(1)),
                    ("ReferencePoint", Value::Str("0.500000 0.500000".into())),
                    ("ErrorReason", Value::Int(0)),
                ]);
            }
            for (k, d) in defaults {
                f.values.entry(id(M, k)).or_insert(d);
            }
        }
        f
    }
}

/// The field order of one description: simple registry values, the
/// leading opaque entries that can be attributes too, complex registry
/// values, the remaining opaque entries.
///
/// The reader reads attributes before elements and keeps opaque entries in
/// that order, and the first occurrence of a name wins; so an opaque entry
/// only becomes an attribute when every opaque entry before it does, and
/// never when it repeats a name (a registry key's second occurrence must
/// stay behind the first). `crs_only`: at the top level, where only `crs:`
/// attributes are read.
fn arrange(
    simple: Vec<XmpField>,
    complex: Vec<XmpField>,
    mut opaque: Vec<XmpField>,
    crs_only: bool,
) -> Vec<XmpField> {
    let key = |f: &XmpField| (f.ns.to_string(), f.name.clone());
    let mut seen: std::collections::HashSet<(String, String)> =
        simple.iter().chain(&complex).map(key).collect();
    let lead = opaque
        .iter()
        .take_while(|f| {
            is_simple(&f.node) && (!crs_only || &*f.ns == CRS_NS) && seen.insert(key(f))
        })
        .count();
    let rest = opaque.split_off(lead);
    let mut out = simple;
    out.extend(opaque);
    out.extend(complex);
    out.extend(rest);
    out
}

/// Namespace prefixes: `crs`, `rdf` and the unprefixed "no namespace" are
/// fixed; any other namespace gets `ns1`, `ns2`, ... in the order it is
/// first written, declared on each element that uses it.
#[derive(Default)]
struct Prefixes(BTreeMap<Arc<str>, String>, usize);

impl Prefixes {
    /// The qualified name of `(ns, name)` and the declaration it needs.
    fn qname(&mut self, ns: &Arc<str>, name: &str) -> (String, Option<(String, Arc<str>)>) {
        match &**ns {
            "" => (name.to_owned(), None),
            CRS_NS => (format!("crs:{name}"), None),
            RDF_NS => (format!("rdf:{name}"), None),
            _ => {
                let prefix = match self.0.get(ns) {
                    Some(p) => p.clone(),
                    None => {
                        self.1 += 1;
                        let p = format!("ns{}", self.1);
                        self.0.insert(ns.clone(), p.clone());
                        p
                    }
                };
                (format!("{prefix}:{name}"), Some((prefix, ns.clone())))
            }
        }
    }
}

/// XMP data model → XML text.
struct Xml {
    out: String,
    prefixes: Prefixes,
}

fn pad(out: &mut String, n: usize) {
    out.extend(std::iter::repeat_n(' ', n));
}

fn esc_attr(s: &str, path: &str) -> Result<String, WriteError> {
    escape_attr(s).map_err(|source| WriteError::InvalidChar {
        path: path.to_owned(),
        source,
    })
}

fn esc_text(s: &str, path: &str) -> Result<String, WriteError> {
    escape_text(s).map_err(|source| WriteError::InvalidChar {
        path: path.to_owned(),
        source,
    })
}

/// How many leading fields can be attributes: plain text, and no name twice
/// (an element may not repeat an attribute).
fn attribute_prefix(fields: &[XmpField]) -> usize {
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        let key = (&*f.ns, f.name.as_str());
        if !is_simple(&f.node) || seen.contains(&key) {
            return i;
        }
        seen.push(key);
    }
    fields.len()
}

impl Xml {
    fn document(fields: &[XmpField]) -> Result<String, WriteError> {
        let mut x = Xml {
            out: String::with_capacity(4096),
            prefixes: Prefixes::default(),
        };
        let tk = esc_attr(&xmptk(), "x:xmptk")?;
        x.out.push_str(&format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"{tk}\">\n <rdf:RDF xmlns:rdf=\"{RDF_NS}\">\n  <rdf:Description rdf:about=\"\"\n    xmlns:crs=\"{CRS_NS}\""
        ));
        let n = attribute_prefix(fields);
        for f in &fields[..n] {
            x.attribute(f, 3, "")?;
        }
        if n == fields.len() {
            x.out.push_str("/>\n");
        } else {
            x.out.push_str(">\n");
            for f in &fields[n..] {
                x.element(&f.ns, &f.name, &f.node, 3, &f.name)?;
            }
            x.out.push_str("  </rdf:Description>\n");
        }
        x.out.push_str(" </rdf:RDF>\n</x:xmpmeta>\n");
        Ok(x.out)
    }

    /// One field as an attribute on its own line at `depth`.
    fn attribute(&mut self, f: &XmpField, depth: usize, path: &str) -> Result<(), WriteError> {
        let XmpValue::Text(s) = &f.node.value else {
            unreachable!("only plain text fields become attributes")
        };
        let (qn, _) = self.prefixes.qname(&f.ns, &f.name);
        let p = child(path, &f.name);
        self.out.push('\n');
        pad(&mut self.out, depth);
        self.out.push_str(&format!("{qn}=\"{}\"", esc_attr(s, &p)?));
        Ok(())
    }

    /// Opens `<qn` with the declarations and `xml:lang` on the same line,
    /// then the field attributes one per line at `depth + 1`; does not close
    /// the tag.
    fn open(
        &mut self,
        qn: &str,
        decls: &mut Vec<(String, Arc<str>)>,
        lang: Option<&str>,
        attrs: &[XmpField],
        depth: usize,
        path: &str,
    ) -> Result<(), WriteError> {
        for f in attrs {
            if let (_, Some(d)) = self.prefixes.qname(&f.ns, &f.name) {
                decls.push(d);
            }
        }
        decls.sort();
        decls.dedup();
        pad(&mut self.out, depth);
        self.out.push('<');
        self.out.push_str(qn);
        for (prefix, uri) in decls.iter() {
            let uri = esc_attr(uri, path)?;
            self.out.push_str(&format!(" xmlns:{prefix}=\"{uri}\""));
        }
        if let Some(l) = lang {
            let l = esc_attr(l, path)?;
            self.out.push_str(&format!(" xml:lang=\"{l}\""));
        }
        for f in attrs {
            self.attribute(f, depth + 1, path)?;
        }
        Ok(())
    }

    /// One property (or `rdf:li`) element at `depth`.
    fn element(
        &mut self,
        ns: &Arc<str>,
        name: &str,
        node: &XmpNode,
        depth: usize,
        path: &str,
    ) -> Result<(), WriteError> {
        let (qn, decl) = self.prefixes.qname(ns, name);
        let mut decls: Vec<_> = decl.into_iter().collect();
        let lang = node.lang.as_deref();
        if !node.qualifiers.is_empty() {
            // A qualified value: rdf:value and the qualifiers in a nested
            // description.
            self.open(&qn, &mut decls, lang, &[], depth, path)?;
            self.out.push_str(">\n");
            pad(&mut self.out, depth + 1);
            self.out.push_str("<rdf:Description>\n");
            let bare = XmpNode {
                value: node.value.clone(),
                lang: None,
                qualifiers: Vec::new(),
            };
            let rdf: Arc<str> = Arc::from(RDF_NS);
            self.element(&rdf, "value", &bare, depth + 1, path)?;
            for q in &node.qualifiers {
                let p = child(path, &q.name);
                self.element(&q.ns, &q.name, &q.node, depth + 1, &p)?;
            }
            pad(&mut self.out, depth + 1);
            self.out.push_str("</rdf:Description>\n");
            return self.close(&qn, depth);
        }
        match &node.value {
            XmpValue::Text(s) => {
                self.open(&qn, &mut decls, lang, &[], depth, path)?;
                if s.is_empty() {
                    self.out.push_str("/>\n");
                } else {
                    let t = esc_text(s, path)?;
                    self.out.push_str(&format!(">{t}</{qn}>\n"));
                }
                Ok(())
            }
            XmpValue::Array { kind, items } => {
                self.open(&qn, &mut decls, lang, &[], depth, path)?;
                self.out.push_str(">\n");
                pad(&mut self.out, depth + 1);
                let container = format!("rdf:{}", kind.name());
                if items.is_empty() {
                    self.out.push_str(&format!("<{container}/>\n"));
                } else {
                    self.out.push_str(&format!("<{container}>\n"));
                    let rdf: Arc<str> = Arc::from(RDF_NS);
                    for (i, item) in items.iter().enumerate() {
                        self.element(&rdf, "li", item, depth + 2, &format!("{path}[{i}]"))?;
                    }
                    pad(&mut self.out, depth + 1);
                    self.out.push_str(&format!("</{container}>\n"));
                }
                self.close(&qn, depth)
            }
            XmpValue::Struct(fields) => {
                let n = attribute_prefix(fields);
                if !fields.is_empty() && n == fields.len() {
                    // Simple fields only: attributes on the element itself.
                    self.open(&qn, &mut decls, lang, fields, depth, path)?;
                    self.out.push_str("/>\n");
                    return Ok(());
                }
                self.open(&qn, &mut decls, lang, &[], depth, path)?;
                self.out.push_str(">\n");
                let mut inner = Vec::new();
                self.open(
                    "rdf:Description",
                    &mut inner,
                    None,
                    &fields[..n],
                    depth + 1,
                    path,
                )?;
                if fields.is_empty() {
                    self.out.push_str("/>\n");
                } else {
                    self.out.push_str(">\n");
                    // Adobe's indentation: the description's elements at
                    // the description's own depth.
                    for f in &fields[n..] {
                        let p = child(path, &f.name);
                        self.element(&f.ns, &f.name, &f.node, depth + 1, &p)?;
                    }
                    pad(&mut self.out, depth + 1);
                    self.out.push_str("</rdf:Description>\n");
                }
                self.close(&qn, depth)
            }
        }
    }

    fn close(&mut self, qn: &str, depth: usize) -> Result<(), WriteError> {
        pad(&mut self.out, depth);
        self.out.push_str(&format!("</{qn}>\n"));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::correction::{
        Combine, LinearGradient, LuminanceRange, RadialGradient, SensorPoint,
    };
    use crate::xmp::read::{parse, XmpKind};

    fn f(x: f64) -> Finite {
        Finite::new(x).unwrap()
    }

    fn set(s: &mut DevelopSettings, name: &str, v: Value) {
        s.insert(id(Level::Global, name), v).unwrap();
    }

    fn hex(n: u32) -> Hex32 {
        Hex32::parse(&format!("{n:032x}")).unwrap()
    }

    fn component(tool: MaskTool, combine: Combine) -> MaskComponent {
        MaskComponent {
            tool,
            combine,
            name: Some("LrGenius · Mask".into()),
            sync_id: Some(hex(0xb1)),
            active: None,
            extra: Fields::default(),
        }
    }

    fn correction(name: &str, masks: Vec<MaskComponent>) -> Correction {
        let mut local = BTreeMap::new();
        local.insert(
            id(Level::Correction, "LocalExposure2012"),
            Value::Real(f(0.0625)),
        );
        Correction {
            name: Some(name.into()),
            sync_id: Some(hex(0xa1)),
            amount: None,
            active: None,
            local,
            masks,
            extra: Fields::default(),
        }
    }

    fn settings() -> DevelopSettings {
        let mut s = DevelopSettings::new();
        s.file_kind = Some(crate::model::FileKind::Raw);
        set(&mut s, "Exposure2012", Value::Real(f(0.5)));
        set(&mut s, "Contrast2012", Value::Int(-12));
        set(&mut s, "Temperature", Value::Int(5600));
        set(&mut s, "Tint", Value::Int(6));
        set(&mut s, "SharpenRadius", Value::Real(f(1.0)));
        set(
            &mut s,
            "ToneCurvePV2012",
            Value::Curve(vec![
                (f(0.0), f(0.0)),
                (f(64.0), f(58.0)),
                (f(255.0), f(255.0)),
            ]),
        );
        set(&mut s, "ProcessVersion", Value::Str("11.0".into()));
        s.corrections.push(correction(
            "LrGenius · Subject",
            vec![component(
                MaskTool::Semantic(Semantic::Subject),
                Combine::Add { inverted: false },
            )],
        ));
        s.corrections.push(correction(
            "LrGenius · Sky",
            vec![
                component(
                    MaskTool::Semantic(Semantic::Sky),
                    Combine::Add { inverted: false },
                ),
                component(
                    MaskTool::LuminanceRange(LuminanceRange {
                        range: [f(0.0), f(0.2), f(0.8), f(1.0)],
                        invert: false,
                        rest: Struct::new(StructKind::CorrectionRangeMask),
                    }),
                    Combine::Intersect,
                ),
            ],
        ));
        s
    }

    fn spec(mixed: bool) -> WriteMode {
        let mut header = PresetHeader::lrgenius(hex(0xabc), "LrGenius · Test & <Co>");
        header.description = Some("Say \"hi\" 'there'".into());
        WriteMode::Preset(PresetSpec {
            header,
            mixed_file_kinds: mixed,
            process_version: Some(ProcessVersion::V6),
        })
    }

    fn preset(s: &DevelopSettings, mixed: bool) -> Written {
        write(s, &spec(mixed)).unwrap_or_else(|e| panic!("{e}"))
    }

    fn has(xmp: &str, needle: &str) -> bool {
        xmp.contains(needle)
    }

    #[test]
    fn a_preset_has_adobes_document_shape() {
        let out = preset(&settings(), false).xmp;
        assert!(out.starts_with(&format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"{}\">\n <rdf:RDF",
            xmptk()
        )));
        assert!(xmptk().starts_with("LrGeniusAI "));
        assert!(out.ends_with(" </rdf:RDF>\n</x:xmpmeta>\n"));
        for line in [
            "  <rdf:Description rdf:about=\"\"\n    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n   crs:PresetType=\"Normal\"\n   crs:Cluster=\"\"\n   crs:UUID=\"00000000000000000000000000000ABC\"\n   crs:SupportsAmount2=\"True\"\n   crs:SupportsAmount=\"True\"",
            "   crs:RequiresRGBTables=\"False\"\n   crs:CameraModelRestriction=\"\"\n   crs:Copyright=\"\"\n   crs:ContactInfo=\"\"\n   crs:Version=\"18.5\"\n   crs:CompatibleVersion=\"234881024\"\n   crs:ProcessVersion=\"15.4\"\n   crs:WhiteBalance=\"Custom\"",
            "   crs:HasSettings=\"True\">\n   <crs:Name>\n    <rdf:Alt>\n     <rdf:li xml:lang=\"x-default\">LrGenius · Test &amp; &lt;Co&gt;</rdf:li>\n    </rdf:Alt>\n   </crs:Name>\n   <crs:ShortName>\n    <rdf:Alt>\n     <rdf:li xml:lang=\"x-default\"/>\n",
            "     <rdf:li xml:lang=\"x-default\">LrGeniusAI</rdf:li>",
            "     <rdf:li xml:lang=\"x-default\">Say &quot;hi&quot; &apos;there&apos;</rdf:li>",
            "   crs:Exposure2012=\"+0.50\"",
            "   crs:Contrast2012=\"-12\"",
            "   crs:Temperature=\"5600\"",
            "   crs:Tint=\"+6\"",
            "   crs:SharpenRadius=\"+1.0\"",
            "   crs:ToneCurveName2012=\"Custom\"",
            "   <crs:ToneCurvePV2012>\n    <rdf:Seq>\n     <rdf:li>0, 0</rdf:li>\n     <rdf:li>64, 58</rdf:li>\n     <rdf:li>255, 255</rdf:li>\n    </rdf:Seq>\n   </crs:ToneCurvePV2012>",
            "   <crs:MaskGroupBasedCorrections>\n    <rdf:Seq>\n     <rdf:li>\n      <rdf:Description\n       crs:What=\"Correction\"\n       crs:CorrectionAmount=\"1\"\n       crs:CorrectionActive=\"true\"\n       crs:CorrectionName=\"LrGenius · Subject\"\n       crs:CorrectionSyncID=\"000000000000000000000000000000A1\"\n       crs:LocalExposure2012=\"0.0625\">\n      <crs:CorrectionMasks>\n       <rdf:Seq>\n        <rdf:li\n         crs:What=\"Mask/Image\"\n         crs:MaskActive=\"true\"\n         crs:MaskName=\"LrGenius · Mask\"\n         crs:MaskBlendMode=\"0\"\n         crs:MaskInverted=\"false\"\n         crs:MaskSyncID=\"000000000000000000000000000000B1\"\n         crs:MaskValue=\"1\"\n         crs:MaskVersion=\"1\"\n         crs:MaskSubType=\"1\"\n         crs:ReferencePoint=\"0.500000 0.500000\"\n         crs:ErrorReason=\"0\"/>\n       </rdf:Seq>\n      </crs:CorrectionMasks>\n      </rdf:Description>\n     </rdf:li>",
            "        <rdf:li>\n         <rdf:Description\n          crs:What=\"Mask/RangeMask\"\n          crs:MaskActive=\"true\"\n          crs:MaskName=\"LrGenius · Mask\"\n          crs:MaskBlendMode=\"1\"\n          crs:MaskInverted=\"true\"\n          crs:MaskSyncID=\"000000000000000000000000000000B1\"\n          crs:MaskValue=\"0\">\n         <crs:CorrectionRangeMask\n          crs:Version=\"3\"\n          crs:Type=\"2\"\n          crs:Invert=\"false\"\n          crs:SampleType=\"0\"\n          crs:LumRange=\"0.000000 0.200000 0.800000 1.000000\"/>\n         </rdf:Description>\n        </rdf:li>",
        ] {
            assert!(has(&out, line), "missing:\n{line}\n--- in ---\n{out}");
        }
        // The example's own process version never reaches a preset.
        assert!(!has(&out, "\"11.0\""));
    }

    #[test]
    fn a_preset_reads_back_as_the_filtered_settings() {
        let s = settings();
        let written = preset(&s, false);
        let doc = parse(written.xmp.as_bytes()).unwrap();
        assert_eq!(doc.kind, XmpKind::Preset);
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        let header = doc.header.unwrap();
        assert_eq!(
            header.uuid.as_ref().map(Hex32::as_str),
            Some("00000000000000000000000000000ABC"),
            "upper-cased"
        );
        assert_eq!(header.name.as_deref(), Some("LrGenius · Test & <Co>"));
        assert_eq!(header.group.as_deref(), Some(PRESET_GROUP));
        assert_eq!(header.short_name.as_deref(), Some(""));
        let d = doc.develop;
        assert_eq!(d.get_by_name("Exposure2012"), Some(&Value::Real(f(0.5))));
        assert_eq!(d.get_by_name("Tint"), Some(&Value::Int(6)));
        assert_eq!(d.process_version(), Some(ProcessVersion::V6));
        assert_eq!(d.corrections.len(), 2);
        assert_eq!(
            d.corrections[0].masks[0].tool,
            MaskTool::Semantic(Semantic::Subject)
        );
        assert_eq!(d.corrections[1].masks[1].combine, Combine::Intersect);
        assert_eq!(
            d.corrections[0].local_value("LocalExposure2012"),
            Some(&Value::Real(f(0.0625)))
        );
        assert!(written.skipped.is_empty(), "{:?}", written.skipped);
    }

    #[test]
    fn output_is_byte_deterministic() {
        let s = settings();
        let a = preset(&s, false).xmp;
        assert_eq!(a, preset(&s, false).xmp);
        // Insertion order does not matter: the registry decides.
        let mut t = DevelopSettings::new();
        t.file_kind = s.file_kind;
        for (k, v) in s.values().collect::<Vec<_>>().into_iter().rev() {
            t.insert(k, v.clone()).unwrap();
        }
        t.corrections = s.corrections.clone();
        assert_eq!(a, preset(&t, false).xmp);
    }

    #[test]
    fn a_mixed_preset_drops_raw_only_white_balance_and_reports_it() {
        let written = preset(&settings(), true);
        assert!(!has(&written.xmp, "Temperature"));
        assert!(!has(&written.xmp, "crs:Tint"));
        assert!(!has(&written.xmp, "WhiteBalance"), "no numbers, no mode");
        let paths: Vec<&str> = written.skipped.iter().map(|k| k.path.as_str()).collect();
        assert_eq!(paths, ["Temperature", "Tint"]);
    }

    #[test]
    fn photo_computed_and_opaque_content_never_reaches_a_preset() {
        let mut s = settings();
        set(&mut s, "CropTop", Value::Real(f(0.1)));
        set(&mut s, "WhiteBalance", Value::Str("Daylight".into()));
        set(
            &mut s,
            "FilterList",
            Value::Struct(Struct::new(StructKind::FilterList)),
        );
        set(
            &mut s,
            "RetouchAreas",
            Value::StructList(vec![Struct::new(StructKind::RetouchArea)]),
        );
        set(&mut s, "EnableDetail", Value::Bool(true));
        s.corrections[0].masks[0].extra.values.insert(
            id(Level::MaskTool, "MaskDigest"),
            Value::Str(format!("{:032X}", 7)),
        );
        s.corrections.push(correction(
            "LrGenius · Gradient",
            vec![component(
                MaskTool::Linear(LinearGradient {
                    zero: SensorPoint {
                        x: f(0.5),
                        y: f(0.0),
                    },
                    full: SensorPoint {
                        x: f(0.5),
                        y: f(0.4),
                    },
                }),
                Combine::Add { inverted: false },
            )],
        ));
        s.opaque.push(OpaqueEntry {
            ns: None,
            name: "SomethingNew".into(),
            value: Opaque::Json(serde_json::json!(1)),
        });
        let written = preset(&s, false);
        for gone in [
            "CropTop",
            "Daylight",
            "WhiteBalance",
            "Temperature",
            "FilterList",
            "RetouchAreas",
            "EnableDetail",
            "MaskDigest",
            "Mask/Gradient",
            "SomethingNew",
        ] {
            assert!(!has(&written.xmp, gone), "{gone} written");
        }
        let paths: Vec<&str> = written.skipped.iter().map(|k| k.path.as_str()).collect();
        for want in [
            "CropTop",
            "WhiteBalance",
            "Temperature",
            "Tint",
            "RetouchAreas",
            "MaskGroupBasedCorrections[0].CorrectionMasks[0].MaskDigest",
            "MaskGroupBasedCorrections[2]",
            "SomethingNew",
        ] {
            assert!(paths.contains(&want), "{want} not reported: {paths:?}");
        }
    }

    #[test]
    fn the_never_write_guard_catches_what_the_filter_would_have_removed() {
        let mut conv = Conv::new(true, Vec::new());
        let mut fields = Fields::default();
        fields.values.insert(
            id(Level::MaskTool, "MaskDigest"),
            Value::Str(format!("{:032X}", 1)),
        );
        fields
            .values
            .insert(id(Level::MaskTool, "MaskID"), Value::Str("x".into()));
        fields
            .values
            .insert(id(Level::MaskTool, "MaskSubType"), Value::Int(1));
        let typed = conv.typed(&fields, "M").unwrap();
        let names: Vec<&str> = typed.iter().map(|(k, _)| k.spec().name).collect();
        assert_eq!(names, ["MaskSubType"]);
        let skipped: Vec<&str> = conv.skipped.iter().map(|k| k.path.as_str()).collect();
        assert_eq!(skipped, ["M.MaskID", "M.MaskDigest"]);
    }

    #[test]
    fn every_never_write_key_is_also_outside_the_preset_policy() {
        let mut flagged = 0;
        for (_, s) in registry::iter().filter(|(_, s)| never_written(s)) {
            flagged += 1;
            let meta_global = s.policy == registry::Policy::Meta && s.level == Level::Global;
            let outside = matches!(
                s.policy,
                registry::Policy::Computed
                    | registry::Policy::Never
                    | registry::Policy::Unknown
                    | registry::Policy::Photo
            );
            assert!(
                meta_global || outside,
                "{}/{} is on the never-write list but {}",
                s.level,
                s.name,
                s.policy.class_name()
            );
        }
        for name in NEVER_WRITE {
            assert!(
                registry::iter().any(|(_, s)| s.name == *name),
                "{name} is no registry key"
            );
        }
        assert!(flagged > NEVER_WRITE.len());
    }

    #[test]
    fn compatible_version_follows_the_feature_table() {
        let mut s = DevelopSettings::new();
        assert_eq!(compatible_version(&s), None);
        let subject = || {
            component(
                MaskTool::Semantic(Semantic::Subject),
                Combine::Add { inverted: false },
            )
        };
        s.corrections.push(correction("a", vec![subject()]));
        assert_eq!(compatible_version(&s), Some(EngineVersion::new(14, 0)));
        let background = component(
            MaskTool::Semantic(Semantic::Background),
            Combine::Add { inverted: false },
        );
        let mut alone = DevelopSettings::new();
        alone.corrections.push(correction("bg", vec![background]));
        assert_eq!(compatible_version(&alone), Some(EngineVersion::new(15, 0)));
        let mut curved = correction("curve", vec![subject()]);
        curved.local.insert(
            id(Level::Correction, "MainCurve"),
            Value::Curve(vec![(f(0.0), f(0.0)), (f(255.0), f(250.0))]),
        );
        let mut alone = DevelopSettings::new();
        alone.corrections.push(curved);
        assert_eq!(compatible_version(&alone), Some(EngineVersion::new(15, 3)));
        let people = component(
            MaskTool::Semantic(Semantic::PeoplePart(5)),
            Combine::Add { inverted: false },
        );
        s.corrections.push(correction("b", vec![people]));
        assert_eq!(compatible_version(&s), Some(EngineVersion::new(15, 0)));
        let land = component(
            MaskTool::Semantic(Semantic::Landscape(50001)),
            Combine::Add { inverted: false },
        );
        s.corrections.push(correction("c", vec![land]));
        assert_eq!(compatible_version(&s), Some(EngineVersion::new(15, 3)));
        set(
            &mut s,
            "LensBlur",
            Value::Struct(Struct::new(StructKind::LensBlur)),
        );
        assert_eq!(compatible_version(&s), Some(EngineVersion::new(16, 0)));
        set(&mut s, "CurveRefineSaturation", Value::Real(f(100.0)));
        assert_eq!(compatible_version(&s), Some(EngineVersion::new(17, 0)));
        for (_, v) in COMPATIBLE_VERSIONS {
            assert!(*v <= TARGET_ENGINE);
        }
        let mut plain = DevelopSettings::new();
        set(&mut plain, "Exposure2012", Value::Real(f(0.5)));
        let out = preset(&plain, true).xmp;
        assert!(!has(&out, "CompatibleVersion"), "no feature, no stamp");
    }

    #[test]
    fn a_linear_point_curve_is_named_linear() {
        let mut s = DevelopSettings::new();
        set(
            &mut s,
            "ToneCurvePV2012",
            Value::Curve(vec![(f(0.0), f(0.0)), (f(255.0), f(255.0))]),
        );
        assert!(has(
            &preset(&s, true).xmp,
            "crs:ToneCurveName2012=\"Linear\""
        ));
    }

    #[test]
    fn a_preset_needs_a_uuid_and_a_name_and_is_a_normal_preset() {
        let s = DevelopSettings::new();
        let mut header = PresetHeader::lrgenius(hex(1), "x");
        header.uuid = None;
        let e = write(&s, &WriteMode::Preset(PresetSpec::new(header))).unwrap_err();
        assert_eq!(e, WriteError::MissingHeaderField("UUID"));
        let mut header = PresetHeader::lrgenius(hex(1), "x");
        header.name = None;
        let e = write(&s, &WriteMode::Preset(PresetSpec::new(header))).unwrap_err();
        assert_eq!(e, WriteError::MissingHeaderField("Name"));
        for blank in ["", "  \t"] {
            let header = PresetHeader::lrgenius(hex(1), blank);
            let e = write(&s, &WriteMode::Preset(PresetSpec::new(header))).unwrap_err();
            assert_eq!(e, WriteError::MissingHeaderField("Name"), "{blank:?}");
        }
        let mut header = PresetHeader::lrgenius(hex(1), "x");
        header.preset_type = Some("Look".into());
        let e = write(&s, &WriteMode::Preset(PresetSpec::new(header))).unwrap_err();
        assert_eq!(e, WriteError::UnsupportedPresetType("Look".into()));
    }

    #[test]
    fn header_flags_and_extra_meta_keys_are_kept_and_others_reported() {
        let mut header = PresetHeader::lrgenius(hex(2), "x");
        header.supports_monochrome = Some(false);
        header.camera_model_restriction = Some("Synthetic Camera".into());
        header
            .rest
            .values
            .insert(id(Level::Header, "ShowInPresets"), Value::Bool(true));
        header.rest.values.insert(
            id(Level::Header, "SupportsOutputReferred"),
            Value::Bool(false),
        );
        header
            .rest
            .values
            .insert(id(Level::Header, "PresetType"), Value::Str("Look".into()));
        header.rest.values.insert(
            id(Level::Header, "Baseline"),
            Value::Str("Adobe Default".into()),
        );
        let written = write(
            &DevelopSettings::new(),
            &WriteMode::Preset(PresetSpec::new(header)),
        )
        .unwrap();
        let out = &written.xmp;
        assert!(
            has(out, "crs:PresetType=\"Normal\""),
            "the writer's stamp wins"
        );
        assert!(has(out, "crs:SupportsMonochrome=\"False\""));
        assert!(has(out, "crs:SupportsOutputReferred=\"False\""));
        assert!(has(out, "crs:ShowInPresets=\"True\""));
        assert!(has(out, "crs:CameraModelRestriction=\"Synthetic Camera\""));
        assert!(!has(out, "Baseline"));
        assert_eq!(written.skipped[0].path, "Baseline");
        assert!(!has(out, "ProcessVersion"), "not asked for");
    }

    #[test]
    fn text_xml_cannot_hold_is_an_error_not_a_broken_file() {
        let header = PresetHeader::lrgenius(hex(3), "bad\u{1}name");
        let e = write(
            &DevelopSettings::new(),
            &WriteMode::Preset(PresetSpec::new(header)),
        )
        .unwrap_err();
        assert!(
            matches!(e, WriteError::InvalidChar { ref path, .. } if path == "Name[0]"),
            "{e:?}"
        );
    }

    #[test]
    fn wrongly_typed_values_are_an_error() {
        let mut s = DevelopSettings::new();
        set(&mut s, "Exposure2012", Value::Str("abc".into()));
        let e = write(&s, &spec(true)).unwrap_err();
        assert!(matches!(e, WriteError::WrongValue { ref path, .. } if path == "Exposure2012"));
    }

    #[test]
    fn radial_and_person_parts_write_their_typed_fields() {
        let conv = Conv::new(false, Vec::new());
        let radial = component(
            MaskTool::Radial(RadialGradient {
                top: f(0.1),
                left: f(0.2),
                bottom: f(0.9),
                right: f(0.8),
                feather: 50,
                midpoint: 50,
                roundness: 0,
                flipped: true,
            }),
            Combine::Add { inverted: false },
        );
        let fields = conv.component_fields(&radial);
        let get = |name: &str| fields.get(Level::MaskTool, name).cloned();
        assert_eq!(get("Angle"), Some(Value::Real(Finite::ZERO)));
        assert_eq!(get("Flipped"), Some(Value::Bool(true)));
        assert_eq!(
            get("What"),
            Some(Value::Str("Mask/CircularGradient".into()))
        );
        let at = component(
            MaskTool::Semantic(Semantic::PersonPartAt {
                part: 5,
                point: SensorPoint {
                    x: f(0.25),
                    y: f(0.75),
                },
            }),
            Combine::Add { inverted: false },
        );
        let fields = Conv::new(true, Vec::new()).component_fields(&at);
        assert_eq!(
            fields.get(Level::MaskTool, "ReferencePoint"),
            Some(&Value::Str("0.250000 0.750000".into()))
        );
        assert_eq!(
            fields.get(Level::MaskTool, "MaskSubCategoryID"),
            Some(&Value::Int(5))
        );
    }

    /// A sidecar-shaped document: a Look with its embedded profile
    /// definition (`Parameters`, referring to a lookup table), the table
    /// blob itself, and a white balance.
    const SIDECAR_WITH_LOOK: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
 crs:ProcessVersion="15.4" crs:WhiteBalance="Custom" crs:Temperature="5150" crs:Tint="+8"
 crs:Exposure2012="+0.30" crs:CameraProfile="Adobe Standard"
 crs:Table_00000000000000000000000000001234="synthetic">
 <crs:Look>
  <rdf:Description crs:Name="Synthetic Profile" crs:Amount="1"
   crs:UUID="00000000000000000000000000005678" crs:SupportsAmount="false"
   crs:SupportsMonochrome="false" crs:SupportsOutputReferred="false" crs:Copyright="(c)">
  <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Profiles</rdf:li></rdf:Alt></crs:Group>
  <crs:Parameters>
   <rdf:Description crs:Version="18.5" crs:ProcessVersion="15.4"
    crs:LookTable="00000000000000000000000000001234"
    crs:RGBTable="00000000000000000000000000001234" crs:ColorVariance="0"/>
  </crs:Parameters>
  </rdf:Description>
 </crs:Look>
</rdf:Description></rdf:RDF></x:xmpmeta>"#;

    #[test]
    fn a_preset_refers_to_the_look_without_its_profile_definition() {
        let doc = parse(SIDECAR_WITH_LOOK.as_bytes()).unwrap();
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert!(doc.develop.look.is_some());
        for mixed in [false, true] {
            let written = preset(&doc.develop, mixed);
            let out = &written.xmp;
            for gone in [
                "crs:Parameters",
                "LookTable",
                "crs:RGBTable",
                "Table_",
                "Copyright=\"(c)\"",
                "Profiles",
                "ColorVariance",
            ] {
                assert!(!has(out, gone), "{gone} written:\n{out}");
            }
            assert!(has(out, "crs:RequiresRGBTables=\"False\""));
            assert!(has(
                out,
                "   <crs:Look\n    crs:Name=\"Synthetic Profile\"\n    crs:UUID=\"00000000000000000000000000005678\"\n    crs:Amount=\"1\"/>"
            ), "{out}");
            let paths: Vec<&str> = written.skipped.iter().map(|k| k.path.as_str()).collect();
            // The table blob is opaque and reported; the Look's own copies of
            // the profile record (Parameters, Group, ...) are not.
            assert!(paths.contains(&"Table_00000000000000000000000000001234"));
            assert!(!paths.iter().any(|p| p.starts_with("Look")), "{paths:?}");
        }
    }

    #[test]
    fn values_kept_whole_never_reach_a_preset_and_are_reported() {
        let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmlns:q="urn:q"
 crs:ProcessVersion="15.4" crs:Exposure2012="+0.30">
 <crs:ToneCurvePV2012><rdf:Bag><rdf:li>0, 0</rdf:li><rdf:li>255, 255</rdf:li></rdf:Bag></crs:ToneCurvePV2012>
 <crs:Texture rdf:parseType="Resource"><rdf:value>5</rdf:value><q:Note>q</q:Note></crs:Texture>
</rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let doc = parse(text.as_bytes()).unwrap();
        assert_eq!(doc.skipped_subtrees.kept_whole, 2);
        let written = preset(&doc.develop, true);
        for gone in ["ToneCurve", "rdf:Bag", "Texture", "urn:q"] {
            assert!(!has(&written.xmp, gone), "{gone}:\n{}", written.xmp);
        }
        assert!(has(&written.xmp, "crs:Exposure2012=\"+0.30\""));
        assert_eq!(
            written.skipped,
            ["Texture", "ToneCurvePV2012"]
                .map(|p| Skipped {
                    path: p.into(),
                    reason: SkipReason::Opaque
                })
                .to_vec()
        );
        let back = parse(written.xmp.as_bytes()).unwrap();
        assert_eq!(back.skipped_subtrees.kept_whole, 0);
        // The writer's own guard, should the filter ever let one through.
        let mut conv = Conv::new(true, Vec::new());
        let node = XmpNode::text("5");
        let spec = id(Level::Global, "Texture").spec();
        let v = Value::Opaque(Opaque::Xmp(node));
        assert_eq!(conv.node(spec, &v, "Texture").unwrap(), None);
        assert_eq!(conv.skipped[0].reason, SkipReason::Opaque);
    }

    fn white_balance(mode: &str) -> DevelopSettings {
        let mut s = DevelopSettings::new();
        s.file_kind = Some(crate::model::FileKind::Raw);
        set(&mut s, "WhiteBalance", Value::Str(mode.into()));
        set(&mut s, "Temperature", Value::Int(5150));
        set(&mut s, "Tint", Value::Int(8));
        set(&mut s, "Exposure2012", Value::Real(f(0.25)));
        s
    }

    #[test]
    fn a_resolved_white_balance_never_becomes_a_custom_one() {
        for mode in ["As Shot", "Auto"] {
            let written = preset(&white_balance(mode), false);
            for gone in ["WhiteBalance", "Temperature", "crs:Tint"] {
                assert!(!has(&written.xmp, gone), "{mode}: {gone}");
            }
            let numbers: Vec<_> = written
                .skipped
                .iter()
                .filter(|k| k.path != "WhiteBalance")
                .map(|k| (k.path.as_str(), k.reason.clone()))
                .collect();
            let why = SkipReason::WhiteBalanceMode(mode.into());
            assert_eq!(
                numbers,
                [("Temperature", why.clone()), ("Tint", why)],
                "{mode}"
            );
        }
        let written = preset(&white_balance("Custom"), false);
        assert!(has(&written.xmp, "crs:WhiteBalance=\"Custom\""));
        assert!(has(&written.xmp, "crs:Temperature=\"5150\""));
        assert!(has(&written.xmp, "crs:Tint=\"+8\""));
        assert!(
            written.skipped.is_empty(),
            "Custom is written back: {:?}",
            written.skipped
        );
        // A mixed preset has no white-balance numbers, so its Custom mode is
        // not carried and says so.
        let written = preset(&white_balance("Custom"), true);
        assert!(!has(&written.xmp, "WhiteBalance"));
        let paths: Vec<&str> = written.skipped.iter().map(|k| k.path.as_str()).collect();
        assert_eq!(paths, ["WhiteBalance", "Temperature", "Tint"]);
    }

    #[test]
    fn values_the_preset_form_writes_back_unchanged_are_not_reported() {
        let text = |point: &str| {
            format!(
                r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:ProcessVersion="15.4">
 <crs:MaskGroupBasedCorrections><rdf:Seq><rdf:li><rdf:Description crs:What="Correction"
   crs:CorrectionAmount="1" crs:CorrectionActive="true" crs:LocalExposure2012="0.1">
  <crs:CorrectionMasks><rdf:Seq>
   <rdf:li crs:What="Mask/Image" crs:MaskActive="true" crs:MaskBlendMode="0" crs:MaskInverted="false"
    crs:MaskValue="1" crs:MaskVersion="1" crs:MaskSubType="1" crs:ReferencePoint="{point}" crs:ErrorReason="0"/>
   <rdf:li><rdf:Description crs:What="Mask/RangeMask" crs:MaskActive="true" crs:MaskBlendMode="1"
    crs:MaskInverted="true" crs:MaskValue="0">
    <crs:CorrectionRangeMask crs:Version="3" crs:Type="2" crs:Invert="true" crs:SampleType="0"
     crs:LumRange="0.000000 0.000000 0.400000 0.500000"/></rdf:Description></rdf:li>
  </rdf:Seq></crs:CorrectionMasks>
 </rdf:Description></rdf:li></rdf:Seq></crs:MaskGroupBasedCorrections>
</rdf:Description></rdf:RDF></x:xmpmeta>"#
            )
        };
        for kind in [None, Some(crate::model::FileKind::NonRaw)] {
            let mut doc = parse(text("0.500000 0.500000").as_bytes()).unwrap();
            assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
            doc.develop.file_kind = kind;
            let written = preset(&doc.develop, true);
            assert!(
                written.skipped.is_empty(),
                "{kind:?}: {:?}",
                written.skipped
            );
            assert!(has(
                &written.xmp,
                "crs:ReferencePoint=\"0.500000 0.500000\""
            ));
        }
        // A subject's point is Lightroom's per-photo result: replaced
        // silently, like Adobe's presets do.
        let doc = parse(text("0.300000 0.600000").as_bytes()).unwrap();
        assert!(preset(&doc.develop, true).skipped.is_empty());
        // A people part's point may pick the person: replacing it is
        // reported (at the centre it is not, see above).
        let people = |point: &str| {
            text(point).replace(
                "crs:MaskSubType=\"1\"",
                "crs:MaskSubType=\"3\" crs:MaskSubCategoryID=\"5\"",
            )
        };
        let doc = parse(people("0.300000 0.600000").as_bytes()).unwrap();
        let paths: Vec<String> = preset(&doc.develop, true)
            .skipped
            .into_iter()
            .map(|k| k.path)
            .collect();
        assert_eq!(
            paths,
            ["MaskGroupBasedCorrections[0].CorrectionMasks[0].ReferencePoint"]
        );
        let doc = parse(people("0.500000 0.500000").as_bytes()).unwrap();
        assert!(preset(&doc.develop, true).skipped.is_empty());
    }

    #[cfg(feature = "test-roundtrip")]
    mod round_trip {
        use super::*;
        use crate::xmp::read::XmpDocument;

        fn again(doc: &XmpDocument) -> (String, XmpDocument) {
            let written = write(&doc.develop, &WriteMode::TestRoundTrip(doc.header.clone()))
                .unwrap_or_else(|e| panic!("{e}"));
            assert!(written.skipped.is_empty());
            let back =
                parse(written.xmp.as_bytes()).unwrap_or_else(|e| panic!("{e}\n{}", written.xmp));
            (written.xmp, back)
        }

        // Every committed fixture: tests/xmp_roundtrip.rs.

        #[test]
        fn a_preset_round_trips_through_the_test_mode() {
            let doc = parse(preset(&settings(), false).xmp.as_bytes()).unwrap();
            let (_, back) = again(&doc);
            assert_eq!(back.develop, doc.develop);
            assert_eq!(back.header, doc.header);
        }

        #[test]
        fn numbers_the_format_would_round_survive_the_test_mode() {
            let mut s = DevelopSettings::new();
            set(&mut s, "Exposure2012", Value::Real(f(0.333)));
            set(&mut s, "CropTop", Value::Real(f(0.1234567)));
            let written = write(&s, &WriteMode::TestRoundTrip(None)).unwrap();
            assert!(has(&written.xmp, "crs:Exposure2012=\"+0.333\""));
            let back = parse(written.xmp.as_bytes()).unwrap();
            assert_eq!(
                back.develop.get_by_name("CropTop"),
                s.get_by_name("CropTop")
            );
        }

        #[test]
        fn lua_content_has_no_xmp_form() {
            let mut s = DevelopSettings::new();
            s.opaque.push(OpaqueEntry {
                ns: None,
                name: "X".into(),
                value: Opaque::Json(serde_json::json!([1])),
            });
            let e = write(&s, &WriteMode::TestRoundTrip(None)).unwrap_err();
            assert_eq!(e, WriteError::NotXmp { path: "X".into() });
        }

        #[test]
        fn foreign_namespaces_qualifiers_and_languages_survive() {
            let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmlns:q="urn:q"
 crs:ProcessVersion="15.4" crs:Mystery="a&#10;b&amp;c">
 <crs:Unknown q:attr="1" crs:x="2"><q:el>t</q:el></crs:Unknown>
 <crs:Qualified><rdf:Description><rdf:value>v</rdf:value><q:note>n</q:note></rdf:Description></crs:Qualified>
 <crs:Langs><rdf:Alt><rdf:li xml:lang="x-default">A</rdf:li><rdf:li xml:lang="de-DE">B</rdf:li></rdf:Alt></crs:Langs>
 <crs:Empty><rdf:Description/></crs:Empty>
 <crs:Bagged><rdf:Bag><rdf:li>1</rdf:li></rdf:Bag></crs:Bagged>
 <crs:Mystery>dup</crs:Mystery>
</rdf:Description></rdf:RDF></x:xmpmeta>"#;
            let doc = parse(text.as_bytes()).unwrap();
            assert_eq!(doc.develop.opaque.len(), 7, "{:?}", doc.develop.opaque);
            let (xmp, back) = again(&doc);
            assert_eq!(back.develop, doc.develop, "\n{xmp}");
            assert!(has(&xmp, "xmlns:ns1=\"urn:q\""));
            assert_eq!(xmp.matches("<rdf:Bag>").count(), 1, "the one kept whole");
        }
    }
}
