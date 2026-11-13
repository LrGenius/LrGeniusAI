//! The hand-test presets of PR 1b, built with the real builders and writer.
//!
//! Always: all three presets build, write without skipping anything, and read
//! back as develop presets whose corrections are the built ones; and, with
//! synthetic ids in place of the derived ones, they match their byte goldens
//! in `testdata/develop/written/` (the writer's exact output, pinned). After
//! an intended format change, regenerate the goldens with
//! `LRG_BLESS=1 cargo test -p lrg-develop --test builder_presets`.
//!
//! By hand: with `LRG_HANDTEST_DIR` set, the test also writes them there, to
//! be imported in the installed Lightroom Classic (Develop → Presets →
//! Import) and applied to photos with a subject, sky and vegetation, and to
//! one with two or more people (does the people part reach all of them?):
//!
//! ```text
//! LRG_HANDTEST_DIR=/some/dir cargo test -p lrg-develop --test builder_presets
//! ```
//!
//! | File | `CompatibleVersion` | Expect in the Masking panel |
//! |---|---|---|
//! | `LrGenius Handtest Subject+Sky.xmp` | 14.0 | Contrast +15; "LrGenius · Subject" (Subject, Exposure +0.30); "LrGenius · Sky" (Sky, Highlights −40, Dehaze +15) |
//! | `LrGenius Handtest Landscape+People.xmp` | 15.3 | "LrGenius · Vegetation" (Vegetation, Saturation +20); "LrGenius · Eyes" (Iris and Pupil of all people, Saturation +30) |
//! | `LrGenius Handtest Combinations+Background.xmp` | 15.0 | "LrGenius · Bright sky" (Sky, minus Subject, intersected with a luminance range 0.3–0.5–1–1: Highlights −30); "LrGenius · Background" (Background, Exposure −0.50, Saturation −30) |
//!
//! The third preset carries what no preset of Adobe's does: a subtract, an
//! intersect, a luminance range (without the eyedropper sample every range
//! in Lightroom's sidecars has) and Select Background.

use std::path::PathBuf;

use lrg_develop::build::{
    corrections, BuildError, CorrectionBuilder, IdSlot, LandscapeClass, LuminanceRange, PeoplePart,
    SyncNamespace,
};
use lrg_develop::model::{
    Correction, DevelopSettings, Fields, Finite, Hex32, MaskTool, Semantic, Value,
};
use lrg_develop::registry::{self, Level, ProcessVersion};
use lrg_develop::xmp::write::xmptk;
use lrg_develop::xmp::{
    compatible_version, parse, write, EngineVersion, PresetHeader, PresetSpec, WriteMode, XmpKind,
};

struct HandTest {
    name: &'static str,
    /// The golden's file name in `testdata/develop/written/`.
    golden: &'static str,
    settings: DevelopSettings,
    compatible: EngineVersion,
}

fn subject_and_sky() -> Result<HandTest, BuildError> {
    let ns = SyncNamespace::lrgenius();
    let mut settings = DevelopSettings::new();
    let contrast = registry::lookup(Level::Global, "Contrast2012").unwrap();
    let (value, clamped) =
        registry::from_ui_value(contrast.spec(), Finite::new(15.0).unwrap()).unwrap();
    assert!(clamped.is_none());
    settings.insert(contrast, value).unwrap();
    settings.corrections = corrections([
        CorrectionBuilder::new("Subject", &ns)
            .local_ui("LocalExposure2012", 0.3)?
            .add(Semantic::Subject),
        CorrectionBuilder::new("Sky", &ns)
            .local_ui("LocalHighlights2012", -40.0)?
            .local_ui("LocalDehaze", 15.0)?
            .add(Semantic::Sky),
    ])?;
    Ok(HandTest {
        name: "LrGenius Handtest Subject+Sky",
        golden: "handtest_subject_sky.xmp",
        settings,
        compatible: EngineVersion::new(14, 0),
    })
}

fn landscape_and_people() -> Result<HandTest, BuildError> {
    let ns = SyncNamespace::lrgenius();
    let mut settings = DevelopSettings::new();
    settings.corrections = corrections([
        CorrectionBuilder::new("Vegetation", &ns)
            .local_ui("LocalSaturation", 20.0)?
            .add(Semantic::landscape(LandscapeClass::Vegetation)),
        CorrectionBuilder::new("Eyes", &ns)
            .local_ui("LocalSaturation", 30.0)?
            .add(Semantic::people_part(PeoplePart::IrisAndPupil)),
    ])?;
    Ok(HandTest {
        name: "LrGenius Handtest Landscape+People",
        golden: "handtest_landscape_people.xmp",
        settings,
        compatible: EngineVersion::new(15, 3),
    })
}

/// What no preset of Adobe's has: subtract, intersect, a luminance range,
/// Select Background.
fn combinations_and_background() -> Result<HandTest, BuildError> {
    let ns = SyncNamespace::lrgenius();
    let mut settings = DevelopSettings::new();
    settings.corrections = corrections([
        CorrectionBuilder::new("Bright sky", &ns)
            .local_ui("LocalHighlights2012", -30.0)?
            .add(Semantic::Sky)
            .subtract(Semantic::Subject)
            .intersect(LuminanceRange::highs(0.5, 0.2)?),
        CorrectionBuilder::new("Background", &ns)
            .local_ui("LocalExposure2012", -0.5)?
            .local_ui("LocalSaturation", -30.0)?
            .add(Semantic::Background),
    ])?;
    Ok(HandTest {
        name: "LrGenius Handtest Combinations+Background",
        golden: "handtest_combinations_background.xmp",
        settings,
        compatible: EngineVersion::new(15, 0),
    })
}

fn all() -> [Result<HandTest, BuildError>; 3] {
    [
        subject_and_sky(),
        landscape_and_people(),
        combinations_and_background(),
    ]
}

/// The typed parts a preset must carry back. A luminance range's
/// `CorrectionRangeMask` rest is left out: the writer completes `Version` 3
/// and `SampleType` 0 there (checked below). `active` and `extra` are not
/// compared here; the byte goldens pin them.
fn typed(c: &Correction) -> impl PartialEq + std::fmt::Debug + '_ {
    let masks: Vec<_> = c
        .masks
        .iter()
        .map(|m| {
            let mut tool = m.tool.clone();
            if let MaskTool::LuminanceRange(lr) = &mut tool {
                lr.rest.fields = Fields::default();
            }
            (tool, m.combine, &m.name, &m.sync_id)
        })
        .collect();
    (&c.name, &c.sync_id, c.amount, &c.local, masks)
}

#[test]
fn the_hand_test_presets_write_and_read_back() {
    let out_dir = std::env::var_os("LRG_HANDTEST_DIR").map(PathBuf::from);
    for t in all() {
        let t = t.unwrap_or_else(|e| panic!("{e}"));
        let uuid = SyncNamespace::new("LrGenius hand test").id(t.name, IdSlot::Correction);
        let mut spec = PresetSpec::new(PresetHeader::lrgenius(uuid, t.name));
        // As Adobe's adaptive presets do: AI masks need the current process
        // version.
        spec.process_version = Some(ProcessVersion::V6);
        let written = write(&t.settings, &WriteMode::Preset(spec)).unwrap();
        assert!(written.skipped.is_empty(), "{:?}", written.skipped);
        assert_eq!(compatible_version(&t.settings), Some(t.compatible));

        let doc = parse(written.xmp.as_bytes()).unwrap();
        assert_eq!(doc.kind, XmpKind::Preset);
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert_eq!(doc.header.and_then(|h| h.name).as_deref(), Some(t.name));
        let read: Vec<_> = doc.develop.corrections.iter().map(typed).collect();
        let built: Vec<_> = t.settings.corrections.iter().map(typed).collect();
        assert_eq!(read, built);
        for (id, v) in t.settings.values() {
            assert_eq!(doc.develop.get(id), Some(v));
        }
        for m in doc.develop.corrections.iter().flat_map(|c| &c.masks) {
            if let MaskTool::LuminanceRange(lr) = &m.tool {
                let crm = |n| lr.rest.get(n).cloned();
                assert_eq!(
                    (crm("Version"), crm("SampleType")),
                    (Some(Value::Int(3)), Some(Value::Int(0)))
                );
                let (_, _, inverted) = m.combine.encode().unwrap();
                assert_eq!(lr.invert, inverted, "Invert mirrors MaskInverted");
            }
        }

        if let Some(dir) = &out_dir {
            std::fs::create_dir_all(dir).unwrap();
            let path = dir.join(format!("{}.xmp", t.name));
            std::fs::write(&path, &written.xmp).unwrap();
            // Re-parse what landed on disk, not the string.
            let again = parse(&std::fs::read(&path).unwrap()).unwrap();
            assert!(again.warnings.is_empty());
            assert_eq!(
                again.develop.corrections.len(),
                t.settings.corrections.len()
            );
            println!("wrote {}", path.display());
        }
    }
}

/// The ids the hygiene rules accept in a committed file: twenty zeros and
/// twelve hex digits (`testdata/develop/README.md`, "Hygiene rules").
fn synthetic(n: u64) -> Hex32 {
    Hex32::parse(&format!("{:020}{n:012X}", 0)).unwrap()
}

/// The derived sync ids (and the preset UUID) replaced by synthetic ones:
/// a derived id is a SHA-256 digest, which the hygiene rules cannot tell
/// from a catalog's. The ids' own determinism is tested in `build::tests`.
fn with_synthetic_ids(mut t: HandTest, first: u64) -> (HandTest, Hex32) {
    let mut n = first;
    let mut next = || {
        n += 1;
        synthetic(n)
    };
    for c in &mut t.settings.corrections {
        c.sync_id = Some(next());
        for m in &mut c.masks {
            m.sync_id = Some(next());
        }
    }
    let uuid = next();
    (t, uuid)
}

#[test]
fn the_hand_test_presets_match_their_byte_goldens() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/develop/written");
    let bless = std::env::var_os("LRG_BLESS").is_some_and(|v| v == "1");
    for (i, t) in all().into_iter().enumerate() {
        let (t, uuid) = with_synthetic_ids(t.unwrap(), 0xB00 + 0x100 * i as u64);
        let mut spec = PresetSpec::new(PresetHeader::lrgenius(uuid, t.name));
        spec.process_version = Some(ProcessVersion::V6);
        let written = write(&t.settings, &WriteMode::Preset(spec)).unwrap();
        // The toolkit string carries the release version when one is baked
        // into the build; the golden holds the development one.
        let text = written.xmp.replace(&xmptk(), "LrGeniusAI 0.0.0-dev");
        let path = dir.join(t.golden);
        if bless {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &text).unwrap();
            continue;
        }
        let golden = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e} (bless with LRG_BLESS=1)", t.golden));
        // A Windows checkout may have turned LF into CRLF; `.gitattributes`
        // keeps this folder LF, but compare content either way.
        assert_eq!(
            golden.replace("\r\n", "\n"),
            text,
            "{}: the writer's output changed; if intended, bless with LRG_BLESS=1",
            t.golden
        );
    }
}
