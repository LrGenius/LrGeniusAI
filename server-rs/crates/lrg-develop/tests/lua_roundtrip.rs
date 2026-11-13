//! The Lua writer against the Lua reader (plan §7.3, round trips 2 and 3
//! and the key enumeration), on committed data only; the same over the
//! private training dump is in `training_dump_local.rs`.
//!
//! - **Round trip 2**, model → Lua → model, lossless: every fixture in
//!   `testdata/develop/lua/` and every wire golden (opaque content and
//!   warnings included), under every encoding the experiments may choose
//!   (mask enums as numbers or strings, flags as numbers or booleans), and
//!   every XMP fixture without what has no Lua form (XMP-only keys,
//!   `PerFormat` values in their XMP shape, content kept as XMP); writing
//!   the read-back model again gives the same table.
//! - **Round trip 3**, Lua → model → XMP preset → model → Lua: every Lua
//!   fixture, every wire golden and every XMP fixture (without what has no
//!   Lua form), mixed and single-kind preset; the values the two Lua tables
//!   share are equal after rounding to the XMP writer's number formats
//!   (`Value::quantized`; Lua carries Lua 5.1 `%.14g` doubles, a preset
//!   `Fixed(2)`/`Trim6`). The check itself is `support/lua_round.rs`, which
//!   the local sweeps run over private data.
//! - **Key enumeration**: every LEARN and PHOTO key Lightroom has in its Lua
//!   tables, at every level, with the enumeration's values
//!   (`support/enumerate.rs`), survives the lossless mode exactly; and in
//!   apply mode each global, correction, `LensBlur` and `Look.Amount` value
//!   either reaches the photo exactly or is reported in
//!   `LuaWritten::skipped` (or is its key's default), never lost silently.
//!   Mask-tool and range-mask values are not placed by the enumeration in
//!   apply mode; the next check covers them.
//! - **Apply mode on the committed fixtures** (`support/lua_apply.rs`):
//!   every Lua and XMP fixture written for its own photo, with the
//!   provisional options and with every single option flip, in apply and
//!   preset mode: wire-safe, read back without a warning, every filtered
//!   value written exactly or reported, nothing added beyond the writer's
//!   fixed form. The same check the local sweeps run over private data.
//!
//! The lossless mode (`LuaMode::TestRoundTrip`) exists only with the crate
//! feature `test-roundtrip`, which this crate's own dev-dependency turns on
//! for its tests.

#[path = "support/enumerate.rs"]
mod enumerate;
#[allow(dead_code)] // `header` is for the XMP tests
#[path = "support/flat.rs"]
mod flat;
#[allow(dead_code)] // the counted sweep entry point is for the local sweeps
#[path = "support/lua_apply.rs"]
mod lua_apply;
#[allow(dead_code)] // round trip 2 is asserted per file here, not counted
#[path = "support/lua_round.rs"]
mod lua_round;
#[allow(dead_code)] // only `quantized` and `same` are used (by `lua_round`)
#[path = "support/preset.rs"]
mod preset;
#[allow(dead_code)] // only `generic_path` is used (by `lua_round`)
mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use enumerate::{correction, kid, leaf, learn_or_photo, one, place, samples};
use lrg_develop::lua::{
    check_wire, from_lua_str, from_lua_value, to_lua_value, LuaMode, LuaOptions, LuaWritten,
    PhotoContext,
};
use lrg_develop::lua::{EnumAs, FlagAs};
use lrg_develop::model::{
    Combine, Correction, DevelopSettings, Fields, FileKind, Hex32, Look, MaskComponent, MaskTool,
    Semantic, Struct, Value,
};
use lrg_develop::registry::{
    self, Def, KeyId, Level, Presence, ProcessVersion, StructKind, ValueKind,
};
use lrg_develop::xmp::parse;
use lrg_develop::{FileKindHint, ParseWarning};
use lua_apply::{flips, LuaApply};
use lua_round::{round_trip, round_trip_with, without_xmp_only, LuaRoundTrips};

fn testdata(dir: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/develop")
        .join(dir)
}

/// The files with `extension` in `testdata/develop/<dir>`, sorted.
fn files(dir: &str, extension: &str) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(testdata(dir))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(extension))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read(&p).unwrap())
        })
        .filter(|(n, _)| n != "manifest.json")
        .collect();
    out.sort();
    out
}

fn kinds(w: &[ParseWarning]) -> Vec<(&'static str, &str)> {
    w.iter().map(|w| (w.kind.name(), w.path.as_str())).collect()
}

// --- round trip 2 ------------------------------------------------------------

/// Round trip 2 under every encoding the lossless mode takes from the
/// options, so whichever form E2 settles on (or a flag form chosen later)
/// reads back: the reader accepts both forms of each.
#[test]
fn every_lua_fixture_and_wire_golden_round_trips_losslessly() {
    let mut n = 0;
    for mask_enum_as in [EnumAs::Number, EnumAs::String] {
        for int_flag_as in [FlagAs::Number, FlagAs::Bool] {
            let opt = LuaOptions {
                mask_enum_as,
                int_flag_as,
                ..LuaOptions::PROVISIONAL
            };
            let how = format!("{mask_enum_as:?}/{int_flag_as:?}");
            for dir in ["lua", "wire"] {
                for (name, bytes) in files(dir, "json") {
                    let text = std::str::from_utf8(&bytes).unwrap();
                    let what = format!("{dir}/{name} ({how})");
                    let (s, w0) = from_lua_str(text, FileKindHint::Unknown)
                        .unwrap_or_else(|e| panic!("{what}: {e}"));
                    let (table, back, w1) = round_trip_with(&s, &opt);
                    assert_eq!(
                        flat::diff(&flat::settings(&s), &flat::settings(&back)),
                        Vec::<String>::new(),
                        "{what}"
                    );
                    assert_eq!(back, s, "{what}");
                    // The same findings again (unknown keys, wrong types,
                    // ...), apart from a rounding: the model holds the
                    // rounded integer.
                    let w0: Vec<ParseWarning> = w0
                        .into_iter()
                        .filter(|w| w.kind.name() != "NonIntegerForIntKey")
                        .collect();
                    assert_eq!(kinds(&w1), kinds(&w0), "{what}");
                    let (again, _, _) = round_trip_with(&back, &opt);
                    assert_eq!(again, table, "{what}: not idempotent");
                    n += 1;
                }
            }
        }
    }
    eprintln!("round trip 2: {n} Lua tables (4 encodings)");
    assert!(n >= 4 * 35, "only {n} files");
}

#[test]
fn every_xmp_fixture_round_trips_through_lua_without_its_xmp_only_parts() {
    let mut n = 0;
    for (name, bytes) in files("xmp", "xmp") {
        let doc = parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let s = without_xmp_only(&doc.develop);
        let (_, back, warnings) = round_trip(&s);
        // Only the fixtures with findings of their own have them again.
        assert!(
            warnings.is_empty() || !doc.warnings.is_empty(),
            "{name}: {warnings:?}"
        );
        assert_eq!(
            flat::diff(&flat::settings(&s), &flat::settings(&back)),
            Vec::<String>::new(),
            "{name}"
        );
        n += 1;
    }
    assert!(n >= 20, "only {n} fixtures");
}

// --- round trip 3 ------------------------------------------------------------

#[test]
fn lua_through_an_xmp_preset_and_back_to_lua_agrees_after_quantization() {
    let mut all = LuaRoundTrips::default();
    let mut sources: Vec<(String, DevelopSettings)> = Vec::new();
    for dir in ["lua", "wire"] {
        for (name, bytes) in files(dir, "json") {
            let text = std::str::from_utf8(&bytes).unwrap();
            let (s, _) = from_lua_str(text, FileKindHint::Unknown).unwrap();
            sources.push((format!("{dir}/{name}"), s));
        }
    }
    for (name, bytes) in files("xmp", "xmp") {
        let doc = parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        sources.push((format!("xmp/{name}"), without_xmp_only(&doc.develop)));
    }
    for (name, s) in &sources {
        // Per source, so a failure names it (the fixtures are public).
        let mut one = LuaRoundTrips::default();
        one.round_trip_3_only(s);
        if one.rt3_preset_errors.is_empty()
            && one.rt3_not_lossless.is_empty()
            && one.rt3_warnings.is_empty()
            && one.rt3_changed.is_empty()
        {
            all.round_trip_3_only(s);
            continue;
        }
        one.report("");
        panic!("{name}: round trip 3 is not clean");
    }
    all.report("");
    assert!(
        all.rt3_compared > 4000,
        "only {} values compared",
        all.rt3_compared
    );
}

// --- the key enumeration -------------------------------------------------------

/// Every LEARN and PHOTO key Lightroom has in its Lua tables, with its
/// values: written losslessly, each value comes back exactly, the rest of
/// the model is unchanged, and writing again gives the same table.
#[test]
fn every_learn_and_photo_key_survives_a_lossless_lua_write() {
    let mut checked = 0;
    let mut not_placed: Vec<String> = Vec::new();
    for (id, spec) in registry::iter() {
        if !learn_or_photo(spec) || !matches!(spec.presence, Presence::Both | Presence::LuaOnly) {
            continue;
        }
        let values = samples(spec, true);
        if values.is_empty() {
            not_placed.push(format!("{}/{}", spec.level, spec.name));
            continue;
        }
        for v in &values {
            let what = format!("{}/{} = {v:?}", spec.level, spec.name);
            let Some((s, path)) = place(id, v) else {
                panic!("{what}: no place for this key; extend place()");
            };
            let (table, back, warnings) = round_trip(&s);
            let warnings: Vec<_> = warnings
                .iter()
                .filter(|w| w.kind.name() != "UnrecognisedMaskCombine")
                .collect();
            assert!(warnings.is_empty(), "{what}: {warnings:?}\n{table}");
            let flat_back = flat::settings(&back);
            assert_eq!(flat_back.get(&path), Some(&leaf(id, v)), "{what}\n{table}");
            let mut changed = flat::diff(&flat::settings(&s), &flat_back);
            changed.retain(|p| p != "(file kind)");
            assert!(changed.is_empty(), "{what}: {changed:?}\n{table}");
            let (again, _, _) = round_trip(&back);
            assert_eq!(again, table, "{what}: not idempotent");
            checked += 1;
        }
    }
    eprintln!("lossless Lua: {checked} values checked");
    not_placed.sort();
    let mut expected: Vec<String> = [
        "global/MaskGroupBasedCorrections",
        "global/LensBlur",
        "global/Look",
        "correction/CorrectionMasks",
        "mask-tool/CorrectionRangeMask",
        "mask-tool/Gesture",
        "struct:CorrectionRangeMask/AreaModels",
        "struct:Gesture/Points",
    ]
    .map(String::from)
    .to_vec();
    expected.sort();
    assert_eq!(not_placed, expected);
    assert!(checked > 600, "only {checked} values checked");
}

/// A correction that reaches a photo: one subject mask, added.
fn subject_correction(local: BTreeMap<KeyId, Value>) -> Correction {
    let subject = MaskComponent {
        tool: MaskTool::Semantic(Semantic::Subject),
        combine: Combine::Add { inverted: false },
        name: None,
        sync_id: None,
        active: Some(true),
        extra: Fields::default(),
    };
    correction(local, vec![subject])
}

/// `v` of `id` where the apply-mode check places it, and how to read it
/// back; `None` for mask-tool, range-mask, gesture and `Look` name fields,
/// which `every_committed_fixture_applies_cleanly_under_every_option_flip`
/// covers through the fixtures that hold them.
fn place_for_apply(id: KeyId, v: &Value) -> Option<(DevelopSettings, String)> {
    let spec = id.spec();
    let mut s = DevelopSettings::new();
    let path = match spec.level {
        Level::Global => {
            s.insert(id, v.clone()).ok()?;
            spec.name.to_owned()
        }
        Level::Correction if spec.name == "CorrectionAmount" => {
            let mut c = subject_correction(
                [(
                    kid(Level::Correction, "LocalExposure2012"),
                    Value::Real(lrg_develop::Finite::new(0.1).unwrap()),
                )]
                .into(),
            );
            c.amount = v.as_finite();
            s.corrections = vec![c];
            "MaskGroupBasedCorrections[0].CorrectionAmount".to_owned()
        }
        Level::Correction if !matches!(spec.kind, ValueKind::ComponentSeq) => {
            s.corrections = vec![subject_correction([(id, v.clone())].into())];
            format!("MaskGroupBasedCorrections[0].{}", spec.name)
        }
        Level::Struct(StructKind::LensBlur) => {
            let lens_blur = one(StructKind::LensBlur, id, v.clone());
            s.insert(kid(Level::Global, "LensBlur"), Value::Struct(lens_blur))
                .unwrap();
            format!("LensBlur.{}", spec.name)
        }
        Level::Struct(StructKind::Look) if spec.name == "Amount" => {
            s.look = Some(Look {
                name: Some("Adobe Color".into()),
                uuid: Hex32::parse("B952C231111CD8E0ECCF14B86BAA7077"),
                amount: v.as_finite(),
                camera_restriction: None,
                rest: Struct::new(StructKind::Look),
            });
            "Look.Amount".to_owned()
        }
        _ => return None,
    };
    Some((s, path))
}

/// Whether `path` or one of its ancestors was reported.
fn reported(w: &LuaWritten, path: &str) -> bool {
    let skipped: Vec<String> = w.skipped.iter().map(|k| k.path.clone()).collect();
    lua_apply::reported(&skipped, path)
}

/// In apply mode, for a raw and a non-raw photo: every LEARN and PHOTO
/// value the enumeration places (global, correction, `LensBlur`,
/// `Look.Amount`; see [`place_for_apply`]) reaches the photo exactly (read
/// back from the table), or is reported, or is its key's default. What is
/// held back is pinned per key and photo kind, so a value that starts going
/// missing fails here.
#[test]
fn every_learn_and_photo_value_reaches_the_photo_or_is_reported() {
    let mut written = 0;
    let mut held: BTreeMap<String, usize> = BTreeMap::new();
    for (id, spec) in registry::iter() {
        if !learn_or_photo(spec) || !matches!(spec.presence, Presence::Both | Presence::LuaOnly) {
            continue;
        }
        for v in samples(spec, true) {
            let Some((s, path)) = place_for_apply(id, &v) else {
                continue;
            };
            for kind in [FileKind::Raw, FileKind::NonRaw] {
                let what = format!("{}/{} = {v:?} ({kind:?})", spec.level, spec.name);
                let photo = PhotoContext {
                    file_kind: Some(kind),
                    process_version: Some(ProcessVersion::V6),
                    camera: None,
                };
                let w = to_lua_value(&s, &LuaMode::Apply(photo), &LuaOptions::default())
                    .unwrap_or_else(|e| panic!("{what}: {e}"));
                check_wire(&w.table).unwrap();
                let (back, warnings) = from_lua_value(&w.table, FileKindHint::Unknown).unwrap();
                assert!(warnings.is_empty(), "{what}: {warnings:?}\n{}", w.table);
                let got = flat::settings(&back);
                match got.get(&path) {
                    Some(g) if *g == leaf(id, &v) => written += 1,
                    // Held back, or replaced by the writer's own form (a
                    // reported correction value under the adaptive-preset
                    // zero): reported, or the key's default.
                    _ => {
                        let default = [spec.default.raw, spec.default.non_raw]
                            .iter()
                            .any(|d| matches!(d, Def::Value(lit) if v.equals_lit(lit)));
                        assert!(
                            reported(&w, &path) || default,
                            "{what}: neither written nor reported: {:?}\n{}",
                            w.skipped,
                            w.table
                        );
                        *held
                            .entry(format!("{}/{} {kind:?}", spec.level, spec.name))
                            .or_default() += 1;
                    }
                }
            }
        }
    }
    eprintln!("apply: {written} values written; held back {held:?}");
    assert!(written > 900, "only {written} values written");
    // Pinned: the white-balance family by file kind, a mode without
    // numbers (`wb_mode_only` off), `LensProfileSetup` outside its listed
    // values, and two PHOTO keys the never-write guard keeps out of every
    // table (a digest, and `orientation`).
    let expected: BTreeMap<String, usize> = [
        ("global/IncrementalTemperature Raw", 4),
        ("global/IncrementalTint Raw", 4),
        ("global/LensProfileDigest NonRaw", 1),
        ("global/LensProfileDigest Raw", 1),
        ("global/LensProfileSetup NonRaw", 1),
        ("global/LensProfileSetup Raw", 1),
        ("global/orientation NonRaw", 1),
        ("global/orientation Raw", 1),
        ("global/Temperature NonRaw", 3),
        ("global/Tint NonRaw", 4),
        ("global/WhiteBalance NonRaw", 9),
        ("global/WhiteBalance Raw", 9),
    ]
    .into_iter()
    .map(|(k, n)| (k.to_owned(), n))
    .collect();
    assert_eq!(held, expected);
}

// --- apply mode on the committed fixtures --------------------------------------

/// Every committed Lua fixture (read as Lightroom wrote it) and XMP fixture
/// (its develop settings) written for its own photo, with the provisional
/// options and with every single option flip, in apply and preset mode:
/// wire-safe, read back without a warning, every filtered value written
/// exactly or reported, nothing added beyond the writer's fixed form. This
/// covers the mask-tool, range-mask and gesture values the enumeration does
/// not place, and proves the Rust side of "flip a field, re-bless".
#[test]
fn every_committed_fixture_applies_cleanly_under_every_option_flip() {
    let mut sources: Vec<(String, DevelopSettings)> = Vec::new();
    for (name, bytes) in files("lua", "json") {
        let text = std::str::from_utf8(&bytes).unwrap();
        let (s, _) = from_lua_str(text, FileKindHint::Unknown).unwrap();
        sources.push((format!("lua/{name}"), s));
    }
    for (name, bytes) in files("xmp", "xmp") {
        let doc = parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        // Camera profiles carry a profile's tables, not develop settings
        // (as in the local sweeps).
        if doc.kind == lrg_develop::xmp::XmpKind::Profile {
            continue;
        }
        sources.push((format!("xmp/{name}"), doc.develop));
    }
    let mut options = vec![("provisional".to_owned(), LuaOptions::PROVISIONAL)];
    options.extend(flips(LuaOptions::PROVISIONAL));
    let mut all = LuaApply::default();
    for (how, opt) in &options {
        for (name, s) in &sources {
            let photo = PhotoContext {
                file_kind: s.file_kind,
                process_version: s.process_version(),
                camera: None,
            };
            for mode in [
                LuaMode::Apply(photo.clone()),
                LuaMode::Preset(photo.clone()),
            ] {
                // Per source, so a failure names it (the fixtures are public).
                let mut one = LuaApply::default();
                one.settings_for(s, &mode, opt);
                if !one.is_clean() {
                    one.report("  ");
                    panic!("{name} ({how}, {mode:?}): not clean");
                }
                all.settings_for(s, &mode, opt);
            }
        }
    }
    all.report("");
    all.assert_clean("committed fixtures");
    eprintln!(
        "{} sources x {} option sets x 2 modes",
        sources.len(),
        options.len()
    );
    assert!(sources.len() >= 40, "only {} sources", sources.len());
    assert!(
        all.compared > 50_000,
        "only {} values compared",
        all.compared
    );
}
