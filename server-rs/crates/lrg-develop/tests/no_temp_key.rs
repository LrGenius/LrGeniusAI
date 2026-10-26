//! `Temp` is not a Lightroom develop key. White balance is `Temperature`/
//! `Tint` on raw files and `IncrementalTemperature`/`IncrementalTint` on
//! everything else; code that reads or writes `Temp` silently does nothing
//! (the style engine learned no white balance from any example because of
//! it). This test keeps the name from coming back as a key literal.
//!
//! Scanned: every `.rs` file under `server-rs/crates/*/src` and the plugin's
//! `DevelopEditManager.lua`, comments excluded. Allowed: the develop
//! experiments (`*DevelopExperiments*.lua`), which write `Temp` on purpose to
//! observe what Lightroom does with it, and the one constant the Lua writer
//! refuses it with (`ALLOWED_LINES`).
//!
//! CI runs it twice: with the rest of `cargo test --workspace` in
//! `server-rs-tests.yml` (only on `server-rs/**` changes), and on its own in
//! the unfiltered `format-lint-rust` job of `lint-format.yml`, so a PR that
//! touches only `DevelopEditManager.lua` is checked too.

use std::path::{Path, PathBuf};

use regex::Regex;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repository root")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Lines allowed to spell the key, by file (relative to the repository
/// root) and trimmed text: the one constant the Lua writer's wire guard
/// refuses the key with.
const ALLOWED_LINES: &[(&str, &str)] = &[(
    "server-rs/crates/lrg-develop/src/lua/write.rs",
    r#"pub const RETIRED_TEMP_KEY: &str = "Temp";"#,
)];

fn is_allowlisted(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.contains("DevelopExperiments"))
}

/// The line without its comment. Naive about comment markers inside string
/// literals, which is the safe direction: at worst it hides the end of a line
/// that contains a string with `//` or `--` in it.
fn code_of<'a>(line: &'a str, marker: &str) -> &'a str {
    match line.find(marker) {
        Some(i) => &line[..i],
        None => line,
    }
}

/// Offending `(line number, line)` pairs of one file.
fn offences(text: &str, lua: bool, pattern: &Regex) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut in_block = false;
    for (i, line) in text.lines().enumerate() {
        let code = if lua {
            // Lua block comments: `--[[ ... ]]` (also `--[==[ ... ]==]`).
            if in_block {
                if line.contains("]]") || line.contains("]=") {
                    in_block = false;
                }
                continue;
            }
            if let Some(start) = line.find("--[") {
                let rest = &line[start..];
                if (rest.starts_with("--[[") || rest.starts_with("--[="))
                    && !rest.contains("]]")
                    && !rest.contains("]=")
                {
                    in_block = true;
                }
            }
            code_of(line, "--")
        } else {
            if in_block {
                if line.contains("*/") {
                    in_block = false;
                }
                continue;
            }
            let code = code_of(line, "//");
            match code.find("/*") {
                Some(start) => {
                    if !code[start..].contains("*/") {
                        in_block = true;
                    }
                    &code[..start]
                }
                None => code,
            }
        };
        if pattern.is_match(code) {
            found.push((i + 1, line.trim().to_string()));
        }
    }
    found
}

fn temp_key_pattern() -> Regex {
    // `"Temp"`, `'Temp'`, `Temp =` (a table field or assignment), `.Temp`
    // (field access); `Temperature`, `TempDir` and friends do not match.
    Regex::new(r#"["']Temp["']|\bTemp\s*=|\.Temp\b"#).unwrap()
}

#[test]
fn the_pattern_matches_key_literals_only() {
    let p = temp_key_pattern();
    for bad in [
        r#"("Temp", "temperature"),"#,
        "\ttemperature = 'Temp',",
        "\tTemp = { min = 2000, max = 50000 },",
        "if developSettings.Temp then",
        r#"if key == "Temp" then"#,
    ] {
        assert!(p.is_match(bad), "{bad}");
    }
    for good in [
        r#""Temperature""#,
        "settings.Temperature = 5600",
        "IncrementalTemperature = 12,",
        "let tmp = TempDir::new();",
        "let temp = 3;",
    ] {
        assert!(!p.is_match(good), "{good}");
    }
    assert!(offences("// \"Temp\" in a comment", false, &p).is_empty());
    assert!(offences("x = 1 -- Temp = 2", true, &p).is_empty());
    assert!(offences("--[[\nTemp = 1\n]]\ny = 2", true, &p).is_empty());
    assert_eq!(offences("a = 1\nb.Temp = 2", true, &p).len(), 1);
}

#[test]
fn no_code_uses_temp_as_a_develop_key() {
    let root = repo_root();
    let pattern = temp_key_pattern();

    let mut files = Vec::new();
    let crates = root.join("server-rs/crates");
    for entry in std::fs::read_dir(&crates).unwrap() {
        let src = entry.unwrap().path().join("src");
        if src.is_dir() {
            rust_sources(&src, &mut files);
        }
    }
    let lua = root.join("plugin/LrGeniusAI.lrdevplugin/DevelopEditManager.lua");
    assert!(lua.is_file(), "{} is missing", lua.display());
    files.push(lua);
    assert!(files.len() > 50, "only {} files found", files.len());

    let mut report = Vec::new();
    let mut allowed_seen = 0;
    for file in files.iter().filter(|f| !is_allowlisted(f)) {
        let text = std::fs::read_to_string(file).unwrap();
        let is_lua = file.extension().is_some_and(|e| e == "lua");
        let rel = file.strip_prefix(&root).unwrap_or(file);
        // `/`-joined, so `ALLOWED_LINES` matches on Windows too (the release
        // job runs this test there; its canonical `\\?\` root yields `\`).
        let rel_s = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        for (line, code) in offences(&text, is_lua, &pattern) {
            if ALLOWED_LINES.contains(&(rel_s.as_str(), code.as_str())) {
                allowed_seen += 1;
                continue;
            }
            report.push(format!("{}:{line}: {code}", rel.display()));
        }
    }
    // No stale allowance: each allowed line exists, once.
    assert_eq!(allowed_seen, ALLOWED_LINES.len(), "{ALLOWED_LINES:?}");
    assert!(
        report.is_empty(),
        "`Temp` is not a Lightroom develop key; use Temperature/Tint (raw) or \
         IncrementalTemperature/IncrementalTint (non-raw):\n{}",
        report.join("\n")
    );
}
