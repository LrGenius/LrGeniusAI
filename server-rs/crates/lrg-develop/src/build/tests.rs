use std::collections::HashSet;

use super::*;
use crate::model::policy::{SkipReason, Skipped};
use crate::model::DevelopSettings;
use crate::registry::{Policy, ProcessVersion};
use crate::xmp::{self, parse, PresetHeader, PresetSpec, WriteMode, XmpKind};

fn ns() -> SyncNamespace {
    SyncNamespace::lrgenius()
}

fn f(x: f64) -> Finite {
    Finite::new(x).unwrap()
}

fn key(name: &str) -> KeyId {
    registry::lookup(C, name).unwrap()
}

/// A correction with one adjustment and the given components.
fn with(role: &str) -> CorrectionBuilder {
    CorrectionBuilder::new(role, &ns())
        .local_ui("LocalExposure2012", 0.25)
        .unwrap()
}

fn one(tool: impl Into<MaskTool>) -> Correction {
    with("Role").add(tool).build().unwrap()
}

fn radial() -> RadialGradient {
    RadialGradient::new(0.2, 0.3, 0.8, 0.7, 40).unwrap()
}

// ---------- the correction ----------

#[test]
fn a_built_correction_has_its_role_name_ids_amount_and_stored_values() {
    let c = CorrectionBuilder::new("Subject", &ns())
        .amount_ui(150.0)
        .unwrap()
        .local_ui("LocalExposure2012", 0.25)
        .unwrap()
        .local_ui("LocalShadows2012", 12.0)
        .unwrap()
        .add(Semantic::Subject)
        .build()
        .unwrap();
    assert_eq!(c.name.as_deref(), Some("LrGenius · Subject"));
    assert_eq!(c.sync_id, Some(ns().id("Subject", IdSlot::Correction)));
    assert_eq!(c.amount, Some(f(1.5)));
    assert_eq!(c.active, Some(true));
    assert_eq!(
        c.local.get(&key("LocalExposure2012")),
        Some(&Value::Real(f(0.0625)))
    );
    assert_eq!(
        c.local.get(&key("LocalShadows2012")),
        Some(&Value::Real(f(0.12)))
    );
    assert!(c.extra.is_empty());
    let m = &c.masks[0];
    assert_eq!(m.name.as_deref(), Some("Subject"));
    assert_eq!(m.sync_id, Some(ns().id("Subject", IdSlot::Component(0))));
    assert_eq!(m.active, Some(true));
    assert!(m.extra.is_empty());
}

#[test]
fn the_amount_defaults_to_one_and_out_of_range_is_an_error() {
    assert_eq!(one(Semantic::Sky).amount, Some(f(1.0)));
    for bad in [-1.0, 201.0] {
        assert!(matches!(
            with("R").amount_ui(bad),
            Err(BuildError::Ui(UiError::OutOfRange { .. }))
        ));
    }
    assert!(matches!(
        with("R").amount_ui(f64::NAN),
        Err(BuildError::NotFinite { .. })
    ));
}

#[test]
fn an_empty_correction_is_an_error() {
    assert_eq!(with("Role").build(), Err(BuildError::Empty));
    assert_eq!(
        CorrectionBuilder::new("Role", &ns())
            .add(Semantic::Sky)
            .build(),
        Err(BuildError::NoAdjustments)
    );
    assert_eq!(
        with("  ").add(Semantic::Sky).build(),
        Err(BuildError::EmptyRole)
    );
}

#[test]
fn the_first_component_must_be_added() {
    let lr = || LuminanceRange::lows(0.4, 0.1).unwrap();
    assert_eq!(
        with("R").subtract(Semantic::Sky).build(),
        Err(BuildError::FirstNotAdd(Combine::Subtract))
    );
    assert_eq!(
        with("R").intersect(lr()).build(),
        Err(BuildError::FirstNotAdd(Combine::Intersect))
    );
    assert!(with("R").add_inverted(Semantic::Sky).build().is_ok());
    assert!(with("R")
        .add(Semantic::Sky)
        .subtract(Semantic::Subject)
        .intersect(lr())
        .build()
        .is_ok());
}

// ---------- adjustments ----------

#[test]
fn local_values_go_through_the_registry_ui_scale() {
    let c = with("R")
        .local_ui("LocalDehaze", 15.0)
        .unwrap()
        .local_ui("LocalHighlights2012", -40.0)
        .unwrap()
        .local_ui("LocalToningHue", 240.0)
        .unwrap()
        .local_ui("LocalCurveRefineSaturation", 30.0)
        .unwrap()
        .add(Semantic::Sky)
        .build()
        .unwrap();
    let get = |k: &str| c.local.get(&key(k)).cloned();
    assert_eq!(get("LocalDehaze"), Some(Value::Real(f(0.15))));
    assert_eq!(get("LocalHighlights2012"), Some(Value::Real(f(-0.4))));
    assert_eq!(get("LocalToningHue"), Some(Value::Real(f(240.0))));
    assert_eq!(
        get("LocalCurveRefineSaturation"),
        Some(Value::Real(f(30.0)))
    );
}

#[test]
fn setting_a_local_value_twice_keeps_the_last() {
    let c = with("R")
        .local_ui("LocalExposure2012", -1.0)
        .unwrap()
        .add(Semantic::Sky)
        .build()
        .unwrap();
    assert_eq!(
        c.local.get(&key("LocalExposure2012")),
        Some(&Value::Real(f(-0.25)))
    );
}

#[test]
fn out_of_range_local_values_are_errors_not_clamped() {
    assert!(matches!(
        with("R").local_ui("LocalExposure2012", 4.5),
        Err(BuildError::Ui(UiError::OutOfRange { .. }))
    ));
    assert!(matches!(
        with("R").local_ui("LocalShadows2012", -101.0),
        Err(BuildError::Ui(UiError::OutOfRange { .. }))
    ));
    assert!(matches!(
        with("R").local_ui("LocalShadows2012", f64::INFINITY),
        Err(BuildError::NotFinite { .. })
    ));
}

#[test]
fn keys_without_established_semantics_or_bookkeeping_are_not_writable() {
    for k in [
        "LocalHue",         // unverified UI scale
        "LocalGrain",       // unverified UI scale
        "LocalPointColors", // UNKNOWN
        "LocalExposure",    // legacy, COMPUTED
        "CorrectionAmount", // bookkeeping: amount_ui
        "CorrectionSyncID", // bookkeeping
        "What",             // bookkeeping
        "CorrectionID",     // runtime id
        "LocalGlow",        // never observed
        "NoSuchKey",        // not a correction key
        "Exposure2012",     // a global key
    ] {
        assert!(
            matches!(
                with("R").local_ui(k, 1.0),
                Err(BuildError::NotWritable { ref key, .. }) if key == k
            ),
            "{k}"
        );
    }
    assert!(matches!(
        with("R").local_ui("MainCurve", 1.0),
        Err(BuildError::NotWritable { .. })
    ));
}

#[test]
fn local_curves_need_increasing_points_on_a_curve_key() {
    let c = with("R")
        .local_curve("MainCurve", &[(0, 0), (128, 140), (255, 255)])
        .unwrap()
        .add(Semantic::Subject)
        .build()
        .unwrap();
    assert_eq!(
        c.local.get(&key("MainCurve")),
        Some(&Value::Curve(vec![
            (f(0.0), f(0.0)),
            (f(128.0), f(140.0)),
            (f(255.0), f(255.0))
        ]))
    );
    for bad in [&[(0, 0)][..], &[(0, 0), (0, 10)], &[(10, 0), (5, 10)]] {
        assert!(matches!(
            with("R").local_curve("BlueCurve", bad),
            Err(BuildError::Curve { .. })
        ));
    }
    assert!(matches!(
        with("R").local_curve("LocalExposure2012", &[(0, 0), (255, 255)]),
        Err(BuildError::NotWritable { .. })
    ));
}

// ---------- semantic masks ----------

#[test]
fn every_semantic_variant_builds_with_its_encoding_and_adobes_name() {
    let point = SensorPoint::new(0.4, 0.3).unwrap();
    let mut cases = vec![
        (Semantic::Subject, (1, None), "Subject"),
        (Semantic::Sky, (2, None), "Sky"),
        (Semantic::Background, (0, Some(22)), "Background"),
    ];
    for p in PeoplePart::ALL {
        cases.push((Semantic::people_part(p), (3, Some(p.id())), p.mask_name()));
        cases.push((
            Semantic::person_part_at(p, point),
            (0, Some(p.id())),
            p.mask_name(),
        ));
    }
    for l in LandscapeClass::ALL {
        cases.push((Semantic::landscape(l), (0, Some(l.id())), l.mask_name()));
    }
    assert_eq!(cases.len(), 3 + 2 * 10 + 8);
    for (s, encoding, name) in cases {
        let c = one(s);
        let m = &c.masks[0];
        assert_eq!(m.tool, MaskTool::Semantic(s));
        assert_eq!(s.encoding(), encoding);
        assert_eq!(m.name.as_deref(), Some(name));
    }
}

#[test]
fn people_part_and_landscape_ids_follow_the_findings_tables() {
    let parts: Vec<i64> = PeoplePart::ALL.iter().map(|p| p.id()).collect();
    assert_eq!(parts, crate::model::correction::PEOPLE_PARTS);
    let classes: Vec<i64> = LandscapeClass::ALL.iter().map(|c| c.id()).collect();
    assert_eq!(
        classes,
        crate::model::correction::LANDSCAPE_CLASSES.collect::<Vec<_>>()
    );
    assert_eq!(LandscapeClass::Vegetation.id(), 50005);
    assert_eq!(PeoplePart::IrisAndPupil.id(), 3);
    for p in PeoplePart::ALL {
        assert_eq!(PeoplePart::from_id(p.id()), Some(p));
    }
    for c in LandscapeClass::ALL {
        assert_eq!(LandscapeClass::from_id(c.id()), Some(c));
    }
    assert_eq!(PeoplePart::from_id(10), None);
    assert_eq!(LandscapeClass::from_id(50009), None);
}

#[test]
fn unknown_categories_are_errors() {
    let point = SensorPoint::new(0.5, 0.5).unwrap();
    for (s, id) in [
        (Semantic::PeoplePart(1), 1),
        (Semantic::PeoplePart(10), 10),
        (Semantic::PeoplePart(50001), 50001),
        (Semantic::Landscape(50009), 50009),
        (Semantic::Landscape(22), 22),
        (
            Semantic::PersonPartAt {
                part: 20036, // whole person: meaning unconfirmed
                point,
            },
            20036,
        ),
    ] {
        assert!(
            matches!(
                with("R").add(s).build(),
                Err(BuildError::InvalidCategory { id: got, .. }) if got == id
            ),
            "{s:?}"
        );
    }
}

// ---------- the combination table ----------

#[test]
fn combinations_encode_as_the_plan_table() {
    let lr = LuminanceRange::lows(0.5, 0.1).unwrap();
    let c = with("R")
        .add(Semantic::Subject)
        .add_inverted(Semantic::Sky)
        .subtract(Semantic::Background)
        .intersect(lr)
        .build()
        .unwrap();
    let encoded: Vec<_> = c
        .masks
        .iter()
        .map(|m| {
            let (mode, value, inverted) = m.combine.encode().unwrap();
            (mode, value.get(), inverted)
        })
        .collect();
    assert_eq!(
        encoded,
        [
            (0, 1.0, false),
            (0, 1.0, true),
            (1, 0.0, false),
            (1, 0.0, true)
        ]
    );
    for (mode, value, _) in encoded {
        assert_eq!(value == 0.0, mode == 1, "MaskValue 0 <=> MaskBlendMode 1");
    }
}

#[test]
fn a_range_invert_is_mask_inverted_under_every_combination() {
    let lr = || LuminanceRange::highs(0.6, 0.1).unwrap();
    let c = with("R")
        .add(lr())
        .add_inverted(lr())
        .subtract(lr())
        .intersect(lr())
        .build()
        .unwrap();
    let mut inverts = Vec::new();
    for m in &c.masks {
        let MaskTool::LuminanceRange(r) = &m.tool else {
            panic!("range expected")
        };
        let (_, _, inverted) = m.combine.encode().unwrap();
        assert_eq!(r.invert, inverted, "{:?}", m.combine);
        inverts.push(r.invert);
    }
    assert_eq!(inverts, [false, true, false, true]);
    // Whatever the caller put in `invert`, the combination decides.
    let mut odd = lr();
    odd.invert = true;
    let c = with("R").add(Semantic::Sky).subtract(odd).build().unwrap();
    assert!(matches!(&c.masks[1].tool, MaskTool::LuminanceRange(r) if !r.invert));
}

#[test]
fn radial_flipped_is_the_inverse_of_mask_inverted() {
    let c = with("R")
        .add(radial())
        .add_inverted(radial())
        .subtract(radial())
        .intersect(radial())
        .build()
        .unwrap();
    for m in &c.masks {
        let MaskTool::Radial(g) = &m.tool else {
            panic!("radial expected")
        };
        let (_, _, inverted) = m.combine.encode().unwrap();
        assert_eq!(g.flipped, !inverted, "{:?}", m.combine);
    }
    let flipped: Vec<bool> = c
        .masks
        .iter()
        .map(|m| matches!(m.tool, MaskTool::Radial(g) if g.flipped))
        .collect();
    assert_eq!(flipped, [true, false, true, false]);
}

#[test]
fn an_unrecognised_combination_or_opaque_tool_cannot_be_built() {
    let mut b = with("R").add(Semantic::Sky);
    b.components
        .push((Combine::Unrecognised, Semantic::Sky.into()));
    assert!(matches!(b.build(), Err(BuildError::NotBuildable(_))));
    let opaque = MaskTool::Opaque {
        what: Some("Mask/Aggregate".into()),
    };
    assert!(matches!(
        with("R").add(opaque).build(),
        Err(BuildError::NotBuildable(_))
    ));
}

// ---------- gradients ----------

#[test]
fn a_radial_gradient_is_unrotated_with_lightrooms_fixed_fields() {
    let c = one(radial());
    let m = &c.masks[0];
    assert_eq!(
        m.tool,
        MaskTool::Radial(RadialGradient {
            top: f(0.2),
            left: f(0.3),
            bottom: f(0.8),
            right: f(0.7),
            feather: 40,
            midpoint: 50,
            roundness: 0,
            flipped: true,
        })
    );
    assert_eq!(m.name.as_deref(), Some("Radial Gradient"));
    assert_eq!(
        m.extra.get(Level::MaskTool, "Version"),
        Some(&Value::Int(RADIAL_VERSION))
    );
    assert_eq!(m.extra.values.len(), 1);
}

#[test]
fn radial_bounds_and_feather_are_checked() {
    for (t, l, b, r, feather) in [
        (0.8, 0.3, 0.2, 0.7, 40), // top below bottom
        (0.2, 0.7, 0.8, 0.3, 40), // left right of right
        (0.2, 0.3, 0.2, 0.7, 40), // no height
        (0.2, 0.3, 0.8, 0.7, -1),
        (0.2, 0.3, 0.8, 0.7, 101),
    ] {
        assert!(matches!(
            RadialGradient::new(t, l, b, r, feather),
            Err(BuildError::Geometry(_))
        ));
    }
    assert!(matches!(
        RadialGradient::new(f64::NAN, 0.3, 0.8, 0.7, 0),
        Err(BuildError::NotFinite { .. })
    ));
    let rounder = RadialGradient {
        roundness: 20,
        ..radial()
    };
    assert!(matches!(
        with("R").add(rounder).build(),
        Err(BuildError::NotBuildable(_))
    ));
}

#[test]
fn a_linear_gradient_needs_two_points() {
    let a = SensorPoint::new(0.5, 0.0).unwrap();
    let b = SensorPoint::new(0.5, 0.6).unwrap();
    let c = one(LinearGradient::new(a, b).unwrap());
    assert_eq!(
        c.masks[0].tool,
        MaskTool::Linear(LinearGradient { zero: a, full: b })
    );
    assert_eq!(c.masks[0].name.as_deref(), Some("Linear Gradient"));
    assert!(c.masks[0].extra.is_empty());
    assert!(matches!(
        LinearGradient::new(a, a),
        Err(BuildError::Geometry(_))
    ));
    // Points may leave 0..1 (a gradient starting outside the frame).
    assert!(SensorPoint::new(-0.2, 1.3).is_ok());
    assert!(matches!(
        SensorPoint::new(0.1, f64::INFINITY),
        Err(BuildError::NotFinite { .. })
    ));
}

// ---------- luminance ranges ----------

#[test]
fn luminance_ranges_are_ordered_between_zero_and_one() {
    let lows = LuminanceRange::lows(0.45, 0.1).unwrap();
    assert_eq!(lows.range, [f(0.0), f(0.0), f(0.45), f(0.55)]);
    let highs = LuminanceRange::highs(0.7, 0.2).unwrap();
    assert_eq!(highs.range.map(Finite::get)[1..], [0.7, 1.0, 1.0]);
    assert!(!highs.invert, "set by the builder, from the combination");
    assert!(LuminanceRange::new([0.0, 0.0, 1.0, 1.0]).is_ok());
    assert!(LuminanceRange::new([0.3, 0.3, 0.3, 0.3]).is_ok());
    for bad in [
        [0.2, 0.1, 0.8, 1.0], // feather above low
        [0.0, 0.8, 0.2, 1.0], // low above high
        [0.0, 0.2, 0.8, 0.7], // high above its feather
        [-0.1, 0.2, 0.8, 1.0],
        [0.0, 0.2, 0.8, 1.1],
        [0.0, f64::NAN, 0.8, 1.0],
    ] {
        assert!(
            matches!(LuminanceRange::new(bad), Err(BuildError::LumRangeOrder(_))),
            "{bad:?}"
        );
    }
    assert!(LuminanceRange::lows(0.95, 0.1).is_err());
    assert!(LuminanceRange::highs(0.05, 0.1).is_err());
}

#[test]
fn a_luminance_range_is_checked_again_when_built() {
    let mut lr = LuminanceRange::lows(0.4, 0.1).unwrap();
    lr.range.swap(1, 2);
    assert!(matches!(
        with("R").add(Semantic::Sky).intersect(lr).build(),
        Err(BuildError::LumRangeOrder(_))
    ));
    let mut sampled = LuminanceRange::lows(0.4, 0.1).unwrap();
    sampled.rest.fields.values.insert(
        registry::lookup(
            Level::Struct(registry::StructKind::CorrectionRangeMask),
            "LuminanceDepthSampleInfo",
        )
        .unwrap(),
        Value::Str("0 0 0".into()),
    );
    assert!(matches!(
        with("R").add(sampled).build(),
        Err(BuildError::NotBuildable(_))
    ));
    let c = one(LuminanceRange::lows(0.4, 0.1).unwrap());
    assert_eq!(c.masks[0].name.as_deref(), Some("Luminance Range"));
}

// ---------- sync ids ----------

#[test]
fn sync_ids_are_32_upper_case_hex_and_deterministic() {
    let id = ns().id("Subject", IdSlot::Correction);
    assert_eq!(id.as_str().len(), 32);
    assert!(id
        .as_str()
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)));
    assert_eq!(id, ns().id("Subject", IdSlot::Correction));
    let a = with("Subject").add(Semantic::Subject).build().unwrap();
    let b = with("Subject").add(Semantic::Subject).build().unwrap();
    assert_eq!(a, b, "same role, same correction");
}

#[test]
fn sync_ids_are_unique_per_namespace_role_and_slot() {
    let mut seen = HashSet::new();
    for n in [ns(), SyncNamespace::new("other")] {
        for role in ["Subject", "Sky", "Background", "Subject 2"] {
            assert!(seen.insert(n.id(role, IdSlot::Correction)));
            for i in 0..8 {
                assert!(seen.insert(n.id(role, IdSlot::Component(i))));
            }
        }
    }
    // The parts are length-prefixed, so shifting a boundary changes the id.
    assert_ne!(
        SyncNamespace::new("ab").id("c", IdSlot::Correction),
        SyncNamespace::new("a").id("bc", IdSlot::Correction)
    );
}

/// Known answers: these ids are what a released LrGeniusAI wrote. Changing
/// the domain tag, the length prefixes or the slot texts changes every
/// `CorrectionSyncID` and `MaskSyncID`, and a later release could no longer
/// replace the corrections an earlier one wrote. (The values were computed
/// independently: Python's `hashlib.sha256` over the same bytes.)
#[test]
fn sync_ids_match_their_known_answers() {
    let lrg = SyncNamespace::lrgenius();
    assert_eq!(
        lrg.id("Subject", IdSlot::Correction).as_str(),
        "1578F53D074D6B0E08DC6ADE4F32DDC1"
    );
    assert_eq!(
        lrg.id("Subject", IdSlot::Component(0)).as_str(),
        "539FD64FFFDD00578B847D3FCCCB36E5"
    );
    assert_eq!(
        SyncNamespace::new("other")
            .id("Sky", IdSlot::Component(2))
            .as_str(),
        "474C41A0AACF4F7EE44AA68BBAC56026"
    );
}

#[test]
fn a_preset_s_corrections_need_distinct_roles() {
    let sky = || with("Sky").add(Semantic::Sky);
    assert_eq!(
        corrections([sky(), with(" Sky ").add(Semantic::Subject)]),
        Err(BuildError::DuplicateRole("Sky".into()))
    );
    let both = corrections([sky(), with("Subject").add(Semantic::Subject)]).unwrap();
    assert_eq!(both.len(), 2);
    assert_ne!(both[0].sync_id, both[1].sync_id);
    let ids: HashSet<_> = both
        .iter()
        .flat_map(|c| c.masks.iter().map(|m| m.sync_id.clone()))
        .collect();
    assert_eq!(ids.len(), 2);
    // A builder's own error comes through.
    assert_eq!(corrections([with("Sky")]), Err(BuildError::Empty));
}

#[test]
fn a_built_correction_has_distinct_ids_for_every_component() {
    let c = with("Sky")
        .add(Semantic::Sky)
        .subtract(Semantic::Subject)
        .intersect(LuminanceRange::highs(0.5, 0.1).unwrap())
        .build()
        .unwrap();
    let mut ids: Vec<_> = c.masks.iter().map(|m| m.sync_id.clone()).collect();
    ids.push(c.sync_id.clone());
    let unique: HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), 4);
    for (i, m) in c.masks.iter().enumerate() {
        assert_eq!(m.sync_id, Some(ns().id("Sky", IdSlot::Component(i))));
    }
}

// ---------- through the writer ----------

fn preset(corrections: Vec<Correction>) -> xmp::Written {
    let mut s = DevelopSettings::new();
    s.corrections = corrections;
    let header = PresetHeader::lrgenius(ns().id("preset", IdSlot::Correction), "Built");
    let mut spec = PresetSpec::new(header);
    spec.process_version = Some(ProcessVersion::V6);
    xmp::write(&s, &WriteMode::Preset(spec)).unwrap()
}

#[test]
fn built_semantic_corrections_survive_a_preset_unchanged() {
    let built = vec![
        CorrectionBuilder::new("Subject", &ns())
            .local_ui("LocalExposure2012", 0.3)
            .unwrap()
            .add(Semantic::Subject)
            .build()
            .unwrap(),
        CorrectionBuilder::new("Sky", &ns())
            .local_ui("LocalHighlights2012", -40.0)
            .unwrap()
            .local_ui("LocalDehaze", 15.0)
            .unwrap()
            .add(Semantic::Sky)
            .intersect(LuminanceRange::highs(0.5, 0.2).unwrap())
            .build()
            .unwrap(),
        CorrectionBuilder::new("Eyes", &ns())
            .local_ui("LocalSaturation", 30.0)
            .unwrap()
            .add(Semantic::people_part(PeoplePart::IrisAndPupil))
            .build()
            .unwrap(),
    ];
    let written = preset(built.clone());
    assert!(written.skipped.is_empty(), "{:?}", written.skipped);
    let doc = parse(written.xmp.as_bytes()).unwrap();
    assert_eq!(doc.kind, XmpKind::Preset);
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    let read = &doc.develop.corrections;
    assert_eq!(read.len(), built.len());
    for (r, b) in read.iter().zip(&built) {
        assert_eq!(
            (&r.name, &r.sync_id, r.amount, r.active, &r.local),
            (&b.name, &b.sync_id, b.amount, b.active, &b.local)
        );
        assert_eq!(r.masks.len(), b.masks.len(), "{:?}", b.name);
        for (rm, bm) in r.masks.iter().zip(&b.masks) {
            // The writer completes a range mask's Version 3 and SampleType 0.
            let mut tool = rm.tool.clone();
            if let MaskTool::LuminanceRange(lr) = &mut tool {
                let crm = |n| lr.rest.get(n).cloned();
                assert_eq!(
                    (crm("Version"), crm("SampleType")),
                    (Some(Value::Int(3)), Some(Value::Int(0)))
                );
                lr.rest.fields = Fields::default();
            }
            assert_eq!(
                (&tool, rm.combine, &rm.name, &rm.sync_id, rm.active),
                (&bm.tool, bm.combine, &bm.name, &bm.sync_id, bm.active)
            );
        }
    }
    // Adobe's AI-mask form, completed by the writer.
    assert!(written.xmp.contains("crs:MaskSubType=\"3\""));
    assert!(written.xmp.contains("crs:MaskSubCategoryID=\"3\""));
    assert!(written
        .xmp
        .contains("crs:ReferencePoint=\"0.500000 0.500000\""));
    assert!(written
        .xmp
        .contains("crs:CorrectionName=\"LrGenius · Sky\""));
    // The intersected range mirrors its MaskInverted, as Lightroom writes it.
    assert!(written
        .xmp
        .contains("crs:MaskBlendMode=\"1\"\n          crs:MaskInverted=\"true\""));
    assert!(written.xmp.contains("crs:Invert=\"true\""));
    assert!(!written.xmp.contains("crs:Invert=\"false\""));
}

#[test]
fn photo_components_keep_their_correction_out_of_a_preset_and_say_so() {
    let point = SensorPoint::new(0.4, 0.3).unwrap();
    let written = preset(vec![
        with("Vignette").add(radial()).build().unwrap(),
        with("Hair")
            .add(Semantic::person_part_at(PeoplePart::Hair, point))
            .build()
            .unwrap(),
    ]);
    assert!(!written.xmp.contains("MaskGroupBasedCorrections"));
    assert_eq!(
        written.skipped,
        [0, 1]
            .map(|i| Skipped {
                path: format!("MaskGroupBasedCorrections[{i}]"),
                reason: SkipReason::Policy(Policy::Photo),
            })
            .to_vec()
    );
}

#[test]
fn preset_output_of_built_corrections_is_byte_deterministic() {
    let build = || {
        vec![with("Sky")
            .add(Semantic::Sky)
            .subtract(Semantic::landscape(LandscapeClass::Mountains))
            .build()
            .unwrap()]
    };
    assert_eq!(preset(build()).xmp, preset(build()).xmp);
}

/// Every built tool, photo-specific ones included, reads back as exactly
/// the built correction through the lossless test mode.
#[cfg(feature = "test-roundtrip")]
#[test]
fn every_built_tool_round_trips_exactly() {
    let a = SensorPoint::new(0.5, 0.1).unwrap();
    let b = SensorPoint::new(0.5, 0.7).unwrap();
    let mut s = DevelopSettings::new();
    s.corrections = vec![
        with("Gradients")
            .add(LinearGradient::new(a, b).unwrap())
            .add_inverted(radial())
            .intersect(radial())
            .build()
            .unwrap(),
        with("People")
            .local_curve("RedCurve", &[(0, 10), (255, 240)])
            .unwrap()
            .add(Semantic::person_part_at(PeoplePart::Lips, a))
            .add(Semantic::people_part(PeoplePart::Teeth))
            .subtract(Semantic::Background)
            .intersect(LuminanceRange::new([0.1, 0.2, 0.6, 0.9]).unwrap())
            .build()
            .unwrap(),
    ];
    let written = xmp::write(&s, &WriteMode::TestRoundTrip(None)).unwrap();
    let doc = parse(written.xmp.as_bytes()).unwrap();
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
    assert_eq!(doc.develop.corrections, s.corrections);
}
