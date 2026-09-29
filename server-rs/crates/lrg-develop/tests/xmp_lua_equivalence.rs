//! The XMP and Lua readers land in the same model.
//!
//! For each XMP fixture below, the same settings are written here by hand in
//! the form `photo:getDevelopSettings()` takes after JSON.lua (booleans as
//! JSON booleans, global curves as flat number lists, local curves as
//! `"x,y"` strings, language alternatives as `{"x-default": ...}`). Both
//! readers must produce equal [`DevelopSettings`]: the same key ids, value
//! variants, correction and mask classification, `Look` and file kind.
//!
//! Two differences are removed from both sides before comparing:
//!
//! - global keys that occur in only one format ([`Presence::XmpOnly`] such
//!   as `HasSettings`/`HasCrop`, [`Presence::LuaOnly`] such as the `Enable*`
//!   panel switches);
//! - `Look.Parameters`, which each reader keeps verbatim in its own format
//!   (`Opaque::Json` vs `Opaque::Xmp`).
//!
//! The other differences that exist by design are not stripped; these
//! fixtures avoid them instead:
//!
//! - `PerFormat` keys (`PointColors`, `RetouchInfo`): a `StructList` from
//!   Lua, a `StrList` from XMP;
//! - `ValueKind::Any` keys (`RGBTables`, `ProfileGainTableMap`, ...):
//!   `Opaque::Json` vs `Opaque::Xmp`;
//! - struct-, correction- and mask-level Lua-only keys (`CorrectionID`,
//!   `CorrectionReferenceX/Y`, `MaskID`, `Look.isAdobeAdaptive`);
//! - the `PointColors` sentinel (one item of 19 × `-1`) Lightroom writes
//!   into every sidecar where `getDevelopSettings()` returns `[]`;
//! - opaque entries (unknown keys, wrong types), which differ the same way;
//!   these fixtures have none (checked).

use std::path::PathBuf;

use lrg_develop::lua::from_lua_value;
use lrg_develop::model::DevelopSettings;
use lrg_develop::registry::{Level, Presence, StructKind};
use lrg_develop::xmp::parse;
use lrg_develop::FileKindHint;
use serde_json::{json, Value as J};

fn xmp(name: &str) -> DevelopSettings {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/develop/xmp")
        .join(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    let doc = parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(doc.warnings.is_empty(), "{name}: {:?}", doc.warnings);
    doc.develop
}

fn lua(v: J) -> DevelopSettings {
    let (s, w) = from_lua_value(&v, FileKindHint::Unknown).expect("a settings table");
    assert!(w.is_empty(), "{w:?}");
    s
}

/// Removes what differs by format (see the module docs).
fn comparable(mut s: DevelopSettings) -> DevelopSettings {
    assert!(s.opaque.is_empty(), "opaque entries: {:?}", s.opaque);
    let one_format: Vec<_> = s
        .values()
        .map(|(id, _)| id)
        .filter(|id| id.spec().presence != Presence::Both)
        .collect();
    for id in one_format {
        s.remove(id);
    }
    if let Some(look) = &mut s.look {
        look.rest
            .fields
            .take(Level::Struct(StructKind::Look), "Parameters")
            .expect("the fixtures' Look has Parameters");
    }
    s
}

fn assert_same(name: &str, lua_form: J) {
    let from_xmp = comparable(xmp(name));
    let from_lua = comparable(lua(lua_form));
    assert_eq!(from_xmp, from_lua, "{name}");
}

/// One correction with a single component, as JSON.lua writes it.
fn correction(n: u32, sync: &str, local: J, mask: J) -> J {
    let mut c = json!({
        "What": "Correction",
        "CorrectionAmount": 1,
        "CorrectionActive": true,
        "CorrectionName": format!("Correction {n}"),
        "CorrectionSyncID": sync,
        "CorrectionMasks": [mask],
    });
    for (k, v) in local.as_object().expect("local adjustments") {
        c[k] = v.clone();
    }
    c
}

/// A `Mask/Image` component (added, not inverted).
fn ai_mask(n: u32, sync: &str, subtype: i64, extra: J) -> J {
    let mut m = json!({
        "What": "Mask/Image",
        "MaskActive": true,
        "MaskName": format!("Mask {n}"),
        "MaskBlendMode": 0,
        "MaskInverted": false,
        "MaskSyncID": sync,
        "MaskValue": 1,
        "MaskVersion": 1,
        "MaskSubType": subtype,
    });
    for (k, v) in extra.as_object().expect("extra fields") {
        m[k] = v.clone();
    }
    m
}

#[test]
fn number_formats() {
    assert_same(
        "number_formats.xmp",
        json!({
            // Synthetic: `Version` and `CompatibleVersion` are `Presence::Both`
            // in the registry, but `getDevelopSettings()` returned neither in
            // any training row; they are here to compare the two readers'
            // typing of a version string and a packed version.
            "Version": "18.5", "CompatibleVersion": 251920384, "ProcessVersion": "15.4",
            "WhiteBalance": "Custom", "Temperature": 5500, "Tint": 6,
            "Exposure2012": 1.35, "Contrast2012": -12, "Highlights2012": -40,
            "Shadows2012": 35, "Whites2012": 0, "Blacks2012": -5,
            "Texture": 8, "Clarity2012": 10, "Dehaze": 4, "Vibrance": 20, "Saturation": -5,
            "Sharpness": 40, "SharpenRadius": 1, "SharpenDetail": 25,
            "PostCropVignetteAmount": -20, "GrainAmount": 10,
            "PerspectiveRotate": -1.5, "PerspectiveX": 0.5,
            "ConvertToGrayscale": false, "AutoLateralCA": 1, "LensProfileEnable": 1,
            "LensProfileSetup": "LensDefaults", "CameraProfile": "Adobe Standard",
            "ToneCurveName2012": "Linear",
            "CropTop": 0.012345, "CropLeft": 0, "CropBottom": 0.987654, "CropRight": 1,
            "CropAngle": -0.8,
            "LensBlur": {"Version": 1, "Active": true, "BlurAmount": 50,
                         "CatEyeAmount": 0, "HighlightsBoost": 50},
            // Lua-only panel switch: present in every Lua table, never in XMP.
            "EnableDetail": true
        }),
    );
}

#[test]
fn curves() {
    assert_same(
        "curves.xmp",
        json!({
            "ProcessVersion": "15.4",
            "ToneCurveName2012": "Custom",
            "ToneCurvePV2012": [0, 0, 64, 58, 192, 200, 255, 255],
            "ToneCurvePV2012Red": [0, 12, 255, 255],
            "ToneCurvePV2012Green": [0, 0, 255, 255],
            "ToneCurvePV2012Blue": [0, 0, 255, 240],
            "MaskGroupBasedCorrections": [correction(
                1,
                "00000000000000000000000000000B01",
                json!({"LocalExposure2012": 0, "LocalContrast2012": 0.15,
                       "MainCurve": ["0,0", "32,16", "128,128", "255,255"],
                       "RedCurve": ["0,0", "255,240"]}),
                ai_mask(1, "00000000000000000000000000000B02", 1, json!({})),
            )]
        }),
    );
}

#[test]
fn ai_masks() {
    let mut c = vec![
        correction(
            1,
            "00000000000000000000000000000C01",
            json!({"LocalExposure2012": 0.0625, "LocalClarity2012": 0.1}),
            ai_mask(1, "00000000000000000000000000000C02", 1, json!({})),
        ),
        correction(
            2,
            "00000000000000000000000000000C03",
            json!({"LocalHighlights2012": -0.3, "LocalSaturation": 0.15, "CorrectionAmount": 0.8}),
            ai_mask(2, "00000000000000000000000000000C04", 2, json!({})),
        ),
        correction(
            3,
            "00000000000000000000000000000C05",
            json!({"LocalExposure2012": -0.05}),
            ai_mask(
                3,
                "00000000000000000000000000000C06",
                0,
                json!({"MaskSubCategoryID": 22}),
            ),
        ),
        correction(
            4,
            "00000000000000000000000000000C07",
            json!({"LocalTexture": -0.2}),
            ai_mask(
                4,
                "00000000000000000000000000000C08",
                3,
                json!({"MaskSubCategoryID": 2}),
            ),
        ),
        correction(
            5,
            "00000000000000000000000000000C09",
            json!({"LocalClarity2012": 0.25}),
            ai_mask(
                5,
                "00000000000000000000000000000C0A",
                0,
                json!({"MaskSubCategoryID": 50002}),
            ),
        ),
        correction(
            6,
            "00000000000000000000000000000C0B",
            json!({"LocalWhites2012": 0.1}),
            ai_mask(
                6,
                "00000000000000000000000000000C0C",
                0,
                json!({"MaskSubCategoryID": 5, "ReferencePoint": "0.433594 0.659824"}),
            ),
        ),
    ];
    c[5]["CorrectionActive"] = json!(false);
    assert_same(
        "masks_ai.xmp",
        json!({"ProcessVersion": "15.4", "MaskGroupBasedCorrections": c}),
    );
}

#[test]
fn linear_gradient() {
    assert_same(
        "mask_linear.xmp",
        json!({"ProcessVersion": "15.4", "MaskGroupBasedCorrections": [correction(
            1,
            "00000000000000000000000000000D01",
            json!({"LocalExposure2012": -0.125, "LocalDehaze": 0.2}),
            json!({"What": "Mask/Gradient", "MaskActive": true, "MaskName": "Mask 1",
                   "MaskBlendMode": 0, "MaskInverted": false,
                   "MaskSyncID": "00000000000000000000000000000D02", "MaskValue": 1,
                   "ZeroX": 0.5, "ZeroY": 0.55, "FullX": 0.5, "FullY": 0.2}),
        )]}),
    );
}

#[test]
fn radial_gradients() {
    let radial = |n: u32, sync: &str, geometry: J| {
        let mut m = json!({"What": "Mask/CircularGradient", "MaskActive": true,
                           "MaskName": format!("Mask {n}"), "MaskBlendMode": 0,
                           "MaskInverted": false, "MaskSyncID": sync, "MaskValue": 1,
                           "Version": 2, "Midpoint": 50, "Roundness": 0});
        for (k, v) in geometry.as_object().unwrap() {
            m[k] = v.clone();
        }
        m
    };
    assert_same(
        "mask_radial.xmp",
        json!({"ProcessVersion": "15.4", "MaskGroupBasedCorrections": [
            correction(
                1,
                "00000000000000000000000000000E01",
                json!({"LocalExposure2012": 0.1}),
                radial(1, "00000000000000000000000000000E02",
                       json!({"Top": 0.4, "Left": 0.3, "Bottom": 0.6, "Right": 0.7,
                              "Angle": 0, "Feather": 68, "Flipped": true})),
            ),
            correction(
                2,
                "00000000000000000000000000000E03",
                json!({"LocalExposure2012": -0.2}),
                radial(2, "00000000000000000000000000000E04",
                       json!({"Top": 0.478149, "Left": 0.15409, "Bottom": 0.635755,
                              "Right": 0.739495, "Angle": -29.0882, "Feather": 45,
                              "Flipped": false})),
            ),
        ]}),
    );
}

fn range_mask(n: u32, sync: &str, crm: J, blend: (i64, i64, bool)) -> J {
    json!({"What": "Mask/RangeMask", "MaskActive": true, "MaskName": format!("Mask {n}"),
           "MaskBlendMode": blend.0, "MaskValue": blend.1, "MaskInverted": blend.2,
           "MaskSyncID": sync, "CorrectionRangeMask": crm})
}

#[test]
fn range_masks() {
    assert_same(
        "mask_range.xmp",
        json!({"ProcessVersion": "15.4", "MaskGroupBasedCorrections": [
            correction(
                1,
                "00000000000000000000000000000F01",
                json!({"LocalShadows2012": 0.2}),
                range_mask(1, "00000000000000000000000000000F02",
                    json!({"Version": 3, "Type": 2, "Invert": false, "SampleType": 0,
                           "LumRange": "0.000000 0.100000 0.350000 0.500000",
                           "LuminanceDepthSampleInfo": "0.250000 0.200000 0.300000"}),
                    (0, 1, false)),
            ),
            correction(
                2,
                "00000000000000000000000000000F03",
                json!({"LocalSaturation": -0.3}),
                range_mask(2, "00000000000000000000000000000F04",
                    json!({"Version": 3, "Type": 1, "ColorAmount": 0.494949, "Invert": false,
                           "SampleType": 0,
                           "PointModels": ["0.255405 0.034566 0.419137 0.692175 0.651248 0"]}),
                    (0, 1, false)),
            ),
        ]}),
    );
}

#[test]
fn combinations() {
    let mut first = correction(
        1,
        "00000000000000000000000000001001",
        json!({"LocalExposure2012": 0.125}),
        ai_mask(1, "00000000000000000000000000001002", 1, json!({})),
    );
    first["CorrectionMasks"].as_array_mut().unwrap().extend([
        ai_mask(
            2,
            "00000000000000000000000000001003",
            2,
            json!({"MaskBlendMode": 1, "MaskValue": 0}),
        ),
        range_mask(
            3,
            "00000000000000000000000000001004",
            json!({"Version": 3, "Type": 2, "Invert": false, "SampleType": 0,
                       "LumRange": "0.500000 0.600000 1.000000 1.000000"}),
            (1, 0, true),
        ),
    ]);
    let second = correction(
        2,
        "00000000000000000000000000001005",
        json!({"LocalExposure2012": -0.1}),
        ai_mask(
            4,
            "00000000000000000000000000001006",
            1,
            json!({"MaskInverted": true}),
        ),
    );
    assert_same(
        "mask_combine.xmp",
        json!({"ProcessVersion": "15.4", "MaskGroupBasedCorrections": [first, second]}),
    );
}

#[test]
fn brush_aggregate() {
    assert_same(
        "mask_brush.xmp",
        json!({"ProcessVersion": "15.4", "MaskGroupBasedCorrections": [correction(
            1,
            "00000000000000000000000000001101",
            json!({"LocalExposure2012": 0.05}),
            json!({"What": "Mask/Aggregate", "MaskActive": true, "MaskName": "Mask 1",
                   "MaskBlendMode": 0, "MaskInverted": false,
                   "MaskSyncID": "00000000000000000000000000001102", "MaskValue": 1,
                   "Masks": [{"What": "Mask/Paint", "MaskActive": true, "MaskBlendMode": 0,
                              "MaskInverted": false,
                              "MaskSyncID": "00000000000000000000000000001103",
                              "MaskValue": 1, "Radius": 0.053214, "Flow": 1,
                              "CenterWeight": 0,
                              "Dabs": ["d 0.520898 0.305817", "d 0.530000 0.310000"]}]}),
        )]}),
    );
}

#[test]
fn look() {
    assert_same(
        "look_stub.xmp",
        json!({
            "ProcessVersion": "15.4",
            "CameraProfile": "Adobe Standard",
            "ConvertToGrayscale": false,
            "Look": {
                "Name": "Adobe Landscape",
                "Amount": 0.85,
                "UUID": "6F9C877E84273F4E8271E6B91BEB36A1",
                "SupportsAmount": false,
                "SupportsMonochrome": false,
                "SupportsOutputReferred": false,
                "Copyright": "Synthetic copyright notice",
                "Group": {"x-default": "Synthetic Group"},
                "Parameters": {"ProcessVersion": "15.4", "ConvertToGrayscale": false}
            }
        }),
    );
}

#[test]
fn the_comparison_is_not_vacuous() {
    // A changed value on either side must break the equality.
    let from_xmp = comparable(xmp("mask_linear.xmp"));
    let changed = comparable(lua(json!({"ProcessVersion": "15.4",
    "MaskGroupBasedCorrections": [correction(
        1,
        "00000000000000000000000000000D01",
        json!({"LocalExposure2012": -0.125, "LocalDehaze": 0.2}),
        json!({"What": "Mask/Gradient", "MaskActive": true, "MaskName": "Mask 1",
               "MaskBlendMode": 0, "MaskInverted": false,
               "MaskSyncID": "00000000000000000000000000000D02", "MaskValue": 1,
               "ZeroX": 0.5, "ZeroY": 0.55, "FullX": 0.5, "FullY": 0.25}),
    )]})));
    assert_ne!(from_xmp, changed);
}
