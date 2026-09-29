//! The Lua/JSON reader over every fixture in `testdata/develop/lua/`.
//!
//! Generated fixtures (listed in `manifest.json`, scrubbed training rows,
//! all raw files) must parse without any warning, and every shape the
//! manifest says a file covers must be recognised by the model. Hand-written
//! fixtures (`hand_*.json`) cover the shapes the training rows lack; each
//! has its own test with the warnings it must produce. No fixture may
//! produce a `ParseError` or an `UnknownKey` warning.

use std::collections::BTreeSet;
use std::path::PathBuf;

use lrg_develop::lua::from_lua_str;
use lrg_develop::model::{
    Combine, Correction, DevelopSettings, Fields, FileKind, MaskComponent, MaskTool, Opaque,
    Semantic, Value, WbFamily, WbMode,
};
use lrg_develop::model::{SkipReason, Target};
use lrg_develop::registry::{KeyId, Level, Policy, ProcessVersion, StructKind};
use lrg_develop::{FileKindHint, ParseWarning, WarningKind};
use serde_json::Value as J;

fn lua_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/develop/lua")
}

fn read_text(name: &str) -> String {
    std::fs::read_to_string(lua_dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn manifest() -> J {
    serde_json::from_str(&read_text("manifest.json")).expect("manifest.json is JSON")
}

/// Every fixture file name (not the manifest), sorted.
fn fixture_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(lua_dir())
        .expect("lua fixture folder")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json") && n != "manifest.json")
        .collect();
    names.sort();
    names
}

/// Generated fixtures come from raw training rows; hand-written ones say
/// nothing about the file kind unless a test passes a hint.
fn default_hint(name: &str) -> FileKindHint {
    if name.starts_with("hand_") {
        FileKindHint::Unknown
    } else {
        FileKindHint::Raw
    }
}

fn parse(name: &str, hint: FileKindHint) -> (DevelopSettings, Vec<ParseWarning>) {
    from_lua_str(&read_text(name), hint).unwrap_or_else(|e| panic!("{name}: ParseError {e}"))
}

fn kinds(w: &[ParseWarning]) -> Vec<&'static str> {
    w.iter().map(|w| w.kind.name()).collect()
}

#[test]
fn every_fixture_parses_without_errors_or_unknown_keys() {
    let names = fixture_names();
    assert!(names.len() >= 17, "fixtures missing: {names:?}");
    for name in names {
        let (_, warnings) = parse(&name, default_hint(&name));
        let unknown: Vec<&str> = warnings
            .iter()
            .filter(|w| matches!(w.kind, WarningKind::UnknownKey))
            .map(|w| w.path.as_str())
            .collect();
        assert!(unknown.is_empty(), "{name}: unknown keys {unknown:?}");
    }
}

#[test]
fn generated_fixtures_parse_without_any_warning() {
    for name in fixture_names().iter().filter(|n| !n.starts_with("hand_")) {
        let (_, warnings) = parse(name, FileKindHint::Raw);
        let text: Vec<String> = warnings.iter().map(ToString::to_string).collect();
        assert!(text.is_empty(), "{name}: {text:#?}");
    }
}

fn components(s: &DevelopSettings) -> impl Iterator<Item = &MaskComponent> {
    s.corrections.iter().flat_map(|c| &c.masks)
}

fn any_tool(s: &DevelopSettings, pred: impl Fn(&MaskComponent) -> bool) -> bool {
    components(s).any(pred)
}

fn opaque_what(m: &MaskComponent, what: &str) -> bool {
    matches!(&m.tool, MaskTool::Opaque { what: Some(w) } if w == what)
}

fn struct_of<'a>(s: &'a DevelopSettings, name: &str) -> Option<&'a lrg_develop::model::Struct> {
    match s.get_by_name(name)? {
        Value::Struct(st) => Some(st),
        _ => None,
    }
}

fn struct_list_len(s: &DevelopSettings, name: &str) -> usize {
    match s.get_by_name(name) {
        Some(Value::StructList(items)) => items.len(),
        _ => 0,
    }
}

fn is_fraction(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Real(r)) if r.get().fract() != 0.0)
}

/// Asserts that `shape` (a manifest `covers` entry) is recognised in `s`.
fn check_shape(file: &str, shape: &str, raw: &J, text: &str, s: &DevelopSettings) {
    let ok = match shape {
        "top_level_empty" => *raw == J::Array(vec![]) && s.is_empty(),
        "pv_11_0" => s.process_version() == Some(ProcessVersion::V5),
        "pv_15_4" => s.process_version() == Some(ProcessVersion::V6),
        "look" => s.look.as_ref().is_some_and(|l| {
            l.uuid.is_some()
                && l.name.is_some()
                && matches!(l.rest.get("Parameters"), None | Some(Value::Opaque(Opaque::Json(_))))
        }),
        "no_look" => s.look.is_none() && raw.get("Look").is_none(),
        "look_camera_restricted" => s
            .look
            .as_ref()
            .is_some_and(|l| l.camera_restriction.is_some()),
        "look_adaptive" => s
            .look
            .as_ref()
            .is_some_and(|l| matches!(l.rest.get("isAdobeAdaptive"), Some(Value::Bool(_)))),
        "look_amount_float" => s
            .look
            .as_ref()
            .and_then(|l| l.amount)
            .is_some_and(|a| a.get().fract() != 0.0),
        "lensblur_empty" => raw["LensBlur"] == J::Array(vec![]) && s.get_by_name("LensBlur").is_none(),
        "lensblur_object" => struct_of(s, "LensBlur").is_some_and(|b| {
            b.kind == StructKind::LensBlur && matches!(b.get("Active"), Some(Value::Bool(_)))
        }),
        "filterlist_empty" => {
            raw["FilterList"] == J::Array(vec![]) && s.get_by_name("FilterList").is_none()
        }
        "filterlist_filters" => struct_of(s, "FilterList").is_some_and(|f| {
            f.kind == StructKind::FilterList && f.fields.opaque.iter().any(|o| o.name == "Filters")
        }),
        "ailook_object" => struct_of(s, "AILook").is_some_and(|a| !a.fields.is_empty()),
        "point_colors" => struct_list_len(s, "PointColors") > 0,
        "retouch_areas" => match s.get_by_name("RetouchAreas") {
            Some(Value::StructList(items)) => items
                .iter()
                .any(|a| a.fields.opaque.iter().any(|o| o.name.starts_with("pm_"))),
            _ => false,
        },
        "remove_areas" => struct_list_len(s, "RemoveAreas") > 0,
        "retouch_info" => struct_list_len(s, "RetouchInfo") > 0,
        "upright_transform" => s
            .opaque
            .iter()
            .any(|o| o.name.starts_with("UprightTransform_")),
        "depth_map_info" => struct_of(s, "DepthMapInfo").is_some(),
        "wb_auto" => {
            s.wb().is_some_and(|wb| wb.mode == WbMode::Auto)
                && s.get_by_name("AutoWhiteVersion").is_some()
        }
        "grain_seed" => s.get_by_name("GrainSeed") == Some(&Value::Int(4_000_000_001)),
        "perspective_rotate_float" => is_fraction(s.get_by_name("PerspectiveRotate")),
        "number_exponent" => {
            // Every global key whose token is in exponent form holds that
            // number in the model.
            let token =
                regex::Regex::new(r#""([A-Za-z0-9_]+)"\s*:\s*(-?[0-9.]+[eE][-+]?[0-9]+)"#).unwrap();
            let mut checked = 0;
            for c in token.captures_iter(text) {
                if let Some(v) = s.get_by_name(&c[1]) {
                    let want: f64 = c[2].parse().unwrap();
                    assert!(
                        matches!(v, Value::Real(r) if r.get() == want),
                        "{file}: {} is {v:?}, the token says {}",
                        &c[1],
                        &c[2]
                    );
                    checked += 1;
                }
            }
            checked > 0
        }
        "local_curve" => s.corrections.iter().any(|c| {
            matches!(c.local_value("MainCurve"), Some(Value::Curve(points)) if points.len() >= 2)
        }),
        "local_legacy_float" => s.corrections.iter().any(|c| {
            ["LocalExposure", "LocalContrast", "LocalClarity"]
                .iter()
                .any(|k| is_fraction(c.extra.get(Level::Correction, k)))
        }),
        "mask_ai_subject" => any_tool(s, |m| m.tool == MaskTool::Semantic(Semantic::Subject)),
        "mask_ai_sky" => any_tool(s, |m| m.tool == MaskTool::Semantic(Semantic::Sky)),
        "mask_ai_background" => any_tool(s, |m| m.tool == MaskTool::Semantic(Semantic::Background)),
        "mask_ai_people_part_all" => {
            any_tool(s, |m| matches!(m.tool, MaskTool::Semantic(Semantic::PeoplePart(_))))
        }
        "mask_ai_people_part_person" => any_tool(s, |m| {
            matches!(m.tool, MaskTool::Semantic(Semantic::PersonPartAt { .. }))
        }),
        "mask_ai_landscape" => {
            any_tool(s, |m| matches!(m.tool, MaskTool::Semantic(Semantic::Landscape(_))))
        }
        "mask_select_object_polygon" => any_tool(s, |m| {
            opaque_what(m, "Mask/Image") && m.extra.get(Level::MaskTool, "Gesture").is_some()
        }),
        "mask_linear_gradient" => any_tool(s, |m| matches!(m.tool, MaskTool::Linear(_))),
        "mask_radial_gradient" => any_tool(s, |m| {
            matches!(m.tool, MaskTool::Radial(_)) || opaque_what(m, "Mask/CircularGradient")
        }),
        "mask_brush_aggregate" => any_tool(s, |m| {
            opaque_what(m, "Mask/Aggregate")
                && matches!(m.extra.get(Level::MaskTool, "Masks"), Some(Value::Tools(t)) if !t.is_empty())
        }),
        "mask_range_luminance" => any_tool(s, |m| matches!(m.tool, MaskTool::LuminanceRange(_))),
        "mask_range_color" => any_tool(s, |m| opaque_what(m, "Mask/RangeMask")),
        "mask_range_color_area" => any_tool(s, |m| {
            opaque_what(m, "Mask/RangeMask")
                && matches!(
                    m.extra.get(Level::MaskTool, "CorrectionRangeMask"),
                    Some(Value::Struct(crm)) if matches!(crm.get("AreaModels"), Some(Value::StructList(_)))
                )
        }),
        "mask_intersect" => any_tool(s, |m| m.combine == Combine::Intersect),
        "mask_subtract" => any_tool(s, |m| m.combine == Combine::Subtract),
        other => panic!("{file}: shape {other:?} has no check in lua_read.rs; add one"),
    };
    assert!(ok, "{file}: shape {shape} not recognised");
}

#[test]
fn every_manifest_shape_is_recognised() {
    let manifest = manifest();
    let files = manifest["files"].as_array().expect("files");
    let mut seen = BTreeSet::new();
    for entry in files {
        let file = entry["file"].as_str().expect("file name");
        let text = read_text(file);
        let raw: J = serde_json::from_str(&text).unwrap();
        let (s, _) = parse(file, FileKindHint::Raw);
        for shape in entry["covers"].as_array().expect("covers") {
            let shape = shape.as_str().unwrap();
            check_shape(file, shape, &raw, &text, &s);
            seen.insert(shape.to_owned());
        }
    }
    // The reader rules (Dev-Develop-Model, "Reading the Lua form") need at
    // least these shapes.
    for must in [
        "lensblur_empty",
        "lensblur_object",
        "filterlist_empty",
        "filterlist_filters",
        "pv_11_0",
        "pv_15_4",
        "top_level_empty",
        "mask_intersect",
        "mask_subtract",
        "look",
    ] {
        assert!(seen.contains(must), "no fixture covers {must}");
    }
}

#[test]
fn every_generated_fixture_is_in_the_manifest_and_vice_versa() {
    let listed: BTreeSet<String> = manifest()["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["file"].as_str().unwrap().to_owned())
        .collect();
    let on_disk: BTreeSet<String> = fixture_names()
        .into_iter()
        .filter(|n| !n.starts_with("hand_"))
        .collect();
    assert_eq!(listed, on_disk);
}

#[test]
fn raw_rows_are_raw_and_every_correction_has_a_recognised_combination() {
    for name in fixture_names().iter().filter(|n| !n.starts_with("hand_")) {
        let (s, _) = parse(name, FileKindHint::Unknown);
        if s.is_empty() {
            continue;
        }
        assert_eq!(s.file_kind, Some(FileKind::Raw), "{name}: family says raw");
        assert_eq!(s.wb().map(|w| w.family), Some(WbFamily::Raw), "{name}");
        for c in &s.corrections {
            assert!(!c.masks.is_empty(), "{name}: correction without masks");
            assert!(
                matches!(c.masks[0].combine, Combine::Add { .. }),
                "{name}: first component must add"
            );
            assert!(c.masks.iter().all(|m| m.combine != Combine::Unrecognised));
        }
    }
}

/// Asserts that everything below a kept value may go into a shared preset:
/// every nested key LEARN, LEARN† or (bookkeeping) META, no opaque entry.
struct PresetWalk<'a> {
    file: &'a str,
}

impl PresetWalk<'_> {
    fn key(&self, id: KeyId, v: &Value, path: &str) {
        let spec = id.spec();
        let path = format!("{path}.{}", spec.name);
        assert!(
            matches!(
                spec.policy,
                Policy::Learn | Policy::LearnGated(_) | Policy::Meta
            ),
            "{}: {path} ({}) kept",
            self.file,
            spec.policy.class_name()
        );
        self.value(v, &path);
    }

    fn value(&self, v: &Value, path: &str) {
        match v {
            Value::Struct(st) => self.fields(&st.fields, path),
            Value::StructList(items) => items.iter().for_each(|st| self.fields(&st.fields, path)),
            Value::Tools(items) => items.iter().for_each(|f| self.fields(f, path)),
            Value::Corrections(cs) => cs.iter().for_each(|c| self.correction(c, path)),
            _ => {}
        }
    }

    fn fields(&self, f: &Fields, path: &str) {
        let opaque: Vec<&str> = f.opaque.iter().map(|o| o.name.as_str()).collect();
        assert!(opaque.is_empty(), "{}: {path} keeps {opaque:?}", self.file);
        for (id, v) in &f.values {
            self.key(*id, v, path);
        }
    }

    fn correction(&self, c: &Correction, path: &str) {
        assert!(
            matches!(
                c.policy(),
                Policy::Learn | Policy::LearnGated(_) | Policy::Meta
            ),
            "{}: correction kept at {path}",
            self.file
        );
        for (id, v) in &c.local {
            self.key(*id, v, path);
        }
        self.fields(&c.extra, path);
        for m in &c.masks {
            self.fields(&m.extra, path);
            if let MaskTool::LuminanceRange(lr) = &m.tool {
                self.fields(&lr.rest.fields, &format!("{path}.CorrectionRangeMask"));
            }
        }
    }
}

#[test]
fn a_mixed_preset_from_any_fixture_holds_only_shareable_content() {
    for name in fixture_names() {
        let (s, _) = parse(&name, default_hint(&name));
        let (out, skipped) = s.filtered(&Target::Preset {
            mixed_file_kinds: true,
        });
        let walk = PresetWalk { file: &name };
        for (id, v) in out.values() {
            let spec = id.spec();
            assert_ne!(
                spec.policy,
                Policy::Meta,
                "{name}: {} is the writer's",
                spec.name
            );
            assert_ne!(spec.name, "Temperature", "{name}");
            walk.key(id, v, "");
        }
        assert!(out.opaque.is_empty(), "{name}");
        for c in &out.corrections {
            walk.correction(c, "MaskGroupBasedCorrections[]");
        }
        // Nothing disappears silently: every correction the preset lost is
        // reported.
        let dropped = s.corrections.len() - out.corrections.len();
        let reported = skipped
            .iter()
            .filter(|k| {
                k.path.starts_with("MaskGroupBasedCorrections[")
                    && !k.path.contains("].")
                    && matches!(k.reason, SkipReason::Policy(_))
            })
            .count();
        assert_eq!(dropped, reported, "{name}");
        let (all, none) = s.filtered(&Target::TestRoundTrip);
        assert_eq!(all, s);
        assert!(none.is_empty());
    }
}

// ---- hand-written shapes -------------------------------------------------

#[test]
fn hand_bool_as_int_normalises_to_the_key_kind() {
    let (s, w) = parse("hand_bool_as_int.json", FileKindHint::Raw);
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(
        s.get_by_name("ConvertToGrayscale"),
        Some(&Value::Bool(true))
    );
    assert_eq!(s.get_by_name("EnableDetail"), Some(&Value::Bool(false)));
    assert_eq!(s.get_by_name("AutoLateralCA"), Some(&Value::Int(1)));
    assert_eq!(s.get_by_name("LensProfileEnable"), Some(&Value::Int(0)));
    let c = &s.corrections[0];
    assert_eq!(c.active, Some(true));
    let m = &c.masks[0];
    assert_eq!(m.active, Some(true));
    assert_eq!(m.combine, Combine::Add { inverted: false });
    assert_eq!(m.tool, MaskTool::Semantic(Semantic::Sky));
}

#[test]
fn hand_non_integer_on_int_key_is_rounded_with_a_warning() {
    let (s, w) = parse("hand_non_integer_on_int_key.json", FileKindHint::Raw);
    let mut paths: Vec<&str> = w.iter().map(|w| w.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["Contrast2012", "Temperature", "Vibrance"]);
    assert!(kinds(&w).iter().all(|k| *k == "NonIntegerForIntKey"));
    assert_eq!(
        s.get_by_name("Contrast2012"),
        Some(&Value::Int(12)),
        "half to even"
    );
    assert_eq!(
        s.get_by_name("Vibrance"),
        Some(&Value::Int(14)),
        "half to even"
    );
    assert_eq!(s.get_by_name("Temperature"), Some(&Value::Int(5650)));
    assert!(matches!(
        s.get_by_name("Exposure2012"),
        Some(Value::Real(_))
    ));
}

#[test]
fn hand_null_in_array_is_kept_verbatim_with_a_warning() {
    let (s, w) = parse("hand_null_in_array.json", FileKindHint::Unknown);
    let mut paths: Vec<&str> = w.iter().map(|w| w.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["RedEyeInfo", "ToneCurvePV2012"]);
    assert!(kinds(&w).iter().all(|k| *k == "WrongType"));
    assert!(s.get_by_name("ToneCurvePV2012").is_none());
    let kept = s
        .opaque
        .iter()
        .find(|o| o.name == "ToneCurvePV2012")
        .unwrap();
    assert_eq!(
        kept.value,
        Opaque::Json(serde_json::json!([0, 0, null, 128, 255, 255]))
    );
    assert!(matches!(s.get_by_name("ToneCurvePV2012Blue"), Some(Value::Curve(p)) if p.len() == 2));
}

#[test]
fn hand_mixed_array_is_kept_verbatim_with_a_warning() {
    let (s, w) = parse("hand_mixed_array.json", FileKindHint::Unknown);
    let mut paths: Vec<&str> = w.iter().map(|w| w.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        [
            "MaskGroupBasedCorrections[0].MainCurve",
            "RetouchInfo",
            "ToneCurvePV2012Red"
        ]
    );
    assert!(kinds(&w).iter().all(|k| *k == "WrongType"));
    assert_eq!(s.opaque.len(), 2);
    let c = &s.corrections[0];
    assert!(c.local_value("MainCurve").is_none());
    assert!(c.extra.opaque.iter().any(|o| o.name == "MainCurve"));
}

#[test]
fn hand_non_raw_white_balance_uses_the_incremental_family() {
    let (s, w) = parse("hand_non_raw_white_balance.json", FileKindHint::NonRaw);
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(s.file_kind, Some(FileKind::NonRaw));
    let wb = s.wb().unwrap();
    assert_eq!(wb.mode, WbMode::Custom);
    assert_eq!(wb.family, WbFamily::NonRaw);
    assert_eq!(wb.temperature.map(|t| t.get()), Some(12.0));
    assert_eq!(wb.tint.map(|t| t.get()), Some(-4.0));
    // Without a hint the family alone decides, silently.
    let (s, w) = parse("hand_non_raw_white_balance.json", FileKindHint::Unknown);
    assert!(w.is_empty());
    assert_eq!(s.file_kind, Some(FileKind::NonRaw));
}

#[test]
fn white_balance_family_contradicting_is_raw_warns_and_the_family_wins() {
    let (s, w) = parse("hand_non_raw_white_balance.json", FileKindHint::Raw);
    assert_eq!(kinds(&w), ["FileKindMismatch"]);
    assert_eq!(s.file_kind, Some(FileKind::NonRaw));
    assert_eq!(s.wb().unwrap().family, WbFamily::NonRaw);
}

#[test]
fn hand_wb_both_families_is_a_conflict() {
    let (s, w) = parse("hand_wb_both_families.json", FileKindHint::Raw);
    assert_eq!(kinds(&w), ["ConflictingFileKind"]);
    assert_eq!(s.file_kind, Some(FileKind::Raw), "falls back to the hint");
}

#[test]
fn hand_pv_2010_is_read_but_flagged() {
    let (s, w) = parse("hand_pv_2010.json", FileKindHint::Raw);
    assert_eq!(kinds(&w), ["UnsupportedProcessVersion"]);
    assert_eq!(s.process_version(), Some(ProcessVersion::PV2010));
    assert!(s.get_by_name("Exposure").is_some());
    assert!(s.get_by_name("HighlightRecovery").is_some());
}

#[test]
fn top_level_empty_fixture_is_empty_settings() {
    let (s, w) = parse("top_level_empty.json", FileKindHint::Raw);
    assert!(s.is_empty());
    assert!(w.is_empty());
}
