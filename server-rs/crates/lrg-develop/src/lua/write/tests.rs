use serde_json::json;

use super::*;
use crate::build::{CorrectionBuilder, LuminanceRange, SyncNamespace};
use crate::lua::read::from_lua_value;
use crate::model::correction::{LinearGradient, RadialGradient, SensorPoint};
use crate::model::value::Hex32;
use crate::model::FileKindHint;

fn read(v: J) -> DevelopSettings {
    let (s, w) = from_lua_value(&v, FileKindHint::Unknown).expect("a settings table");
    assert!(w.is_empty(), "{w:?}");
    s
}

/// As [`read`], allowing warnings (unknown keys on purpose).
fn read_lenient(v: J) -> DevelopSettings {
    from_lua_value(&v, FileKindHint::Unknown)
        .expect("a settings table")
        .0
}

fn raw_photo() -> LuaMode {
    LuaMode::Apply(PhotoContext {
        file_kind: Some(FileKind::Raw),
        process_version: Some(ProcessVersion::V6),
        camera: None,
    })
}

fn photo(kind: Option<FileKind>) -> LuaMode {
    LuaMode::Apply(PhotoContext {
        file_kind: kind,
        ..PhotoContext::default()
    })
}

fn apply(s: &DevelopSettings) -> LuaWritten {
    to_lua_value(s, &raw_photo(), &LuaOptions::default()).unwrap()
}

fn apply_with(s: &DevelopSettings, opt: LuaOptions) -> LuaWritten {
    to_lua_value(s, &raw_photo(), &opt).unwrap()
}

fn paths(w: &LuaWritten) -> Vec<&str> {
    w.skipped.iter().map(|k| k.path.as_str()).collect()
}

/// Every key name anywhere in `j`.
fn all_keys(j: &J) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(j: &J, out: &mut Vec<String>) {
        match j {
            J::Object(m) => {
                for (k, v) in m {
                    out.push(k.clone());
                    walk(v, out);
                }
            }
            J::Array(a) => a.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    walk(j, &mut out);
    out
}

fn subject_correction() -> Correction {
    CorrectionBuilder::new("Subject", &SyncNamespace::lrgenius())
        .local_ui("LocalExposure2012", 0.5)
        .unwrap()
        .add(Semantic::Subject)
        .build()
        .unwrap()
}

fn sky_correction() -> Correction {
    CorrectionBuilder::new("Sky", &SyncNamespace::lrgenius())
        .local_ui("LocalHighlights2012", -40.0)
        .unwrap()
        .add(Semantic::Sky)
        .build()
        .unwrap()
}

fn mask(w: &LuaWritten, c: usize, m: usize) -> &Map<String, J> {
    w.table["MaskGroupBasedCorrections"][c]["CorrectionMasks"][m]
        .as_object()
        .unwrap()
}

// --- the provisional options ------------------------------------------------

#[test]
fn the_default_options_are_the_provisional_ones() {
    assert_eq!(LuaOptions::default(), LuaOptions::PROVISIONAL);
}

/// Pins every provisional choice: changing one is a decision (an experiment
/// answered), which also re-blesses the wire goldens and updates the wiki.
#[test]
fn the_provisional_choices_are_pinned() {
    let p = LuaOptions::PROVISIONAL;
    assert_eq!(p.mask_enum_as, EnumAs::Number);
    assert_eq!(p.int_flag_as, FlagAs::Number);
    assert_eq!(p.panel_switches, PanelSwitches::MaskOnly);
    assert_eq!(p.mask_form, MaskForm::PresetForm);
    assert_eq!(p.local_form, LocalForm::AdaptivePreset);
    assert!(p.wb_custom_with_numbers);
    assert!(!p.wb_mode_only);
    assert!(!p.flatten_auto_now);
    assert!(p.ai_update);
    assert_eq!(p.look_form, LookForm::Stub);
    assert!(!p.preset_amount_flags);
}

/// One evidence row per field, in field order; a new field cannot be added
/// without saying how far the experiments carry it (the destructuring below
/// stops compiling).
#[test]
fn every_option_has_an_evidence_row() {
    let LuaOptions {
        mask_enum_as: _,
        int_flag_as: _,
        panel_switches: _,
        mask_form: _,
        local_form: _,
        wb_custom_with_numbers: _,
        wb_mode_only: _,
        flatten_auto_now: _,
        ai_update: _,
        look_form: _,
        preset_amount_flags: _,
    } = LuaOptions::PROVISIONAL;
    let fields: Vec<&str> = LuaOptions::EVIDENCE.iter().map(|e| e.field).collect();
    assert_eq!(
        fields,
        [
            "mask_enum_as",
            "int_flag_as",
            "panel_switches",
            "mask_form",
            "local_form",
            "wb_custom_with_numbers",
            "wb_mode_only",
            "flatten_auto_now",
            "ai_update",
            "look_form",
            "preset_amount_flags",
        ]
    );
    for e in LuaOptions::EVIDENCE {
        assert!(!e.open.is_empty(), "{}: say what is still open", e.field);
        match e.status {
            EvidenceStatus::Open => {
                assert!(e.by.is_empty() && e.scope.is_empty(), "{}", e.field);
            }
            EvidenceStatus::Settled | EvidenceStatus::Supported => {
                assert!(!e.by.is_empty() && !e.scope.is_empty(), "{}", e.field);
            }
        }
    }
}

/// The first run's statuses: one non-raw photo, so nothing on the raw side
/// is settled, the fields no experiment answered stay open, and
/// `panel_switches` is only supported (E2 sent a switch the photo already
/// had on, E13).
#[test]
fn the_first_run_statuses_are_pinned() {
    let status = |f: &str| {
        LuaOptions::EVIDENCE
            .iter()
            .find(|e| e.field == f)
            .unwrap()
            .status
    };
    for f in ["int_flag_as", "look_form"] {
        assert_eq!(status(f), EvidenceStatus::Open, "{f}");
    }
    for f in ["wb_custom_with_numbers", "panel_switches"] {
        assert_eq!(status(f), EvidenceStatus::Supported, "{f}");
    }
    for e in LuaOptions::EVIDENCE {
        assert!(
            e.status == EvidenceStatus::Open || e.scope == FIRST_RUN,
            "{}",
            e.field
        );
    }
}

// --- rule 1: keys, policy, guard -------------------------------------------

#[test]
fn keys_have_no_prefix_and_only_what_reaches_the_photo_is_written() {
    let s = read_lenient(json!({
        "Exposure2012": 0.5,
        "CropTop": 0.1,
        "ProcessVersion": "15.4",
        "EnableDetail": true,
        "RetouchAreas": [{"Feather": 0.5}],
        "SomethingNew": 3
    }));
    let w = apply(&s);
    let t = w.table.as_object().unwrap();
    assert_eq!(t["Exposure2012"], json!(0.5));
    // PHOTO reaches the photo the settings are for.
    assert_eq!(t["CropTop"], json!(0.1));
    // Global META is set by the writer, never copied: no process version,
    // no panel switch.
    assert!(!t.contains_key("ProcessVersion"));
    assert!(!t.contains_key("EnableDetail"));
    assert!(t.keys().all(|k| !k.starts_with("crs:")));
    assert!(!t.contains_key("RetouchAreas"));
    assert!(!t.contains_key("SomethingNew"));
    // Every dropped non-default value is reported, META silently dropped.
    assert_eq!(paths(&w), ["RetouchAreas", "SomethingNew"]);
}

#[test]
fn the_guard_covers_every_never_write_key_and_the_xmp_only_ones() {
    for name in NEVER_WRITE {
        let id = registry::iter().find(|(_, s)| s.name == *name);
        if let Some((_, spec)) = id {
            assert!(lua_never_written(spec), "{name}");
        }
    }
    let spec = |level, name| registry::lookup(level, name).unwrap().spec();
    assert!(lua_never_written(spec(Level::Global, "HasSettings")));
    assert!(lua_never_written(spec(Level::Global, "EnableDetail")));
    assert!(lua_never_written(spec(Level::MaskTool, "MaskDigest")));
    assert!(!lua_never_written(spec(Level::Global, "Exposure2012")));
    assert!(!lua_never_written(spec(Level::MaskTool, "MaskSubType")));
}

#[test]
fn computed_mask_bookkeeping_never_reaches_the_table() {
    // A computed subject mask as Lightroom reads it back: digests, runtime
    // ids, sizes, origin, model version.
    let s = read(json!({"MaskGroupBasedCorrections": [{
        "What": "Correction", "CorrectionAmount": 1, "CorrectionActive": true,
        "CorrectionID": "00000000-0000-4000-8000-000000000001",
        "CorrectionSyncID": "0000000000000000000000000000000A",
        "CorrectionReferenceX": 0.5, "CorrectionReferenceY": 0.5,
        "LocalExposure2012": 0.25,
        "CorrectionMasks": [{
            "What": "Mask/Image", "MaskSubType": 1, "MaskBlendMode": 0,
            "MaskValue": 1, "MaskInverted": false, "MaskActive": true,
            "MaskID": "00000000-0000-4000-8000-000000000002",
            "MaskSyncID": "0000000000000000000000000000000B",
            "MaskDigest": "0000000000000000000000000000000C",
            "InputDigest": "0000000000000000000000000000000D",
            "FullMaskSize": "1024 768", "WholeImageArea": "0 0 1 1",
            "Origin": "0 0", "ModelVersion": 123, "ErrorReason": 0,
            "ReferencePoint": "0.433594 0.659824", "MaskVersion": 1
        }]
    }]}));
    let w = apply(&s);
    let keys = all_keys(&w.table);
    for never in [
        "CorrectionID",
        "MaskID",
        "CorrectionReferenceX",
        "CorrectionReferenceY",
        "FullMaskSize",
        "WholeImageArea",
        "Origin",
        "ModelVersion",
        "MaskDigest",
        "InputDigest",
    ] {
        assert!(!keys.iter().any(|k| k == never), "{never} written");
    }
    let m = mask(&w, 0, 0);
    assert_eq!(m["MaskSubType"], json!(1));
    assert_eq!(m["MaskSyncID"], json!("0000000000000000000000000000000B"));
    assert_eq!(
        w.table["MaskGroupBasedCorrections"][0]["LocalExposure2012"],
        json!(0.25)
    );
}

// --- rules 2 and 3: numbers and booleans ----------------------------------

#[test]
fn integers_reals_booleans_and_flags_keep_their_json_types() {
    let s = read(json!({
        "Contrast2012": 12, "Exposure2012": 1, "ConvertToGrayscale": false,
        "LensProfileEnable": 1, "AutoLateralCA": 0, "PostCropVignetteStyle": 1
    }));
    let t = apply(&s).table;
    assert!(t["Contrast2012"].is_i64());
    assert!(t["Exposure2012"].is_f64(), "a real key is a real: {t}");
    assert_eq!(t["ConvertToGrayscale"], json!(false));
    assert_eq!(t["LensProfileEnable"], json!(1));
    assert_eq!(t["AutoLateralCA"], json!(0));
    assert!(t["PostCropVignetteStyle"].is_i64());
}

#[test]
fn flags_as_booleans_on_request() {
    let s = read(json!({"LensProfileEnable": 1, "AutoLateralCA": 0}));
    let opt = LuaOptions {
        int_flag_as: FlagAs::Bool,
        ..LuaOptions::default()
    };
    let t = apply_with(&s, opt).table;
    assert_eq!(t["LensProfileEnable"], json!(true));
    assert_eq!(t["AutoLateralCA"], json!(false));
}

#[test]
fn a_real_on_an_integer_key_is_coerced_half_to_even() {
    let mut s = DevelopSettings::new();
    let contrast = registry::lookup(Level::Global, "Contrast2012").unwrap();
    s.insert(contrast, Value::Real(Finite::new_const(12.5)))
        .unwrap();
    assert_eq!(apply(&s).table["Contrast2012"], json!(12));
    // A value no integer key can take is an error, not a guess.
    let flag = registry::lookup(Level::Global, "LensProfileEnable").unwrap();
    s.insert(flag, Value::Real(Finite::new_const(3.0))).unwrap();
    assert!(matches!(
        to_lua_value(&s, &raw_photo(), &LuaOptions::default()),
        Err(WireError::WrongValue { .. })
    ));
}

#[test]
fn a_value_of_the_wrong_variant_is_an_error() {
    let mut s = DevelopSettings::new();
    let exposure = registry::lookup(Level::Global, "Exposure2012").unwrap();
    s.insert(exposure, Value::Str("bright".into())).unwrap();
    let err = to_lua_value(&s, &raw_photo(), &LuaOptions::default()).unwrap_err();
    assert!(
        matches!(&err, WireError::WrongValue { path, .. } if path == "Exposure2012"),
        "{err}"
    );
}

// --- rule 4: numeric-looking strings ---------------------------------------

#[test]
fn numeric_looking_strings_stay_strings() {
    let mut s = read(json!({"Look": {
        "Name": "Adobe Color", "UUID": "B952C231111CD8E0ECCF14B86BAA7077", "Amount": 1
    }}));
    let range = CorrectionBuilder::new("Bright", &SyncNamespace::lrgenius())
        .local_ui("LocalExposure2012", 0.3)
        .unwrap()
        .add(Semantic::Sky)
        .intersect(LuminanceRange::highs(0.5, 0.2).unwrap())
        .build()
        .unwrap();
    let person = CorrectionBuilder::new("Person", &SyncNamespace::lrgenius())
        .local_ui("LocalSaturation", 10.0)
        .unwrap()
        .local_curve("MainCurve", &[(0, 0), (128, 140), (255, 255)])
        .unwrap()
        .add(Semantic::PersonPartAt {
            part: 5,
            point: SensorPoint::new(0.25, 0.75).unwrap(),
        })
        .build()
        .unwrap();
    s.corrections = vec![range, person];
    let t = apply(&s).table;
    assert_eq!(t["Look"]["UUID"], json!("B952C231111CD8E0ECCF14B86BAA7077"));
    let crm = &t["MaskGroupBasedCorrections"][0]["CorrectionMasks"][1]["CorrectionRangeMask"];
    assert!(crm["LumRange"].is_string(), "{crm}");
    assert_eq!(
        crm["LumRange"],
        json!("0.300000 0.500000 1.000000 1.000000")
    );
    let person = &t["MaskGroupBasedCorrections"][1];
    assert_eq!(
        person["CorrectionMasks"][0]["ReferencePoint"],
        json!("0.250000 0.750000")
    );
    assert_eq!(person["MainCurve"], json!(["0,0", "128,140", "255,255"]));
}

#[cfg(feature = "test-roundtrip")]
#[test]
fn the_process_version_and_focal_range_stay_strings_in_a_round_trip() {
    let s = read(json!({
        "ProcessVersion": "15.4",
        "LensBlur": {"Active": true, "FocalRange": "0 0 100 100"}
    }));
    let t = to_lua_value(&s, &LuaMode::TestRoundTrip, &LuaOptions::default())
        .unwrap()
        .table;
    assert_eq!(t["ProcessVersion"], json!("15.4"));
    assert_eq!(t["LensBlur"]["FocalRange"], json!("0 0 100 100"));
}

#[test]
fn compound_strings_keep_their_spelling_and_must_be_numbers() {
    let mut s = DevelopSettings::new();
    let mut c = subject_correction();
    c.masks[0].extra.values.insert(
        registry::lookup(Level::MaskTool, "ReferencePoint").unwrap(),
        Value::Str("0.25 0.5".into()),
    );
    s.corrections = vec![c.clone()];
    assert_eq!(mask(&apply(&s), 0, 0)["ReferencePoint"], json!("0.25 0.5"));
    c.masks[0].extra.values.insert(
        registry::lookup(Level::MaskTool, "ReferencePoint").unwrap(),
        Value::Str("left top".into()),
    );
    s.corrections = vec![c];
    assert!(matches!(
        to_lua_value(&s, &raw_photo(), &LuaOptions::default()),
        Err(WireError::NotCompound { .. })
    ));
}

#[test]
fn ids_are_upper_cased_and_must_be_hex() {
    let mut s = DevelopSettings::new();
    let mut c = subject_correction();
    c.sync_id = Hex32::parse("0000000000000000000000000000abcd");
    s.corrections = vec![c];
    let t = apply(&s).table;
    assert_eq!(
        t["MaskGroupBasedCorrections"][0]["CorrectionSyncID"],
        json!("0000000000000000000000000000ABCD")
    );
    let s = read(json!({"Look": {"Name": "x", "UUID": "b952c231111cd8e0eccf14b86baa7077"}}));
    assert_eq!(
        apply(&s).table["Look"]["UUID"],
        json!("B952C231111CD8E0ECCF14B86BAA7077")
    );
    // A malformed id held as text on a Hex32 key is refused, not written.
    let mut s = DevelopSettings::new();
    let mut c = subject_correction();
    c.extra.values.insert(
        registry::lookup(Level::Correction, "CorrectionSyncID").unwrap(),
        Value::Str("12".into()),
    );
    c.sync_id = None;
    s.corrections = vec![c];
    assert!(matches!(
        to_lua_value(&s, &raw_photo(), &LuaOptions::default()),
        Err(WireError::NotHex32 { .. })
    ));
}

// --- rule 5: curves ---------------------------------------------------------

#[test]
fn global_curves_are_flat_even_number_lists_and_local_curves_strings() {
    let s = read(json!({
        "ToneCurvePV2012": [0, 0, 64, 50, 192, 210, 255, 255],
        "ToneCurvePV2012Red": [0, 0, 255, 255]
    }));
    let t = apply(&s).table;
    let curve = t["ToneCurvePV2012"].as_array().unwrap();
    assert_eq!(curve.len(), 8);
    assert!(curve.iter().all(J::is_number));
    assert_eq!(t["ToneCurvePV2012Red"], json!([0, 0, 255, 255]));
    // The name the writer derives, as in a preset.
    assert_eq!(t["ToneCurveName2012"], json!("Custom"));
    let linear = read(json!({"ToneCurvePV2012": [0, 0, 255, 255]}));
    assert_eq!(apply(&linear).table["ToneCurveName2012"], json!("Linear"));
}

#[test]
fn a_fractional_curve_point_is_written_exactly() {
    let mut s = DevelopSettings::new();
    let id = registry::lookup(Level::Global, "ToneCurvePV2012").unwrap();
    s.insert(
        id,
        Value::Curve(vec![
            (Finite::ZERO, Finite::ZERO),
            (Finite::new_const(127.5), Finite::new_const(130.25)),
            (Finite::new_const(255.0), Finite::new_const(255.0)),
        ]),
    )
    .unwrap();
    assert_eq!(
        apply(&s).table["ToneCurvePV2012"],
        json!([0, 0, 127.5, 130.25, 255, 255])
    );
}

#[test]
fn a_one_point_curve_is_an_error() {
    let mut s = DevelopSettings::new();
    let id = registry::lookup(Level::Global, "ToneCurvePV2012").unwrap();
    s.insert(id, Value::Curve(vec![(Finite::ZERO, Finite::ZERO)]))
        .unwrap();
    assert!(matches!(
        to_lua_value(&s, &raw_photo(), &LuaOptions::default()),
        Err(WireError::ShortCurve { points: 1, .. })
    ));
}

// --- rule 6: corrections ----------------------------------------------------

#[test]
fn corrections_keep_panel_order_with_local_values_on_the_correction() {
    let mut s = DevelopSettings::new();
    s.corrections = vec![subject_correction(), sky_correction()];
    let w = apply(&s);
    let list = w.table["MaskGroupBasedCorrections"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["CorrectionName"], json!("LrGenius · Subject"));
    assert_eq!(list[1]["CorrectionName"], json!("LrGenius · Sky"));
    assert_eq!(list[0]["LocalExposure2012"], json!(0.125));
    assert_eq!(list[1]["LocalHighlights2012"], json!(-0.4));
    for c in list {
        assert_eq!(c["What"], json!("Correction"));
        assert_eq!(c["CorrectionAmount"], json!(1.0));
        assert_eq!(c["CorrectionActive"], json!(true));
        assert!(!c["CorrectionMasks"].as_array().unwrap().is_empty());
    }
    assert!(w.skipped.is_empty(), "{:?}", w.skipped);
}

#[test]
fn corrections_carry_the_adaptive_preset_locals_unless_sparse() {
    let mut s = DevelopSettings::new();
    s.corrections = vec![subject_correction()];
    let c = &apply(&s).table["MaskGroupBasedCorrections"][0];
    for k in ADAPTIVE_PRESET_LOCALS {
        let want = if *k == "LocalExposure2012" {
            json!(0.125)
        } else {
            json!(0.0)
        };
        assert_eq!(c[*k], want, "{k}");
    }
    assert_eq!(ADAPTIVE_PRESET_LOCALS.len(), 23);
    let sparse = apply_with(
        &s,
        LuaOptions {
            local_form: LocalForm::Sparse,
            ..LuaOptions::default()
        },
    );
    let c = sparse.table["MaskGroupBasedCorrections"][0]
        .as_object()
        .unwrap();
    let locals: Vec<&String> = c.keys().filter(|k| k.starts_with("Local")).collect();
    assert_eq!(locals, ["LocalExposure2012"]);
    // A legacy value the filter drops is reported; the form's 0 stays.
    let legacy = read(json!({"MaskGroupBasedCorrections": [{
        "What": "Correction", "CorrectionAmount": 1, "CorrectionActive": true,
        "LocalExposure": 0.3, "LocalExposure2012": 0.1,
        "CorrectionMasks": [{"What": "Mask/Image", "MaskActive": true, "MaskBlendMode": 0,
            "MaskInverted": false, "MaskValue": 1, "MaskSubType": 1}]
    }]}));
    let w = apply(&legacy);
    assert_eq!(
        w.table["MaskGroupBasedCorrections"][0]["LocalExposure"],
        json!(0.0)
    );
    assert!(
        paths(&w).contains(&"MaskGroupBasedCorrections[0].LocalExposure"),
        "{:?}",
        w.skipped
    );
}

#[test]
fn gradients_reach_the_photo_and_need_no_ai_update() {
    let ns = SyncNamespace::lrgenius();
    let mut s = DevelopSettings::new();
    s.corrections = vec![
        CorrectionBuilder::new("Sky gradient", &ns)
            .local_ui("LocalExposure2012", -0.5)
            .unwrap()
            .add(
                LinearGradient::new(
                    SensorPoint::new(0.5, 0.6).unwrap(),
                    SensorPoint::new(0.5, 0.2).unwrap(),
                )
                .unwrap(),
            )
            .build()
            .unwrap(),
        CorrectionBuilder::new("Vignette", &ns)
            .local_ui("LocalExposure2012", 0.3)
            .unwrap()
            .add(RadialGradient::new(0.2, 0.2, 0.8, 0.8, 50).unwrap())
            .build()
            .unwrap(),
    ];
    let w = apply(&s);
    let linear = mask(&w, 0, 0);
    assert_eq!(linear["What"], json!("Mask/Gradient"));
    assert_eq!(linear["ZeroY"], json!(0.6));
    let radial = mask(&w, 1, 0);
    assert_eq!(radial["What"], json!("Mask/CircularGradient"));
    assert_eq!(radial["Angle"], json!(0.0));
    assert_eq!(radial["Feather"], json!(50));
    assert_eq!(radial["Flipped"], json!(true));
    assert!(!w.call.update_ai_settings);
}

// --- rule 7: no empty containers -------------------------------------------

#[test]
fn without_corrections_the_key_is_absent_never_an_empty_list() {
    let s = read(json!({"Exposure2012": 0.5}));
    let w = apply(&s);
    assert!(w.table.get("MaskGroupBasedCorrections").is_none());
    // Brush strokes never reach a photo: the correction is reported and the
    // key stays out, rather than an empty list deleting the photo's masks.
    let s = read(json!({
        "Exposure2012": 0.5,
        "MaskGroupBasedCorrections": [{"What": "Correction", "LocalExposure2012": 0.1,
            "CorrectionMasks": [{"What": "Mask/Aggregate", "MaskBlendMode": 0,
                "MaskValue": 1, "MaskInverted": false}]}]
    }));
    let w = apply(&s);
    assert!(
        w.table.get("MaskGroupBasedCorrections").is_none(),
        "{}",
        w.table
    );
    assert_eq!(paths(&w), ["MaskGroupBasedCorrections[0]"]);
    check_wire(&w.table).unwrap();
}

#[test]
fn a_structure_left_empty_is_absent() {
    // `LensBlur` with only a field that does not reach the photo.
    let s = read(json!({"LensBlur": {"FocalRange": "0 0 100 100"}}));
    let w = apply(&s);
    assert!(w.table.get("LensBlur").is_none(), "{}", w.table);
    assert_eq!(paths(&w), ["LensBlur.FocalRange"]);
}

#[test]
fn nothing_to_write_is_an_empty_table() {
    let w = apply(&DevelopSettings::new());
    assert!(w.is_empty());
    assert_eq!(w.table, json!({}));
    assert!(w.skipped.is_empty());
}

// --- rule 8 and the guard ---------------------------------------------------

/// `{ <the retired key>: 5000 }`.
fn temp_table() -> J {
    let mut m = Map::new();
    m.insert(RETIRED_TEMP_KEY.to_owned(), json!(5000));
    J::Object(m)
}

#[test]
fn check_wire_refuses_what_json_lua_cannot_carry() {
    let bad = [
        (json!({"a": null}), "a"),
        (json!({"a": [1, null]}), "a"),
        (json!({"a": []}), "a"),
        (json!({"a": {}}), "a"),
        (json!({"a": [{"b": []}]}), "a[0].b"),
        (json!({"a": [1, "x"]}), "a"),
        (json!({"a": [{"x": 1}, 2]}), "a"),
        (temp_table(), ""),
        (json!({ "a": temp_table() }), "a"),
    ];
    for (j, path) in bad {
        match check_wire(&j) {
            Err(WireError::NotWireSafe { path: p, .. }) => assert_eq!(p, path, "{j}"),
            other => panic!("{j}: {other:?}"),
        }
    }
    assert!(check_wire(&json!([])).is_err(), "the top level is a table");
    assert!(
        check_wire(&json!({})).is_ok(),
        "an empty top level applies nothing"
    );
    assert!(check_wire(&json!({"a": [[1, 2], [3]]})).is_ok());
}

// --- rule 9: white balance --------------------------------------------------

#[test]
fn raw_custom_white_balance_goes_as_a_family() {
    let s = read(json!({"WhiteBalance": "Custom", "Temperature": 5600, "Tint": 6}));
    let w = apply(&s);
    assert_eq!(
        w.table,
        json!({"WhiteBalance": "Custom", "Temperature": 5600, "Tint": 6})
    );
    assert!(w.skipped.is_empty());
}

#[test]
fn non_raw_white_balance_uses_the_incremental_keys() {
    let s = read(json!({"IncrementalTemperature": 12, "IncrementalTint": -4}));
    let w = to_lua_value(&s, &photo(Some(FileKind::NonRaw)), &LuaOptions::default()).unwrap();
    assert_eq!(
        w.table,
        json!({"WhiteBalance": "Custom", "IncrementalTemperature": 12, "IncrementalTint": -4})
    );
    // The other family never reaches the photo.
    let w = to_lua_value(&s, &photo(Some(FileKind::Raw)), &LuaOptions::default()).unwrap();
    assert!(w.is_empty(), "{}", w.table);
    assert_eq!(paths(&w), ["IncrementalTemperature", "IncrementalTint"]);
}

#[test]
fn an_unknown_file_kind_gets_no_white_balance_numbers() {
    let s = read(json!({"WhiteBalance": "Custom", "Temperature": 5600, "Tint": 6}));
    let w = to_lua_value(&s, &photo(None), &LuaOptions::default()).unwrap();
    assert!(w.is_empty(), "{}", w.table);
    assert_eq!(paths(&w), ["Temperature", "Tint", "WhiteBalance"]);
}

#[test]
fn numbers_next_to_another_mode_are_skipped_with_the_mode() {
    let s = read(json!({"WhiteBalance": "As Shot", "Temperature": 5200, "Tint": 3}));
    let w = apply(&s);
    assert!(w.is_empty(), "{}", w.table);
    assert_eq!(paths(&w), ["Temperature", "Tint"], "As Shot is the default");
    assert!(w
        .skipped
        .iter()
        .all(|k| k.reason == SkipReason::WhiteBalanceMode("As Shot".into())));

    let s = read(json!({"WhiteBalance": "Daylight", "Temperature": 5500, "Tint": 10}));
    let w = apply(&s);
    assert!(w.is_empty());
    assert_eq!(paths(&w), ["Temperature", "Tint", "WhiteBalance"]);
}

#[test]
fn a_mode_alone_on_request_and_auto_flattened_on_request() {
    let s = read(json!({"WhiteBalance": "Auto", "Temperature": 5000, "Tint": 2}));
    let opt = LuaOptions {
        wb_mode_only: true,
        ..LuaOptions::default()
    };
    let w = apply_with(&s, opt);
    assert_eq!(w.table, json!({"WhiteBalance": "Auto"}));
    assert_eq!(paths(&w), ["Temperature", "Tint"]);
    assert!(!w.call.flatten_auto_now);
    let w = apply_with(
        &s,
        LuaOptions {
            flatten_auto_now: true,
            ..opt
        },
    );
    assert!(w.call.flatten_auto_now);
    // Not for a Custom white balance.
    let custom = read(json!({"WhiteBalance": "Custom", "Temperature": 5000, "Tint": 2}));
    let w = apply_with(
        &custom,
        LuaOptions {
            flatten_auto_now: true,
            ..opt
        },
    );
    assert!(!w.call.flatten_auto_now);
}

#[test]
fn numbers_without_a_mode_get_custom_unless_switched_off() {
    let s = read(json!({"Temperature": 5600, "Tint": 6}));
    assert_eq!(apply(&s).table["WhiteBalance"], json!("Custom"));
    let opt = LuaOptions {
        wb_custom_with_numbers: false,
        ..LuaOptions::default()
    };
    let w = apply_with(&s, opt);
    assert_eq!(w.table, json!({"Temperature": 5600, "Tint": 6}));
}

#[test]
fn custom_without_numbers_is_never_written_alone() {
    // Raw numbers for a non-raw photo, and any numbers for an unknown file
    // kind, are dropped by the filter; `Custom` alone would pin whatever the
    // photo has, so it goes too, even with `wb_mode_only`.
    let s = read(json!({"WhiteBalance": "Custom", "Temperature": 5600, "Tint": 6}));
    let opt = LuaOptions {
        wb_mode_only: true,
        ..LuaOptions::default()
    };
    for kind in [Some(FileKind::NonRaw), None] {
        let w = to_lua_value(&s, &photo(kind), &opt).unwrap();
        assert!(w.is_empty(), "{kind:?}: {}", w.table);
        assert!(
            paths(&w).contains(&"WhiteBalance"),
            "{kind:?}: {:?}",
            w.skipped
        );
        assert!(w.skipped.iter().any(|k| k.path == "WhiteBalance"
            && k.reason == SkipReason::WhiteBalanceMode("Custom".into())));
    }
    // A named mode still goes alone on request, to a raw photo.
    let auto = read(json!({"WhiteBalance": "Auto"}));
    let w = to_lua_value(&auto, &photo(Some(FileKind::Raw)), &opt).unwrap();
    assert_eq!(w.table, json!({"WhiteBalance": "Auto"}));
    // Never to a non-raw one: there `Auto` alone kept the old numbers
    // (E1f/E1g), so it is skipped and reported whatever the option says.
    let w = to_lua_value(&auto, &photo(Some(FileKind::NonRaw)), &opt).unwrap();
    assert!(w.is_empty(), "{}", w.table);
    assert_eq!(
        w.skipped,
        [Skipped {
            path: "WhiteBalance".into(),
            reason: SkipReason::WhiteBalanceMode("Auto".into()),
        }]
    );
    assert!(!w.call.flatten_auto_now);
}

#[test]
fn temp_is_never_written() {
    let mut input = temp_table();
    input["Exposure2012"] = json!(0.1);
    let (s, _) = from_lua_value(&input, FileKindHint::Raw).unwrap();
    let w = apply(&s);
    assert!(w.table.get(RETIRED_TEMP_KEY).is_none());
    assert_eq!(paths(&w), [RETIRED_TEMP_KEY]);
}

// --- rule 10: the options ---------------------------------------------------

#[test]
fn mask_enums_as_numeric_strings_on_request() {
    let mut s = DevelopSettings::new();
    let hair = CorrectionBuilder::new("Hair", &SyncNamespace::lrgenius())
        .local_ui("LocalExposure2012", 0.3)
        .unwrap()
        .add(Semantic::PeoplePart(5))
        .intersect(LuminanceRange::highs(0.5, 0.2).unwrap())
        .build()
        .unwrap();
    s.corrections = vec![hair];
    let opt = LuaOptions {
        mask_enum_as: EnumAs::String,
        ..LuaOptions::default()
    };
    let w = apply_with(&s, opt);
    let m = mask(&w, 0, 0);
    assert_eq!(m["MaskSubType"], json!("3"));
    assert_eq!(m["MaskSubCategoryID"], json!("5"));
    assert_eq!(m["MaskBlendMode"], json!("0"));
    assert_eq!(m["ErrorReason"], json!("0"));
    assert_eq!(m["MaskVersion"], json!(1), "not an enum");
    let range = mask(&w, 0, 1);
    assert_eq!(range["CorrectionRangeMask"]["Type"], json!("2"));
    assert_eq!(range["CorrectionRangeMask"]["SampleType"], json!("0"));
    // Global integer sets keep their numbers.
    let v = read(json!({"PostCropVignetteStyle": 1}));
    assert_eq!(apply_with(&v, opt).table["PostCropVignetteStyle"], json!(1));
    // And numbers by default.
    assert_eq!(mask(&apply(&s), 0, 0)["MaskSubType"], json!(3));
}

#[test]
fn the_preset_form_completes_ai_and_range_masks_the_minimal_form_does_not() {
    let mut s = DevelopSettings::new();
    let c = CorrectionBuilder::new("Bright sky", &SyncNamespace::lrgenius())
        .local_ui("LocalHighlights2012", -30.0)
        .unwrap()
        .add(Semantic::Sky)
        .intersect(LuminanceRange::highs(0.5, 0.2).unwrap())
        .build()
        .unwrap();
    s.corrections = vec![c];
    let w = apply(&s);
    let ai = mask(&w, 0, 0);
    assert_eq!(ai["MaskVersion"], json!(1));
    assert_eq!(ai["ReferencePoint"], json!("0.500000 0.500000"));
    assert_eq!(ai["ErrorReason"], json!(0));
    assert_eq!(ai["MaskActive"], json!(true));
    let range = &mask(&w, 0, 1)["CorrectionRangeMask"];
    assert_eq!(range["Version"], json!(3));
    assert_eq!(range["SampleType"], json!(0));
    assert_eq!(range["Invert"], json!(true), "intersect");

    let minimal = apply_with(
        &s,
        LuaOptions {
            mask_form: MaskForm::Minimal,
            ..LuaOptions::default()
        },
    );
    let ai = mask(&minimal, 0, 0);
    for k in ["MaskVersion", "ReferencePoint", "ErrorReason"] {
        assert!(!ai.contains_key(k), "{k}");
    }
    assert_eq!(ai["MaskActive"], json!(true), "every form");
    let range = &mask(&minimal, 0, 1)["CorrectionRangeMask"];
    assert!(range.get("Version").is_none() && range.get("SampleType").is_none());
}

#[test]
fn panel_switches_on_request_for_the_panels_written() {
    let mut s = read(json!({
        "Exposure2012": 0.2, "Sharpness": 40, "HueAdjustmentRed": 5,
        "ToneCurvePV2012": [0, 0, 128, 140, 255, 255]
    }));
    s.corrections = vec![subject_correction()];
    let none = LuaOptions {
        panel_switches: PanelSwitches::None,
        ..LuaOptions::default()
    };
    assert!(all_keys(&apply_with(&s, none).table)
        .iter()
        .all(|k| !k.starts_with("Enable")));
    // What E2 and E4 send, and the default (a no-op on E2's photo, so not
    // told from `None` yet): the mask switch next to corrections, nothing
    // else.
    let mask_only = apply(&s).table;
    let switches: Vec<&String> = mask_only
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| k.starts_with("Enable"))
        .collect();
    assert_eq!(switches, [MASK_SWITCH]);
    assert_eq!(mask_only[MASK_SWITCH], json!(true));
    let globals_only = read(json!({"Exposure2012": 0.2}));
    assert!(apply_with(
        &globals_only,
        LuaOptions {
            panel_switches: PanelSwitches::MaskOnly,
            ..LuaOptions::default()
        }
    )
    .table
    .get(MASK_SWITCH)
    .is_none());
    let opt = LuaOptions {
        panel_switches: PanelSwitches::All,
        ..LuaOptions::default()
    };
    let t = apply_with(&s, opt).table;
    let switches: Vec<&str> = t
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| k.starts_with("Enable"))
        .map(String::as_str)
        .collect();
    let mut switches = switches;
    switches.sort_unstable();
    assert_eq!(
        switches,
        [
            "EnableColorAdjustments",
            "EnableDetail",
            "EnableMaskGroupBasedCorrections",
            "EnableToneCurve"
        ]
    );
    assert!(t
        .as_object()
        .unwrap()
        .iter()
        .filter(|(k, _)| k.starts_with("Enable"))
        .all(|(_, v)| *v == json!(true)));
}

#[test]
fn every_panel_switch_and_prefix_names_registry_keys() {
    let globals: Vec<&str> = registry::iter()
        .filter(|(_, s)| s.level == Level::Global)
        .map(|(_, s)| s.name)
        .collect();
    for (switch, prefixes) in PANEL_SWITCHES {
        assert!(globals.contains(switch), "{switch} is not a registry key");
        for p in *prefixes {
            assert!(
                globals
                    .iter()
                    .any(|g| g.starts_with(p) && !g.starts_with("Enable")),
                "{switch}: no registry key starts with {p}"
            );
        }
    }
    assert!(globals.contains(&MASK_SWITCH));
}

#[test]
fn ai_masks_and_lens_blur_ask_for_an_ai_update() {
    let mut s = DevelopSettings::new();
    s.corrections = vec![subject_correction()];
    assert!(apply(&s).call.update_ai_settings);
    let off = LuaOptions {
        ai_update: false,
        ..LuaOptions::default()
    };
    assert!(!apply_with(&s, off).call.update_ai_settings);
    let blur = read(json!({"LensBlur": {"Active": true, "BlurAmount": 50}}));
    let w = apply(&blur);
    assert!(w.table.get("LensBlur").is_some(), "{}", w.table);
    assert!(w.call.update_ai_settings);
    assert!(
        !apply(&read(json!({"Exposure2012": 0.1})))
            .call
            .update_ai_settings
    );
}

fn look_settings() -> DevelopSettings {
    read(json!({"Look": {
        "Name": "Adobe Color", "UUID": "B952C231111CD8E0ECCF14B86BAA7077",
        "Amount": 0.8, "Group": {"x-default": "Profiles"}, "SupportsAmount": true,
        "Parameters": {"ConvertToGrayscale": false, "ProcessVersion": "15.4"}
    }}))
}

#[test]
fn the_look_goes_as_a_stub_by_default() {
    let w = apply(&look_settings());
    assert_eq!(
        w.table["Look"],
        json!({"Name": "Adobe Color", "UUID": "B952C231111CD8E0ECCF14B86BAA7077",
               "Amount": 0.8, "Stubbed": true})
    );
    // The profile's computed copies are not reported: Lightroom fills them in.
    assert!(w.skipped.is_empty(), "{:?}", w.skipped);
    let bare = apply_with(
        &look_settings(),
        LuaOptions {
            look_form: LookForm::BareStub,
            ..LuaOptions::default()
        },
    );
    assert!(bare.table["Look"].get("Stubbed").is_none());
}

#[test]
fn the_full_look_carries_its_record_and_parameters_on_request() {
    let w = apply_with(
        &look_settings(),
        LuaOptions {
            look_form: LookForm::Full,
            ..LuaOptions::default()
        },
    );
    let look = &w.table["Look"];
    assert_eq!(look["Group"], json!({"x-default": "Profiles"}));
    assert_eq!(look["SupportsAmount"], json!(true));
    assert_eq!(
        look["Parameters"],
        json!({"ConvertToGrayscale": false, "ProcessVersion": "15.4"})
    );
    assert!(w.skipped.is_empty(), "{:?}", w.skipped);
}

#[test]
fn a_camera_restricted_look_only_reaches_that_camera() {
    let s = read(json!({"Look": {"Name": "Synthetic Profile",
        "UUID": "00000000000000000000000000000001",
        "CameraModelRestriction": "Synthetic Camera"}}));
    let w = apply(&s);
    assert!(w.table.get("Look").is_none());
    assert_eq!(paths(&w), ["Look"]);
    let same = LuaMode::Apply(PhotoContext {
        camera: Some("Synthetic Camera".into()),
        ..PhotoContext::default()
    });
    let w = to_lua_value(&s, &same, &LuaOptions::default()).unwrap();
    assert!(w.table.get("Look").is_some());
    // The stub carries no restriction of its own: a COMPUTED copy Lightroom
    // refills, and the filter has already kept the Look to its camera, so
    // nothing is reported.
    assert!(w.table["Look"].get("CameraModelRestriction").is_none());
    assert!(w.skipped.is_empty(), "{:?}", w.skipped);
}

/// A synthetic Adobe Adaptive stub: the `AILook` state never goes (COMPUTED,
/// reported), so the table asks for the AI update that computes it.
#[test]
fn an_adaptive_look_asks_for_an_ai_update_in_every_form() {
    let s = read(json!({
        "Look": {"Name": "Synthetic Adaptive", "UUID": "00000000000000000000000000000002",
                 "Amount": 1, "isAdobeAdaptive": true, "Stubbed": true},
        "AILook": {"Version": 1}
    }));
    for form in [LookForm::Stub, LookForm::BareStub, LookForm::Full] {
        let opt = LuaOptions {
            look_form: form,
            ..LuaOptions::default()
        };
        let w = apply_with(&s, opt);
        assert!(w.table.get("Look").is_some(), "{form:?}");
        assert!(w.table.get("AILook").is_none(), "{form:?}");
        assert!(paths(&w).contains(&"AILook"), "{form:?}: {:?}", w.skipped);
        assert!(w.call.update_ai_settings, "{form:?}");
        let off = apply_with(
            &s,
            LuaOptions {
                ai_update: false,
                ..opt
            },
        );
        assert!(!off.call.update_ai_settings, "{form:?}");
    }
    // An ordinary profile does not.
    assert!(!apply(&look_settings()).call.update_ai_settings);
}

#[test]
fn amount_flags_only_for_a_preset_on_request_and_never_on_an_empty_table() {
    let s = read(json!({"Contrast2012": 40}));
    let preset = LuaMode::Preset(PhotoContext {
        file_kind: Some(FileKind::Raw),
        process_version: Some(ProcessVersion::V6),
        camera: None,
    });
    let opt = LuaOptions {
        preset_amount_flags: true,
        ..LuaOptions::default()
    };
    assert!(to_lua_value(&s, &preset, &LuaOptions::default())
        .unwrap()
        .table
        .get("SupportsAmount")
        .is_none());
    let t = to_lua_value(&s, &preset, &opt).unwrap().table;
    assert_eq!(t["SupportsAmount"], json!(true));
    assert_eq!(t["SupportsAmount2"], json!(true));
    assert!(to_lua_value(&DevelopSettings::new(), &preset, &opt)
        .unwrap()
        .is_empty());
    // The applyDevelopSettings route never carries them, whatever the
    // options say; otherwise a preset table and an applied one are the same.
    let applied = apply_with(&s, opt);
    assert!(applied.table.get("SupportsAmount").is_none());
    let mut flagged = t.clone();
    flagged.as_object_mut().unwrap().remove("SupportsAmount");
    flagged.as_object_mut().unwrap().remove("SupportsAmount2");
    assert_eq!(flagged, applied.table);
}

// --- determinism and reading back ------------------------------------------

#[test]
fn the_written_table_reads_back_into_the_filtered_model() {
    let mut s = read(json!({
        "Exposure2012": 0.35, "Contrast2012": 12, "LensProfileEnable": 1,
        "WhiteBalance": "Custom", "Temperature": 5600, "Tint": 6,
        "ToneCurvePV2012": [0, 0, 64, 50, 255, 255]
    }));
    s.corrections = vec![subject_correction(), sky_correction()];
    let w = apply(&s);
    let (back, warnings) = from_lua_value(&w.table, FileKindHint::Raw).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    for (id, v) in s.values() {
        assert_eq!(back.get(id), Some(v), "{}", id.spec().name);
    }
    assert_eq!(back.corrections.len(), 2);
    for (a, b) in back.corrections.iter().zip(&s.corrections) {
        // The model's own values, and the adaptive-preset zeros (the
        // legacy PV2010 ones read back as bookkeeping, `extra`).
        let mut want = b.local.clone();
        for k in ADAPTIVE_PRESET_LOCALS {
            let kid = id(Level::Correction, k);
            if crate::model::correction::is_local_adjustment(kid.spec()) {
                want.entry(kid).or_insert(Value::Real(Finite::ZERO));
            } else {
                assert_eq!(
                    a.extra.values.get(&kid),
                    Some(&Value::Real(Finite::ZERO)),
                    "{k}"
                );
            }
        }
        assert_eq!(a.local, want);
        assert_eq!(a.sync_id, b.sync_id);
        assert_eq!(a.masks.len(), b.masks.len());
        for (x, y) in a.masks.iter().zip(&b.masks) {
            assert_eq!((&x.tool, x.combine), (&y.tool, y.combine));
        }
    }
    assert_eq!(w, apply(&s), "deterministic");
}

#[cfg(feature = "test-roundtrip")]
#[test]
fn the_round_trip_mode_keeps_opaque_lua_content_and_refuses_xmp_content() {
    let s = read(json!({"Exposure2012": 0.5, "UprightTransform_0": "1 0 0"}));
    let t = to_lua_value(&s, &LuaMode::TestRoundTrip, &LuaOptions::default())
        .unwrap()
        .table;
    assert_eq!(t["UprightTransform_0"], json!("1 0 0"));
    let mut x = DevelopSettings::new();
    x.opaque.push(OpaqueEntry {
        ns: None,
        name: "Foreign".into(),
        value: Opaque::Xmp(crate::model::XmpNode::text("x")),
    });
    assert!(matches!(
        to_lua_value(&x, &LuaMode::TestRoundTrip, &LuaOptions::default()),
        Err(WireError::NotLua { .. })
    ));
}
