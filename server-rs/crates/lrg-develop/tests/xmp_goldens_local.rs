//! Local-only: the XMP reader over real files that can never enter the
//! repository or CI: Lightroom Classic's own preset and profile bundle, the
//! user's Camera Raw presets, and a sidecar corpus.
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
//! recognise and on a number outside its key's registry range: the files are
//! Adobe's own output, so either means the registry is wrong, not the file.
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
mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lrg_develop::xmp::{parse, SkippedCounts, XmpKind};
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
}

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
        eprintln!(
            "  values outside their registry range: {:?}",
            self.ranges.outside
        );
        for (what, n) in &self.warnings_per_path {
            eprintln!("  {n:>6}  {what}");
        }
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
            self.ranges.outside.is_empty(),
            "{title}: values outside the registry range (widen the range in registry/table.rs): {:#?}",
            self.ranges.outside
        );
    }

    /// Fails when a registry key was kept whole (an `rdf:Bag`, a second
    /// language, qualifiers): Lightroom does not write those forms, so a
    /// count means a key silently lost its typing.
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
