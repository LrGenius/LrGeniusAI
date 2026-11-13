//! JSON.lua output → [`DevelopSettings`] + warnings.
//!
//! Accepts exactly what `JSON.lua` makes of `photo:getDevelopSettings()`
//! (checked against 1,294 real training rows):
//!
//! - top-level `[]` is the empty table (JSON.lua cannot tell `{}` from `[]`);
//! - an empty table (`[]` or `{}`) as the value of any key, at any depth,
//!   means "key absent", without a warning (`LensBlur`, `AILook`,
//!   `FilterList`, `RedEyeInfo`, `PointColors`, ...); an empty table as an
//!   element of a list of objects (masks, corrections, structures) is no
//!   element, also without a warning;
//! - an integer where a real is expected is fine; a non-integer on an integer
//!   key is rounded half-to-even ([`KeySpec::coerce`]) and warned about;
//! - booleans and 0/1 are both accepted on bool and 0/1-flag keys and
//!   normalised to the key's kind;
//! - an integer string (`"1"`) on a mask enum ([`KeySpec::is_mask_enum`]:
//!   `MaskSubType`, `MaskBlendMode`, `CorrectionRangeMask.Type`, ...) is
//!   read as that integer, without a warning: the Lua writer's
//!   `EnumAs::String` form (`lua::LuaOptions::mask_enum_as`), so whichever
//!   form experiment E2 settles on reads back;
//! - a known key whose value has the wrong shape (`null` in an array, a mixed
//!   array, a string for a number) is kept verbatim as [`Opaque::Json`] with a
//!   [`WarningKind::WrongType`], and never written;
//! - an unknown key is kept verbatim with a [`WarningKind::UnknownKey`];
//!   never an error. Keys of a pattern family (`Table_<md5>`, `pm_*`,
//!   `UprightTransform_N`, the FilterList payload) are kept verbatim without
//!   a warning;
//! - masks are identified by structure, never by their (localised) names;
//! - the file kind comes from the white-balance key family (`Temperature` ⇒
//!   raw, `IncrementalTemperature` ⇒ non-raw); a caller hint that
//!   contradicts it is warned about and loses;
//! - `ProcessVersion` below 6.7 (PV2003/PV2010) is warned about: such
//!   settings are read but must not be learned;
//! - keys are read in name order, so opaque entries and warnings come out in
//!   the same order in every build (`serde_json`'s map order depends on its
//!   `preserve_order` feature, which the workspace turns on and a lone
//!   `cargo test -p lrg-develop` does not).
//!
//! Typing, the file-kind and process-version checks and the assembly of the
//! settings are shared with the XMP reader (`crate::reader`), and so is the
//! correction and mask classification (`crate::model::correction`), so
//! both formats land in the same model.

use serde_json::{Map, Value as J};

use crate::model::correction::Correction;
use crate::model::value::{Fields, Finite, Opaque, OpaqueEntry, Struct, Value};
use crate::model::{DevelopSettings, FileKindHint};
use crate::parse::{ParseError, ParseWarning, WarningKind};
use crate::reader::{self, child, parse_point, ScalarIn};
use crate::registry::{self, CurveKind, KeySpec, Level, Resolved, ValueKind};

/// Largest `develop_settings` text [`from_lua_str`] accepts (4 MiB; real
/// blobs are at most ~50 KB).
pub const MAX_LUA_JSON_BYTES: usize = 4 * 1024 * 1024;

/// Parses a `develop_settings` JSON text. See [`from_lua_value`].
pub fn from_lua_str(
    text: &str,
    hint: FileKindHint,
) -> Result<(DevelopSettings, Vec<ParseWarning>), ParseError> {
    if text.len() > MAX_LUA_JSON_BYTES {
        return Err(ParseError::TooLarge {
            size: text.len(),
            max: MAX_LUA_JSON_BYTES,
        });
    }
    let value: J = serde_json::from_str(text)?;
    from_lua_value(&value, hint)
}

/// Reads one decoded `develop_settings` value into the model.
///
/// Errors only when the top level is not a table; every problem below it is
/// a [`ParseWarning`] and the value is kept (see the module docs for the
/// rules).
pub fn from_lua_value(
    value: &J,
    hint: FileKindHint,
) -> Result<(DevelopSettings, Vec<ParseWarning>), ParseError> {
    let obj = match value {
        J::Object(obj) => obj,
        J::Array(a) if a.is_empty() => {
            let mut settings = DevelopSettings::default();
            settings.file_kind = hint.kind();
            return Ok((settings, Vec::new()));
        }
        J::Array(_) => {
            return Err(ParseError::NotATable {
                found: "a non-empty array",
            })
        }
        J::String(_) => return Err(ParseError::NotATable { found: "a string" }),
        J::Number(_) => return Err(ParseError::NotATable { found: "a number" }),
        J::Bool(_) => return Err(ParseError::NotATable { found: "a boolean" }),
        J::Null => return Err(ParseError::NotATable { found: "null" }),
    };
    let mut reader = Reader::default();
    let fields = reader.fields(obj, Level::Global, "");
    let settings = reader::finish(fields, hint, &mut reader.warnings);
    Ok((settings, reader.warnings))
}

/// `"3"`, `"-1"`: an optional minus and ASCII digits, nothing else.
fn is_integer_text(t: &str) -> bool {
    let digits = t.strip_prefix('-').unwrap_or(t);
    !digits.is_empty() && digits.len() <= 18 && digits.bytes().all(|b| b.is_ascii_digit())
}

#[derive(Default)]
struct Reader {
    warnings: Vec<ParseWarning>,
}

fn is_empty_table(v: &J) -> bool {
    match v {
        J::Array(a) => a.is_empty(),
        J::Object(o) => o.is_empty(),
        _ => false,
    }
}

/// The value as compact JSON, cut to a readable length.
fn snippet(v: &J) -> String {
    const MAX: usize = 80;
    let s = v.to_string();
    if s.len() <= MAX {
        return s;
    }
    let mut end = MAX;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn describe(kind: ValueKind) -> String {
    match kind {
        ValueKind::Int => "an integer".into(),
        ValueKind::Real => "a number".into(),
        ValueKind::Bool(_) => "a boolean (or 0/1)".into(),
        ValueKind::IntFlag => "0/1 (or a boolean)".into(),
        ValueKind::Enum(set) => format!("one of {set:?}"),
        ValueKind::EnumInt(set) => format!("one of {set:?}"),
        ValueKind::Str | ValueKind::VersionStr | ValueKind::Hex32 => "a string".into(),
        ValueKind::VersionU32 => "a packed version number".into(),
        ValueKind::Curve(CurveKind::Global) => "a flat list of numbers (x, y, x, y, ...)".into(),
        ValueKind::Curve(CurveKind::Local) => "a list of \"x,y\" strings".into(),
        ValueKind::StrSeq => "a list of strings".into(),
        ValueKind::LangAlt => "an object of language strings".into(),
        ValueKind::Struct(k) => format!("a {} object", k.name()),
        ValueKind::StructSeq(k) => format!("a list of {} objects", k.name()),
        ValueKind::CorrectionSeq => "a list of correction objects".into(),
        ValueKind::ComponentSeq => "a list of mask objects".into(),
        ValueKind::Settings | ValueKind::Any | ValueKind::PerFormat { .. } => "any value".into(),
    }
}

fn opaque(name: &str, v: &J) -> OpaqueEntry {
    OpaqueEntry {
        ns: None,
        name: name.to_owned(),
        value: Opaque::Json(v.clone()),
    }
}

impl Reader {
    fn warn(&mut self, path: &str, key: &str, raw: Option<&J>, kind: WarningKind) {
        reader::warn(&mut self.warnings, path, key, raw.map(snippet), kind);
    }

    /// Reads one table at `level`, in key-name order. Empty tables below it
    /// are absent keys.
    fn fields(&mut self, obj: &Map<String, J>, level: Level, path: &str) -> Fields {
        let mut out = Fields::default();
        let mut entries: Vec<(&String, &J)> = obj.iter().collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        for (key, v) in entries {
            if is_empty_table(v) {
                continue;
            }
            let p = child(path, key);
            match registry::resolve(level, key) {
                Resolved::Key(id) => match self.value(id.spec(), v, &p, key) {
                    Some(value) => {
                        out.values.insert(id, value);
                    }
                    None => out.opaque.push(opaque(key, v)),
                },
                Resolved::Pattern(_) => out.opaque.push(opaque(key, v)),
                Resolved::Unknown => {
                    self.warn(&p, key, Some(v), WarningKind::UnknownKey);
                    out.opaque.push(opaque(key, v));
                }
            }
        }
        out
    }

    fn correction(&mut self, obj: &Map<String, J>, path: &str) -> Correction {
        let fields = self.fields(obj, Level::Correction, path);
        Correction::from_fields(fields, path, &mut self.warnings)
    }

    /// Reads `v` as `spec`'s kind; `None` (after a warning) when it does not
    /// have that shape.
    fn value(&mut self, spec: &KeySpec, v: &J, path: &str, key: &str) -> Option<Value> {
        let kind = spec.kind.lua_form();
        let value = match kind {
            ValueKind::Int
            | ValueKind::Real
            | ValueKind::IntFlag
            | ValueKind::EnumInt(_)
            | ValueKind::VersionU32
            | ValueKind::Bool(_) => self.scalar(spec, v, path, key),
            ValueKind::Enum(_) | ValueKind::Str | ValueKind::VersionStr | ValueKind::Hex32 => v
                .as_str()
                .and_then(|s| reader::text(kind, s, path, key, || snippet(v), &mut self.warnings)),
            ValueKind::Curve(CurveKind::Global) => v.as_array().and_then(|a| {
                if a.len() % 2 != 0 {
                    return None;
                }
                let nums: Option<Vec<Finite>> = a
                    .iter()
                    .map(|x| x.as_f64().and_then(|f| Finite::new(f).ok()))
                    .collect();
                Some(Value::Curve(
                    nums?
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|&[x, y]| (x, y))
                        .collect(),
                ))
            }),
            ValueKind::Curve(CurveKind::Local) => v.as_array().and_then(|a| {
                a.iter()
                    .map(|x| x.as_str().and_then(parse_point))
                    .collect::<Option<Vec<_>>>()
                    .map(Value::Curve)
            }),
            ValueKind::StrSeq => v.as_array().and_then(|a| {
                a.iter()
                    .map(|x| x.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
                    .map(Value::StrList)
            }),
            ValueKind::LangAlt => v.as_object().and_then(|o| {
                if !o.values().all(J::is_string) {
                    None
                } else if let (1, Some(J::String(s))) = (o.len(), o.get("x-default")) {
                    Some(Value::Alt(s.clone()))
                } else {
                    // Several languages: a valid shape the model keeps whole.
                    Some(Value::Opaque(Opaque::Json(v.clone())))
                }
            }),
            ValueKind::Struct(k) => v.as_object().map(|o| {
                Value::Struct(Struct {
                    kind: k,
                    fields: self.fields(o, Level::Struct(k), path),
                })
            }),
            ValueKind::StructSeq(k) => self.objects(v).map(|items| {
                Value::StructList(
                    items
                        .into_iter()
                        .enumerate()
                        .map(|(i, o)| Struct {
                            kind: k,
                            fields: self.fields(o, Level::Struct(k), &format!("{path}[{i}]")),
                        })
                        .collect(),
                )
            }),
            ValueKind::CorrectionSeq => self.objects(v).map(|items| {
                Value::Corrections(
                    items
                        .into_iter()
                        .enumerate()
                        .map(|(i, o)| self.correction(o, &format!("{path}[{i}]")))
                        .collect(),
                )
            }),
            ValueKind::ComponentSeq => self.objects(v).map(|items| {
                Value::Tools(
                    items
                        .into_iter()
                        .enumerate()
                        .map(|(i, o)| self.fields(o, Level::MaskTool, &format!("{path}[{i}]")))
                        .collect(),
                )
            }),
            ValueKind::Settings | ValueKind::Any | ValueKind::PerFormat { .. } => {
                Some(Value::Opaque(Opaque::Json(v.clone())))
            }
        };
        if value.is_none() {
            self.warn(
                path,
                key,
                Some(v),
                WarningKind::WrongType {
                    expected: describe(kind),
                },
            );
        }
        value
    }

    /// `Some` when `v` is an array of objects only; empty tables in it are
    /// no elements (JSON.lua writes an empty Lua table as `[]`).
    fn objects<'v>(&self, v: &'v J) -> Option<Vec<&'v Map<String, J>>> {
        v.as_array()?
            .iter()
            .filter(|x| !is_empty_table(x))
            .map(J::as_object)
            .collect()
    }

    fn scalar(&mut self, spec: &KeySpec, v: &J, path: &str, key: &str) -> Option<Value> {
        let input = match v {
            J::Bool(b) => ScalarIn::Bool(*b),
            J::Number(n) => ScalarIn::Num(n.as_f64().and_then(|f| Finite::new(f).ok())?),
            // The writer's `EnumAs::String` form; plain digits only.
            J::String(t) if spec.is_mask_enum() && is_integer_text(t) => {
                ScalarIn::Num(Finite::from_i64(t.parse().ok()?))
            }
            _ => return None,
        };
        reader::scalar(spec, input, path, key, || snippet(v), &mut self.warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Combine, FileKind, MaskTool, Semantic};
    use crate::registry::ProcessVersion;
    use serde_json::json;

    fn read(v: J) -> (DevelopSettings, Vec<ParseWarning>) {
        from_lua_value(&v, FileKindHint::Unknown).unwrap()
    }

    fn kinds(w: &[ParseWarning]) -> Vec<&'static str> {
        w.iter().map(|w| w.kind.name()).collect()
    }

    #[test]
    fn top_level_empty_array_is_empty_settings() {
        let (s, w) = from_lua_value(&json!([]), FileKindHint::Raw).unwrap();
        assert!(s.is_empty());
        assert!(w.is_empty());
        assert_eq!(s.file_kind, Some(FileKind::Raw));
    }

    #[test]
    fn other_top_levels_are_errors() {
        for v in [json!([1]), json!("x"), json!(1), json!(null), json!(true)] {
            assert!(matches!(
                from_lua_value(&v, FileKindHint::Unknown),
                Err(ParseError::NotATable { .. })
            ));
        }
        assert!(matches!(
            from_lua_str("{", FileKindHint::Unknown),
            Err(ParseError::Json(_))
        ));
    }

    #[test]
    fn oversized_text_is_refused() {
        let text = format!(
            "{{\"CameraProfile\":\"{}\"}}",
            "x".repeat(MAX_LUA_JSON_BYTES)
        );
        assert!(matches!(
            from_lua_str(&text, FileKindHint::Unknown),
            Err(ParseError::TooLarge { .. })
        ));
    }

    #[test]
    fn empty_tables_are_absent_keys_without_warning() {
        let (s, w) = read(json!({
            "LensBlur": [], "FilterList": {}, "PointColors": [], "AILook": [],
            "MaskGroupBasedCorrections": [], "Look": {}
        }));
        assert!(s.is_empty(), "{s:?}");
        assert!(w.is_empty());
    }

    #[test]
    fn integers_are_fine_for_reals_and_non_integers_are_rounded_on_ints() {
        let (s, w) = read(json!({"Exposure2012": 1, "Contrast2012": 12.5}));
        assert_eq!(
            s.get_by_name("Exposure2012"),
            Some(&Value::Real(Finite::new_const(1.0)))
        );
        assert_eq!(s.get_by_name("Contrast2012"), Some(&Value::Int(12)));
        assert_eq!(kinds(&w), ["NonIntegerForIntKey"]);
        assert_eq!(w[0].path, "Contrast2012");
    }

    #[test]
    fn bools_and_flags_are_normalised() {
        let (s, w) = read(json!({
            "ConvertToGrayscale": 1, "AutoLateralCA": true, "LensProfileEnable": 0
        }));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(
            s.get_by_name("ConvertToGrayscale"),
            Some(&Value::Bool(true))
        );
        assert_eq!(s.get_by_name("AutoLateralCA"), Some(&Value::Int(1)));
        assert_eq!(s.get_by_name("LensProfileEnable"), Some(&Value::Int(0)));
    }

    #[test]
    fn wrong_shapes_are_kept_verbatim_with_a_warning() {
        let (s, w) = read(json!({
            "ToneCurvePV2012": [0, 0, null, 255],
            "Exposure2012": "abc",
            "LensProfileEnable": 2
        }));
        assert_eq!(kinds(&w), ["WrongType", "WrongType", "WrongType"]);
        assert_eq!(s.value_count(), 0);
        assert_eq!(s.opaque.len(), 3);
        assert!(s
            .opaque
            .iter()
            .any(|o| o.name == "ToneCurvePV2012"
                && o.value == Opaque::Json(json!([0, 0, null, 255]))));
    }

    #[test]
    fn unknown_keys_are_kept_and_warned_about_never_an_error() {
        let (s, w) = read(json!({"BrandNewSlider": 5, "Nested": {"a": [1, null]}}));
        assert_eq!(kinds(&w), ["UnknownKey", "UnknownKey"]);
        assert_eq!(s.opaque.len(), 2);
    }

    #[test]
    fn pattern_keys_are_kept_without_a_warning() {
        let (s, w) = read(json!({"UprightTransform_0": "1 0 0 0 1 0 0 0 1"}));
        assert!(w.is_empty());
        assert_eq!(s.opaque[0].name, "UprightTransform_0");
    }

    #[test]
    fn warning_paths_point_into_corrections() {
        let (s, w) = read(json!({"MaskGroupBasedCorrections": [
            {"What": "Correction", "CorrectionMasks": [
                {"What": "Mask/Image", "MaskSubType": 1, "MaskBlendMode": 0,
                 "MaskValue": 1, "MaskInverted": false, "Mystery": 1}
            ]},
            {"What": "Correction", "CorrectionMasks": [
                {"What": "Mask/Image", "MaskSubType": 2, "MaskBlendMode": 0,
                 "MaskValue": 1, "MaskInverted": false, "Mystery": 2}
            ]}
        ]}));
        assert_eq!(kinds(&w), ["UnknownKey", "UnknownKey"]);
        assert_eq!(
            w[1].path,
            "MaskGroupBasedCorrections[1].CorrectionMasks[0].Mystery"
        );
        assert_eq!(s.corrections.len(), 2);
        assert_eq!(
            s.corrections[1].masks[0].tool,
            MaskTool::Semantic(Semantic::Sky)
        );
        assert_eq!(
            s.corrections[1].masks[0].combine,
            Combine::Add { inverted: false }
        );
    }

    #[test]
    fn masks_are_never_identified_by_name() {
        let (s, _) = read(json!({"MaskGroupBasedCorrections": [
            {"CorrectionName": "Himmel", "CorrectionMasks": [
                {"What": "Mask/Image", "MaskSubType": 1, "MaskName": "Himmel",
                 "MaskBlendMode": 0, "MaskValue": 1, "MaskInverted": false}
            ]}
        ]}));
        assert_eq!(
            s.corrections[0].masks[0].tool,
            MaskTool::Semantic(Semantic::Subject)
        );
    }

    #[test]
    fn unrecognised_combination_is_kept_with_a_warning() {
        let (s, w) = read(json!({"MaskGroupBasedCorrections": [
            {"CorrectionMasks": [
                {"What": "Mask/Image", "MaskSubType": 1, "MaskBlendMode": 1,
                 "MaskValue": 1, "MaskInverted": false}
            ]}
        ]}));
        assert_eq!(kinds(&w), ["UnrecognisedMaskCombine"]);
        let m = &s.corrections[0].masks[0];
        assert_eq!(m.combine, Combine::Unrecognised);
        assert!(m.extra.get(Level::MaskTool, "MaskBlendMode").is_some());
    }

    #[test]
    fn file_kind_comes_from_the_white_balance_family() {
        let v = json!({"IncrementalTemperature": 5, "IncrementalTint": 0});
        let (s, w) = from_lua_value(&v, FileKindHint::Raw).unwrap();
        assert_eq!(s.file_kind, Some(FileKind::NonRaw));
        assert_eq!(kinds(&w), ["FileKindMismatch"]);
        let (s, w) = from_lua_value(&v, FileKindHint::NonRaw).unwrap();
        assert_eq!(s.file_kind, Some(FileKind::NonRaw));
        assert!(w.is_empty());
        let (s, w) = from_lua_value(&json!({"Exposure2012": 0}), FileKindHint::Raw).unwrap();
        assert_eq!(
            s.file_kind,
            Some(FileKind::Raw),
            "hint when the keys say nothing"
        );
        assert!(w.is_empty());
        let (s, w) = read(json!({"Temperature": 5000, "IncrementalTint": 3}));
        assert_eq!(s.file_kind, None);
        assert_eq!(kinds(&w), ["ConflictingFileKind"]);
        assert_eq!(w[0].path, "Temperature/IncrementalTint");
    }

    #[test]
    fn the_raw_family_read_with_a_non_raw_hint_warns_and_the_family_wins() {
        let v = json!({"Temperature": 5600, "Tint": 4});
        let (s, w) = from_lua_value(&v, FileKindHint::NonRaw).unwrap();
        assert_eq!(s.file_kind, Some(FileKind::Raw));
        assert_eq!(kinds(&w), ["FileKindMismatch"]);
        assert_eq!(w[0].path, "Temperature");
    }

    #[test]
    fn opaque_entries_and_warnings_come_out_in_key_order() {
        // `serde_json` keeps input order with `preserve_order` (on in the
        // workspace build) and sorts without it; the reader must not care.
        let (s, w) = read(json!({
            "ZetaUnknown": 1, "UprightTransform_0": "1 0 0", "AlphaUnknown": 2,
            "Exposure2012": "x", "Contrast2012": 1.5
        }));
        let names: Vec<&str> = s.opaque.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "AlphaUnknown",
                "Exposure2012",
                "UprightTransform_0",
                "ZetaUnknown"
            ]
        );
        let keys: Vec<&str> = w.iter().map(|w| w.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "AlphaUnknown",
                "Contrast2012",
                "Exposure2012",
                "ZetaUnknown"
            ]
        );
    }

    #[test]
    fn nested_empty_tables_are_absent_keys_without_warning() {
        let (s, w) = read(json!({
            "LensBlur": {"Active": true, "FocalRange": []},
            "Look": {"Name": "Synthetic Profile", "Group": {}},
            "MaskGroupBasedCorrections": [
                {"What": "Correction", "LocalExposure2012": 0.1, "CorrectionName": [],
                 "CorrectionMasks": [
                    {"What": "Mask/Image", "MaskSubType": 1, "MaskBlendMode": 0,
                     "MaskValue": 1, "MaskInverted": false, "Gesture": []}
                ]}
            ]
        }));
        assert!(w.is_empty(), "{w:?}");
        let Some(Value::Struct(blur)) = s.get_by_name("LensBlur") else {
            panic!("LensBlur");
        };
        assert!(blur.get("FocalRange").is_none());
        assert!(blur.fields.opaque.is_empty());
        let look = s.look.as_ref().unwrap();
        assert!(look.rest.fields.is_empty(), "{look:?}");
        let c = &s.corrections[0];
        assert_eq!(c.name, None);
        assert!(c.extra.get(Level::Correction, "CorrectionName").is_none());
        let m = &c.masks[0];
        assert_eq!(m.tool, MaskTool::Semantic(Semantic::Subject), "no gesture");
        assert!(m.extra.is_empty());
    }

    #[test]
    fn empty_tables_in_lists_of_objects_are_no_elements() {
        let (s, w) = read(json!({"MaskGroupBasedCorrections": [
            [],
            {"What": "Correction", "CorrectionMasks": [
                [],
                {"What": "Mask/Image", "MaskSubType": 2, "MaskBlendMode": 0,
                 "MaskValue": 1, "MaskInverted": false}
            ]}
        ]}));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.corrections.len(), 1);
        assert_eq!(s.corrections[0].masks.len(), 1);
        assert_eq!(
            s.corrections[0].masks[0].tool,
            MaskTool::Semantic(Semantic::Sky)
        );
    }

    #[test]
    fn struct_bools_and_mask_enums_accept_the_lua_forms() {
        let (s, w) = read(json!({
            "LensBlur": {"Active": 1},
            "Look": {"Name": "Synthetic Profile", "isAdobeAdaptive": 0},
            "MaskGroupBasedCorrections": [{"CorrectionMasks": [
                {"What": "Mask/Image", "MaskSubType": 1.5, "MaskBlendMode": 0,
                 "MaskValue": 1, "MaskInverted": false}
            ]}]
        }));
        let Some(Value::Struct(blur)) = s.get_by_name("LensBlur") else {
            panic!("LensBlur");
        };
        assert_eq!(blur.get("Active"), Some(&Value::Bool(true)));
        assert_eq!(
            s.look.as_ref().unwrap().rest.get("isAdobeAdaptive"),
            Some(&Value::Bool(false))
        );
        // 1.5 rounds half-to-even to 2 (Sky), with a warning.
        assert_eq!(kinds(&w), ["NonIntegerForIntKey"]);
        assert_eq!(
            w[0].path,
            "MaskGroupBasedCorrections[0].CorrectionMasks[0].MaskSubType"
        );
        assert_eq!(
            s.corrections[0].masks[0].tool,
            MaskTool::Semantic(Semantic::Sky)
        );
    }

    #[test]
    fn an_empty_camera_restriction_stays_in_the_look() {
        let (s, w) = read(json!({"Look": {"Name": "x", "CameraModelRestriction": ""}}));
        assert!(w.is_empty());
        let look = s.look.unwrap();
        assert_eq!(look.camera_restriction, None);
        assert_eq!(
            look.rest.get("CameraModelRestriction"),
            Some(&Value::Str(String::new()))
        );
        let (s, _) =
            read(json!({"Look": {"Name": "x", "CameraModelRestriction": "Synthetic Camera"}}));
        let look = s.look.unwrap();
        assert_eq!(look.camera_restriction.as_deref(), Some("Synthetic Camera"));
        assert!(look.rest.get("CameraModelRestriction").is_none());
    }

    #[test]
    fn integer_strings_are_read_on_mask_enums_only() {
        // The Lua writer's `EnumAs::String` form reads back as numbers.
        let (s, w) = read(json!({"MaskGroupBasedCorrections": [{
            "What": "Correction", "CorrectionAmount": 1, "CorrectionActive": true,
            "LocalExposure2012": 0.1,
            "CorrectionMasks": [
                {"What": "Mask/Image", "MaskActive": true, "MaskBlendMode": "0",
                 "MaskInverted": false, "MaskValue": 1, "MaskSubType": "3",
                 "MaskSubCategoryID": "5", "ErrorReason": "0"},
                {"What": "Mask/RangeMask", "MaskActive": true, "MaskBlendMode": "1",
                 "MaskInverted": true, "MaskValue": 0,
                 "CorrectionRangeMask": {"Type": "2", "Invert": true, "Version": 3,
                    "SampleType": "0", "LumRange": "0.5 0.7 1 1"}}
            ]
        }]}));
        assert!(w.is_empty(), "{w:?}");
        let masks = &s.corrections[0].masks;
        assert_eq!(masks[0].tool, MaskTool::Semantic(Semantic::PeoplePart(5)));
        assert_eq!(masks[1].combine, Combine::Intersect);
        assert!(matches!(masks[1].tool, MaskTool::LuminanceRange(_)));
        // Not a mask enum, or not an integer: the wrong type, as before.
        let (_, w) = read(json!({"PostCropVignetteStyle": "1"}));
        assert_eq!(kinds(&w), ["WrongType"]);
        for bad in ["1.0", " 1", "x", "", "+1"] {
            let (_, w) = read(json!({"MaskGroupBasedCorrections": [{
                "What": "Correction",
                "CorrectionMasks": [{"What": "Mask/Image", "MaskSubType": bad}]
            }]}));
            assert!(kinds(&w).contains(&"WrongType"), "{bad:?}: {w:?}");
        }
    }

    #[test]
    fn old_process_versions_are_flagged() {
        let (s, w) = read(json!({"ProcessVersion": "5.7"}));
        assert_eq!(s.process_version(), Some(ProcessVersion::PV2010));
        assert_eq!(kinds(&w), ["UnsupportedProcessVersion"]);
        let (_, w) = read(json!({"ProcessVersion": "6.7"}));
        assert!(w.is_empty());
        let (_, w) = read(json!({"ProcessVersion": "abc"}));
        assert_eq!(kinds(&w), ["InvalidProcessVersion"]);
    }
}
