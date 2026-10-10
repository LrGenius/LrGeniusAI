//! The XMP writer against the reader (plan §7.3, round trip 1 and the key
//! enumeration), on committed data only; the same round trip over
//! Lightroom's bundle and a sidecar corpus is in `xmp_goldens_local.rs`.
//!
//! - Every fixture in `testdata/develop/xmp/` and every writer golden in
//!   `testdata/develop/written/`: XMP → model → XMP → model gives the same
//!   model, header, kind and opaque content, and writing the result again
//!   gives the same bytes.
//! - Every LEARN and PHOTO key Lightroom writes to XMP, at every level:
//!   its minimum, maximum, default and midpoint (and a negative value for a
//!   signed range; every value of a closed set) survive a lossless write and
//!   parse exactly; the LEARN ones also survive a preset, after rounding to
//!   the key's number format ([`Value::quantized`]); what a gate holds back
//!   is pinned per key, gate and mode.
//! - A preset written from every Lua and XMP fixture (mixed and single-kind)
//!   reads back as a preset with no warning, without any key whose policy
//!   does not reach a preset or anything kept whole, and with exactly the
//!   policy-filtered settings, rounded as written (`support/preset.rs`).
//!
//! The lossless mode (`WriteMode::TestRoundTrip`) exists only with the crate
//! feature `test-roundtrip`, which this crate's own dev-dependency turns on
//! for its tests.

#[path = "support/enumerate.rs"]
mod enumerate;
#[path = "support/flat.rs"]
mod flat;
#[path = "support/preset.rs"]
mod preset;

use std::collections::BTreeMap;
use std::path::PathBuf;

use enumerate::{bare_component, correction, f, kid, leaf, learn_or_photo, one, place, samples};
use lrg_develop::build::{IdSlot, SyncNamespace};
use lrg_develop::lua::from_lua_str;
use lrg_develop::model::{
    Combine, Correction, DevelopSettings, Fields, Hex32, Look, MaskComponent, MaskTool, Semantic,
    Struct, Target, Value,
};
use lrg_develop::registry::{self, KeyId, Level, Presence, ProcessVersion, StructKind, ValueKind};
use lrg_develop::xmp::{parse, write, PresetHeader, PresetSpec, WriteMode, XmpDocument, XmpKind};
use lrg_develop::FileKindHint;

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
        .collect();
    out.sort();
    out
}

/// Writes `doc` losslessly and parses the result.
fn round_trip(doc: &XmpDocument) -> (String, XmpDocument) {
    let written = write(&doc.develop, &WriteMode::TestRoundTrip(doc.header.clone()))
        .unwrap_or_else(|e| panic!("write: {e}"));
    assert!(written.skipped.is_empty(), "{:?}", written.skipped);
    let back = parse(written.xmp.as_bytes()).unwrap_or_else(|e| panic!("{e}\n{}", written.xmp));
    (written.xmp, back)
}

/// The keys two documents disagree on (model and header).
fn differences(a: &XmpDocument, b: &XmpDocument) -> Vec<String> {
    let mut d = flat::diff(&flat::settings(&a.develop), &flat::settings(&b.develop));
    let empty = PresetHeader::default();
    let header = |doc: &XmpDocument| flat::header(doc.header.as_ref().unwrap_or(&empty));
    d.extend(flat::diff(&header(a), &header(b)));
    d
}

#[test]
fn every_xmp_fixture_round_trips_losslessly_and_deterministically() {
    let fixtures = files("xmp", "xmp");
    assert!(fixtures.len() >= 20, "only {} fixtures", fixtures.len());
    let written = files("written", "xmp");
    assert!(!written.is_empty(), "no writer golden");
    for (name, bytes) in fixtures.iter().chain(&written) {
        let doc = parse(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (xmp, back) = round_trip(&doc);
        assert_eq!(differences(&doc, &back), Vec::<String>::new(), "{name}");
        assert_eq!(back.develop, doc.develop, "{name}");
        assert_eq!(back.header, doc.header, "{name}");
        assert_eq!(back.kind, doc.kind, "{name}");
        assert_eq!(
            back.skipped_subtrees.kept_whole, doc.skipped_subtrees.kept_whole,
            "{name}"
        );
        let (again, _) = round_trip(&back);
        assert_eq!(
            again, xmp,
            "{name}: writing the read-back model changed the bytes"
        );
    }
}

// --- enumeration -------------------------------------------------------

/// Every LEARN and PHOTO key with an XMP form, with its values: written
/// losslessly, each value comes back exactly, the rest of the model is
/// unchanged, and writing again gives the same bytes.
#[test]
fn every_learn_and_photo_key_survives_a_lossless_write() {
    let mut checked = 0;
    let mut not_placed: Vec<String> = Vec::new();
    for (id, spec) in registry::iter() {
        if !learn_or_photo(spec) || spec.presence == Presence::LuaOnly {
            continue;
        }
        let values = samples(spec, false);
        if values.is_empty() {
            not_placed.push(format!("{}/{}", spec.level, spec.name));
            continue;
        }
        for v in &values {
            let what = format!("{}/{} = {v:?}", spec.level, spec.name);
            let Some((s, path)) = place(id, v) else {
                panic!("{what}: no place for this key; extend place()");
            };
            let written = write(&s, &WriteMode::TestRoundTrip(None))
                .unwrap_or_else(|e| panic!("{what}: {e}"));
            let doc = parse(written.xmp.as_bytes()).unwrap_or_else(|e| panic!("{what}: {e}"));
            let warnings: Vec<_> = doc
                .warnings
                .iter()
                .filter(|w| w.kind.name() != "UnrecognisedMaskCombine")
                .collect();
            assert!(warnings.is_empty(), "{what}: {warnings:?}\n{}", written.xmp);
            let back = flat::settings(&doc.develop);
            assert_eq!(
                back.get(&path),
                Some(&leaf(id, v)),
                "{what}\n{}",
                written.xmp
            );
            let mut changed = flat::diff(&flat::settings(&s), &back);
            // The reader derives the file kind from white-balance keys.
            changed.retain(|p| p != "(file kind)");
            assert!(changed.is_empty(), "{what}: {changed:?}\n{}", written.xmp);
            let again = write(&doc.develop, &WriteMode::TestRoundTrip(None)).unwrap();
            assert_eq!(again.xmp, written.xmp, "{what}: not idempotent");
            checked += 1;
        }
    }
    // Containers and sequences of structures: their fields are enumerated
    // at their own level above; the containers themselves are exercised by
    // the fixtures and the builder tests.
    let expected_containers = [
        "global/MaskGroupBasedCorrections",
        "global/LensBlur",
        "global/Look",
        "correction/CorrectionMasks",
        "mask-tool/CorrectionRangeMask",
        "mask-tool/Gesture",
        "struct:CorrectionRangeMask/AreaModels",
        "struct:Gesture/Points",
    ];
    eprintln!("lossless: {checked} values checked");
    not_placed.sort();
    let mut expected: Vec<String> = expected_containers.map(String::from).to_vec();
    expected.sort();
    assert_eq!(not_placed, expected);
    assert!(checked > 600, "only {checked} values checked");
}

/// A correction that reaches a preset: one subject mask, added.
fn subject_correction(local: BTreeMap<KeyId, Value>) -> Correction {
    let subject = MaskComponent {
        tool: MaskTool::Semantic(Semantic::Subject),
        combine: Combine::Add { inverted: false },
        ..bare_component(Fields::default())
    };
    correction(local, vec![subject])
}

fn preset_spec(mixed_file_kinds: bool) -> WriteMode {
    let uuid = SyncNamespace::new("xmp_roundtrip").id("preset", IdSlot::Correction);
    WriteMode::Preset(PresetSpec {
        mixed_file_kinds,
        process_version: Some(ProcessVersion::V6),
        ..PresetSpec::new(PresetHeader::lrgenius(uuid, "Round trip"))
    })
}

/// The LEARN keys a preset can carry by themselves (global values, a
/// correction's adjustments, `LensBlur` fields, the `Look` amount): each
/// value comes back from a preset rounded to its number format. A LEARN†
/// key may be held back by its gate (reported in `Written::skipped`); a
/// plain LEARN key never is.
#[test]
fn every_learn_value_survives_a_preset_after_rounding() {
    let mut checked = 0;
    let mut gated: BTreeMap<String, usize> = BTreeMap::new();
    for (id, spec) in registry::iter() {
        if !spec.policy.is_learnable() || spec.presence == Presence::LuaOnly {
            continue;
        }
        for v in samples(spec, false) {
            let what = format!("{}/{} = {v:?}", spec.level, spec.name);
            let mut s = DevelopSettings::new();
            type Get = fn(&DevelopSettings, KeyId) -> Option<Value>;
            let got: Get = match spec.level {
                Level::Global => {
                    if s.insert(id, v.clone()).is_err() {
                        continue; // Look, MaskGroupBasedCorrections: typed
                    }
                    |d, id| d.get(id).cloned()
                }
                Level::Correction if spec.name == "CorrectionAmount" => {
                    let mut c = subject_correction(BTreeMap::new());
                    c.local.insert(
                        kid(Level::Correction, "LocalExposure2012"),
                        Value::Real(f(0.1)),
                    );
                    c.amount = v.as_finite();
                    s.corrections = vec![c];
                    |d, _| Some(Value::Real(d.corrections.first()?.amount?))
                }
                Level::Correction if !matches!(spec.kind, ValueKind::ComponentSeq) => {
                    s.corrections = vec![subject_correction([(id, v.clone())].into())];
                    |d, id| d.corrections.first()?.local.get(&id).cloned()
                }
                Level::Struct(StructKind::LensBlur) => {
                    let lens_blur = one(StructKind::LensBlur, id, v.clone());
                    s.insert(kid(Level::Global, "LensBlur"), Value::Struct(lens_blur))
                        .unwrap();
                    |d, id| match d.get_by_name("LensBlur")? {
                        Value::Struct(lb) => lb.fields.values.get(&id).cloned(),
                        _ => None,
                    }
                }
                Level::Struct(StructKind::Look) if spec.name == "Amount" => {
                    s.look = Some(Look {
                        name: Some("Adobe Color".into()),
                        uuid: Hex32::parse("B952C231111CD8E0ECCF14B86BAA7077"),
                        amount: v.as_finite(),
                        camera_restriction: None,
                        rest: Struct::new(StructKind::Look),
                    });
                    |d, _| Some(Value::Real(d.look.as_ref()?.amount?))
                }
                // Mask and range-mask fields: typed, built by the builders
                // and checked through presets in the builder tests.
                _ => continue,
            };
            for mixed in [false, true] {
                let written =
                    write(&s, &preset_spec(mixed)).unwrap_or_else(|e| panic!("{what}: {e}"));
                let doc = parse(written.xmp.as_bytes()).unwrap_or_else(|e| panic!("{what}: {e}"));
                assert_eq!(doc.kind, XmpKind::Preset, "{what}");
                assert!(doc.warnings.is_empty(), "{what}: {:?}", doc.warnings);
                match got(&doc.develop, id) {
                    Some(back) => {
                        assert_eq!(back, v.quantized(spec), "{what}\n{}", written.xmp);
                        checked += 1;
                    }
                    None => {
                        let gate = spec
                            .policy
                            .gate()
                            .unwrap_or_else(|| panic!("{what}: a LEARN key missing from a preset"));
                        let mode = if mixed { "mixed" } else { "single-kind" };
                        *gated
                            .entry(format!("{}/{} {gate:?} {mode}", spec.level, spec.name))
                            .or_default() += 1;
                    }
                }
            }
        }
    }
    eprintln!("preset: {checked} values checked; held back by their gate: {gated:?}");
    assert!(checked > 500, "only {checked} values checked");
    // Pinned, so a gate that starts holding back more (a single-kind raw
    // preset losing `Temperature`, say) fails here. Only a mixed preset
    // drops a file-kind key; `LensProfileSetup` keeps only the values its
    // `OnlyValues` gate lists (the sample `Custom` fails it in both modes).
    let expected: BTreeMap<String, usize> = [
        ("global/ColorNoiseReduction FileKindDefault mixed", 4),
        ("global/IncrementalTemperature NonRawOnly mixed", 4),
        ("global/IncrementalTint NonRawOnly mixed", 4),
        (
            "global/LensProfileSetup OnlyValues([\"LensDefaults\", \"Auto\"]) mixed",
            1,
        ),
        (
            "global/LensProfileSetup OnlyValues([\"LensDefaults\", \"Auto\"]) single-kind",
            1,
        ),
        ("global/Sharpness FileKindDefault mixed", 4),
        ("global/Temperature RawOnly mixed", 3),
        ("global/Tint RawOnly mixed", 4),
    ]
    .into_iter()
    .map(|(k, n)| (k.to_owned(), n))
    .collect();
    assert_eq!(gated, expected);
}

// --- presets from the fixtures ----------------------------------------

/// Writes `settings` as a mixed and a single-kind preset and checks both
/// read back clean: a preset without a warning, without any key a preset
/// must not carry or anything kept whole ([`preset::violations`]), and with
/// exactly the filtered settings ([`preset::differences`]). Returns the
/// number of values compared.
fn check_preset(
    what: &str,
    settings: &DevelopSettings,
    bad: &mut std::collections::BTreeSet<String>,
) -> usize {
    let mut checked = 0;
    for mixed in [true, false] {
        let written =
            write(settings, &preset_spec(mixed)).unwrap_or_else(|e| panic!("{what}: {e}"));
        let again = write(settings, &preset_spec(mixed)).unwrap();
        assert_eq!(again.xmp, written.xmp, "{what}: not deterministic");
        let doc = parse(written.xmp.as_bytes()).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(doc.kind, XmpKind::Preset, "{what}");
        assert!(doc.warnings.is_empty(), "{what}: {:?}", doc.warnings);
        assert!(!written.xmp.contains("crss:"), "{what}");
        assert!(!written.xmp.contains("Digest"), "{what}");
        assert!(!written.xmp.contains("rdf:Bag"), "{what}");
        let mode = if mixed { "mixed" } else { "single-kind" };
        for v in preset::violations(&doc) {
            bad.insert(format!("{what} ({mode}): {v}"));
        }
        let (filtered, _) = settings.filtered(&Target::Preset {
            mixed_file_kinds: mixed,
        });
        for d in preset::differences(&filtered, &doc.develop) {
            bad.insert(format!("{what} ({mode}): {d}"));
        }
        checked += flat::settings(&doc.develop).len();
    }
    checked
}

#[test]
fn presets_written_from_every_fixture_read_back_clean() {
    let mut checked = 0;
    let mut bad = std::collections::BTreeSet::new();
    let lua = files("lua", "json");
    for (name, bytes) in lua.iter().filter(|(n, _)| n != "manifest.json") {
        let text = std::str::from_utf8(bytes).unwrap();
        let (settings, _) =
            from_lua_str(text, FileKindHint::Unknown).unwrap_or_else(|e| panic!("{name}: {e}"));
        checked += check_preset(name, &settings, &mut bad);
    }
    for (name, bytes) in files("xmp", "xmp") {
        let doc = parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        checked += check_preset(&name, &doc.develop, &mut bad);
    }
    eprintln!("presets from fixtures: {checked} values checked");
    assert!(bad.is_empty(), "presets that are not clean: {bad:#?}");
    assert!(checked > 1000, "only {checked} values checked");
}

/// The two preset checks above catch what they are for: a value lost or
/// changed on the way, a key that must not be in a preset, content kept
/// whole.
#[test]
fn the_preset_checks_are_not_vacuous() {
    let mut s = DevelopSettings::new();
    s.insert(kid(Level::Global, "Exposure2012"), Value::Real(f(0.5)))
        .unwrap();
    s.insert(kid(Level::Global, "Contrast2012"), Value::Int(10))
        .unwrap();
    s.corrections = vec![subject_correction(
        [(kid(Level::Correction, "LocalDehaze"), Value::Real(f(0.2)))].into(),
    )];
    let written = write(&s, &preset_spec(true)).unwrap();
    let doc = parse(written.xmp.as_bytes()).unwrap();
    let (filtered, _) = s.filtered(&Target::Preset {
        mixed_file_kinds: true,
    });
    assert_eq!(
        preset::differences(&filtered, &doc.develop),
        Vec::<String>::new()
    );
    assert_eq!(preset::violations(&doc), Vec::<String>::new());

    let mut bad = doc.clone();
    bad.develop.remove(kid(Level::Global, "Exposure2012"));
    bad.develop
        .insert(kid(Level::Global, "Contrast2012"), Value::Int(11))
        .unwrap();
    bad.develop
        .insert(kid(Level::Global, "CropTop"), Value::Real(f(0.1)))
        .unwrap();
    bad.develop
        .insert(
            kid(Level::Global, "Texture"),
            Value::Opaque(lrg_develop::model::Opaque::Json(serde_json::json!(1))),
        )
        .unwrap();
    assert_eq!(
        preset::differences(&filtered, &bad.develop),
        [
            "changed Contrast2012",
            "missing Exposure2012",
            "extra CropTop",
            "extra Texture"
        ]
    );
    assert_eq!(
        preset::violations(&bad),
        ["kept whole global/Texture", "PHOTO global/CropTop"]
    );
}
