//! Local-only: the XMP reader and writer over real files that can never
//! enter the repository or CI: Lightroom Classic's own preset and profile
//! bundle, the user's Camera Raw presets, and a sidecar corpus.
//!
//! ```text
//! LRG_LRC_PRESETS_DIR="/Applications/Adobe Lightroom Classic/Adobe Lightroom Classic.app/Contents/Resources/Settings" \
//! LRG_ACR_PRESETS_DIR="$HOME/Library/Application Support/Adobe/CameraRaw" \
//! LRG_XMP_CORPUS_DIR=/path/to/photos LRG_XMP_CORPUS_SAMPLE=3000 \
//!     cargo test -p lrg-develop --test xmp_goldens_local -- --nocapture
//! ```
//!
//! | Variable | Family | Files | Must hold |
//! |---|---|---|---|
//! | `LRG_LRC_PRESETS_DIR` | `lrc-presets` | every `*.xmp` below it | 0 `ParseError`, 0 `UnknownKey`, no warning outside [`ADOBE_ALLOWED`], 0 keys kept whole |
//! | `LRG_ACR_PRESETS_DIR` | `acr-presets` | the presets and profiles among the `*.xmp` below it (Camera Raw's user folder: its presets and profiles) | as the bundle; keys kept whole are only printed |
//! | `LRG_XMP_CORPUS_DIR` | `xmp-corpus` | sidecars with `crs:ProcessVersion` | 0 `ParseError`, no warning at all, 0 keys kept whole, every file a `Sidecar` |
//!
//! Camera Raw's folder also holds its own state in `crs:` form
//! (`Defaults/Preferences.xmp`, `Defaults/Previous.xmp`, `GPU/...`), which
//! is not a develop preset and uses keys no preset or sidecar carries; that
//! sweep therefore parses every file but checks only the presets and
//! profiles, and counts the rest per document kind. The bundle must consist
//! of presets and profiles only.
//!
//! Every sweep also fails on a mask combination the model does not
//! recognise, on a mask component against a relation the builders rely on
//! (`Flipped` other than `!MaskInverted`, a range's `Invert` other than
//! `MaskInverted`, a range `Version` other than 3) and on a number outside
//! its key's registry range: the files are Adobe's own output, so any of
//! these means the registry or the builders are wrong, not the file.
//!
//! Every checked document is also written back ([`RoundTrip`],
//! [`PresetOut`]): losslessly (round trip 1 of the plan: the same model,
//! header, kind and opaque content after reading back, idempotent bytes,
//! and every `crs:` value spelled as the source spelled it apart from
//! [`ADOBE_SPELLING`]/[`SIDECAR_SPELLING`]; differences counted per key) and
//! as a develop preset, mixed and single-kind (no write error, a preset
//! without warnings, without keys outside the preset policy or anything kept
//! whole, holding exactly the policy-filtered settings; what the policy
//! holds back is printed per mode, key and reason). A profile is not written
//! as a preset. [`lightroom_adaptive_presets_rebuild_with_the_builders`] rebuilds
//! the bundle's adaptive presets with the mask builders and compares them
//! field by field.
//!
//! The corpus walk skips `* [conflicted].xmp` (sync-conflict copies),
//! darktable's `<name>.<ext>.xmp` sidecars without any `crs:` property, and
//! every other file without `crs:ProcessVersion` (metadata-only sidecars),
//! and counts each group. `LRG_XMP_CORPUS_SAMPLE=N` parses a random sample
//! of N eligible files instead of all of them; the seed is fixed, so the
//! same corpus gives the same sample.
//!
//! An unset variable skips through the shared golden gate; the families are
//! local-only, so `LRG_REQUIRE_GOLDENS=all` does not include them and only
//! naming one (`LRG_REQUIRE_GOLDENS=lrc-presets`) turns its skip into a
//! failure. A set variable asks for the run: a path that is not a directory,
//! or a directory without a single eligible file, fails.
//!
//! Privacy: the corpus is private and the bundle is Adobe's. Output names
//! warning kinds, key names and counts, never a value; a corpus or
//! Camera Raw file is named by its ordinal only, a bundle file by its path
//! below the bundle root.

#[path = "../../lrg-ml/tests/common/mod.rs"]
mod common;
#[path = "support/flat.rs"]
mod flat;
#[path = "support/preset.rs"]
mod preset;
mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lrg_develop::build::{BuildError, CorrectionBuilder, IdSlot, SyncNamespace};
use lrg_develop::model::{
    Combine, Correction, DevelopSettings, LuminanceRange, MaskTool, Struct, Target, Value,
};
use lrg_develop::registry::{self, to_ui, Level, StructKind};
use lrg_develop::xmp::read::CRS_NS;
use lrg_develop::xmp::{
    parse, write, PresetHeader, PresetSpec, SkippedCounts, WriteError, WriteMode, XmpDocument,
    XmpKind,
};
use lrg_develop::{ParseError, WarningKind};
use support::{generic_path, MaskCounts, RangeCheck};

const PRESETS_VAR: &str = "LRG_LRC_PRESETS_DIR";
const ACR_VAR: &str = "LRG_ACR_PRESETS_DIR";
const CORPUS_VAR: &str = "LRG_XMP_CORPUS_DIR";
const SAMPLE_VAR: &str = "LRG_XMP_CORPUS_SAMPLE";
const SAMPLE_SEED: u64 = 42;

/// Warnings Adobe's own preset and profile files legitimately produce, as
/// `(kind, key)`.
///
/// - `DuplicateKey` on the header's `Group`: a few of the bundled camera
///   profiles (Google, Samsung and Xiaomi raw profiles) carry `crs:Group`
///   twice in the same `rdf:Description`, with different values. That is
///   invalid XMP in Adobe's file, not a registry gap: the reader keeps the
///   first and the second verbatim, and says so.
const ADOBE_ALLOWED: &[(&str, &str)] = &[("DuplicateKey", "Group")];

/// The directory in `var`, or `None` when the variable is unset (after the
/// gate has had its say).
fn dir_from_env(var: &str, family: &str) -> Option<PathBuf> {
    match std::env::var_os(var) {
        None => {
            if !common::assets_ready(family, false, &format!("{var} is not set")) {
                return None;
            }
            unreachable!("assets_ready panics when the family is required")
        }
        Some(p) => {
            let p = PathBuf::from(p);
            assert!(p.is_dir(), "{var}={} is not a directory", p.display());
            Some(p)
        }
    }
}

/// Every `*.xmp` (any case) below `dir`, sorted; symbolic links are not
/// followed.
fn xmp_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries =
            std::fs::read_dir(&d).unwrap_or_else(|e| panic!("cannot list a directory: {e}"));
        for entry in entries {
            let entry = entry.unwrap_or_else(|e| panic!("cannot list a directory: {e}"));
            let ty = entry
                .file_type()
                .unwrap_or_else(|e| panic!("cannot stat an entry: {e}"));
            let path = entry.path();
            if ty.is_dir() {
                stack.push(path);
            } else if ty.is_file()
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn error_name(e: &ParseError) -> &'static str {
    match e {
        ParseError::Json(_) => "Json",
        ParseError::TooLarge { .. } => "TooLarge",
        ParseError::NotATable { .. } => "NotATable",
        ParseError::Encoding(_) => "Encoding",
        ParseError::Xml(_) => "Xml",
        ParseError::NoRdf => "NoRdf",
        ParseError::TooDeep { .. } => "TooDeep",
        ParseError::Limit { .. } => "Limit",
    }
}

/// What one sweep saw, as counts and key names.
#[derive(Default)]
struct Sweep {
    /// Check only presets and profiles; count other documents in
    /// `not_presets` instead.
    presets_only: bool,
    not_presets: BTreeMap<String, usize>,
    files: usize,
    /// `label: error kind` per failing file.
    errors: Vec<String>,
    kinds: BTreeMap<String, usize>,
    warnings_per_kind: BTreeMap<&'static str, usize>,
    /// Per `(kind, key)`: for the allow-list.
    warnings_per_key: BTreeMap<(&'static str, String), usize>,
    /// Per `kind path` with indices removed: for the report.
    warnings_per_path: BTreeMap<String, usize>,
    process_versions: BTreeMap<String, usize>,
    skipped: SkippedCounts,
    masks: MaskCounts,
    ranges: RangeCheck,
    round_trip: RoundTrip,
    preset: PresetOut,
}

/// Known differences between the source's spelling and the writer's, as
/// `(key, reason)`, for Lightroom's bundle and Camera Raw's folder. Each is
/// a spelling Adobe uses rarely for the key; the writer keeps the registry
/// row's, which is what Lightroom writes into sidecars.
const ADOBE_SPELLING: &[(&str, &str)] = &[
    (
        "Amount",
        "Adobe's presets write Look.Amount as 1.000000, sidecars and the writer as 1",
    ),
    (
        "Clarity",
        "legacy PV2010, COMPUTED: written with a + the registry row does not have",
    ),
    (
        "Exposure",
        "legacy PV2010, COMPUTED: Adobe writes two decimals, the row is Trim6",
    ),
    (
        "PerspectiveX",
        "Classic/General/Zeroed.xmp writes 0 where the row is Fixed(2)",
    ),
    (
        "PerspectiveY",
        "Classic/General/Zeroed.xmp writes 0 where the row is Fixed(2)",
    ),
    (
        "LocalTemperature",
        "one Adobe Sky preset writes two decimals (0.10), the row is Trim6",
    ),
    (
        "LocalShadows2012",
        "a Camera Raw user preset writes two decimals, the row is Trim6",
    ),
];

/// Known differences between Lightroom's sidecars and the writer's spelling
/// (all of them over the maintainer's 10,814 sidecars): each a key the
/// writer never puts into a preset.
const SIDECAR_SPELLING: &[(&str, &str)] = &[
    (
        "Clarity",
        "legacy PV2010, COMPUTED: written with a + the registry row does not have",
    ),
    (
        "CropAngle",
        "two sidecars write crop values untrimmed at %.6f, the row is Trim6 (PHOTO)",
    ),
    (
        "CropBottom",
        "two sidecars write crop values untrimmed at %.6f, the row is Trim6 (PHOTO)",
    ),
    (
        "CropTop",
        "two sidecars write crop values untrimmed at %.6f, the row is Trim6 (PHOTO)",
    ),
];

impl Sweep {
    fn file(&mut self, label: impl FnOnce() -> String, bytes: &[u8]) {
        let doc = match parse(bytes) {
            Ok(d) => d,
            Err(e) => {
                self.files += 1;
                // The error's own text can quote the file; only its kind is
                // printed.
                self.errors.push(format!("{}: {}", label(), error_name(&e)));
                return;
            }
        };
        let kind = format!("{:?}", doc.kind);
        if self.presets_only && !matches!(doc.kind, XmpKind::Preset | XmpKind::Profile) {
            *self.not_presets.entry(kind).or_default() += 1;
            return;
        }
        self.files += 1;
        *self.kinds.entry(kind).or_default() += 1;
        for w in &doc.warnings {
            let kind = w.kind.name();
            *self.warnings_per_kind.entry(kind).or_default() += 1;
            *self
                .warnings_per_key
                .entry((kind, w.key.clone()))
                .or_default() += 1;
            *self
                .warnings_per_path
                .entry(format!("{kind} {}", generic_path(&w.path)))
                .or_default() += 1;
        }
        *self
            .process_versions
            .entry(
                doc.develop
                    .process_version()
                    .map_or_else(|| "none".into(), |pv| pv.to_string()),
            )
            .or_default() += 1;
        let s = &doc.skipped_subtrees;
        self.skipped.crss += s.crss;
        self.skipped.preset_parameters += s.preset_parameters;
        self.skipped.look_parameters += s.look_parameters;
        self.skipped.other_namespaces += s.other_namespaces;
        self.skipped.kept_whole += s.kept_whole;
        self.masks.add(&doc.develop);
        self.ranges.settings(&doc.develop);
        self.round_trip.file(&doc, bytes);
        if doc.kind != XmpKind::Profile {
            self.preset.file(&doc);
        }
    }

    fn report(&self, title: &str) {
        eprintln!(
            "{title}: {} files, {} parse errors",
            self.files,
            self.errors.len()
        );
        eprintln!("  document kinds: {:?}", self.kinds);
        if self.presets_only {
            eprintln!("  not presets, not checked: {:?}", self.not_presets);
        }
        eprintln!("  warnings per kind: {:?}", self.warnings_per_kind);
        eprintln!("  process versions: {:?}", self.process_versions);
        eprintln!("  skipped subtrees: {:?}", self.skipped);
        eprintln!(
            "  corrections: {}; mask tools: {:?}",
            self.masks.corrections, self.masks.tools
        );
        eprintln!("  combinations: {:?}", self.masks.combines);
        eprintln!("  mask forms: {:?}", self.masks.forms);
        eprintln!(
            "  values outside their registry range: {:?}",
            self.ranges.outside
        );
        for (what, n) in &self.warnings_per_path {
            eprintln!("  {n:>6}  {what}");
        }
        self.round_trip.report();
        self.preset.report();
    }

    /// Fails on any parse error, any `UnknownKey`, any other warning not in
    /// `allowed`, an unrecognised mask combination or a number outside its
    /// registry range. Whether keys kept whole are allowed is up to the
    /// caller ([`Self::assert_all_typed`]).
    fn assert_clean(&self, title: &str, allowed: &[(&str, &str)]) {
        assert!(self.files > 0, "{title}: no XMP file to check found");
        assert!(
            self.errors.is_empty(),
            "{title}: parse errors: {:#?}",
            self.errors
        );
        let unknown: Vec<&String> = self
            .warnings_per_key
            .keys()
            .filter(|(k, _)| *k == WarningKind::UnknownKey.name())
            .map(|(_, key)| key)
            .collect();
        assert!(
            unknown.is_empty(),
            "{title}: unknown keys (add a row to registry/table.rs): {unknown:?}"
        );
        let unexpected: Vec<String> = self
            .warnings_per_key
            .iter()
            .filter(|((kind, key), _)| !allowed.iter().any(|(k, n)| k == kind && n == key))
            .map(|((kind, key), n)| format!("{n} x {kind} {key}"))
            .collect();
        assert!(
            unexpected.is_empty(),
            "{title}: warnings (extend the registry or fix the reader): {unexpected:#?}"
        );
        assert_eq!(
            self.masks.combines.get("unrecognised"),
            None,
            "{title}: unrecognised mask combinations"
        );
        assert!(
            self.masks.broken_forms().is_empty(),
            "{title}: mask components against a relation the builders rely on: {:?}",
            self.masks.broken_forms()
        );
        assert!(
            self.ranges.outside.is_empty(),
            "{title}: values outside the registry range (widen the range in registry/table.rs): {:#?}",
            self.ranges.outside
        );
    }

    /// Fails when a registry key was kept whole (an `rdf:Bag`, a second
    /// language, qualifiers): Lightroom does not write those forms, so a
    /// count means a key silently lost its typing.
    fn assert_writes_back(&self, title: &str, spelling: &[(&str, &str)]) {
        self.round_trip.assert_clean(title, spelling);
        self.preset.assert_clean(title);
    }

    fn assert_all_typed(&self, title: &str) {
        assert_eq!(
            self.skipped.kept_whole, 0,
            "{title}: registry keys kept whole instead of typed (see SkippedCounts::kept_whole)"
        );
    }
}

/// Parses every `*.xmp` below `dir`. `by_path` names a failing file by its
/// path below `dir` (for Adobe's bundle), otherwise by its ordinal.
fn sweep_presets(dir: &Path, by_path: bool, presets_only: bool) -> Sweep {
    let mut sweep = Sweep {
        presets_only,
        ..Sweep::default()
    };
    for (i, path) in xmp_files(dir).iter().enumerate() {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("file #{i}: {e}"));
        sweep.file(
            || {
                if by_path {
                    path.strip_prefix(dir).unwrap_or(path).display().to_string()
                } else {
                    format!("file #{i}")
                }
            },
            &bytes,
        );
    }
    sweep
}

#[test]
fn lightroom_bundle_presets_and_profiles_parse_cleanly() {
    let Some(dir) = dir_from_env(PRESETS_VAR, "lrc-presets") else {
        return;
    };
    let sweep = sweep_presets(&dir, true, false);
    sweep.report("Lightroom bundle");
    sweep.assert_clean("Lightroom bundle", ADOBE_ALLOWED);
    sweep.assert_all_typed("Lightroom bundle");
    sweep.assert_writes_back("Lightroom bundle", ADOBE_SPELLING);
    let other: Vec<&String> = sweep
        .kinds
        .keys()
        .filter(|k| !matches!(k.as_str(), "Preset" | "Profile"))
        .collect();
    assert!(
        other.is_empty(),
        "Lightroom bundle: documents that are neither a preset nor a profile: {other:?}"
    );
}

#[test]
fn camera_raw_presets_parse_cleanly() {
    let Some(dir) = dir_from_env(ACR_VAR, "acr-presets") else {
        return;
    };
    let sweep = sweep_presets(&dir, false, true);
    sweep.report("Camera Raw presets");
    sweep.assert_clean("Camera Raw presets", ADOBE_ALLOWED);
    sweep.assert_writes_back("Camera Raw presets", ADOBE_SPELLING);
    // Printed in the report, not asserted: presets the user imported or
    // wrote may carry a localized name, which is valid and kept whole.
}

/// Why a corpus file was not parsed.
#[derive(Default, Debug)]
struct CorpusSkips {
    conflicted: usize,
    darktable: usize,
    no_process_version: usize,
}

/// The sidecars below `dir` that hold develop settings, sorted.
fn corpus_files(dir: &Path) -> (usize, Vec<PathBuf>, CorpusSkips) {
    let all = xmp_files(dir);
    let mut skips = CorpusSkips::default();
    let mut eligible = Vec::new();
    for (i, path) in all.iter().enumerate() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.ends_with(" [conflicted].xmp") {
            skips.conflicted += 1;
            continue;
        }
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("file #{i}: {e}"));
        let text = String::from_utf8_lossy(&bytes);
        if text.contains("crs:ProcessVersion") {
            eligible.push(path.clone());
            continue;
        }
        // `photo.ext.xmp`: darktable's sidecar naming (Lightroom writes
        // `photo.xmp`).
        let double_extension = Path::new(path.file_stem().unwrap_or_default())
            .extension()
            .is_some();
        if double_extension && !text.contains("crs:") {
            skips.darktable += 1;
        } else {
            skips.no_process_version += 1;
        }
    }
    (all.len(), eligible, skips)
}

/// SplitMix64: a fixed-seed generator for the sample, so the test needs no
/// random-number crate and the same corpus always gives the same sample.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n > 0`); the modulo bias is irrelevant here.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// `n` of `files` chosen with [`SAMPLE_SEED`] (a partial Fisher-Yates
/// shuffle of the sorted list), in path order; all of them when `n` is not
/// smaller.
fn sample(mut files: Vec<PathBuf>, n: usize) -> Vec<PathBuf> {
    if n >= files.len() {
        return files;
    }
    let mut rng = SplitMix64(SAMPLE_SEED);
    for i in 0..n {
        let j = i + rng.below(files.len() - i);
        files.swap(i, j);
    }
    files.truncate(n);
    files.sort();
    files
}

#[test]
fn sample_is_deterministic_and_a_subset() {
    let files: Vec<PathBuf> = (0..100).map(|i| PathBuf::from(format!("{i:03}"))).collect();
    let a = sample(files.clone(), 10);
    assert_eq!(a, sample(files.clone(), 10));
    assert_eq!(a.len(), 10);
    assert!(a.windows(2).all(|w| w[0] < w[1]), "sorted, no repeats");
    assert!(a.iter().all(|p| files.contains(p)));
    assert_eq!(sample(files.clone(), 1000), files);
}

#[test]
fn sidecar_corpus_parses_cleanly() {
    let Some(dir) = dir_from_env(CORPUS_VAR, "xmp-corpus") else {
        return;
    };
    let limit = std::env::var(SAMPLE_VAR).ok().map(|s| {
        let n: usize = s
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{SAMPLE_VAR}={s} is not a number"));
        assert!(n > 0, "{SAMPLE_VAR} must be at least 1");
        n
    });
    let (seen, eligible, skips) = corpus_files(&dir);
    let eligible_count = eligible.len();
    let files = match limit {
        Some(n) => sample(eligible, n),
        None => eligible,
    };
    eprintln!(
        "sidecar corpus: {seen} XMP files, {eligible_count} with crs:ProcessVersion, \
         {} parsed{}; skipped: {skips:?}",
        files.len(),
        limit.map_or_else(String::new, |n| format!(
            " ({SAMPLE_VAR}={n}, seed {SAMPLE_SEED})"
        )),
    );
    let mut sweep = Sweep::default();
    // Corpus files are private: named by their ordinal in the parsed list.
    for (i, path) in files.iter().enumerate() {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("file #{i}: {e}"));
        sweep.file(|| format!("file #{i}"), &bytes);
    }
    sweep.report("sidecar corpus");
    // Lightroom's own output: any warning means the registry or the reader
    // is wrong. An allow-list, empty today, so a new kind fails here too.
    sweep.assert_clean("sidecar corpus", &[]);
    sweep.assert_all_typed("sidecar corpus");
    sweep.assert_writes_back("sidecar corpus", SIDECAR_SPELLING);
    // Eligible files carry crs:ProcessVersion, so each must read as a
    // sidecar (this also proves ProcessVersion was read).
    let other: Vec<_> = sweep
        .kinds
        .iter()
        .filter(|(k, _)| k.as_str() != "Sidecar")
        .collect();
    assert!(
        other.is_empty(),
        "sidecar corpus: eligible files that did not parse as a sidecar: {other:?}"
    );
}

// --- writing back ------------------------------------------------------

fn write_error_name(e: &WriteError) -> &'static str {
    match e {
        WriteError::MissingHeaderField(_) => "MissingHeaderField",
        WriteError::UnsupportedPresetType(_) => "UnsupportedPresetType",
        WriteError::WrongValue { .. } => "WrongValue",
        WriteError::NotHex32 { .. } => "NotHex32",
        WriteError::NotCompound { .. } => "NotCompound",
        WriteError::NotXmp { .. } => "NotXmp",
        WriteError::InvalidChar { .. } => "InvalidChar",
    }
}

/// A path with pattern-family names folded (`Table_<md5>` → `Table_*`,
/// `UprightTransform_3` → `UprightTransform_*`): the md5 names a private
/// file's lookup table, and one line per family is enough.
fn family(path: &str) -> String {
    path.split('.')
        .map(|seg| match seg.rsplit_once('_') {
            Some((head, tail))
                if !tail.is_empty()
                    && (tail.bytes().all(|b| b.is_ascii_digit())
                        || (tail.len() == 32 && tail.bytes().all(|b| b.is_ascii_hexdigit()))) =>
            {
                format!("{head}_*")
            }
            _ => seg.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Adds one to `key`'s count.
fn bump(map: &mut BTreeMap<String, usize>, key: impl Into<String>) {
    *map.entry(key.into()).or_default() += 1;
}

/// The keys two documents disagree on (model and header), indices removed.
fn differing_keys(a: &XmpDocument, b: &XmpDocument) -> Vec<String> {
    let empty = PresetHeader::default();
    let header = |d: &XmpDocument| flat::header(d.header.as_ref().unwrap_or(&empty));
    let mut keys = flat::diff(&flat::settings(&a.develop), &flat::settings(&b.develop));
    keys.extend(flat::diff(&header(a), &header(b)));
    let mut keys: Vec<String> = keys.iter().map(|k| generic_path(k)).collect();
    keys.dedup();
    keys
}

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// `(key, text)` → how often a document spells it.
type Spellings = BTreeMap<(String, String), usize>;

/// Every `crs:` value text of a document's develop settings: attributes and
/// text-only property elements under their name, `rdf:li` texts under
/// `<name>[]`, at any depth. Subtrees in other namespaces (`crss:`
/// snapshots, `dc:`, ...) are left out, as the model leaves them out.
/// `None` when the text is not UTF-8 XML roxmltree reads.
fn spellings(bytes: &[u8]) -> Option<Spellings> {
    let text = std::str::from_utf8(bytes).ok()?;
    let doc = roxmltree::Document::parse(text.trim_start_matches('\u{feff}')).ok()?;
    let mut out = Spellings::new();
    for d in doc.descendants().filter(|n| {
        n.has_tag_name((RDF_NS, "Description"))
            && n.parent().is_some_and(|p| p.has_tag_name((RDF_NS, "RDF")))
    }) {
        struct_spellings(d, &mut out);
    }
    Some(out)
}

fn spell(out: &mut Spellings, key: String, text: &str) {
    *out.entry((key, text.to_owned())).or_default() += 1;
}

/// A description, a structure's property element or an `rdf:li`: its
/// `crs:` attributes, its `crs:` properties, nested descriptions.
fn struct_spellings(node: roxmltree::Node, out: &mut Spellings) {
    for a in node.attributes() {
        if a.namespace() == Some(CRS_NS) {
            spell(out, a.name().to_owned(), a.value());
        }
    }
    for c in node.children().filter(|c| c.is_element()) {
        if c.tag_name().namespace() == Some(CRS_NS) {
            property_spellings(c, out);
        } else if c.has_tag_name((RDF_NS, "Description")) {
            struct_spellings(c, out);
        }
    }
}

fn property_spellings(p: roxmltree::Node, out: &mut Spellings) {
    let name = p.tag_name().name();
    let crs_attributes = p.attributes().any(|a| a.namespace() == Some(CRS_NS));
    let has_elements = p.children().any(|c| c.is_element());
    if !has_elements && !crs_attributes {
        return spell(out, name.to_owned(), p.text().unwrap_or(""));
    }
    struct_spellings(p, out);
    for array in p.children().filter(|c| {
        ["Seq", "Alt", "Bag"]
            .iter()
            .any(|k| c.has_tag_name((RDF_NS, *k)))
    }) {
        for li in array.children().filter(|c| c.is_element()) {
            let complex = li.children().any(|c| c.is_element())
                || li.attributes().any(|a| a.namespace() == Some(CRS_NS));
            if complex {
                struct_spellings(li, out);
            } else {
                spell(out, format!("{name}[]"), li.text().unwrap_or(""));
            }
        }
    }
}

/// Round trip 1 (plan §7.3): XMP → model → XMP (lossless test mode) →
/// model gives the same model, header, kind and opaque content, and
/// writing the read-back model again gives the same bytes. Also the byte
/// rules against the source (plan §7.4): every `crs:` value the writer
/// emits is spelled as the source spelled it, compared as a multiset of
/// `(key, text)` per file ([`spellings`]); a known variance is allowed per
/// sweep by key name.
#[derive(Default)]
struct RoundTrip {
    files: usize,
    write_errors: BTreeMap<String, usize>,
    parse_errors: usize,
    /// Per key (indices removed): files whose key differs after the trip.
    mismatches: BTreeMap<String, usize>,
    /// Files whose model differs although no flattened key does.
    unequal_models: usize,
    kinds: usize,
    not_idempotent: usize,
    /// Files roxmltree could not read for the spelling check.
    spelling_unread: usize,
    /// Per key name: values spelled differently from the source.
    spelling: BTreeMap<String, usize>,
}

impl RoundTrip {
    fn file(&mut self, doc: &XmpDocument, source: &[u8]) {
        self.files += 1;
        let mode = WriteMode::TestRoundTrip(doc.header.clone());
        let written = match write(&doc.develop, &mode) {
            Ok(w) => w,
            Err(e) => return bump(&mut self.write_errors, write_error_name(&e)),
        };
        let Ok(back) = parse(written.xmp.as_bytes()) else {
            self.parse_errors += 1;
            return;
        };
        let keys = differing_keys(doc, &back);
        for k in &keys {
            bump(&mut self.mismatches, k.clone());
        }
        if keys.is_empty() && (back.develop != doc.develop || back.header != doc.header) {
            self.unequal_models += 1;
        }
        if back.kind != doc.kind {
            self.kinds += 1;
        }
        let again = write(
            &back.develop,
            &WriteMode::TestRoundTrip(back.header.clone()),
        );
        if again.map(|w| w.xmp).ok().as_deref() != Some(written.xmp.as_str()) {
            self.not_idempotent += 1;
        }
        match (spellings(source), spellings(written.xmp.as_bytes())) {
            (Some(theirs), Some(ours)) => {
                for ((key, text), n) in &theirs {
                    let m = ours.get(&(key.clone(), text.clone())).copied().unwrap_or(0);
                    if m != *n {
                        *self.spelling.entry(key.clone()).or_default() += n.abs_diff(m);
                    }
                }
                for ((key, text), n) in &ours {
                    if !theirs.contains_key(&(key.clone(), text.clone())) {
                        *self.spelling.entry(key.clone()).or_default() += n;
                    }
                }
            }
            _ => self.spelling_unread += 1,
        }
    }

    fn report(&self) {
        eprintln!(
            "  round trip: {} files, write errors {:?}, parse errors {}, keys differing {:?}, \
             unequal models {}, kind changed {}, not idempotent {}",
            self.files,
            self.write_errors,
            self.parse_errors,
            self.mismatches,
            self.unequal_models,
            self.kinds,
            self.not_idempotent
        );
        eprintln!(
            "  spelled unlike the source (key: values without a match, either side): {:?}; \
             files not read for it: {}",
            self.spelling, self.spelling_unread
        );
    }

    fn assert_clean(&self, title: &str, spelling: &[(&str, &str)]) {
        assert!(
            self.write_errors.is_empty()
                && self.parse_errors == 0
                && self.mismatches.is_empty()
                && self.unequal_models == 0
                && self.kinds == 0
                && self.not_idempotent == 0,
            "{title}: the round trip is not lossless (see the report above)"
        );
        let unexpected: Vec<&String> = self
            .spelling
            .keys()
            .filter(|k| !spelling.iter().any(|(allowed, _)| allowed == k))
            .collect();
        assert!(
            unexpected.is_empty() && self.spelling_unread == 0,
            "{title}: values spelled unlike the source (check the registry row's \
             NumFmt/plus_sign/BoolStyle): {unexpected:?}"
        );
    }
}

/// Every develop preset and sidecar written as a preset (production mode),
/// mixed and single-kind: no write error, reads back as a preset without a
/// warning, carries no key outside the preset policy and nothing kept whole
/// ([`preset::violations`]), and holds exactly the policy-filtered settings
/// ([`preset::differences`]). What the policy filter holds back is counted
/// per key and reason, for the report.
#[derive(Default)]
struct PresetOut {
    files: usize,
    write_errors: BTreeMap<String, usize>,
    parse_errors: usize,
    not_a_preset: usize,
    warnings: BTreeMap<String, usize>,
    /// Keys a preset must not carry, per class and key.
    violations: BTreeMap<String, usize>,
    /// Filtered values the preset does not hold as written, per kind and
    /// key (indices removed).
    differences: BTreeMap<String, usize>,
    /// Values compared.
    compared: usize,
    skipped: BTreeMap<String, usize>,
}

fn preset_spec(label: &str, mixed_file_kinds: bool) -> WriteMode {
    let uuid = SyncNamespace::new("xmp_goldens_local").id(label, IdSlot::Correction);
    WriteMode::Preset(PresetSpec {
        mixed_file_kinds,
        ..PresetSpec::new(PresetHeader::lrgenius(uuid, "Sweep"))
    })
}

impl PresetOut {
    fn file(&mut self, doc: &XmpDocument) {
        self.files += 1;
        for mixed in [true, false] {
            let mode = if mixed { "mixed" } else { "single-kind" };
            let written = match write(&doc.develop, &preset_spec("sweep", mixed)) {
                Ok(w) => w,
                Err(e) => {
                    bump(&mut self.write_errors, write_error_name(&e));
                    continue;
                }
            };
            for s in &written.skipped {
                bump(
                    &mut self.skipped,
                    format!("{mode} {} {:?}", family(&generic_path(&s.path)), s.reason),
                );
            }
            let Ok(back) = parse(written.xmp.as_bytes()) else {
                self.parse_errors += 1;
                continue;
            };
            if back.kind != XmpKind::Preset {
                self.not_a_preset += 1;
            }
            for w in &back.warnings {
                bump(&mut self.warnings, format!("{} {}", w.kind.name(), w.key));
            }
            for v in preset::violations(&back) {
                bump(&mut self.violations, format!("{mode} {v}"));
            }
            let (filtered, _) = doc.develop.filtered(&Target::Preset {
                mixed_file_kinds: mixed,
            });
            self.compared += flat::settings(&filtered).len();
            for d in preset::differences(&filtered, &back.develop) {
                bump(
                    &mut self.differences,
                    format!("{mode} {}", family(&generic_path(&d))),
                );
            }
        }
    }

    fn report(&self) {
        eprintln!(
            "  as presets: {} files (each mixed and single-kind), write errors {:?}, \
             parse errors {}, not a preset {}, warnings {:?}, keys a preset must not carry {:?}",
            self.files,
            self.write_errors,
            self.parse_errors,
            self.not_a_preset,
            self.warnings,
            self.violations
        );
        eprintln!(
            "  preset against the filtered settings: {} values compared, differing {:?}",
            self.compared, self.differences
        );
        eprintln!("  held back from the presets (mode, key, reason: count):");
        for (what, n) in &self.skipped {
            eprintln!("  {n:>6}  {what}");
        }
    }

    fn assert_clean(&self, title: &str) {
        assert!(
            self.write_errors.is_empty()
                && self.parse_errors == 0
                && self.not_a_preset == 0
                && self.warnings.is_empty()
                && self.violations.is_empty()
                && self.differences.is_empty(),
            "{title}: writing as a preset is not clean (see the report above)"
        );
    }
}

// --- builder parity ----------------------------------------------------

/// Why a correction of Adobe's could not be rebuilt, or a key of it was
/// left out.
fn build_error_name(e: &BuildError) -> String {
    match e {
        BuildError::NotWritable { key, .. } => format!("NotWritable {key}"),
        BuildError::Ui(u) => format!("Ui {u:?}")
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" "),
        other => format!("{other:?}")
            .split(['(', ' ', '{'])
            .next()
            .unwrap_or("?")
            .to_owned(),
    }
}

/// The builder calls for one of Adobe's corrections. Keys the builder
/// refuses (not writable: unverified scale or not learnable) are left out
/// and named in `left_out`; anything else that fails is the error.
fn rebuild(
    c: &Correction,
    role: &str,
    left_out: &mut BTreeMap<String, usize>,
) -> Result<Correction, String> {
    let ns = SyncNamespace::new("builder parity");
    let mut b = CorrectionBuilder::new(role, &ns);
    let amount_spec = registry::lookup(Level::Correction, "CorrectionAmount")
        .unwrap()
        .spec();
    if let Some(a) = c.amount {
        let ui = to_ui(amount_spec, a).map_err(|e| format!("amount {e:?}"))?;
        b = b.amount_ui(ui.get()).map_err(|e| build_error_name(&e))?;
    }
    for (id, v) in &c.local {
        let spec = id.spec();
        let next = match v {
            Value::Curve(points) => {
                let pts: Option<Vec<(u8, u8)>> = points
                    .iter()
                    .map(|(x, y)| {
                        let byte = |f: lrg_develop::model::Finite| {
                            let g = f.get();
                            (g.fract() == 0.0 && (0.0..=255.0).contains(&g)).then_some(g as u8)
                        };
                        Some((byte(*x)?, byte(*y)?))
                    })
                    .collect();
                let pts = pts.ok_or_else(|| format!("curve {} not in bytes", spec.name))?;
                b.clone().local_curve(spec.name, &pts)
            }
            v => match v.as_finite().map(|x| to_ui(spec, x)) {
                Some(Ok(ui)) => b.clone().local_ui(spec.name, ui.get()),
                Some(Err(_)) | None => Err(BuildError::NotWritable {
                    key: spec.name.to_owned(),
                    reason: "no UI value",
                }),
            },
        };
        match next {
            Ok(nb) => b = nb,
            Err(e @ BuildError::NotWritable { .. }) => bump(left_out, build_error_name(&e)),
            Err(e) => return Err(format!("{}: {}", spec.name, build_error_name(&e))),
        }
    }
    for m in &c.masks {
        let tool = match &m.tool {
            MaskTool::LuminanceRange(lr) => MaskTool::LuminanceRange(LuminanceRange {
                range: lr.range,
                invert: lr.invert,
                // Version and SampleType come from the writer.
                rest: Struct::new(StructKind::CorrectionRangeMask),
            }),
            MaskTool::Opaque { what } => {
                return Err(format!("opaque tool {}", what.as_deref().unwrap_or("?")))
            }
            t => t.clone(),
        };
        b = match m.combine {
            Combine::Add { inverted: false } => b.add(tool),
            Combine::Add { inverted: true } => b.add_inverted(tool),
            Combine::Subtract => b.subtract(tool),
            Combine::Intersect => b.intersect(tool),
            Combine::Unrecognised => return Err("unrecognised combination".into()),
        };
    }
    b.build().map_err(|e| build_error_name(&e))
}

/// Names and sync ids are ours by design.
fn ours_by_design(path: &str) -> bool {
    [
        "CorrectionName",
        "CorrectionSyncID",
        "MaskName",
        "MaskSyncID",
    ]
    .iter()
    .any(|k| path.ends_with(k))
}

fn correction_paths(s: &DevelopSettings) -> flat::Flat {
    flat::settings(s)
        .into_iter()
        .filter(|(p, _)| p.starts_with("MaskGroupBasedCorrections[") && !ours_by_design(p))
        .collect()
}

/// Plan §7.4: every correction of Adobe's adaptive presets, rebuilt with the
/// builders from the parsed model and written as a preset, reads back equal
/// field by field (names and sync ids apart) to Adobe's settings written by
/// the same writer. Against Adobe's file as it is, the only differences
/// allowed are fields the writer also holds back from Adobe's own
/// correction (digests, legacy PV2010 `Local*` keys, keys whose scale is not
/// verified); adjustments the builder refuses are counted in the report.
/// `CompatibleVersion` of both files is compared for the report only.
#[test]
fn lightroom_adaptive_presets_rebuild_with_the_builders() {
    let Some(dir) = dir_from_env(PRESETS_VAR, "lrc-presets") else {
        return;
    };
    let mut presets = 0;
    let mut corrections = 0;
    let mut not_rebuilt: BTreeMap<String, usize> = BTreeMap::new();
    let mut left_out: BTreeMap<String, usize> = BTreeMap::new();
    let mut vs_file: BTreeMap<String, usize> = BTreeMap::new();
    let mut vs_file_held_back: BTreeMap<String, usize> = BTreeMap::new();
    let mut vs_writer: BTreeMap<String, usize> = BTreeMap::new();
    let mut compatible: BTreeMap<String, usize> = BTreeMap::new();
    let mut write_errors: BTreeMap<String, usize> = BTreeMap::new();
    for (i, path) in xmp_files(&dir).iter().enumerate() {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("file #{i}: {e}"));
        let Ok(doc) = parse(&bytes) else { continue };
        if doc.kind != XmpKind::Preset || doc.develop.corrections.is_empty() {
            continue;
        }
        presets += 1;
        let mut ours = doc.develop.clone();
        for (j, c) in doc.develop.corrections.iter().enumerate() {
            corrections += 1;
            match rebuild(c, &format!("correction {j}"), &mut left_out) {
                Ok(r) => ours.corrections[j] = r,
                Err(why) => bump(&mut not_rebuilt, why),
            }
        }
        let mode = |label: &str| match preset_spec(label, true) {
            WriteMode::Preset(spec) => WriteMode::Preset(PresetSpec {
                process_version: doc.develop.process_version(),
                ..spec
            }),
            other => other,
        };
        let written = match write(&ours, &mode("ours")) {
            Ok(w) => w,
            Err(e) => {
                bump(&mut write_errors, write_error_name(&e));
                continue;
            }
        };
        let back = parse(written.xmp.as_bytes()).expect("our preset parses");
        assert!(back.warnings.is_empty(), "file #{i}: {:?}", back.warnings);
        // Against Adobe's settings written by the same writer (both sides
        // filtered by the same policy): must be equal.
        let theirs = write(&doc.develop, &mode("theirs")).expect("Adobe's preset writes");
        let theirs = parse(theirs.xmp.as_bytes()).expect("parses");
        let theirs = correction_paths(&theirs.develop);
        let ours = correction_paths(&back.develop);
        for k in flat::diff(&theirs, &ours) {
            bump(&mut vs_writer, generic_path(&k));
        }
        // Against Adobe's file as it is: only what the writer holds back
        // from Adobe's own correction too may differ.
        let adobe = correction_paths(&doc.develop);
        let held_back = flat::diff(&adobe, &theirs);
        for k in flat::diff(&adobe, &ours) {
            let key = generic_path(&k);
            if held_back.contains(&k) {
                bump(&mut vs_file_held_back, key);
            } else {
                bump(&mut vs_file, key);
            }
        }
        let cv = |d: &DevelopSettings| d.get_by_name("CompatibleVersion").and_then(Value::as_int);
        let version = |v: Option<i64>| {
            v.map_or_else(
                || "none".to_owned(),
                |v| format!("{}.{}", v >> 24, (v >> 16) & 0xff),
            )
        };
        bump(
            &mut compatible,
            format!(
                "Adobe {}, ours {}",
                version(cv(&doc.develop)),
                version(cv(&back.develop))
            ),
        );
    }
    eprintln!("builder parity: {presets} adaptive presets, {corrections} corrections");
    eprintln!("  not rebuilt: {not_rebuilt:?}");
    eprintln!("  adjustments left out of the rebuild: {left_out:?}");
    eprintln!("  write errors: {write_errors:?}");
    eprintln!("  fields differing from Adobe's settings through our writer: {vs_writer:?}");
    eprintln!(
        "  fields differing from Adobe's file, held back by the writer from Adobe's too: \
         {vs_file_held_back:?}"
    );
    eprintln!("  fields differing from Adobe's file otherwise: {vs_file:?}");
    eprintln!("  CompatibleVersion: {compatible:?}");
    assert!(presets > 0, "no adaptive preset with masks found");
    assert!(
        not_rebuilt.is_empty(),
        "corrections not rebuilt: {not_rebuilt:?}"
    );
    assert!(write_errors.is_empty());
    assert!(
        vs_writer.is_empty(),
        "the builders do not reproduce Adobe's corrections: {vs_writer:?}"
    );
    assert!(
        vs_file.is_empty(),
        "fields differing from Adobe's file that the writer does not hold back: {vs_file:?}"
    );
}
