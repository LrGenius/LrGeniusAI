//! No private or third-party content in `server-rs/testdata/develop/`.
//!
//! Mirrors `extract_fixtures.py --check-only`
//! (`server-rs/scripts/develop_registry/`); the rules are documented in
//! `server-rs/testdata/develop/README.md` ("Hygiene rules"), and a change to
//! one side must be made on the other. In short:
//!
//! - scope: every `*.json`/`*.xmp` below the folder (never the README);
//! - hard text rules everywhere: photo-id prefix, `file:`, user/volume/temp
//!   paths, drive and UNC paths, raw/image file names, the dropped table-key
//!   names;
//! - keys are identifiers, no table keys, `Look.Parameters` only as the stub,
//!   `Look.UUID`/`Name` only from [`ADOBE_LOOKS`] or synthetic, every 32-hex
//!   value and GUID synthetic, no dates, no number ≥ 1e9 outside the version
//!   keys and the synthetic `GrainSeed`;
//! - generated files (listed in `lua/manifest.json`) additionally follow the
//!   per-key value rules; any other `lua/*.json` must be named `hand_*.json`;
//! - `registry_snapshot.json` (the registry's own description) gets the text
//!   rules minus the table-key names, and the 32-hex/GUID rule.
//!
//! Patterns match whole strings (Python: `\Z`, not `$`, which also matches
//! before a trailing newline) and digits are ASCII (`[0-9]`; Python compiles
//! with `re.ASCII`), so both sides accept exactly the same inputs.
//! `dispatch_rules_flag_exactly_the_crafted_files` pins the file-level rules
//! and the inputs the two sides once disagreed on;
//! `extract_fixtures.py --self-test` runs the same cases.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value as J};

/// Profiles whose name and UUID may be published: verified to exist as
/// `crs:UUID` in Lightroom Classic's bundled profiles (LrC 15.5.1). Same
/// constant as `ADOBE_LOOKS` in `extract_fixtures.py`; never read from the
/// manifest.
const ADOBE_LOOKS: &[(&str, &str)] = &[
    ("B952C231111CD8E0ECCF14B86BAA7077", "Adobe Color"),
    ("6F9C877E84273F4E8271E6B91BEB36A1", "Adobe Landscape"),
    ("0CFE8F8AB5F63B2A73CE0B0077D20817", "Adobe Monochrome"),
    ("D6496412E06A83789C499DF9540AA616", "Adobe Portrait"),
    ("EA1DE074F188405965EF399C72C221D9", "Adobe Vivid"),
    ("226E6263ED94405B973B41F5A6E67BE9", "Adaptive Color"),
    ("4B2C86502E5F2637A81F96F8D96AC1F9", "Artistic 04"),
    ("ADB81986EAF84E0999C0112BDE1F93D1", "B&W 03"),
    ("9D1ED025F3DE4C94BC78481AA67B3C89", "B&W 11"),
    ("8FD143CED5B04137ACE4B5A63962293F", "B&W 12"),
    ("7B79093CEC7726D67B352CAD78D1601F", "Modern 07"),
    ("DA1C3775662D6B6A75F8BC2CEEB3724A", "Modern 08"),
    ("97291D549FC232787BDFD3353151BB62", "Vintage 01"),
];

const SYNTHETIC_DATE: &str = "2000-01-01T00:00:00+00:00";
const SYNTHETIC_TEXT: &str = "Synthetic Text";
const SYNTHETIC_PROFILE: &str = "Synthetic Profile";
const SYNTHETIC_GRAIN_SEED: u64 = 4_000_000_001;
const STUB_KEYS: &[&str] = &["ConvertToGrayscale", "ProcessVersion"];
const BIG_NUMBER: f64 = 1e9;
const BIG_NUMBER_KEYS: &[&str] = &[
    "UprightVersion",
    "ModelVersion",
    "MinEditVersion",
    "MinDisplayVersion",
    "CompatibleVersion",
    "AutoWhiteVersion",
];
const REGISTRY_FILES: &[&str] = &["registry_snapshot.json"];
const CAMERA_PROFILES: &[&str] = &[
    "Adobe Standard",
    "Adobe Standard v2",
    "Camera Standard",
    "Camera Standard v2",
    "Camera Neutral",
    "Camera Neutral v2",
    "Camera Landscape",
    "Camera Landscape v2",
    "Camera Faithful",
    "Camera Faithful v2",
    "Camera Portrait",
    "Camera Portrait v2",
    "Camera Fine Detail",
    "Camera Monochrome",
];
const TONE_CURVE_NAMES: &[&str] = &[
    "Linear",
    "Medium Contrast",
    "Strong Contrast",
    "Custom",
    "Matter Kontrast",
];
const FIXED_VALUES: &[(&str, &str)] = &[
    ("LensProfileName", "Adobe (Synthetic Lens)"),
    ("LensProfileFilename", "Synthetic Lens - RAW.lcp"),
    ("pm_clio_model_version", "synthetic-model-version"),
    ("Copyright", "Synthetic copyright notice"),
    ("Cluster", "Synthetic"),
    ("CameraModelRestriction", "Synthetic Camera"),
];
const FIXED_IN_PARENT: &[((&str, &str), &str)] = &[
    (("GenAIInfo", "Name"), "Synthetic Generative Model"),
    (("GenAIInfo", "SoftwareAgent"), "Synthetic Agent"),
];
const LOOK_ALT_FIELDS: &[(&str, &str)] = &[("Group", "Synthetic Group"), ("SortName", "Synthetic")];
const FILTER_NAMES: &[&str] = &[
    "Enhance",
    "Denoise",
    "Raw Details",
    "Super Resolution",
    "Dust Removal",
    "Distracting People Removal",
    "Reflection Removal",
];

fn re(s: &str) -> Regex {
    Regex::new(s).expect("valid pattern")
}

const NUM_TOKEN: &str = r"[-+]?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][-+]?[0-9]+)?";

static GUID_ANY: LazyLock<Regex> = LazyLock::new(|| {
    re(r"[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}")
});
static SYNTH_HEX: LazyLock<Regex> = LazyLock::new(|| re(r"^0{20}[0-9A-Fa-f]{12}$"));
static SYNTH_GUID: LazyLock<Regex> =
    LazyLock::new(|| re(r"^00000000-0000-4000-8000-[0-9A-Fa-f]{12}$"));
static DATE: LazyLock<Regex> = LazyLock::new(|| re(r"[0-9]{4}[-:][0-9]{2}[-:][0-9]{2}"));
static TABLE_KEY: LazyLock<Regex> = LazyLock::new(|| re(r"^(Brush)?Table_[0-9A-Fa-f]{32}$"));
static KEY: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z][A-Za-z0-9_]*$"));
static LANG: LazyLock<Regex> =
    LazyLock::new(|| re(r"^(x-default|[a-z]{2,3}(-[A-Za-z0-9]{2,8})*)$"));
static NUMERIC_TEXT: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"^{NUM_TOKEN}([ ,/]+{NUM_TOKEN})*$")));
static NUMERIC_SPLIT: LazyLock<Regex> = LazyLock::new(|| re(r"[ ,/]+"));
static DAB: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"^[A-Za-z]( {NUM_TOKEN})+$")));
static VERSION: LazyLock<Regex> = LazyLock::new(|| re(r"^[0-9]+(\.[0-9]+){0,2}$"));

/// The hard text rules; the last three are the dropped table-key names.
static HARD_TEXT: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)md5p:",
        r"(?i)\bfile:",
        r"/Users/",
        r"/Volumes/",
        r"/home/",
        r"/private/",
        r"/tmp/",
        r"/var/",
        r"/Library/",
        r"(?i)\b[A-Z]:[\\/]",
        r"\\\\",
        r"~/",
        r"(?i)\.(cr2|cr3|crw|nef|nrw|arw|srf|sr2|raf|dng|orf|rw2|pef|srw|x3f|3fr|iiq|jpe?g|tiff?|heic|heif|psd)\b",
        r"Table_",
        r"LookTable",
        r"RGBTable",
    ]
    .iter()
    .map(|p| re(p))
    .collect()
});
const TABLE_NAME_RULES: usize = 3;

/// 32-hex runs not embedded in a longer hex run (the Python look-arounds).
fn hex32_runs(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_hexdigit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i - start == 32 {
                out.push(&s[start..i]);
            }
        } else {
            i += 1;
        }
    }
    out
}

fn adobe_look_name(uuid: &str) -> Option<&'static str> {
    let upper = uuid.to_ascii_uppercase();
    ADOBE_LOOKS
        .iter()
        .find(|(u, _)| *u == upper)
        .map(|(_, n)| *n)
}

fn numeric_text_ok(v: &str) -> bool {
    if !NUMERIC_TEXT.is_match(v) {
        return false;
    }
    let tokens: Vec<&str> = NUMERIC_SPLIT.split(v).filter(|t| !t.is_empty()).collect();
    let lone_huge = tokens.len() == 1
        && !v.contains('.')
        && v.parse::<f64>().is_ok_and(|x| x.abs() >= BIG_NUMBER);
    !lone_huge
}

fn generic_string_ok(v: &str) -> bool {
    matches!(v, "" | SYNTHETIC_TEXT | SYNTHETIC_DATE)
        || SYNTH_HEX.is_match(v)
        || SYNTH_GUID.is_match(v)
        || numeric_text_ok(v)
        || DAB.is_match(v)
}

/// The per-key value rule (`STRING_RULES` in the extractor), if any.
fn key_rule(v: &str, key: &str, parent: &str) -> Option<bool> {
    if let Some((_, fixed)) = FIXED_IN_PARENT.iter().find(|(k, _)| *k == (parent, key)) {
        return Some(v == *fixed);
    }
    match (parent, key) {
        ("Filters", "Name") => return Some(FILTER_NAMES.contains(&v)),
        ("Filters", "Title") => {
            return Some(re(r"^\$\$\$/CRaw/Filter/[A-Za-z/]+=[A-Za-z ]+$").is_match(v))
        }
        _ => {}
    }
    if let Some((_, fixed)) = FIXED_VALUES.iter().find(|(k, _)| *k == key) {
        return Some(v == *fixed);
    }
    let one_of = |set: &[&str]| set.contains(&v);
    Some(match key {
        "What" => re(r"^(Correction|Mask/[A-Za-z]+)$").is_match(v),
        "WhiteBalance" => one_of(&[
            "As Shot",
            "Auto",
            "Custom",
            "Daylight",
            "Cloudy",
            "Shade",
            "Tungsten",
            "Fluorescent",
            "Flash",
        ]),
        "ProcessVersion" | "Version" => VERSION.is_match(v),
        "orientation" => re(r"^[A-D]{2}$").is_match(v),
        "LensProfileSetup" => one_of(&["LensDefaults", "Custom", "Auto"]),
        "CameraProfile" => one_of(CAMERA_PROFILES),
        "ToneCurveName" | "ToneCurveName2012" => one_of(TONE_CURVE_NAMES),
        "Method" => one_of(&["poisson", "gaussian", "clone"]),
        "SourceState" | "sourceState" => one_of(&["sourceAutoComputed", "sourceSetExplicitly"]),
        "SpotType" | "spotType" => one_of(&["heal", "clone", "heal_patchmatch"]),
        "fill_method" => one_of(&["firefly"]),
        "pm_source_type" => one_of(&["full"]),
        "CorrectionName" => re(r"^Correction [0-9]+$").is_match(v),
        "MaskName" => re(r"^Mask [0-9]+$").is_match(v),
        _ => return None,
    })
}

fn string_ok(v: &str, key: &str, parent: &str) -> bool {
    if v == SYNTHETIC_TEXT || SYNTH_HEX.is_match(v) || SYNTH_GUID.is_match(v) {
        return true;
    }
    key_rule(v, key, parent).unwrap_or_else(|| generic_string_ok(v))
}

fn is_dropped_key(key: &str) -> bool {
    TABLE_KEY.is_match(key)
        || key.starts_with("LookTable")
        || key.starts_with("RGBTable")
        || key == "MaskBrushTable"
}

fn hard_text_problems(text: &str, name: &str, skip_table_names: bool) -> Vec<String> {
    let rules = if skip_table_names {
        &HARD_TEXT[..HARD_TEXT.len() - TABLE_NAME_RULES]
    } else {
        &HARD_TEXT[..]
    };
    rules
        .iter()
        .filter(|r| r.is_match(text))
        .map(|r| format!("{name}: forbidden text matching {:?}", r.as_str()))
        .collect()
}

/// Every 32-hex value and GUID in keys and strings must be synthetic.
fn check_ids_only(data: &J, name: &str) -> Vec<String> {
    fn text(s: &str, name: &str, out: &mut Vec<String>) {
        for h in hex32_runs(s) {
            if !SYNTH_HEX.is_match(h) {
                out.push(format!(
                    "{name}: 32-hex value {}... is not synthetic",
                    &h[..6]
                ));
            }
        }
        for g in GUID_ANY.find_iter(s) {
            if !SYNTH_GUID.is_match(g.as_str()) {
                out.push(format!(
                    "{name}: GUID {}... is not synthetic",
                    &g.as_str()[..8]
                ));
            }
        }
    }
    fn walk(node: &J, name: &str, out: &mut Vec<String>) {
        match node {
            J::Object(o) => {
                for (k, v) in o {
                    text(k, name, out);
                    walk(v, name, out);
                }
            }
            J::Array(a) => a.iter().for_each(|v| walk(v, name, out)),
            J::String(s) => text(s, name, out),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(data, name, &mut out);
    out
}

/// `check_json_tree` of the extractor.
struct Tree<'a> {
    name: &'a str,
    strict: bool,
    problems: Vec<String>,
}

impl Tree<'_> {
    fn number(&mut self, n: &serde_json::Number, path: &str, key: &str) {
        let Some(x) = n.as_f64() else { return };
        if x.abs() < BIG_NUMBER {
            return;
        }
        if BIG_NUMBER_KEYS.contains(&key)
            || (key == "GrainSeed" && n.as_u64() == Some(SYNTHETIC_GRAIN_SEED))
        {
            return;
        }
        self.problems.push(format!(
            "{}: number >= 1e9 at {path} (per-photo value?)",
            self.name
        ));
    }

    fn string(&mut self, v: &str, path: &str, key: &str, parent: &str, value_rule: bool) {
        let name = self.name;
        for h in hex32_runs(v) {
            if !SYNTH_HEX.is_match(h) {
                self.problems.push(format!(
                    "{name}: 32-hex value {}... at {path} is not synthetic",
                    &h[..6]
                ));
            }
        }
        for g in GUID_ANY.find_iter(v) {
            if !SYNTH_GUID.is_match(g.as_str()) {
                self.problems.push(format!(
                    "{name}: GUID {}... at {path} is not synthetic",
                    &g.as_str()[..8]
                ));
            }
        }
        if DATE.is_match(v) && v != SYNTHETIC_DATE {
            self.problems
                .push(format!("{name}: date-like text at {path}"));
        }
        if NUMERIC_TEXT.is_match(v) && !numeric_text_ok(v) {
            self.problems
                .push(format!("{name}: numeric string >= 1e9 at {path}"));
        }
        if value_rule && !string_ok(v, key, parent) {
            let short: String = v.chars().take(30).collect();
            self.problems
                .push(format!("{name}: unexplained string at {path}: {short:?}"));
        }
    }

    fn look(&mut self, node: &Map<String, J>, path: &str) {
        let name = self.name;
        if let Some(params) = node.get("Parameters") {
            let is_stub = match params {
                J::Array(a) => a.is_empty(),
                J::Object(o) => {
                    let version = match o.get("ProcessVersion") {
                        None => "1".to_owned(),
                        Some(J::String(s)) => s.clone(),
                        Some(other) => other.to_string(),
                    };
                    !o.is_empty()
                        && o.keys().all(|k| STUB_KEYS.contains(&k.as_str()))
                        && o.get("ConvertToGrayscale").is_none_or(J::is_boolean)
                        && VERSION.is_match(&version)
                }
                _ => false,
            };
            if !is_stub {
                self.problems.push(format!(
                    "{name}: {path}.Parameters is not the stub (keys within {STUB_KEYS:?})"
                ));
            }
        }
        let uuid = node.get("UUID").and_then(J::as_str);
        // `null` counts as absent, as in the Python check.
        let lname = node.get("Name").filter(|n| !n.is_null());
        match uuid {
            Some(u) if adobe_look_name(u).is_some() => {
                if lname.is_some_and(|n| n.as_str() != adobe_look_name(u)) {
                    self.problems
                        .push(format!("{name}: {path}.Name does not match its Adobe UUID"));
                }
            }
            Some(u) if !SYNTH_HEX.is_match(u) => self.problems.push(format!(
                "{name}: {path}.UUID is neither a verified Adobe profile nor synthetic"
            )),
            _ => {
                if lname.is_some_and(|n| n.as_str() != Some(SYNTHETIC_PROFILE)) {
                    self.problems
                        .push(format!("{name}: {path}.Name without a verified Adobe UUID"));
                }
            }
        }
        for (k, v) in node {
            if matches!(k.as_str(), "Parameters" | "UUID" | "Name") {
                continue;
            }
            if let (Some((_, synthetic)), J::Object(alt)) =
                (LOOK_ALT_FIELDS.iter().find(|(f, _)| f == k), v)
            {
                for (lang, text) in alt {
                    if !LANG.is_match(lang) {
                        self.problems
                            .push(format!("{name}: bad language tag at {path}.{k}"));
                    }
                    if self.strict && text.as_str() != Some(synthetic) {
                        self.problems
                            .push(format!("{name}: unexplained string at {path}.{k}.{lang}"));
                    }
                    if let J::String(t) = text {
                        self.string(t, &format!("{path}.{k}.{lang}"), lang, k, false);
                    }
                }
                continue;
            }
            self.walk(v, &format!("{path}.{k}"), k, "Look");
        }
    }

    fn walk(&mut self, node: &J, path: &str, key: &str, parent: &str) {
        match node {
            J::Object(o) => {
                for (k, v) in o {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    if !KEY.is_match(k) {
                        self.problems
                            .push(format!("{}: key {p:?} is not an identifier", self.name));
                    }
                    if is_dropped_key(k) || k.starts_with("Table_") || k.starts_with("BrushTable_")
                    {
                        self.problems
                            .push(format!("{}: forbidden key {p}", self.name));
                    }
                    if let ("Look", J::Object(look)) = (k.as_str(), v) {
                        self.look(look, &p);
                        continue;
                    }
                    self.walk(v, &p, k, key);
                }
            }
            J::Array(a) => {
                for v in a {
                    self.walk(v, &format!("{path}[]"), key, parent);
                }
            }
            J::String(s) => {
                let strict = self.strict;
                self.string(s, path, key, parent, strict);
            }
            J::Number(n) => self.number(n, path, key),
            J::Bool(_) | J::Null => {}
        }
    }
}

fn check_json_tree(data: &J, name: &str, strict: bool) -> Vec<String> {
    let mut t = Tree {
        name,
        strict,
        problems: Vec::new(),
    };
    t.walk(data, "", "", "");
    t.problems
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/develop")
}

fn files_below(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable fixture folder") {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_below(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("json" | "xmp")
        ) {
            out.push(path);
        }
    }
}

/// `hygiene_check(out_dir=root/lua, root=root)` of the extractor.
fn hygiene_check(root: &Path) -> Vec<String> {
    let lua = root.join("lua");
    let owned: BTreeSet<String> = std::fs::read_to_string(lua.join("manifest.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<J>(&t).ok())
        .and_then(|m| {
            m["files"].as_array().map(|files| {
                files
                    .iter()
                    .filter_map(|e| e["file"].as_str().map(str::to_owned))
                    .collect()
            })
        })
        .unwrap_or_default();
    let mut files = Vec::new();
    files_below(root, &mut files);
    files.sort();
    let mut problems = Vec::new();
    for f in files {
        let name = f.display().to_string();
        let file_name = f.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("{name}: {e}"));
        let in_root = f.parent() == Some(root);
        if in_root && REGISTRY_FILES.contains(&file_name.as_str()) {
            problems.extend(hard_text_problems(&text, &name, true));
            match serde_json::from_str::<J>(&text) {
                Ok(data) => problems.extend(check_ids_only(&data, &name)),
                Err(e) => problems.push(format!("{name}: not valid JSON ({e})")),
            }
            continue;
        }
        problems.extend(hard_text_problems(&text, &name, false));
        if f.extension().is_some_and(|e| e == "xmp") {
            for h in hex32_runs(&text) {
                if !SYNTH_HEX.is_match(h) && adobe_look_name(h).is_none() {
                    problems.push(format!(
                        "{name}: 32-hex value {}... is not synthetic",
                        &h[..6]
                    ));
                }
            }
            continue;
        }
        let data: J = match serde_json::from_str(&text) {
            Ok(d) => d,
            Err(e) => {
                problems.push(format!("{name}: not valid JSON ({e})"));
                continue;
            }
        };
        let in_lua = f.parent() == Some(lua.as_path());
        if in_lua && file_name == "manifest.json" {
            problems.extend(check_ids_only(&data, &name));
            continue;
        }
        if in_lua && !owned.contains(&file_name) && !file_name.starts_with("hand_") {
            problems.push(format!(
                "{name}: not listed in manifest.json; hand-written fixtures must be named hand_*.json"
            ));
        }
        problems.extend(check_json_tree(
            &data,
            &name,
            in_lua && owned.contains(&file_name),
        ));
    }
    problems
}

#[test]
fn testdata_holds_no_private_or_third_party_content() {
    let problems = hygiene_check(&root());
    assert!(
        problems.is_empty(),
        "HYGIENE CHECK FAILED ({} problems):\n{}",
        problems.len(),
        problems.join("\n")
    );
}

// ---- the checker itself ----------------------------------------------------

fn tree(v: J, strict: bool) -> Vec<String> {
    check_json_tree(&v, "t", strict)
}

#[test]
fn checker_rejects_private_ids_and_table_keys() {
    use serde_json::json;
    let hex = "0123456789ABCDEF0123456789ABCDEF";
    assert!(!tree(json!({ "MaskDigest": hex }), false).is_empty());
    assert!(!tree(json!({ format!("Table_{hex}"): "x" }), false).is_empty());
    assert!(!tree(
        json!({ "CorrectionID": "12345678-1234-1234-1234-123456789012" }),
        false
    )
    .is_empty());
    assert!(!tree(json!({ "Seed": 1_500_000_000u64 }), false).is_empty());
    assert!(!tree(json!({ "Note": "2024:05:12 10:00:00" }), false).is_empty());
    assert!(!tree(json!({ "bad key": 1 }), false).is_empty());
    assert!(tree(
        json!({ "MaskDigest": "00000000000000000000000000000001" }),
        false
    )
    .is_empty());
    assert!(tree(
        json!({ "GrainSeed": 4_000_000_001u64, "ModelVersion": 3_000_000_000u64 }),
        false
    )
    .is_empty());
    assert!(!hard_text_problems("{\"p\": \"md5p:abc\"}", "t", false).is_empty());
    assert!(!hard_text_problems("C:\\photos", "t", false).is_empty());
    assert!(!hard_text_problems("IMG_0001.CR3", "t", false).is_empty());
    assert!(hard_text_problems("LookTable", "t", true).is_empty());
}

#[test]
fn checker_enforces_the_look_rules() {
    use serde_json::json;
    let stub = json!({ "Look": { "Parameters": { "ConvertToGrayscale": false, "ProcessVersion": "15.4" },
                                 "UUID": "6F9C877E84273F4E8271E6B91BEB36A1", "Name": "Adobe Landscape" } });
    assert!(tree(stub, true).is_empty());
    let full = json!({ "Look": { "Parameters": { "Exposure2012": 0.5 } } });
    assert!(!tree(full, false).is_empty());
    let wrong_name =
        json!({ "Look": { "UUID": "6F9C877E84273F4E8271E6B91BEB36A1", "Name": "Other" } });
    assert!(!tree(wrong_name, false).is_empty());
    let foreign =
        json!({ "Look": { "UUID": "11111111111111111111111111111111", "Name": "Camera X" } });
    assert!(!tree(foreign, false).is_empty());
    let unnamed =
        json!({ "Look": { "UUID": "00000000000000000000000000000009", "Name": "Real Name" } });
    assert!(!tree(unnamed, false).is_empty());
}

#[test]
fn checker_applies_value_rules_to_generated_files_only() {
    use serde_json::json;
    let v = json!({ "MaskGroupBasedCorrections": [{ "CorrectionName": "Himmel" }] });
    assert!(!tree(v.clone(), true).is_empty());
    assert!(tree(v, false).is_empty());
    assert!(tree(
        json!({ "CameraProfile": "Adobe Standard", "Dabs": ["d 0.1 0.2"] }),
        true
    )
    .is_empty());
    assert!(!tree(json!({ "CameraProfile": "My Profile" }), true).is_empty());
}

#[test]
fn hex_runs_ignore_longer_hex_strings() {
    let h = "0123456789abcdef0123456789abcdef";
    assert_eq!(hex32_runs(&format!("x{h}y")), [h]);
    assert!(hex32_runs(&format!("{h}0")).is_empty());
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

/// The file-level rules (scope, manifest ownership, the registry exemption,
/// XMP ids) and the inputs the Rust and Python checks once disagreed on, on a
/// copy of the real tree with one crafted file per case. Mirrored by
/// `extract_fixtures.py --self-test`.
#[test]
fn dispatch_rules_flag_exactly_the_crafted_files() {
    let tmp = std::env::temp_dir().join(format!(
        "lrg-develop-hygiene-{}-dispatch",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    copy_dir(&root(), &tmp);
    let foreign_hex = "0123456789ABCDEF0123456789ABCDEF";
    let masked =
        r#"{"MaskGroupBasedCorrections": [{"CorrectionMasks": [{"MaskName": "Himmel"}]}]}"#;
    let manifest_path = tmp.join("lua/manifest.json");
    let mut manifest: J =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["files"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "file": "owned_crafted.json", "covers": [] }));
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    std::fs::create_dir_all(tmp.join("xmp")).unwrap();
    let crafted: &[(&str, String, bool)] = &[
        // (file, content, flagged)
        ("lua/unlisted.json", r#"{"Exposure2012": 0.5}"#.into(), true),
        (
            "lua/registry_snapshot.json",
            r#"{"rows": ["LookTable"]}"#.into(),
            true,
        ),
        (
            "xmp/foreign_id.xmp",
            format!("<x crs:MaskDigest=\"{foreign_hex}\"/>"),
            true,
        ),
        (
            "xmp/adobe_look.xmp",
            "<x crs:UUID=\"B952C231111CD8E0ECCF14B86BAA7077\"/>".into(),
            false,
        ),
        ("lua/owned_crafted.json", masked.into(), true),
        ("lua/hand_localised.json", masked.into(), false),
        ("lua/stray.xmp", "<x id=\"md5p:0\"/>".into(), true),
        // Inputs the two checks used to judge differently.
        ("lua/hand_newline_number.json", "{\"Foo\": \"5000000000\\n\"}".into(), false),
        (
            "lua/hand_arabic_digits.json",
            "{\"Foo\": \"\u{0665}\u{0660}\u{0660}\u{0660}\u{0660}\u{0660}\u{0660}\u{0660}\u{0660}\u{0660}\"}"
                .into(),
            false,
        ),
        ("lua/hand_newline_key.json", "{\"Exposure\\n\": 1}".into(), true),
        (
            "lua/hand_newline_version.json",
            "{\"Look\": {\"Parameters\": {\"ProcessVersion\": \"15.4\\n\"}}}".into(),
            true,
        ),
        ("lua/hand_null_name.json", r#"{"Look": {"Name": null}}"#.into(), false),
    ];
    for (file, content, _) in crafted {
        std::fs::write(tmp.join(file), content).unwrap();
    }
    let problems = hygiene_check(&tmp);
    let flagged = |file: &str| {
        let prefix = format!("{}: ", tmp.join(file).display());
        problems.iter().any(|p| p.starts_with(&prefix))
    };
    let wrong: Vec<String> = crafted
        .iter()
        .filter(|(file, _, want)| flagged(file) != *want)
        .map(|(file, _, want)| format!("{file}: expected flagged={want}"))
        .collect();
    let crafted_prefixes: Vec<String> = crafted
        .iter()
        .map(|(f, _, _)| format!("{}: ", tmp.join(f).display()))
        .collect();
    let other: Vec<&String> = problems
        .iter()
        .filter(|p| !crafted_prefixes.iter().any(|c| p.starts_with(c.as_str())))
        .collect();
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(wrong.is_empty(), "{wrong:#?}\nproblems: {problems:#?}");
    assert!(
        other.is_empty(),
        "the real files must stay clean: {other:#?}"
    );
}
