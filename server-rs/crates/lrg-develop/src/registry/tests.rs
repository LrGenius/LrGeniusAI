//! Registry invariants (see Dev-Develop-Model, "Maintaining the registry")
//! and spot checks.

use std::collections::{HashMap, HashSet};

use super::*;

fn row(level: Level, name: &str) -> &'static KeySpec {
    lookup(level, name)
        .unwrap_or_else(|| panic!("{level}/{name} missing"))
        .spec()
}

fn f(x: f64) -> Finite {
    Finite::new(x).unwrap()
}

/// Every numeric literal of a default, as a float.
fn numeric_default(d: Def) -> Option<f64> {
    match d {
        Def::Value(Lit::Int(i)) => Some(i as f64),
        Def::Value(Lit::Real(r)) => Some(r.get()),
        _ => None,
    }
}

#[test]
fn level_and_name_identify_a_row() {
    let mut seen = HashSet::new();
    for (id, spec) in iter() {
        assert!(
            seen.insert((spec.level, spec.name)),
            "duplicate row {}/{}",
            spec.level,
            spec.name
        );
        assert_eq!(lookup(spec.level, spec.name), Some(id));
        assert_eq!(id.spec().name, spec.name);
    }
    assert_eq!(seen.len(), table().len());
    assert!(table().len() < usize::from(u16::MAX));
}

#[test]
fn the_same_name_can_mean_different_keys_on_different_levels() {
    let global = row(Level::Global, "Version");
    let radial = row(Level::MaskTool, "Version");
    assert_eq!(global.kind, ValueKind::VersionStr);
    assert_eq!(radial.kind, ValueKind::Int);
    assert_ne!(
        lookup(Level::Global, "Version"),
        lookup(Level::MaskTool, "Version")
    );
}

#[test]
fn ranges_are_ordered_and_integer_keys_have_integer_bounds() {
    for (_, s) in iter() {
        let Some((min, max)) = s.range else { continue };
        assert!(
            s.kind.is_numeric(),
            "{}: range on a non-numeric key",
            s.name
        );
        assert!(min < max, "{}: empty range", s.name);
        if s.kind.is_integer() {
            assert!(
                min.get().fract() == 0.0 && max.get().fract() == 0.0,
                "{}: fractional bound on an integer key",
                s.name
            );
        }
    }
}

#[test]
fn every_range_contains_its_default() {
    for (_, s) in iter() {
        let Some((min, max)) = s.range else { continue };
        for d in [s.default.raw, s.default.non_raw] {
            if let Some(v) = numeric_default(d) {
                assert!(
                    min.get() <= v && v <= max.get(),
                    "{}/{}: default {v} outside {min}..{max}",
                    s.level,
                    s.name
                );
            }
        }
    }
}

#[test]
fn defaults_match_the_value_kind() {
    for (_, s) in iter() {
        for d in [s.default.raw, s.default.non_raw] {
            let Def::Value(lit) = d else { continue };
            let ok = match (s.kind, lit) {
                (ValueKind::Int | ValueKind::VersionU32, Lit::Int(_)) => true,
                (ValueKind::IntFlag, Lit::Int(i)) => i == 0 || i == 1,
                (ValueKind::EnumInt(set), Lit::Int(i)) => set.contains(&i),
                (ValueKind::Real, Lit::Real(_)) => true,
                (ValueKind::Bool(_), Lit::Bool(_)) => true,
                (ValueKind::Enum(set), Lit::Str(v)) => set.contains(&v),
                (ValueKind::Str, Lit::Str(_)) => true,
                (ValueKind::Curve(CurveKind::Global), Lit::IntList(v)) => {
                    v.len() >= 4 && v.len() % 2 == 0
                }
                _ => false,
            };
            assert!(
                ok,
                "{}/{}: default {lit:?} vs {:?}",
                s.level, s.name, s.kind
            );
        }
    }
}

#[test]
fn plus_sign_only_on_the_global_level() {
    for (_, s) in iter() {
        if s.plus_sign {
            assert_eq!(s.level, Level::Global, "{}", s.name);
            assert!(
                matches!(s.kind, ValueKind::Int | ValueKind::Real),
                "{}",
                s.name
            );
        }
    }
    // XMP rule 8: signed sliders, plus the two keys that always carry "+".
    for name in [
        "Exposure2012",
        "Tint",
        "SharpenRadius",
        "HDRMaxValue",
        "VignetteAmount",
    ] {
        assert!(row(Level::Global, name).plus_sign, "{name}");
    }
    for name in [
        "Temperature",
        "CropAngle",
        "CropTop",
        "Sharpness",
        "SplitToningShadowHue",
    ] {
        assert!(!row(Level::Global, name).plus_sign, "{name}");
    }
    assert!(!row(Level::Correction, "LocalExposure2012").plus_sign);
}

#[test]
fn bool_spelling_follows_the_level() {
    fn check(level: Level, name: &str, kind: ValueKind) {
        match kind {
            ValueKind::Bool(BoolStyle::Lower) => {
                assert!(
                    level.is_inside_struct(),
                    "{level}/{name}: lower-case bool outside a struct"
                )
            }
            ValueKind::Bool(BoolStyle::TitleCase) => assert!(
                !level.is_inside_struct(),
                "{level}/{name}: title-case bool inside a struct"
            ),
            _ => {}
        }
    }
    for (_, s) in iter() {
        check(s.level, s.name, s.kind.lua_form());
        check(s.level, s.name, s.kind.xmp_form());
    }
}

#[test]
fn recipe_aliases_are_unique_and_only_on_learnable_global_keys() {
    let mut seen = HashMap::new();
    for (_, s) in iter() {
        let Some(alias) = s.recipe_alias else {
            continue;
        };
        assert!(
            s.level == Level::Global && s.policy.is_learnable(),
            "{}: alias on a non-learnable or non-global key",
            s.name
        );
        assert!(
            alias.starts_with("global.")
                && alias.split('.').all(
                    |p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                ),
            "{}: malformed alias {alias}",
            s.name
        );
        if let Some(other) = seen.insert(alias, s.name) {
            panic!("alias {alias} on both {other} and {}", s.name);
        }
    }
    // Every field the installed plugin applies from `global` (1f), minus
    // white balance (1d): 36 sliders and parametric curve values, 24
    // HSL values, 5 colour-grading values.
    assert_eq!(seen.len(), 65);
    // A path must never end where another continues (`global.hsl` next to
    // `global.hsl.red.hue`): the recipe builder nests objects along them.
    for a in seen.keys() {
        for b in seen.keys() {
            assert!(
                !b.starts_with(&format!("{a}.")),
                "alias {a} is a prefix of {b}"
            );
        }
    }
    assert_eq!(
        row(Level::Global, "HueAdjustmentAqua").recipe_alias,
        Some("global.hsl.aqua.hue")
    );
    assert_eq!(
        row(Level::Global, "SplitToningHighlightSaturation").recipe_alias,
        Some("global.color_grading.highlights.saturation")
    );
    assert_eq!(
        row(Level::Global, "ParametricMidtoneSplit").recipe_alias,
        Some("global.tone_curve.midtone_split")
    );
    assert_eq!(
        row(Level::Global, "GrainFrequency").recipe_alias,
        Some("global.grain_roughness")
    );
    // The installed plugin warns about and drops these (1f leaves them out).
    for name in [
        "ColorGradeMidtoneHue",
        "ColorGradeGlobalHue",
        "ColorGradeShadowLum",
        "ColorGradeBlending",
        "ToneCurvePV2012",
    ] {
        assert_eq!(row(Level::Global, name).recipe_alias, None, "{name}");
    }
    assert_eq!(
        row(Level::Global, "Exposure2012").recipe_alias,
        Some("global.exposure")
    );
    assert_eq!(
        row(Level::Global, "ParametricShadows").recipe_alias,
        Some("global.tone_curve.shadows")
    );
    assert_eq!(row(Level::Global, "Temperature").recipe_alias, None);
    assert_eq!(row(Level::Global, "Tint").recipe_alias, None);
}

#[test]
fn raw_only_keys_are_absent_for_non_raw_files_and_vice_versa() {
    for (_, s) in iter() {
        match s.policy.gate() {
            Some(Gate::RawOnly) => {
                assert_eq!(s.default.non_raw, Def::Absent, "{}", s.name);
                assert_eq!(s.file_kind, FileScope::RawOnly, "{}", s.name);
            }
            Some(Gate::NonRawOnly) => {
                assert_eq!(s.default.raw, Def::Absent, "{}", s.name);
                assert_eq!(s.file_kind, FileScope::NonRawOnly, "{}", s.name);
            }
            _ => assert_eq!(s.file_kind, FileScope::Both, "{}", s.name),
        }
    }
    for name in ["Temperature", "Tint"] {
        assert_eq!(
            row(Level::Global, name).policy,
            Policy::LearnGated(Gate::RawOnly)
        );
    }
    for name in ["IncrementalTemperature", "IncrementalTint"] {
        assert_eq!(
            row(Level::Global, name).policy,
            Policy::LearnGated(Gate::NonRawOnly)
        );
    }
}

/// Lightroom's non-raw defaults as read back by experiment E13 (one non-raw
/// photo without develop edits; provenance inside the file). The generator
/// (`scripts/develop_registry/gen_table_rs.py`) fills the non-raw column from
/// it; this file is what a claimed non-raw default must come from.
const NON_RAW_READBACK: &str =
    include_str!("../../../../testdata/develop/defaults/non_raw_lrc15.6.json");

/// Readback keys held back from the non-raw defaults because the one photo
/// read may carry a per-image value; the same set as the generator's
/// `NON_RAW_READBACK_NOT_CONSTANT`.
const NON_RAW_READBACK_NOT_CONSTANT: &[&str] = &["HDRMaxValue"];

fn non_raw_readback() -> serde_json::Map<String, serde_json::Value> {
    let doc: serde_json::Value = serde_json::from_str(NON_RAW_READBACK).unwrap();
    for k in ["lightroom", "process_version", "file_kind"] {
        assert!(
            doc["provenance"][k].as_str().is_some_and(|s| !s.is_empty()),
            "provenance.{k} missing"
        );
    }
    doc["settings"].as_object().unwrap().clone()
}

fn lit_equals_json(lit: Lit, v: &serde_json::Value) -> bool {
    match lit {
        Lit::Int(i) => v.as_i64() == Some(i),
        Lit::Real(r) => v.as_f64() == Some(r.get()),
        Lit::Bool(b) => v.as_bool() == Some(b),
        Lit::Str(s) => v.as_str() == Some(s),
        Lit::IntList(l) => v.as_array().is_some_and(|a| {
            a.len() == l.len() && a.iter().zip(l).all(|(x, y)| x.as_i64() == Some(*y))
        }),
    }
}

/// Every claimed non-raw default is a value Lightroom read back for a
/// non-raw photo, and every global row the readback covers that the text
/// left unverified got its value: nothing is invented for a key the
/// readback does not have (absence can mean "written only when used").
#[test]
fn non_raw_defaults_come_from_the_readback() {
    let readback = non_raw_readback();
    for (_, s) in iter() {
        let read = (s.level == Level::Global)
            .then(|| readback.get(s.name))
            .flatten();
        match (s.default.non_raw, read) {
            (Def::Value(lit), Some(v)) => assert!(
                lit_equals_json(lit, v),
                "{}: non-raw default {lit:?}, read back {v}",
                s.name
            ),
            (Def::Value(lit), None) => {
                panic!("{}: non-raw default {lit:?} without a readback", s.name)
            }
            (Def::Unverified, Some(_)) if NON_RAW_READBACK_NOT_CONSTANT.contains(&s.name) => {}
            (Def::Unverified, Some(v)) => {
                panic!("{}: read back as {v} but still unverified", s.name)
            }
            (Def::Absent, Some(v)) => panic!("{}: absent for non-raw, read back as {v}", s.name),
            _ => {}
        }
    }
    for key in readback.keys() {
        assert!(lookup(Level::Global, key).is_some(), "{key}: no global row");
    }
    for key in NON_RAW_READBACK_NOT_CONSTANT {
        assert!(readback.contains_key(*key), "{key}: held back but not read");
        assert_eq!(row(Level::Global, key).default.non_raw, Def::Unverified);
    }
}

/// What the readback changes against the raw column: the keys whose
/// defaults really differ by file kind.
#[test]
fn the_non_raw_readback_differs_from_raw_where_expected() {
    let non_raw = |name| row(Level::Global, name).default.non_raw;
    assert_eq!(non_raw("Sharpness"), Def::Value(Lit::Int(0)));
    assert_eq!(
        row(Level::Global, "Sharpness").default.raw,
        Def::Value(Lit::Int(40))
    );
    assert_eq!(non_raw("ColorNoiseReduction"), Def::Value(Lit::Int(0)));
    assert_eq!(
        row(Level::Global, "ColorNoiseReduction").default.raw,
        Def::Value(Lit::Int(25))
    );
    assert_eq!(non_raw("CameraProfile"), Def::Value(Lit::Str("Embedded")));
    assert_eq!(non_raw("WhiteBalance"), Def::Value(Lit::Str("As Shot")));
    assert_eq!(non_raw("IncrementalTemperature"), Def::Value(Lit::Int(0)));
    assert_eq!(non_raw("IncrementalTint"), Def::Value(Lit::Int(0)));
    // Raw-only keys stay absent: the readback has no `Temperature`.
    assert_eq!(non_raw("Temperature"), Def::Absent);
    assert_eq!(non_raw("Tint"), Def::Absent);
    // Per-photo or written-when-used keys keep `NoDefault` whatever the
    // readback shows (the Upright solver state).
    assert_eq!(non_raw("UprightVersion"), Def::NoDefault);
}

#[test]
fn lua_only_keys_carry_no_xmp_format_rules() {
    for (_, s) in iter() {
        if s.presence == Presence::LuaOnly {
            assert!(!s.plus_sign, "{}", s.name);
            assert_eq!(s.fmt, NumFmt::Text, "{}", s.name);
        }
    }
    for name in [
        "EnableCalibration",
        "orientation",
        "CropConstrainAspectRatio",
    ] {
        assert_eq!(
            row(Level::Global, name).presence,
            Presence::LuaOnly,
            "{name}"
        );
    }
    assert_eq!(
        row(Level::Correction, "CorrectionID").presence,
        Presence::LuaOnly
    );
    assert_eq!(row(Level::MaskTool, "MaskID").presence, Presence::LuaOnly);
}

#[test]
fn preset_header_and_derived_flags_are_xmp_only() {
    for (_, s) in iter() {
        if s.level == Level::Header {
            assert_eq!(s.presence, Presence::XmpOnly, "{}", s.name);
            // The header is ours to write; only `Baseline` is not understood.
            assert!(
                matches!(s.policy, Policy::Meta | Policy::Unknown),
                "{}",
                s.name
            );
        }
    }
    for name in ["HasCrop", "HasSettings", "AlreadyApplied", "RawFileName"] {
        assert_eq!(
            row(Level::Global, name).presence,
            Presence::XmpOnly,
            "{name}"
        );
    }
}

#[test]
fn number_formats_fit_the_value_kind() {
    for (_, s) in iter() {
        let ok = match s.fmt {
            NumFmt::Int => s.kind.is_integer() || matches!(s.kind, ValueKind::Curve(_)),
            NumFmt::Fixed(_) => matches!(s.kind, ValueKind::Real | ValueKind::VersionStr),
            NumFmt::Trim6 => s.kind == ValueKind::Real,
            NumFmt::CompoundFixed6 => {
                matches!(s.kind.xmp_form(), ValueKind::Str | ValueKind::StrSeq)
            }
            NumFmt::Text => !s.kind.is_numeric() || s.presence == Presence::LuaOnly,
        };
        assert!(ok, "{}/{}: {:?} on {:?}", s.level, s.name, s.fmt, s.kind);
    }
    assert_eq!(row(Level::Global, "Exposure2012").fmt, NumFmt::Fixed(2));
    assert_eq!(row(Level::Global, "SharpenRadius").fmt, NumFmt::Fixed(1));
    assert_eq!(row(Level::Global, "ProcessVersion").fmt, NumFmt::Fixed(1));
    assert_eq!(row(Level::Global, "CropAngle").fmt, NumFmt::Trim6);
    assert_eq!(
        row(Level::MaskTool, "ReferencePoint").fmt,
        NumFmt::CompoundFixed6
    );
}

#[test]
fn ui_scales_apply_to_numbers_only() {
    for (_, s) in iter() {
        match s.ui {
            UiScale::Identity => {}
            UiScale::Div(d) => {
                assert!(d > 0.0 && d.is_finite(), "{}", s.name);
                assert_eq!(s.kind, ValueKind::Real, "{}: scaled non-real key", s.name);
            }
            UiScale::Unknown => assert!(s.kind.is_numeric(), "{}", s.name),
        }
    }
    let exp = row(Level::Correction, "LocalExposure2012");
    assert_eq!(exp.ui, UiScale::Div(4.0));
    assert_eq!(exp.range, Some((f(-1.0), f(1.0))));
    let amount = row(Level::Correction, "CorrectionAmount");
    assert_eq!(amount.ui, UiScale::Div(100.0));
    assert_eq!(amount.range, Some((f(0.0), f(2.0))));
    assert_eq!(
        row(Level::Correction, "LocalToningHue").ui,
        UiScale::Identity
    );
    assert_eq!(
        row(Level::Correction, "LocalCurveRefineSaturation").ui,
        UiScale::Identity
    );
    assert_eq!(row(Level::Correction, "LocalHue").ui, UiScale::Unknown);
    assert_eq!(row(Level::Correction, "LocalGrain").ui, UiScale::Unknown);
    assert_eq!(row(Level::Global, "Exposure2012").ui, UiScale::Identity);
}

#[test]
fn frame_scope_follows_the_policy_class() {
    for (_, s) in iter() {
        let ok = match s.policy {
            Policy::Computed | Policy::Meta | Policy::Unknown => {
                s.frame == FrameScope::NotApplicable
            }
            Policy::Never => s.frame == FrameScope::PerFrameOnly,
            Policy::Photo => matches!(
                s.frame,
                FrameScope::PerFrameOnly | FrameScope::PerFrameAdjusted
            ),
            Policy::Learn | Policy::LearnGated(_) => {
                matches!(
                    s.frame,
                    FrameScope::Shareable | FrameScope::PerFrameAdjusted
                )
            }
        };
        assert!(
            ok,
            "{}/{}: {:?} with {:?}",
            s.level, s.name, s.frame, s.policy
        );
    }
    assert_eq!(
        row(Level::Global, "Exposure2012").frame,
        FrameScope::PerFrameAdjusted
    );
    assert_eq!(
        row(Level::Global, "WhiteBalance").frame,
        FrameScope::PerFrameAdjusted
    );
    assert_eq!(
        row(Level::Global, "CropTop").frame,
        FrameScope::PerFrameOnly
    );
    assert_eq!(
        row(Level::Global, "HueAdjustmentRed").frame,
        FrameScope::Shareable
    );
}

#[test]
fn gates_are_consistent() {
    for (_, s) in iter() {
        match s.policy.gate() {
            Some(Gate::DependsOn(k)) => {
                assert!(
                    lookup(Level::Global, k).is_some(),
                    "{}: gate names unknown {k}",
                    s.name
                )
            }
            Some(Gate::OnlyValues(values)) => {
                let ValueKind::Enum(all) = s.kind else {
                    panic!("{}: OnlyValues on a non-enum key", s.name)
                };
                assert!(values.iter().all(|v| all.contains(v)), "{}", s.name);
            }
            Some(Gate::CircularHue(weight)) => {
                let (_, max) = s
                    .range
                    .unwrap_or_else(|| panic!("{}: hue without range", s.name));
                assert!(max.get() >= 359.0, "{}: not an angle", s.name);
                // The weight is a saturation of the same level, never negative.
                let w = lookup(s.level, weight)
                    .unwrap_or_else(|| panic!("{}: weight {weight} unknown", s.name))
                    .spec();
                assert!(w.name.contains("Sat"), "{}: weight {weight}", s.name);
                assert!(
                    w.range.is_some_and(|(lo, _)| lo.get() >= 0.0),
                    "{}: weight {weight} can be negative",
                    s.name
                );
            }
            _ => {}
        }
    }
    // Hue offsets of the colour mixer are linear, the grading hues circular.
    assert_eq!(row(Level::Global, "HueAdjustmentRed").policy, Policy::Learn);
    assert_eq!(
        row(Level::Global, "SplitToningShadowHue").policy,
        Policy::LearnGated(Gate::CircularHue("SplitToningShadowSaturation"))
    );
    assert_eq!(
        row(Level::Global, "ConvertToGrayscale").policy,
        Policy::LearnGated(Gate::Categorical)
    );
}

#[test]
fn unobserved_names_carry_no_semantics() {
    for (_, s) in iter() {
        if s.presence == Presence::Unobserved {
            assert!(
                matches!(s.policy, Policy::Unknown | Policy::Computed),
                "{}: unobserved key with policy {:?}",
                s.name,
                s.policy
            );
            assert!(s.recipe_alias.is_none() && !s.plus_sign, "{}", s.name);
        }
    }
}

#[test]
fn every_structure_is_reachable_and_has_fields() {
    let mut referenced = HashSet::new();
    let mut note = |k: ValueKind| {
        if let ValueKind::Struct(sk) | ValueKind::StructSeq(sk) = k {
            referenced.insert(sk);
        }
    };
    for (_, s) in iter() {
        note(s.kind.lua_form());
        note(s.kind.xmp_form());
    }
    for &kind in StructKind::ALL {
        if kind == StructKind::Unknown {
            continue;
        }
        assert!(
            referenced.contains(&kind),
            "{kind:?} is not referenced by any key"
        );
        let level = Level::Struct(kind);
        let has_rows = iter().any(|(_, s)| s.level == level);
        let has_pattern = PATTERNS.iter().any(|p| p.levels.contains(&level));
        assert!(has_rows || has_pattern, "{kind:?} has no fields");
    }
    for (_, s) in iter() {
        if let Level::Struct(kind) = s.level {
            assert!(
                referenced.contains(&kind),
                "{}: level {kind:?} unreachable",
                s.name
            );
        }
    }
}

#[test]
fn patterns_never_shadow_a_row() {
    for (_, s) in iter() {
        if let Some(p) = match_pattern(s.level, s.name) {
            panic!("{}/{} also matches pattern {}", s.level, s.name, p.id);
        }
    }
}

#[test]
fn resolve_prefers_rows_then_patterns() {
    assert!(matches!(
        resolve(Level::Global, "Exposure2012"),
        Resolved::Key(_)
    ));
    assert!(matches!(
        resolve(Level::Global, "UprightTransform_3"),
        Resolved::Pattern(p) if p.id == "upright_transform"
    ));
    assert!(matches!(
        resolve(Level::Global, "NoSuchKey"),
        Resolved::Unknown
    ));
}

#[test]
fn the_plugin_temperature_bug_key_is_not_a_registry_key() {
    // Built from pieces so no backend source holds the misspelt key as a
    // literal, which a later grep check enforces.
    let bug = concat!("Te", "mp");
    assert!(matches!(resolve(Level::Global, bug), Resolved::Unknown));
}

#[test]
fn per_format_shapes_are_limited_to_the_known_keys() {
    let per_format: Vec<_> = iter()
        .filter(|(_, s)| matches!(s.kind, ValueKind::PerFormat { .. }))
        .map(|(_, s)| s.name)
        .collect();
    assert_eq!(per_format, ["PointColors", "RetouchInfo"]);
    let pc = row(Level::Global, "PointColors");
    assert_eq!(pc.kind.xmp_form(), ValueKind::StrSeq);
    assert_eq!(
        pc.kind.lua_form(),
        ValueKind::StructSeq(StructKind::PointColor)
    );
}

#[test]
fn process_versions_follow_adobe_numbering() {
    // The registry draft had "6.7" as PV2010; Adobe's numbering is 5.7 = PV2010,
    // 6.7 = PV2012.
    assert_eq!(ProcessVersion::parse("5.7"), Some(ProcessVersion::PV2010));
    assert_eq!(ProcessVersion::parse("6.7"), Some(ProcessVersion::PV2012));
    assert_eq!(ProcessVersion::PV2012.label(), Some("PV2012"));
    assert_eq!(ProcessVersion::parse("15.4"), Some(ProcessVersion::V6));
    assert_eq!(ProcessVersion::parse("11.0").unwrap().to_string(), "11.0");
    assert!(ProcessVersion::PV2010 < ProcessVersion::PV2012);
    assert!(ProcessVersion::PV2012 < ProcessVersion::V4);
    assert!(ProcessVersion::parse("15").is_none());
    assert!(ProcessVersion::parse("x.4").is_none());
    assert!(ProcessVersion::parse("").is_none());
    assert_eq!(
        row(Level::Global, "ProcessVersion").kind,
        ValueKind::VersionStr
    );
}

#[test]
fn min_pv_marks_the_2012_keys() {
    for (_, s) in iter() {
        if let Some(pv) = s.min_pv {
            assert_eq!(pv, ProcessVersion::PV2012, "{}", s.name);
        }
    }
    for name in ["Exposure2012", "Contrast2012", "ToneCurvePV2012"] {
        assert_eq!(
            row(Level::Global, name).min_pv,
            Some(ProcessVersion::PV2012),
            "{name}"
        );
    }
    assert_eq!(
        row(Level::Correction, "LocalExposure2012").min_pv,
        Some(ProcessVersion::PV2012)
    );
    assert_eq!(row(Level::Global, "Vibrance").min_pv, None);
}

#[test]
fn group_rows_are_expanded() {
    for color in [
        "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
    ] {
        for prefix in [
            "HueAdjustment",
            "SaturationAdjustment",
            "LuminanceAdjustment",
            "GrayMixer",
        ] {
            let name = format!("{prefix}{color}");
            assert!(lookup(Level::Global, &name).is_some(), "{name}");
        }
    }
    assert!(lookup(Level::Global, "SDRWhites").is_some());
    assert!(lookup(Level::Correction, "BlueCurve").is_some());
}

#[test]
fn level_names_are_stable() {
    assert_eq!(Level::Global.to_string(), "global");
    assert_eq!(Level::MaskTool.to_string(), "mask-tool");
    assert_eq!(
        Level::Struct(StructKind::AiLook).to_string(),
        "struct:AILook"
    );
    assert!(!Level::Header.is_inside_struct());
    assert!(Level::Correction.is_inside_struct());
}
