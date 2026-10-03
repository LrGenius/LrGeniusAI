//! The Lua writer's wire goldens: `server-rs/testdata/develop/wire/*.json`.
//!
//! Each golden is the table `lua::to_lua_value` writes in apply mode for one
//! shape, with the provisional options ([`LuaOptions::PROVISIONAL`]): exactly
//! what the plugin decodes with JSON.lua and hands to
//! `photo:applyDevelopSettings()`. The plugin spec
//! `plugin/spec/native_wire_format_spec.lua` reads the same files.
//!
//! **Not frozen**: they change when experiments E1, E2, E4 and E11 settle a
//! provisional option. After an intended change to the writer or to
//! `LuaOptions::PROVISIONAL`, regenerate them and review the diff:
//!
//! ```text
//! LRG_BLESS=1 cargo test -p lrg-develop --test lua_wire_goldens
//! ```
//!
//! Source models are committed scrubbed fixtures (`testdata/develop/lua/`)
//! or built here (builders, hand-written tables); built sync ids are replaced
//! by synthetic ones (twenty zeros and twelve hex digits), as in the XMP
//! writer goldens, so the files pass the fixture-hygiene checks.
//!
//! Besides the bytes, every case pins what was skipped, how to apply it
//! ([`ApplyCall`]), that the table passes [`check_wire`], and that the
//! golden file reads back without a warning into exactly the values that
//! were written: every filtered value is in it or reported, and it holds
//! nothing beyond those and the writer's fixed form
//! (`support/lua_apply.rs`, the check the sweeps run).
//!
//! Also here, because the plugin side reads them:
//!
//! - every case under every single option flip
//!   (`every_golden_case_is_clean_under_every_option_flip`): flipping a
//!   `LuaOptions` field and re-blessing needs no test or reader change;
//! - `wire/key_classes.txt`, the registry's key classes for the plugin
//!   spec, generated here like the goldens;
//! - the adaptive-preset `Local*` list against the experiments' own
//!   (`DevelopExperiments.lua`).

#[allow(dead_code)] // `header` is for the XMP tests
#[path = "support/flat.rs"]
mod flat;
#[allow(dead_code)] // the counted sweep entry point is for the local sweeps
#[path = "support/lua_apply.rs"]
mod lua_apply;
#[allow(dead_code)] // only `same` is used (by `lua_apply`)
#[path = "support/preset.rs"]
mod preset;
#[allow(dead_code)] // only `generic_path` is used (by `lua_apply`)
mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use lrg_develop::build::{
    corrections, CorrectionBuilder, LuminanceRange, PeoplePart, SensorPoint, SyncNamespace,
};
use lrg_develop::lua::{
    check_wire, from_lua_str, from_lua_value, lua_never_written, to_lua_value, ApplyCall, LookForm,
    LuaMode, LuaOptions, PhotoContext, ADAPTIVE_PRESET_LOCALS,
};
use lrg_develop::model::{
    DevelopSettings, FileKind, Hex32, LinearGradient, RadialGradient, Semantic,
};
use lrg_develop::registry::{self, CurveKind, Level, NumFmt, ProcessVersion, ValueKind};
use lrg_develop::FileKindHint;
use lua_apply::{flips, LuaApply};
use serde_json::{json, Map, Value as J};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/develop/wire")
}

fn fixture(name: &str) -> DevelopSettings {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/develop/lua")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    let (s, w) = from_lua_str(&text, FileKindHint::Unknown).unwrap();
    assert!(w.is_empty(), "{name}: {w:?}");
    s
}

fn table(v: J, hint: FileKindHint) -> DevelopSettings {
    let (s, w) = from_lua_value(&v, hint).unwrap();
    assert!(w.is_empty(), "{w:?}");
    s
}

fn synthetic(n: u64) -> Hex32 {
    Hex32::parse(&format!("{:020}{n:012X}", 0)).unwrap()
}

/// Replaces the derived (SHA-256) sync ids with synthetic ones.
fn with_synthetic_ids(mut s: DevelopSettings, first: u64) -> DevelopSettings {
    let mut n = first;
    for c in &mut s.corrections {
        n += 1;
        c.sync_id = Some(synthetic(n));
        for m in &mut c.masks {
            n += 1;
            m.sync_id = Some(synthetic(n));
        }
    }
    s
}

struct Case {
    golden: &'static str,
    settings: DevelopSettings,
    photo: PhotoContext,
    options: LuaOptions,
    /// The paths `LuaWritten::skipped` must name, in order.
    skipped: Vec<String>,
    call: ApplyCall,
}

fn raw() -> PhotoContext {
    PhotoContext {
        file_kind: Some(FileKind::Raw),
        process_version: Some(ProcessVersion::V6),
        camera: None,
    }
}

fn non_raw() -> PhotoContext {
    PhotoContext {
        file_kind: Some(FileKind::NonRaw),
        ..raw()
    }
}

fn case(golden: &'static str, settings: DevelopSettings, photo: PhotoContext) -> Case {
    Case {
        golden,
        settings,
        photo,
        options: LuaOptions::PROVISIONAL,
        skipped: Vec::new(),
        call: ApplyCall::default(),
    }
}

const AI: ApplyCall = ApplyCall {
    update_ai_settings: true,
    flatten_auto_now: false,
};

fn cases() -> Vec<Case> {
    let ns = SyncNamespace::lrgenius();
    let mut out = Vec::new();

    // Basic global sliders. `ProcessVersion` is the example's and is never
    // written; the flags go as 0/1.
    out.push(case(
        "global_basic.json",
        table(
            json!({
                "ProcessVersion": "15.4",
                "CameraProfile": "Adobe Standard",
                "Exposure2012": 0.35, "Contrast2012": 12, "Highlights2012": -40,
                "Shadows2012": 30, "Whites2012": 10, "Blacks2012": -8,
                "Texture": 10, "Clarity2012": 15, "Dehaze": 5,
                "Vibrance": 20, "Saturation": -5,
                "Sharpness": 40, "SharpenRadius": 1, "SharpenDetail": 25,
                "LuminanceSmoothing": 10,
                "PostCropVignetteAmount": -10, "GrainAmount": 15,
                "LensProfileEnable": 1, "AutoLateralCA": 1,
                "ConvertToGrayscale": false
            }),
            FileKindHint::Raw,
        ),
        raw(),
    ));

    // White balance of a raw file: the Kelvin family with Custom.
    out.push(case(
        "wb_raw_custom.json",
        table(
            json!({"WhiteBalance": "Custom", "Temperature": 5600, "Tint": 6}),
            FileKindHint::Raw,
        ),
        raw(),
    ));

    // White balance of a non-raw file: the incremental family; no mode in
    // the source, so the writer stamps Custom.
    out.push(case(
        "wb_non_raw_incremental.json",
        table(
            json!({"IncrementalTemperature": 12, "IncrementalTint": -4}),
            FileKindHint::NonRaw,
        ),
        non_raw(),
    ));

    // Global tone curves (flat number lists) and the parametric curve.
    out.push(case(
        "tone_curves.json",
        table(
            json!({
                "ToneCurvePV2012": [0, 0, 64, 52, 128, 135, 192, 210, 255, 255],
                "ToneCurvePV2012Red": [0, 0, 128, 132, 255, 255],
                "ToneCurvePV2012Green": [0, 0, 255, 255],
                "ToneCurvePV2012Blue": [0, 8, 128, 124, 255, 245],
                "ParametricShadows": -10, "ParametricDarks": 5,
                "ParametricLights": 8, "ParametricHighlights": -12,
                "ParametricShadowSplit": 25, "ParametricMidtoneSplit": 50,
                "ParametricHighlightSplit": 75
            }),
            FileKindHint::Raw,
        ),
        raw(),
    ));

    // HSL, colour grading and the split-toning keys it shares.
    out.push(case(
        "hsl_color_grading.json",
        table(
            json!({
                "HueAdjustmentRed": 5, "HueAdjustmentOrange": -8, "HueAdjustmentBlue": 12,
                "SaturationAdjustmentOrange": -10, "SaturationAdjustmentGreen": -25,
                "SaturationAdjustmentBlue": 15,
                "LuminanceAdjustmentOrange": 10, "LuminanceAdjustmentBlue": -20,
                "SplitToningShadowHue": 215, "SplitToningShadowSaturation": 12,
                "SplitToningHighlightHue": 45, "SplitToningHighlightSaturation": 10,
                "SplitToningBalance": 10,
                "ColorGradeMidtoneHue": 30, "ColorGradeMidtoneSat": 6,
                "ColorGradeShadowLum": -5, "ColorGradeHighlightLum": 4,
                "ColorGradeBlending": 60, "ColorGradeGlobalHue": 0, "ColorGradeGlobalSat": 0
            }),
            FileKindHint::Raw,
        ),
        raw(),
    ));

    // A correction set with semantic masks: subject and sky.
    let mut s = DevelopSettings::new();
    s.corrections = corrections([
        CorrectionBuilder::new("Subject", &ns)
            .local_ui("LocalExposure2012", 0.3)
            .unwrap()
            .local_ui("LocalClarity2012", 10.0)
            .unwrap()
            .add(Semantic::Subject),
        CorrectionBuilder::new("Sky", &ns)
            .local_ui("LocalHighlights2012", -40.0)
            .unwrap()
            .local_ui("LocalDehaze", 15.0)
            .unwrap()
            .add(Semantic::Sky),
    ])
    .unwrap();
    out.push(Case {
        call: AI,
        ..case(
            "masks_subject_sky.json",
            with_synthetic_ids(s, 0xD00),
            raw(),
        )
    });

    // Linear and radial gradients (PHOTO: for the photo they were made for).
    let mut s = DevelopSettings::new();
    s.corrections = corrections([
        CorrectionBuilder::new("Darker sky", &ns)
            .local_ui("LocalExposure2012", -0.5)
            .unwrap()
            .add(
                LinearGradient::new(
                    SensorPoint::new(0.5, 0.55).unwrap(),
                    SensorPoint::new(0.5, 0.15).unwrap(),
                )
                .unwrap(),
            ),
        CorrectionBuilder::new("Spotlight", &ns)
            .local_ui("LocalExposure2012", 0.25)
            .unwrap()
            .add(RadialGradient::new(0.25, 0.3, 0.75, 0.7, 60).unwrap()),
    ])
    .unwrap();
    out.push(case(
        "masks_linear_radial.json",
        with_synthetic_ids(s, 0xE00),
        raw(),
    ));

    // A luminance range intersected with a semantic mask, and a subtract.
    let mut s = DevelopSettings::new();
    s.corrections = corrections([CorrectionBuilder::new("Bright sky", &ns)
        .local_ui("LocalHighlights2012", -30.0)
        .unwrap()
        .add(Semantic::Sky)
        .subtract(Semantic::Subject)
        .intersect(LuminanceRange::highs(0.5, 0.2).unwrap())])
    .unwrap();
    out.push(Case {
        call: AI,
        ..case(
            "mask_luminance_intersect.json",
            with_synthetic_ids(s, 0xF00),
            raw(),
        )
    });

    // A people part in preset form (every person).
    let mut s = DevelopSettings::new();
    s.corrections = corrections([CorrectionBuilder::new("Eyes", &ns)
        .local_ui("LocalSaturation", 30.0)
        .unwrap()
        .add(Semantic::people_part(PeoplePart::IrisAndPupil))])
    .unwrap();
    out.push(Case {
        call: AI,
        ..case(
            "mask_people_part.json",
            with_synthetic_ids(s, 0x1000),
            raw(),
        )
    });

    // Local point curves on a correction: lists of "x,y" strings (rule 5),
    // next to the subject mask they apply through.
    out.push(Case {
        call: AI,
        ..case(
            "mask_local_curves.json",
            table(
                json!({
                    "MaskGroupBasedCorrections": [{
                        "What": "Correction", "CorrectionAmount": 1, "CorrectionActive": true,
                        "CorrectionName": "Curves",
                        "CorrectionSyncID": synthetic(0x1101).as_str(),
                        "LocalCurveRefineSaturation": 90,
                        "MainCurve": ["0,0", "64,56", "128,136", "255,255"],
                        "BlueCurve": ["0,8", "255,248"],
                        "CorrectionMasks": [{
                            "What": "Mask/Image", "MaskActive": true, "MaskName": "Subject",
                            "MaskBlendMode": 0, "MaskInverted": false, "MaskValue": 1,
                            "MaskSubType": 1, "MaskSyncID": synthetic(0x1102).as_str()
                        }]
                    }]
                }),
                FileKindHint::Raw,
            ),
            raw(),
        )
    });

    // The Look of a committed fixture, as a stub (provisional) and whole.
    let mut look = DevelopSettings::new();
    look.look = fixture("mask_range_color_area.json").look;
    assert!(look.look.is_some());
    out.push(case("look_stub.json", look.clone(), raw()));
    out.push(Case {
        options: LuaOptions {
            look_form: LookForm::Full,
            ..LuaOptions::PROVISIONAL
        },
        ..case("look_full.json", look, raw())
    });

    // No masks: the source's only correction is a brush, which never
    // reaches a photo, so `MaskGroupBasedCorrections` is absent (never `[]`).
    out.push(Case {
        skipped: vec!["MaskGroupBasedCorrections[0]".into()],
        ..case(
            "no_masks.json",
            table(
                json!({
                    "Exposure2012": 0.5,
                    "MaskGroupBasedCorrections": [{
                        "What": "Correction", "CorrectionAmount": 1, "CorrectionActive": true,
                        "LocalExposure2012": 0.1,
                        "CorrectionMasks": [{"What": "Mask/Aggregate", "MaskActive": true,
                            "MaskBlendMode": 0, "MaskValue": 1, "MaskInverted": false}]
                    }]
                }),
                FileKindHint::Raw,
            ),
            raw(),
        )
    });

    // A real read-back (a committed, scrubbed fixture) applied to its own
    // photo: digests, runtime ids and computed mask fields are gone (and
    // reported, as every non-default value the filter removes), the legacy
    // PV2010 sliders and Upright's computed state too, the As Shot white
    // balance is not pinned, the sky mask stays.
    const M: &str = "MaskGroupBasedCorrections[0].CorrectionMasks[0]";
    out.push(Case {
        skipped: [
            "Exposure",
            "Contrast",
            "Brightness",
            "Shadows",
            "FillLight",
            "HighlightRecovery",
            "Clarity",
            "Defringe",
            "ChromaticAberrationR",
            "ChromaticAberrationB",
            "UprightVersion",
            "UprightTransformCount",
            "UprightCenterMode",
            "UprightCenterNormX",
            "UprightCenterNormY",
            "UprightFocalMode",
            "UprightFocalLength35mm",
            "UprightPreview",
            "UprightFourSegmentsCount",
            "GrainSeed",
        ]
        .into_iter()
        .map(String::from)
        .chain(
            [
                "MaskID",
                "MaskDigest",
                "InputDigest",
                "InputDigestVersion",
                "LocalInputDigest",
                "LocalInputDigestVersion",
                "ModelVersion",
                "WholeImageArea",
                "Origin",
                "FullMaskSize",
            ]
            .into_iter()
            .map(|k| format!("{M}.{k}")),
        )
        .chain(
            [
                "CorrectionID",
                "CorrectionReferenceX",
                "CorrectionReferenceY",
            ]
            .into_iter()
            .map(|k| format!("MaskGroupBasedCorrections[0].{k}")),
        )
        .chain(
            ["Temperature", "Tint", "LensProfileDigest", "orientation"]
                .into_iter()
                .map(String::from),
        )
        .collect(),
        call: AI,
        ..case(
            "fixture_mask_ai_sky.json",
            fixture("mask_ai_sky.json"),
            raw(),
        )
    });

    // More committed, scrubbed readbacks applied to their own photo, for
    // shapes apply mode writes that the cases above do not hold. Each loses
    // what the sky fixture loses: the legacy sliders and Upright's state,
    // runtime ids, and (where present) digests and computed mask fields.
    let c = |i: usize| -> Vec<String> {
        [
            "CorrectionID",
            "CorrectionReferenceX",
            "CorrectionReferenceY",
        ]
        .iter()
        .map(|k| format!("MaskGroupBasedCorrections[{i}].{k}"))
        .collect()
    };
    let m = |i: usize, j: usize, keys: &[&str]| -> Vec<String> {
        keys.iter()
            .map(|k| format!("MaskGroupBasedCorrections[{i}].CorrectionMasks[{j}].{k}"))
            .collect()
    };
    let strings = |keys: &[&str]| -> Vec<String> { keys.iter().map(|k| (*k).to_owned()).collect() };
    const COMPUTED_AI: &[&str] = &[
        "MaskDigest",
        "InputDigest",
        "InputDigestVersion",
        "ModelVersion",
        "WholeImageArea",
        "Origin",
        "FullMaskSize",
    ];
    let with_override = |i: usize| -> Vec<String> {
        let mut keys = vec!["MaskID"];
        keys.extend(COMPUTED_AI);
        keys.push("DidOverrideInputDigestMismatch");
        [m(i, 0, &keys), c(i)].concat()
    };
    let object = |i: usize| -> Vec<String> {
        let mut keys = vec!["MaskID", "Gesture[0].MaskID"];
        keys.extend(COMPUTED_AI);
        [m(i, 0, &keys), m(i, 1, &["MaskID"]), c(i)].concat()
    };
    let plain = |i: usize| -> Vec<String> { [m(i, 0, &["MaskID"]), c(i)].concat() };
    let legacy = || strings(LEGACY_AND_UPRIGHT);

    // A colour range: `PointModels` (a list of compound strings),
    // `ColorAmount`, `CorrectionRangeMask.Type` 1.
    out.push(Case {
        skipped: [
            legacy(),
            plain(0),
            strings(&["Temperature", "Tint", "LensProfileDigest", "orientation"]),
        ]
        .concat(),
        ..case(
            "fixture_mask_range_color.json",
            fixture("mask_range_color.json"),
            raw(),
        )
    });
    // AI masks next to an area colour range (`AreaModels`,
    // `ColorRangeMaskAreaSampleInfo`); the range's `SampleType` (not 0
    // here) has an UNKNOWN policy and is reported.
    out.push(Case {
        skipped: [
            legacy(),
            with_override(0),
            with_override(1),
            with_override(2),
            m(3, 0, &["MaskID", "CorrectionRangeMask.SampleType"]),
            c(3),
            strings(&["Temperature", "Tint", "orientation"]),
        ]
        .concat(),
        call: AI,
        ..case(
            "fixture_mask_range_color_area.json",
            fixture("mask_range_color_area.json"),
            raw(),
        )
    });
    // Select Object: the polygon `Gesture`/`Points`, an untyped AI mask
    // (`What = "Mask/Image"` without a subtype), and an adaptive profile's
    // `AILook` state, which is never written.
    out.push(Case {
        skipped: [
            legacy(),
            strings(&["AILook"]),
            plain(0),
            plain(1),
            object(2),
            object(3),
            plain(4),
            plain(5),
            strings(&["Temperature", "Tint", "LensProfileDigest", "orientation"]),
        ]
        .concat(),
        call: AI,
        ..case(
            "fixture_mask_select_object_polygon.json",
            fixture("mask_select_object_polygon.json"),
            raw(),
        )
    });
    // The `LensBlur` struct, whose gate asks for the AI update (its sampled
    // focal state belongs to the source photo).
    out.push(Case {
        skipped: [
            legacy(),
            strings(&[
                "LensBlur.FocalRange",
                "LensBlur.FocalRangeSource",
                "LensBlur.SampledArea",
                "LensBlur.SampledRange",
                "LensBlur.SubjectRange",
                "LensBlur.ImageOrientation",
            ]),
            plain(0),
            plain(1),
            strings(&["LensProfileDigest", "orientation"]),
        ]
        .concat(),
        call: AI,
        ..case(
            "fixture_lensblur_object.json",
            fixture("lensblur_object.json"),
            raw(),
        )
    });
    out
}

/// What every committed readback fixture loses when applied: the legacy
/// PV2010 sliders and Upright's computed state.
const LEGACY_AND_UPRIGHT: &[&str] = &[
    "Exposure",
    "Contrast",
    "Brightness",
    "Shadows",
    "FillLight",
    "HighlightRecovery",
    "Clarity",
    "Defringe",
    "ChromaticAberrationR",
    "ChromaticAberrationB",
    "UprightVersion",
    "UprightTransformCount",
    "UprightCenterMode",
    "UprightCenterNormX",
    "UprightCenterNormY",
    "UprightFocalMode",
    "UprightFocalLength35mm",
    "UprightPreview",
    "UprightFourSegmentsCount",
];

/// `j` with every object's keys sorted, whatever map order `serde_json` was
/// built with.
fn canonical(j: &J) -> J {
    match j {
        J::Object(m) => {
            let sorted: BTreeMap<&String, J> = m.iter().map(|(k, v)| (k, canonical(v))).collect();
            let mut out = Map::new();
            for (k, v) in sorted {
                out.insert(k.clone(), v);
            }
            J::Object(out)
        }
        J::Array(a) => J::Array(a.iter().map(canonical).collect()),
        v => v.clone(),
    }
}

fn text(j: &J) -> String {
    serde_json::to_string_pretty(&canonical(j)).unwrap() + "\n"
}

#[test]
fn the_wire_goldens_match_the_writer() {
    let bless = std::env::var_os("LRG_BLESS").is_some_and(|v| v == "1");
    let cases = cases();
    for c in &cases {
        let w = to_lua_value(&c.settings, &LuaMode::Apply(c.photo.clone()), &c.options)
            .unwrap_or_else(|e| panic!("{}: {e}", c.golden));
        check_wire(&w.table).unwrap_or_else(|e| panic!("{}: {e}", c.golden));
        assert!(!w.is_empty(), "{}: an empty table", c.golden);
        let skipped: Vec<String> = w.skipped.iter().map(|k| k.path.clone()).collect();
        assert_eq!(skipped, c.skipped, "{}", c.golden);
        assert_eq!(w.call, c.call, "{}", c.golden);
        let fresh = text(&w.table);
        let path = dir().join(c.golden);
        if bless {
            std::fs::create_dir_all(dir()).unwrap();
            std::fs::write(&path, &fresh).unwrap();
            continue;
        }
        let golden = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e} (bless with LRG_BLESS=1)", c.golden));
        // A Windows checkout may have turned LF into CRLF; `.gitattributes`
        // keeps this folder LF, but compare content either way.
        assert_eq!(
            golden.replace("\r\n", "\n"),
            fresh,
            "{}: the writer's output changed; if intended, bless with LRG_BLESS=1",
            c.golden
        );
    }
    // No stale golden: every file in wire/ is a case.
    let mut on_disk: Vec<String> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .collect();
    on_disk.sort();
    let mut expected: Vec<String> = cases.iter().map(|c| c.golden.to_owned()).collect();
    expected.sort();
    assert_eq!(on_disk, expected, "wire/ holds a file no case writes");
}

/// Each golden file reads back without a warning into exactly what was
/// written: every value of the filtered settings is in it (or named, or an
/// ancestor named, in the case's `skipped`), and it holds nothing beyond
/// those and the writer's fixed form under the case's options.
#[test]
fn the_wire_goldens_read_back_into_what_was_written() {
    // A blessing run writes the files in the other test, concurrently; this
    // one checks them on the next, ordinary run.
    if std::env::var_os("LRG_BLESS").is_some_and(|v| v == "1") {
        return;
    }
    let mut all = LuaApply::default();
    for c in cases() {
        let path = dir().join(c.golden);
        let Ok(text) = std::fs::read_to_string(&path) else {
            panic!("{}: missing (bless with LRG_BLESS=1)", c.golden);
        };
        let table: J = serde_json::from_str(&text).unwrap();
        let mut one = LuaApply::default();
        one.check(&c.settings, &c.photo, false, &c.options, &table, &c.skipped);
        if !one.is_clean() {
            one.report("  ");
            panic!("{}: does not read back into what was written", c.golden);
        }
        all.check(&c.settings, &c.photo, false, &c.options, &table, &c.skipped);
    }
    all.report("");
    assert!(all.compared > 300, "only {} values compared", all.compared);
}

/// Every case with each `LuaOptions` field flipped to each of its other
/// values in turn, in apply mode and as a plugin preset: a wire-safe table
/// that reads back clean both ways. So when an experiment flips a field in
/// `LuaOptions::PROVISIONAL`, re-blessing is all the Rust side needs.
#[test]
fn every_golden_case_is_clean_under_every_option_flip() {
    let mut n = 0;
    for c in cases() {
        for (how, opt) in flips(c.options) {
            for mode in [
                LuaMode::Apply(c.photo.clone()),
                LuaMode::Preset(c.photo.clone()),
            ] {
                let mut one = LuaApply::default();
                let w = one
                    .settings_for(&c.settings, &mode, &opt)
                    .unwrap_or_else(|| panic!("{} ({how}): write error", c.golden));
                if !one.is_clean() {
                    one.report("  ");
                    panic!("{} ({how}, {mode:?}): not clean", c.golden);
                }
                assert!(!w.is_empty(), "{} ({how})", c.golden);
                n += 1;
            }
        }
    }
    eprintln!("{n} flipped tables checked");
    assert!(n > 300, "only {n} tables");
}

// --- the plugin side's inputs ----------------------------------------------

/// The `Local*` keys of Adobe's adaptive-preset corrections: the writer's
/// [`ADAPTIVE_PRESET_LOCALS`] is the list the experiments apply
/// (`LOCAL_KEYS` in `DevelopExperiments.lua`), in the same order, so a
/// passing E2/E4 vouches for the form `LocalForm::AdaptivePreset` writes.
#[test]
fn the_adaptive_preset_locals_are_the_experiments_own() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../plugin/LrGeniusAI.lrdevplugin/DevelopExperiments.lua");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    let start = text
        .find("local LOCAL_KEYS = {")
        .expect("DevelopExperiments.lua has no LOCAL_KEYS");
    let body = &text[start..];
    let body = &body[..body.find('}').expect("LOCAL_KEYS is not closed")];
    let theirs: Vec<&str> = body.split('"').skip(1).step_by(2).collect();
    assert_eq!(theirs, ADAPTIVE_PRESET_LOCALS);
    for k in ADAPTIVE_PRESET_LOCALS {
        assert!(
            registry::lookup(Level::Correction, k).is_some(),
            "{k} is not a correction key"
        );
    }
}

/// The registry's classes of every key a wire table can hold, by name, for
/// `plugin/spec/native_wire_format_spec.lua` (which checks every value's
/// Lua type against them). A name at several levels gets every class it
/// has; `never` alone marks a name the writer never writes at any level.
fn key_classes() -> String {
    let mut classes: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut written: BTreeSet<&str> = BTreeSet::new();
    let mut names: BTreeSet<&str> = BTreeSet::new();
    for (_, spec) in registry::iter() {
        // The preset header never reaches a Lua table.
        if spec.level == Level::Header {
            continue;
        }
        names.insert(spec.name);
        // The panel switches are written on request (`panel_switches`).
        let switch = spec.level == Level::Global && spec.name.starts_with("Enable");
        if lua_never_written(spec) && !switch {
            continue;
        }
        written.insert(spec.name);
        let compound = spec.fmt == NumFmt::CompoundFixed6;
        let class = if spec.is_mask_enum() {
            "maskenum"
        } else {
            match spec.kind.lua_form() {
                ValueKind::Int | ValueKind::EnumInt(_) | ValueKind::VersionU32 => "integer",
                ValueKind::Real => "number",
                ValueKind::IntFlag => "flag",
                ValueKind::Bool(_) => "boolean",
                ValueKind::Enum(_) | ValueKind::Str | ValueKind::VersionStr | ValueKind::Hex32 => {
                    if compound {
                        "compound"
                    } else {
                        "string"
                    }
                }
                ValueKind::StrSeq => {
                    if compound {
                        "compounds"
                    } else {
                        "strings"
                    }
                }
                ValueKind::Curve(CurveKind::Global) => "globalcurve",
                ValueKind::Curve(CurveKind::Local) => "localcurve",
                ValueKind::LangAlt | ValueKind::Struct(_) | ValueKind::Settings => "table",
                ValueKind::StructSeq(_) | ValueKind::CorrectionSeq | ValueKind::ComponentSeq => {
                    "list"
                }
                ValueKind::Any | ValueKind::PerFormat { .. } => "any",
            }
        };
        classes.entry(spec.name).or_default().insert(class);
    }
    // A plugin preset's amount flags (`LuaMode::Preset`): `SupportsAmount`
    // is a `Look` field too, `SupportsAmount2` only a preset header key.
    for flag in ["SupportsAmount", "SupportsAmount2"] {
        names.insert(flag);
        written.insert(flag);
        classes.entry(flag).or_default().insert("boolean");
    }
    let mut out = String::from(
        "# Generated by server-rs/crates/lrg-develop/tests/lua_wire_goldens.rs from\n\
         # lrg-develop's key registry; do not edit. Regenerate with\n\
         # LRG_BLESS=1 cargo test -p lrg-develop --test lua_wire_goldens\n\
         # Read by plugin/spec/native_wire_format_spec.lua: one key name per line,\n\
         # then the classes of its values (a name at several levels has several);\n\
         # `never`: the Lua writer never writes it, at any level.\n",
    );
    for name in &names {
        let line = match classes.get(name) {
            Some(c) => c.iter().copied().collect::<Vec<_>>().join(" "),
            None => {
                assert!(!written.contains(name));
                "never".to_owned()
            }
        };
        out.push_str(&format!("{name} {line}\n"));
    }
    out
}

#[test]
fn the_key_classes_match_the_registry() {
    let path = dir().join("key_classes.txt");
    let fresh = key_classes();
    if std::env::var_os("LRG_BLESS").is_some_and(|v| v == "1") {
        std::fs::write(&path, &fresh).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path:?}: {e} (bless with LRG_BLESS=1)"));
    assert_eq!(
        committed.replace("\r\n", "\n"),
        fresh,
        "the registry changed; bless with LRG_BLESS=1 and re-run busted"
    );
    // The classes the spec relies on are there.
    for (name, class) in [
        ("Exposure2012", "number"),
        ("MaskSubType", "maskenum"),
        ("ErrorReason", "maskenum"),
        ("Type", "maskenum"),
        ("LensProfileEnable", "flag"),
        ("ReferencePoint", "compound"),
        ("PointModels", "compounds"),
        ("MainCurve", "localcurve"),
        ("ToneCurvePV2012", "globalcurve"),
        ("CorrectionID", "never"),
        ("AILook", "never"),
    ] {
        assert!(
            fresh
                .lines()
                .any(|l| l.split(' ').next() == Some(name) && l.split(' ').any(|c| c == class)),
            "{name} is not {class}"
        );
    }
}
