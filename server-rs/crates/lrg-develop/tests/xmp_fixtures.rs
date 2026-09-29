//! The XMP reader over every fixture in `testdata/develop/xmp/`.
//!
//! The fixtures are self-authored (no Adobe files, no excerpts of real
//! sidecars), one per syntax form or develop shape. Every one must parse
//! without a `ParseError`; only `unknown_keys.xmp` may produce an
//! `UnknownKey` warning and only it and `coercion_warnings.xmp` any warning
//! at all. Every fixture is listed in [`FIXTURES`], so a new file needs a
//! test, and uses only keys Lightroom writes to XMP (no Lua-only key).

use std::path::PathBuf;

use lrg_develop::model::{
    Combine, Correction, DevelopSettings, Fields, FileKind, Finite, MaskTool, Opaque, Semantic,
    SensorPoint, Struct, Value, WbMode, XmpArrayKind, XmpValue,
};
use lrg_develop::registry::{KeyId, Level, Presence, ProcessVersion, StructKind};
use lrg_develop::xmp::{parse, XmpDocument, XmpKind};
use lrg_develop::WarningKind;

/// Every fixture file, in name order.
const FIXTURES: &[&str] = &[
    "alt_second_language.xmp",
    "bag.xmp",
    "bom.xmp",
    "coercion_warnings.xmp",
    "curves.xmp",
    "element_form.xmp",
    "look_stub.xmp",
    "mask_brush.xmp",
    "mask_combine.xmp",
    "mask_linear.xmp",
    "mask_radial.xmp",
    "mask_range.xmp",
    "masks_ai.xmp",
    "multiple_descriptions.xmp",
    "not_develop.xmp",
    "number_formats.xmp",
    "parse_type_resource.xmp",
    "preset_header.xmp",
    "preset_record.xmp",
    "profile.xmp",
    "snapshot_crss.xmp",
    "unknown_keys.xmp",
];

/// The fixtures that exist to produce warnings.
const WITH_WARNINGS: &[&str] = &["coercion_warnings.xmp", "unknown_keys.xmp"];

fn xmp_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/develop/xmp")
}

fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(xmp_dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn read(name: &str) -> XmpDocument {
    parse(&bytes(name)).unwrap_or_else(|e| panic!("{name}: ParseError {e}"))
}

fn warnings(d: &XmpDocument) -> Vec<(String, &'static str)> {
    d.warnings
        .iter()
        .map(|w| (w.path.clone(), w.kind.name()))
        .collect()
}

fn real(x: f64) -> Value {
    Value::Real(Finite::new_const(x))
}

fn f(x: f64) -> Finite {
    Finite::new_const(x)
}

fn points(p: &[(f64, f64)]) -> Value {
    Value::Curve(p.iter().map(|&(x, y)| (f(x), f(y))).collect())
}

fn struct_of<'a>(s: &'a DevelopSettings, name: &str) -> &'a Struct {
    match s.get_by_name(name) {
        Some(Value::Struct(st)) => st,
        other => panic!("{name}: {other:?}"),
    }
}

fn mask_field<'a>(c: &'a Correction, mask: usize, name: &str) -> Option<&'a Value> {
    c.masks[mask].extra.get(Level::MaskTool, name)
}

// ---- every fixture ---------------------------------------------------------

#[test]
fn the_fixture_list_matches_the_folder() {
    let mut names: Vec<String> = std::fs::read_dir(xmp_dir())
        .expect("xmp fixture folder")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".xmp"))
        .collect();
    names.sort();
    assert_eq!(names, FIXTURES, "add a new fixture to FIXTURES and test it");
}

#[test]
fn every_fixture_parses_and_only_the_warning_fixtures_warn() {
    for name in FIXTURES {
        let d = read(name);
        let unknown: Vec<&str> = d
            .warnings
            .iter()
            .filter(|w| w.kind == WarningKind::UnknownKey)
            .map(|w| w.path.as_str())
            .collect();
        if *name != "unknown_keys.xmp" {
            assert!(unknown.is_empty(), "{name}: unknown keys {unknown:?}");
        }
        if !WITH_WARNINGS.contains(name) {
            assert!(d.warnings.is_empty(), "{name}: {:?}", d.warnings);
        }
        // Only the two fixtures built for it keep a registry key whole.
        let kept_whole = match *name {
            "bag.xmp" => 1,
            "alt_second_language.xmp" => 2,
            _ => 0,
        };
        assert_eq!(d.skipped_subtrees.kept_whole, kept_whole, "{name}");
    }
}

/// Every key id reachable from `v`.
fn value_keys(v: &Value, out: &mut Vec<KeyId>) {
    match v {
        Value::Struct(s) => fields_keys(&s.fields, out),
        Value::StructList(items) => items.iter().for_each(|s| fields_keys(&s.fields, out)),
        Value::Tools(items) => items.iter().for_each(|f| fields_keys(f, out)),
        Value::Corrections(cs) => cs.iter().for_each(|c| correction_keys(c, out)),
        _ => {}
    }
}

fn fields_keys(f: &Fields, out: &mut Vec<KeyId>) {
    for (id, v) in &f.values {
        out.push(*id);
        value_keys(v, out);
    }
}

fn correction_keys(c: &Correction, out: &mut Vec<KeyId>) {
    for (id, v) in &c.local {
        out.push(*id);
        value_keys(v, out);
    }
    fields_keys(&c.extra, out);
    for m in &c.masks {
        fields_keys(&m.extra, out);
    }
}

#[test]
fn fixtures_use_only_keys_lightroom_writes_to_xmp() {
    for name in FIXTURES {
        let d = read(name);
        let mut ids = Vec::new();
        for (id, v) in d.develop.values() {
            ids.push(id);
            value_keys(v, &mut ids);
        }
        d.develop
            .corrections
            .iter()
            .for_each(|c| correction_keys(c, &mut ids));
        if let Some(look) = &d.develop.look {
            fields_keys(&look.rest.fields, &mut ids);
        }
        let lua_only: Vec<&str> = ids
            .iter()
            .map(|id| id.spec())
            .filter(|s| s.presence == Presence::LuaOnly)
            .map(|s| s.name)
            .collect();
        assert!(lua_only.is_empty(), "{name}: Lua-only keys {lua_only:?}");
    }
}

// ---- per form ----------------------------------------------------------------

#[test]
fn preset_header_with_empty_alternatives() {
    let d = read("preset_header.xmp");
    assert_eq!(d.kind, XmpKind::Preset);
    let h = d.header.as_ref().expect("header");
    assert_eq!(h.preset_type.as_deref(), Some("Normal"));
    assert_eq!(
        h.uuid.as_ref().map(|u| u.as_str()),
        Some("00000000000000000000000000000A01")
    );
    assert_eq!(h.name.as_deref(), Some("Synthetic Text"));
    assert_eq!(h.group.as_deref(), Some("Synthetic Group"));
    // Empty `rdf:li` items are empty strings, not missing values.
    assert_eq!(h.short_name.as_deref(), Some(""));
    assert_eq!(h.sort_name.as_deref(), Some(""));
    assert_eq!(h.description.as_deref(), Some(""));
    assert_eq!(h.cluster.as_deref(), Some(""));
    assert_eq!(h.supports_amount, Some(true));
    assert_eq!(h.supports_amount2, Some(true));
    assert_eq!(h.supports_color, Some(true));
    assert_eq!(h.supports_monochrome, Some(true));
    // An empty restriction means "none" and stays in `rest` as it was.
    assert_eq!(h.camera_model_restriction, None);
    assert_eq!(
        h.rest.get(Level::Header, "CameraModelRestriction"),
        Some(&Value::Str(String::new()))
    );
    assert_eq!(
        h.rest.get(Level::Header, "SupportsOutputReferred"),
        Some(&Value::Bool(false))
    );
    assert_eq!(
        h.rest.get(Level::Header, "ShowInQuickActions"),
        Some(&Value::Bool(false))
    );
    let s = &d.develop;
    assert_eq!(s.get_by_name("Exposure2012"), Some(&real(0.5)));
    assert_eq!(s.get_by_name("Contrast2012"), Some(&Value::Int(10)));
    assert_eq!(s.get_by_name("HasSettings"), Some(&Value::Bool(true)));
    assert_eq!(s.get_by_name("Version"), Some(&Value::Str("18.5".into())));
    assert_eq!(s.file_kind, None, "a preset without white balance");
}

#[test]
fn number_formats_are_typed_by_the_registry() {
    let d = read("number_formats.xmp");
    assert_eq!(d.kind, XmpKind::Sidecar);
    let s = &d.develop;
    let cases = [
        ("Exposure2012", real(1.35)),
        ("Tint", Value::Int(6)),
        ("Temperature", Value::Int(5500)),
        ("Contrast2012", Value::Int(-12)),
        ("Whites2012", Value::Int(0)),
        ("Shadows2012", Value::Int(35)),
        ("SharpenRadius", real(1.0)),
        ("PerspectiveRotate", real(-1.5)),
        ("PerspectiveX", real(0.5)),
        ("CropTop", real(0.012345)),
        ("CropLeft", real(0.0)),
        ("CropRight", real(1.0)),
        ("CropAngle", real(-0.8)),
        ("CompatibleVersion", Value::Int(251_920_384)),
        ("ConvertToGrayscale", Value::Bool(false)),
        ("HasCrop", Value::Bool(true)),
        ("AutoLateralCA", Value::Int(1)),
        ("LensProfileEnable", Value::Int(1)),
        ("WhiteBalance", Value::Str("Custom".into())),
        ("ProcessVersion", Value::Str("15.4".into())),
    ];
    for (key, want) in cases {
        assert_eq!(s.get_by_name(key), Some(&want), "{key}");
    }
    assert_eq!(s.process_version(), Some(ProcessVersion::V6));
    assert_eq!(s.file_kind, Some(FileKind::Raw));
    let wb = s.wb().expect("white balance");
    assert_eq!(wb.mode, WbMode::Custom);
    // Struct booleans are lower case in XMP; both spellings read the same.
    let blur = struct_of(s, "LensBlur");
    assert_eq!(blur.kind, StructKind::LensBlur);
    assert_eq!(blur.get("Active"), Some(&Value::Bool(true)));
    assert_eq!(blur.get("Version"), Some(&Value::Int(1)));
    assert_eq!(blur.get("BlurAmount"), Some(&Value::Int(50)));
}

#[test]
fn both_curve_kinds_read_into_points() {
    let d = read("curves.xmp");
    let s = &d.develop;
    assert_eq!(
        s.get_by_name("ToneCurvePV2012"),
        Some(&points(&[
            (0.0, 0.0),
            (64.0, 58.0),
            (192.0, 200.0),
            (255.0, 255.0)
        ]))
    );
    assert_eq!(
        s.get_by_name("ToneCurvePV2012Red"),
        Some(&points(&[(0.0, 12.0), (255.0, 255.0)]))
    );
    let c = &s.corrections[0];
    assert_eq!(
        c.local_value("MainCurve"),
        Some(&points(&[
            (0.0, 0.0),
            (32.0, 16.0),
            (128.0, 128.0),
            (255.0, 255.0)
        ]))
    );
    assert_eq!(
        c.local_value("RedCurve"),
        Some(&points(&[(0.0, 0.0), (255.0, 240.0)]))
    );
}

#[test]
fn ai_masks_are_classified_by_structure() {
    let d = read("masks_ai.xmp");
    let tools: Vec<&MaskTool> = d
        .develop
        .corrections
        .iter()
        .map(|c| &c.masks[0].tool)
        .collect();
    assert_eq!(
        tools,
        [
            &MaskTool::Semantic(Semantic::Subject),
            &MaskTool::Semantic(Semantic::Sky),
            &MaskTool::Semantic(Semantic::Background),
            &MaskTool::Semantic(Semantic::PeoplePart(2)),
            &MaskTool::Semantic(Semantic::Landscape(50002)),
            &MaskTool::Semantic(Semantic::PersonPartAt {
                part: 5,
                point: SensorPoint {
                    x: f(0.433594),
                    y: f(0.659824)
                }
            }),
        ]
    );
    let c = &d.develop.corrections;
    assert_eq!(c[0].local_value("LocalExposure2012"), Some(&real(0.0625)));
    assert_eq!(c[1].amount, Some(f(0.8)));
    assert_eq!(c[5].active, Some(false));
    assert_eq!(c[0].name.as_deref(), Some("Correction 1"));
    assert_eq!(
        c[0].sync_id.as_ref().map(|h| h.as_str()),
        Some("00000000000000000000000000000C01")
    );
    let m = &c[0].masks[0];
    assert_eq!(m.combine, Combine::Add { inverted: false });
    assert_eq!(m.name.as_deref(), Some("Mask 1"));
    assert_eq!(m.active, Some(true));
    assert_eq!(mask_field(&c[0], 0, "MaskVersion"), Some(&Value::Int(1)));
}

#[test]
fn a_linear_gradient_and_its_element_form_twin_read_the_same() {
    let attrs = read("mask_linear.xmp");
    let elements = read("element_form.xmp");
    assert_eq!(attrs.develop, elements.develop);
    let MaskTool::Linear(g) = &attrs.develop.corrections[0].masks[0].tool else {
        panic!("linear gradient");
    };
    assert_eq!(
        g.zero,
        SensorPoint {
            x: f(0.5),
            y: f(0.55)
        }
    );
    assert_eq!(
        g.full,
        SensorPoint {
            x: f(0.5),
            y: f(0.2)
        }
    );
    assert_eq!(
        attrs.develop.corrections[0].local_value("LocalExposure2012"),
        Some(&real(-0.125))
    );
}

#[test]
fn radial_gradients_are_typed_only_without_rotation() {
    let d = read("mask_radial.xmp");
    let c = &d.develop.corrections;
    let MaskTool::Radial(r) = &c[0].masks[0].tool else {
        panic!("radial gradient with Angle 0");
    };
    assert_eq!(
        (r.top, r.left, r.bottom, r.right),
        (f(0.4), f(0.3), f(0.6), f(0.7))
    );
    assert_eq!((r.feather, r.midpoint, r.roundness), (68, 50, 0));
    assert!(r.flipped);
    assert_eq!(
        c[1].masks[0].tool,
        MaskTool::Opaque {
            what: Some("Mask/CircularGradient".into())
        }
    );
    // The rotated one keeps every field for the round trip.
    assert_eq!(mask_field(&c[1], 0, "Angle"), Some(&real(-29.0882)));
    assert_eq!(mask_field(&c[1], 0, "Flipped"), Some(&Value::Bool(false)));
}

#[test]
fn range_masks_and_their_parse_type_twin_read_the_same() {
    let plain = read("mask_range.xmp");
    let resource = read("parse_type_resource.xmp");
    assert_eq!(plain.develop, resource.develop);
    let c = &plain.develop.corrections;
    let MaskTool::LuminanceRange(l) = &c[0].masks[0].tool else {
        panic!("luminance range");
    };
    assert_eq!(l.range, [f(0.0), f(0.1), f(0.35), f(0.5)]);
    assert!(!l.invert);
    assert_eq!(l.rest.get("Version"), Some(&Value::Int(3)));
    assert_eq!(
        l.rest.get("LuminanceDepthSampleInfo"),
        Some(&Value::Str("0.250000 0.200000 0.300000".into()))
    );
    // A colour range stays opaque with its whole CorrectionRangeMask.
    assert_eq!(
        c[1].masks[0].tool,
        MaskTool::Opaque {
            what: Some("Mask/RangeMask".into())
        }
    );
    let Some(Value::Struct(crm)) = mask_field(&c[1], 0, "CorrectionRangeMask") else {
        panic!("CorrectionRangeMask");
    };
    assert_eq!(crm.get("Type"), Some(&Value::Int(1)));
    assert_eq!(
        crm.get("PointModels"),
        Some(&Value::StrList(vec![
            "0.255405 0.034566 0.419137 0.692175 0.651248 0".into()
        ]))
    );
}

#[test]
fn intersect_subtract_and_inverted_add() {
    let d = read("mask_combine.xmp");
    let c = &d.develop.corrections;
    let combos: Vec<Combine> = c[0].masks.iter().map(|m| m.combine).collect();
    assert_eq!(
        combos,
        [
            Combine::Add { inverted: false },
            Combine::Subtract,
            Combine::Intersect
        ]
    );
    assert_eq!(c[0].masks[1].tool, MaskTool::Semantic(Semantic::Sky));
    assert!(matches!(c[0].masks[2].tool, MaskTool::LuminanceRange(_)));
    assert_eq!(c[1].masks[0].combine, Combine::Add { inverted: true });
    // The combination fields are consumed by the typed view.
    assert!(mask_field(&c[0], 1, "MaskBlendMode").is_none());
}

#[test]
fn a_brush_aggregate_keeps_its_strokes() {
    let d = read("mask_brush.xmp");
    let c = &d.develop.corrections[0];
    assert_eq!(
        c.masks[0].tool,
        MaskTool::Opaque {
            what: Some("Mask/Aggregate".into())
        }
    );
    let Some(Value::Tools(strokes)) = mask_field(c, 0, "Masks") else {
        panic!("Masks");
    };
    assert_eq!(strokes.len(), 1);
    let stroke = &strokes[0];
    assert_eq!(
        stroke.get(Level::MaskTool, "What"),
        Some(&Value::Str("Mask/Paint".into()))
    );
    assert_eq!(stroke.get(Level::MaskTool, "Radius"), Some(&real(0.053214)));
    assert_eq!(
        stroke.get(Level::MaskTool, "Dabs"),
        Some(&Value::StrList(vec![
            "d 0.520898 0.305817".into(),
            "d 0.530000 0.310000".into()
        ]))
    );
}

#[test]
fn a_look_keeps_its_parameters_opaque() {
    let d = read("look_stub.xmp");
    assert_eq!(d.skipped_subtrees.look_parameters, 1);
    let look = d.develop.look.as_ref().expect("Look");
    assert_eq!(look.name.as_deref(), Some("Adobe Landscape"));
    assert_eq!(
        look.uuid.as_ref().map(|u| u.as_str()),
        Some("6F9C877E84273F4E8271E6B91BEB36A1")
    );
    assert_eq!(look.amount, Some(f(0.85)));
    assert_eq!(look.camera_restriction, None);
    assert_eq!(
        look.rest.get("Group"),
        Some(&Value::Alt("Synthetic Group".into()))
    );
    assert_eq!(look.rest.get("SupportsAmount"), Some(&Value::Bool(false)));
    let Some(Value::Opaque(Opaque::Xmp(params))) = look.rest.get("Parameters") else {
        panic!("Parameters kept as XMP");
    };
    let XmpValue::Struct(fields) = &params.value else {
        panic!("Parameters is a structure");
    };
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["ProcessVersion", "ConvertToGrayscale"]);
}

#[test]
fn a_preset_record_is_read_without_its_parameters() {
    let d = read("preset_record.xmp");
    assert_eq!(d.kind, XmpKind::Sidecar);
    assert_eq!(d.skipped_subtrees.preset_parameters, 1);
    // The photo's own value, not the one inside Preset.Parameters.
    assert_eq!(d.develop.get_by_name("Exposure2012"), Some(&real(0.3)));
    let preset = struct_of(&d.develop, "Preset");
    assert_eq!(preset.kind, StructKind::Preset);
    assert_eq!(
        preset.get("Name"),
        Some(&Value::Str("Synthetic Text".into()))
    );
    assert_eq!(preset.get("Amount"), Some(&real(0.75)));
    assert_eq!(preset.get("SupportsAmount"), Some(&Value::Bool(true)));
    assert_eq!(
        preset.get("Group"),
        Some(&Value::Alt("Synthetic Group".into()))
    );
    assert!(matches!(
        preset.get("Parameters"),
        Some(Value::Opaque(Opaque::Xmp(_)))
    ));
}

#[test]
fn snapshots_are_skipped_entirely() {
    let d = read("snapshot_crss.xmp");
    assert_eq!(d.skipped_subtrees.crss, 1);
    assert_eq!(d.develop.get_by_name("Exposure2012"), Some(&real(0.2)));
    assert!(d.develop.opaque.is_empty());
    assert_eq!(d.develop.value_count(), 2);
}

#[test]
fn several_descriptions_are_merged() {
    let d = read("multiple_descriptions.xmp");
    assert_eq!(d.kind, XmpKind::Sidecar);
    let s = &d.develop;
    assert_eq!(s.get_by_name("Exposure2012"), Some(&real(0.25)));
    assert_eq!(s.get_by_name("Vibrance"), Some(&Value::Int(10)));
    assert_eq!(s.value_count(), 8);
    assert_eq!(s.wb().expect("white balance").mode, WbMode::AsShot);
    // xmp:ModifyDate, xmp:Rating, dc:format, dc:subject.
    assert_eq!(d.skipped_subtrees.other_namespaces, 4);
}

#[test]
fn unknown_keys_are_kept_and_reported_at_every_level() {
    let d = read("unknown_keys.xmp");
    let expected = [
        ("FutureSlider2031", "UnknownKey"),
        ("FutureStructure", "UnknownKey"),
        ("LensBlur.FutureLensBlurField", "UnknownKey"),
        (
            "MaskGroupBasedCorrections[0].LocalFutureAdjustment",
            "UnknownKey",
        ),
        ("MaskGroupBasedCorrections[0].Annotation", "UnknownKey"),
        (
            "MaskGroupBasedCorrections[0].CorrectionMasks[0].FutureMaskField",
            "UnknownKey",
        ),
    ];
    let got = warnings(&d);
    let got: Vec<(&str, &str)> = got.iter().map(|(p, k)| (p.as_str(), *k)).collect();
    assert_eq!(got, expected);
    // Kept in document order, verbatim.
    let top: Vec<&str> = d.develop.opaque.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(top, ["FutureSlider2031", "FutureStructure"]);
    let Opaque::Xmp(slider) = &d.develop.opaque[0].value else {
        panic!("XMP node");
    };
    assert_eq!(slider.as_text(), Some("+7"));
    let blur = struct_of(&d.develop, "LensBlur");
    assert_eq!(blur.fields.opaque[0].name, "FutureLensBlurField");
    let c = &d.develop.corrections[0];
    let foreign = c
        .extra
        .opaque
        .iter()
        .find(|o| o.name == "Annotation")
        .expect("foreign field kept");
    assert_eq!(foreign.ns.as_deref(), Some("urn:lrgenius:test:extension"));
    assert_eq!(c.masks[0].tool, MaskTool::Semantic(Semantic::Subject));
    assert_eq!(c.masks[0].extra.opaque[0].name, "FutureMaskField");
}

#[test]
fn a_bag_where_a_seq_belongs_is_kept_whole() {
    let d = read("bag.xmp");
    let Some(Value::Opaque(Opaque::Xmp(node))) = d.develop.get_by_name("PointColors") else {
        panic!("PointColors kept whole");
    };
    let XmpValue::Array { kind, items } = &node.value else {
        panic!("array");
    };
    assert_eq!(*kind, XmpArrayKind::Bag);
    assert_eq!(items.len(), 2);
    assert_eq!(
        d.develop.get_by_name("ColorVariance"),
        Some(&Value::StrList(vec!["0.000000, 0.000000, 0.000000".into()]))
    );
    assert_eq!(d.skipped_subtrees.other_namespaces, 1, "dc:subject");
    assert_eq!(d.skipped_subtrees.kept_whole, 1, "PointColors");
}

#[test]
fn a_second_language_keeps_the_alternative_whole() {
    let d = read("alt_second_language.xmp");
    let h = d.header.as_ref().expect("header");
    assert_eq!(h.name, None);
    assert_eq!(h.description, None);
    assert_eq!(h.group.as_deref(), Some("Synthetic Group"));
    assert_eq!(h.supports_amount, Some(false));
    for key in ["Name", "Description"] {
        let Some(Value::Opaque(Opaque::Xmp(alt))) = h.rest.get(Level::Header, key) else {
            panic!("{key} kept whole");
        };
        assert_eq!(alt.alt_default(), Some("Synthetic Text"), "{key}");
        let XmpValue::Array { items, .. } = &alt.value else {
            panic!("{key}: array");
        };
        assert_eq!(items.len(), 2, "{key}");
    }
    assert_eq!(d.skipped_subtrees.kept_whole, 2, "Name and Description");
}

#[test]
fn a_byte_order_mark_is_ignored() {
    let with_bom = bytes("bom.xmp");
    assert_eq!(
        &with_bom[..3],
        b"\xEF\xBB\xBF",
        "the fixture starts with a BOM"
    );
    let d = read("bom.xmp");
    let without = parse(&with_bom[3..]).expect("parses without the BOM");
    assert_eq!(d, without);
    assert_eq!(d.develop.get_by_name("Exposure2012"), Some(&real(0.5)));
}

#[test]
fn a_profile_is_its_definition() {
    let d = read("profile.xmp");
    assert_eq!(d.kind, XmpKind::Profile);
    let h = d.header.as_ref().expect("header");
    assert_eq!(h.preset_type.as_deref(), Some("Look"));
    assert_eq!(h.name.as_deref(), Some("Synthetic Profile"));
    assert_eq!(
        d.develop.get_by_name("ConvertToGrayscale"),
        Some(&Value::Bool(true))
    );
}

#[test]
fn a_file_without_develop_settings_is_not_an_error() {
    let d = read("not_develop.xmp");
    assert_eq!(d.kind, XmpKind::NotDevelop);
    assert!(d.header.is_none());
    assert!(d.develop.is_empty());
    assert_eq!(d.skipped_subtrees.other_namespaces, 3);
}

#[test]
fn coercion_problems_are_warnings_with_paths() {
    let d = read("coercion_warnings.xmp");
    let expected = [
        ("Exposure2012", "WrongType"),
        ("Contrast2012", "NonIntegerForIntKey"),
        ("WhiteBalance", "ValueNotInSet"),
        ("LensProfileDigest", "NotHex32"),
        ("LensProfileEnable", "WrongType"),
        ("ToneCurvePV2012", "WrongType"),
        (
            "MaskGroupBasedCorrections[0].CorrectionMasks[0].MaskSubType",
            "NonIntegerForIntKey",
        ),
        (
            "MaskGroupBasedCorrections[0].CorrectionMasks[0].MaskBlendMode",
            "UnrecognisedMaskCombine",
        ),
    ];
    let got = warnings(&d);
    let got: Vec<(&str, &str)> = got.iter().map(|(p, k)| (p.as_str(), *k)).collect();
    assert_eq!(got, expected);
    let s = &d.develop;
    // Rounded values stay in the model, wrong shapes go to `opaque`.
    assert_eq!(s.get_by_name("Contrast2012"), Some(&Value::Int(12)));
    assert_eq!(
        s.get_by_name("WhiteBalance"),
        Some(&Value::Str("Candlelight".into()))
    );
    let opaque: Vec<&str> = s.opaque.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(
        opaque,
        ["Exposure2012", "LensProfileEnable", "ToneCurvePV2012"]
    );
    let m = &s.corrections[0].masks[0];
    assert_eq!(m.tool, MaskTool::Semantic(Semantic::Sky), "1.5 rounds to 2");
    assert_eq!(m.combine, Combine::Unrecognised);
}
