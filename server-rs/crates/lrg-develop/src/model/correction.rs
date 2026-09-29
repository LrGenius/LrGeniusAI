//! Local corrections (`MaskGroupBasedCorrections`) and their mask components.
//!
//! A [`Correction`] is one entry of the Masking panel: its adjustments
//! (`Local*`, local curves) in the stored unit and its mask components in
//! evaluation order. Each [`MaskComponent`] is typed where the backend needs logic
//! (semantic AI masks, linear and radial gradients without rotation,
//! luminance ranges) and otherwise kept as [`MaskTool::Opaque`] with all of
//! its fields, so nothing is lost on a round trip.
//!
//! Components are recognised by their structure only (`What`,
//! `MaskSubType`, `MaskSubCategoryID`, ...), never by their names: Lightroom
//! localises mask names ("Künstlicher Boden", "Maske 1").

use std::collections::BTreeMap;

use super::value::{Fields, Finite, Hex32, Struct, Value};
use crate::parse::{ParseWarning, WarningKind};
use crate::registry::{KeyId, KeySpec, Level, Policy, StructKind};

/// `MaskSubCategoryID`s of the people parts (face skin, iris/pupil, body
/// skin, hair, lips, beard, sclera, eyebrows, clothes, teeth).
pub const PEOPLE_PARTS: &[i64] = &[2, 3, 4, 5, 6, 7, 8, 9, 11, 12];
/// `MaskSubCategoryID`s of the landscape classes (architecture ... snow).
pub const LANDSCAPE_CLASSES: std::ops::RangeInclusive<i64> = 50001..=50008;
/// `MaskSubCategoryID` of "Select Background".
pub const BACKGROUND_CATEGORY: i64 = 22;

/// A point normalised per axis in the uncropped image in sensor orientation
/// (the frame gradient coordinates and `ReferencePoint` live in).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SensorPoint {
    /// 0 = left edge, 1 = right edge (may leave [0, 1]).
    pub x: Finite,
    /// 0 = top edge, 1 = bottom edge (may leave [0, 1]).
    pub y: Finite,
}

impl SensorPoint {
    /// Parses a `ReferencePoint` string (`"0.433594 0.659824"`).
    pub fn parse_pair(s: &str) -> Option<SensorPoint> {
        let mut it = s
            .split_whitespace()
            .map(|t| t.parse::<f64>().ok().and_then(|x| Finite::new(x).ok()));
        let (x, y) = (it.next()??, it.next()??);
        it.next().is_none().then_some(SensorPoint { x, y })
    }
}

/// One `MaskGroupBasedCorrections` item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Correction {
    /// `CorrectionName` (display only, possibly localised; never used to
    /// identify anything).
    pub name: Option<String>,
    /// `CorrectionSyncID`.
    pub sync_id: Option<Hex32>,
    /// `CorrectionAmount` (stored unit, 1 = 100 %).
    pub amount: Option<Finite>,
    /// `CorrectionActive`.
    pub active: Option<bool>,
    /// The adjustments (`Local*`, local curves) in the stored unit.
    pub local: BTreeMap<KeyId, Value>,
    /// `CorrectionMasks`, in evaluation order.
    pub masks: Vec<MaskComponent>,
    /// Every other field (`What`, runtime ids, legacy `LocalExposure`, ...)
    /// and what the registry does not know, for the round trip.
    pub extra: Fields,
}

/// How a component combines with the ones before it (the table on the wiki
/// page Dev-Develop-Model, "Corrections and masks").
///
/// | `Combine` | `MaskBlendMode` | `MaskValue` | `MaskInverted` |
/// |---|---|---|---|
/// | `Add` | 0 | 1 | `inverted` |
/// | `Subtract` | 1 | 0 | false |
/// | `Intersect` | 1 | 0 | true (A ∩ B = A − ¬B) |
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Combine {
    /// Adds the (optionally inverted) component.
    Add {
        /// `MaskInverted`.
        inverted: bool,
    },
    /// Subtracts the component.
    Subtract,
    /// Intersects with the component.
    Intersect,
    /// The three fields are missing or form none of the above; they stay in
    /// [`MaskComponent::extra`] as they are.
    Unrecognised,
}

impl Combine {
    /// Reads the combination from `(MaskBlendMode, MaskValue, MaskInverted)`.
    pub fn decode(blend_mode: i64, value: Finite, inverted: bool) -> Option<Combine> {
        match (blend_mode, value.get(), inverted) {
            (0, 1.0, inverted) => Some(Combine::Add { inverted }),
            (1, 0.0, false) => Some(Combine::Subtract),
            (1, 0.0, true) => Some(Combine::Intersect),
            _ => None,
        }
    }

    /// `(MaskBlendMode, MaskValue, MaskInverted)`; `None` for
    /// [`Combine::Unrecognised`].
    pub fn encode(self) -> Option<(i64, Finite, bool)> {
        match self {
            Combine::Add { inverted } => Some((0, Finite::new_const(1.0), inverted)),
            Combine::Subtract => Some((1, Finite::ZERO, false)),
            Combine::Intersect => Some((1, Finite::ZERO, true)),
            Combine::Unrecognised => None,
        }
    }
}

/// One `CorrectionMasks` item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskComponent {
    /// What the component selects.
    pub tool: MaskTool,
    /// How it combines with the components before it.
    pub combine: Combine,
    /// `MaskName` (display only, possibly localised).
    pub name: Option<String>,
    /// `MaskSyncID`.
    pub sync_id: Option<Hex32>,
    /// `MaskActive`.
    pub active: Option<bool>,
    /// Every field the typed parts do not hold (digests, versions, reference
    /// points, gestures, ...), for the round trip.
    pub extra: Fields,
}

/// The mask tool of a component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaskTool {
    /// `Mask/Image`: an AI mask in a recognised form.
    Semantic(Semantic),
    /// `Mask/Gradient`.
    Linear(LinearGradient),
    /// `Mask/CircularGradient` with `Angle` 0.
    Radial(RadialGradient),
    /// `Mask/RangeMask` with `CorrectionRangeMask.Type` 2.
    LuminanceRange(LuminanceRange),
    /// Anything else (brush aggregates, Select Object, colour and depth
    /// ranges, rotated radial gradients, unknown tools): kept for the round
    /// trip only; every field is in [`MaskComponent::extra`].
    Opaque {
        /// The `What` value, if any.
        what: Option<String>,
    },
}

/// The recognised forms of an AI (`Mask/Image`) mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Semantic {
    /// `MaskSubType` 1.
    Subject,
    /// `MaskSubType` 2.
    Sky,
    /// `MaskSubType` 0, `MaskSubCategoryID` 22.
    Background,
    /// `MaskSubType` 3 with a people-part category: the preset form ("all
    /// people").
    PeoplePart(i64),
    /// `MaskSubType` 0 with a landscape class (50001..50008).
    Landscape(i64),
    /// `MaskSubType` 0 with a people-part category and a `ReferencePoint`:
    /// the part of the one person at that point (photo-specific).
    PersonPartAt {
        /// People-part category.
        part: i64,
        /// The person's reference point.
        point: SensorPoint,
    },
}

impl Semantic {
    /// `(MaskSubType, MaskSubCategoryID)`.
    pub fn encoding(self) -> (i64, Option<i64>) {
        match self {
            Semantic::Subject => (1, None),
            Semantic::Sky => (2, None),
            Semantic::Background => (0, Some(BACKGROUND_CATEGORY)),
            Semantic::PeoplePart(part) => (3, Some(part)),
            Semantic::Landscape(class) => (0, Some(class)),
            Semantic::PersonPartAt { part, .. } => (0, Some(part)),
        }
    }
}

/// A linear gradient: full effect at `full`, none at `zero`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LinearGradient {
    /// `ZeroX`/`ZeroY`.
    pub zero: SensorPoint,
    /// `FullX`/`FullY`.
    pub full: SensorPoint,
}

/// A radial gradient without rotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RadialGradient {
    /// `Top`.
    pub top: Finite,
    /// `Left`.
    pub left: Finite,
    /// `Bottom`.
    pub bottom: Finite,
    /// `Right`.
    pub right: Finite,
    /// `Feather` (0..100).
    pub feather: i64,
    /// `Midpoint`.
    pub midpoint: i64,
    /// `Roundness`.
    pub roundness: i64,
    /// `Flipped` (true: the effect is inside the ellipse).
    pub flipped: bool,
}

/// A luminance range mask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuminanceRange {
    /// `LumRange`: low feather, low, high, high feather (0..1).
    pub range: [Finite; 4],
    /// `CorrectionRangeMask.Invert`.
    pub invert: bool,
    /// The other `CorrectionRangeMask` fields (`Version`, `SampleType`,
    /// `LuminanceDepthSampleInfo`, ...).
    pub rest: Struct,
}

/// Whether a correction-level key is one of the adjustments
/// ([`Correction::local`]) rather than bookkeeping.
pub fn is_local_adjustment(spec: &KeySpec) -> bool {
    spec.level == Level::Correction
        && !matches!(spec.policy, Policy::Computed | Policy::Meta | Policy::Never)
        && !matches!(spec.name, "CorrectionAmount" | "CorrectionMasks")
}

const C: Level = Level::Correction;
const M: Level = Level::MaskTool;

fn string(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

fn hex(v: &Value) -> Option<Hex32> {
    v.as_str().and_then(Hex32::parse)
}

impl Correction {
    /// Builds the typed view of a correction's fields. `path` is the
    /// correction's own path (`MaskGroupBasedCorrections[2]`).
    pub(crate) fn from_fields(
        mut f: Fields,
        path: &str,
        warnings: &mut Vec<ParseWarning>,
    ) -> Correction {
        let name = f.take_if(C, "CorrectionName", string);
        let sync_id = f.take_if(C, "CorrectionSyncID", hex);
        let amount = f.take_if(C, "CorrectionAmount", Value::as_finite);
        let active = f.take_if(C, "CorrectionActive", Value::as_bool);
        let masks = match f.take(C, "CorrectionMasks") {
            Some(Value::Tools(items)) => items
                .into_iter()
                .enumerate()
                .map(|(i, item)| {
                    MaskComponent::from_fields(
                        item,
                        &format!("{path}.CorrectionMasks[{i}]"),
                        warnings,
                    )
                })
                .collect(),
            Some(other) => {
                // Not reachable from the readers (they only produce `Tools`
                // for this key); keep the value rather than drop it.
                let id = crate::registry::lookup(C, "CorrectionMasks").expect("registry row");
                f.values.insert(id, other);
                Vec::new()
            }
            None => Vec::new(),
        };
        let local_ids: Vec<KeyId> = f
            .values
            .keys()
            .copied()
            .filter(|id| is_local_adjustment(id.spec()))
            .collect();
        let local = local_ids
            .into_iter()
            .filter_map(|id| f.values.remove(&id).map(|v| (id, v)))
            .collect();
        Correction {
            name,
            sync_id,
            amount,
            active,
            local,
            masks,
            extra: f,
        }
    }

    /// The adjustment `name` (`"LocalExposure2012"`), stored unit.
    pub fn local_value(&self, name: &str) -> Option<&Value> {
        self.local.get(&crate::registry::lookup(C, name)?)
    }
}

impl MaskComponent {
    /// Builds the typed view of a component's fields. `path` is the
    /// component's own path (`MaskGroupBasedCorrections[2].CorrectionMasks[0]`).
    pub(crate) fn from_fields(
        mut f: Fields,
        path: &str,
        warnings: &mut Vec<ParseWarning>,
    ) -> MaskComponent {
        let name = f.take_if(M, "MaskName", string);
        let sync_id = f.take_if(M, "MaskSyncID", hex);
        let active = f.take_if(M, "MaskActive", Value::as_bool);
        let combine = take_combine(&mut f, path, warnings);
        let tool = take_tool(&mut f);
        MaskComponent {
            tool,
            combine,
            name,
            sync_id,
            active,
            extra: f,
        }
    }
}

fn take_combine(f: &mut Fields, path: &str, warnings: &mut Vec<ParseWarning>) -> Combine {
    let mode = f.get(M, "MaskBlendMode").and_then(Value::as_int);
    let value = f.get(M, "MaskValue").and_then(Value::as_finite);
    let inverted = f.get(M, "MaskInverted").and_then(Value::as_bool);
    if let (Some(mode), Some(value), Some(inverted)) = (mode, value, inverted) {
        if let Some(c) = Combine::decode(mode, value, inverted) {
            for key in ["MaskBlendMode", "MaskValue", "MaskInverted"] {
                f.take(M, key);
            }
            return c;
        }
    }
    warnings.push(ParseWarning {
        path: format!("{path}.MaskBlendMode"),
        key: "MaskBlendMode".into(),
        raw: Some(format!(
            "MaskBlendMode={mode:?} MaskValue={:?} MaskInverted={inverted:?}",
            value.map(Finite::get)
        )),
        kind: WarningKind::UnrecognisedMaskCombine,
    });
    Combine::Unrecognised
}

fn take_tool(f: &mut Fields) -> MaskTool {
    let what = f.get(M, "What").and_then(Value::as_str).map(str::to_owned);
    let typed = match what.as_deref() {
        Some("Mask/Image") => semantic(f).map(MaskTool::Semantic),
        Some("Mask/Gradient") => linear(f).map(MaskTool::Linear),
        Some("Mask/CircularGradient") => radial(f).map(MaskTool::Radial),
        Some("Mask/RangeMask") => luminance_range(f).map(MaskTool::LuminanceRange),
        _ => None,
    };
    if what.is_some() {
        f.take(M, "What");
    }
    typed.unwrap_or(MaskTool::Opaque { what })
}

fn int(f: &Fields, name: &str) -> Option<i64> {
    f.get(M, name).and_then(Value::as_int)
}

fn real(f: &Fields, name: &str) -> Option<Finite> {
    f.get(M, name).and_then(Value::as_finite)
}

fn remove(f: &mut Fields, names: &[&str]) {
    for n in names {
        f.take(M, n);
    }
}

fn semantic(f: &mut Fields) -> Option<Semantic> {
    if f.get(M, "Gesture").is_some() {
        return None; // Select Object: photo-specific strokes/polygons
    }
    let sub = int(f, "MaskSubType")?;
    let cat = int(f, "MaskSubCategoryID");
    let s = match (sub, cat) {
        (1, None) => Semantic::Subject,
        (2, None) => Semantic::Sky,
        (0, Some(BACKGROUND_CATEGORY)) => Semantic::Background,
        (3, Some(p)) if PEOPLE_PARTS.contains(&p) => Semantic::PeoplePart(p),
        (0, Some(c)) if LANDSCAPE_CLASSES.contains(&c) => Semantic::Landscape(c),
        (0, Some(p)) if PEOPLE_PARTS.contains(&p) => {
            let point = f
                .get(M, "ReferencePoint")
                .and_then(Value::as_str)
                .and_then(SensorPoint::parse_pair)?;
            f.take(M, "ReferencePoint");
            Semantic::PersonPartAt { part: p, point }
        }
        _ => return None,
    };
    remove(f, &["MaskSubType", "MaskSubCategoryID"]);
    Some(s)
}

fn linear(f: &mut Fields) -> Option<LinearGradient> {
    let g = LinearGradient {
        zero: SensorPoint {
            x: real(f, "ZeroX")?,
            y: real(f, "ZeroY")?,
        },
        full: SensorPoint {
            x: real(f, "FullX")?,
            y: real(f, "FullY")?,
        },
    };
    remove(f, &["ZeroX", "ZeroY", "FullX", "FullY"]);
    Some(g)
}

fn radial(f: &mut Fields) -> Option<RadialGradient> {
    // Only Angle 0 until experiment E6 settles the rotation convention.
    if real(f, "Angle")? != Finite::ZERO {
        return None;
    }
    let g = RadialGradient {
        top: real(f, "Top")?,
        left: real(f, "Left")?,
        bottom: real(f, "Bottom")?,
        right: real(f, "Right")?,
        feather: int(f, "Feather")?,
        midpoint: int(f, "Midpoint")?,
        roundness: int(f, "Roundness")?,
        flipped: f.get(M, "Flipped").and_then(Value::as_bool)?,
    };
    remove(
        f,
        &[
            "Angle",
            "Top",
            "Left",
            "Bottom",
            "Right",
            "Feather",
            "Midpoint",
            "Roundness",
            "Flipped",
        ],
    );
    Some(g)
}

fn luminance_range(f: &mut Fields) -> Option<LuminanceRange> {
    let Some(Value::Struct(crm)) = f.get(M, "CorrectionRangeMask") else {
        return None;
    };
    let level = Level::Struct(StructKind::CorrectionRangeMask);
    if crm.get("Type").and_then(Value::as_int) != Some(2) {
        return None;
    }
    let invert = crm.get("Invert").and_then(Value::as_bool)?;
    let range = parse_lum_range(crm.get("LumRange").and_then(Value::as_str)?)?;
    let Some(Value::Struct(mut rest)) = f.take(M, "CorrectionRangeMask") else {
        unreachable!("checked above");
    };
    for n in ["Type", "Invert", "LumRange"] {
        rest.fields.take(level, n);
    }
    Some(LuminanceRange {
        range,
        invert,
        rest,
    })
}

/// Parses `LumRange` (`"0.000000 0.200000 0.800000 1.000000"`).
pub fn parse_lum_range(s: &str) -> Option<[Finite; 4]> {
    let v: Vec<Finite> = s
        .split_whitespace()
        .map(|t| t.parse::<f64>().ok().and_then(|x| Finite::new(x).ok()))
        .collect::<Option<_>>()?;
    v.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(x: f64) -> Finite {
        Finite::new(x).unwrap()
    }

    #[test]
    fn combine_table_round_trips() {
        for c in [
            Combine::Add { inverted: false },
            Combine::Add { inverted: true },
            Combine::Subtract,
            Combine::Intersect,
        ] {
            let (m, v, i) = c.encode().unwrap();
            assert_eq!(Combine::decode(m, v, i), Some(c));
        }
        assert_eq!(Combine::decode(1, f(1.0), false), None);
        assert_eq!(Combine::decode(0, f(0.0), false), None);
        assert_eq!(Combine::Unrecognised.encode(), None);
    }

    #[test]
    fn reference_point_needs_exactly_two_numbers() {
        assert_eq!(
            SensorPoint::parse_pair("0.25 0.75"),
            Some(SensorPoint {
                x: f(0.25),
                y: f(0.75)
            })
        );
        assert_eq!(SensorPoint::parse_pair("0.25"), None);
        assert_eq!(SensorPoint::parse_pair("0.25 0.75 1"), None);
        assert_eq!(SensorPoint::parse_pair("a b"), None);
    }

    #[test]
    fn lum_range_needs_four_numbers() {
        assert_eq!(
            parse_lum_range("0 0.2 0.8 1"),
            Some([f(0.0), f(0.2), f(0.8), f(1.0)])
        );
        assert_eq!(parse_lum_range("0 0.2 0.8"), None);
    }

    #[test]
    fn semantic_encodings_follow_the_plan_table() {
        assert_eq!(Semantic::Subject.encoding(), (1, None));
        assert_eq!(Semantic::Sky.encoding(), (2, None));
        assert_eq!(Semantic::Background.encoding(), (0, Some(22)));
        assert_eq!(Semantic::PeoplePart(5).encoding(), (3, Some(5)));
        assert_eq!(Semantic::Landscape(50001).encoding(), (0, Some(50001)));
    }

    #[test]
    fn local_adjustments_exclude_bookkeeping() {
        let spec = |n| crate::registry::lookup(C, n).unwrap().spec();
        assert!(is_local_adjustment(spec("LocalExposure2012")));
        assert!(is_local_adjustment(spec("MainCurve")));
        assert!(!is_local_adjustment(spec("CorrectionAmount")));
        assert!(!is_local_adjustment(spec("What")));
        assert!(!is_local_adjustment(spec("CorrectionID")));
        assert!(!is_local_adjustment(spec("LocalExposure")));
    }
}
