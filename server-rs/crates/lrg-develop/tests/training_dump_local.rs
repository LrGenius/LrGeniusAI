//! Local-only: the Lua reader over every row of the maintainer's private
//! training dump.
//!
//! The dump (`edit_training.lance` exported by
//! `server-rs/scripts/develop_registry/dump_training.py`) is private and never
//! enters the repository, so CI cannot run this. Point
//! `LRG_DEVELOP_TRAINING_DUMP` at it to run it:
//!
//! ```text
//! LRG_DEVELOP_TRAINING_DUMP=/path/to/training_rows.json \
//!     cargo test -p lrg-develop --test training_dump_local -- --nocapture
//! ```
//!
//! Format: a JSON array of `{ "id", "metadata" }`, where `metadata` is a JSON
//! string whose `develop_settings` is itself a JSON string (the blob the
//! plugin sent) and `is_raw` the plugin's flag.
//!
//! Without the variable the test skips through the shared golden gate
//! (family `training-dump`, local-only: `LRG_REQUIRE_GOLDENS=all` does not
//! include it; `LRG_REQUIRE_GOLDENS=training-dump` turns the skip into a
//! failure). Setting the variable asks for the run, so a path that is not a
//! file is a failure, never a skip.
//!
//! Every row must parse without an error and without any warning (the rows
//! are Lightroom's own output, so any warning means the registry is wrong),
//! and every number must lie within its key's registry range. Output names
//! keys, row numbers and counts only, never values or photo ids: the rows are
//! private.

#[path = "../../lrg-ml/tests/common/mod.rs"]
mod common;
mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use lrg_develop::lua::from_lua_str;
use lrg_develop::FileKindHint;
use serde_json::Value as J;
use support::{generic_path, MaskCounts, RangeCheck};

const VAR: &str = "LRG_DEVELOP_TRAINING_DUMP";

#[test]
fn every_training_row_parses_without_errors_or_unknown_keys() {
    let path = match std::env::var_os(VAR) {
        None => {
            if !common::assets_ready("training-dump", false, &format!("{VAR} is not set")) {
                return;
            }
            unreachable!("assets_ready panics when the family is required")
        }
        Some(p) => {
            let p = PathBuf::from(p);
            assert!(p.is_file(), "{VAR}={} is not a file", p.display());
            p
        }
    };
    let text = std::fs::read_to_string(&path).expect("read the dump");
    let rows: Vec<J> = serde_json::from_str(&text).expect("the dump is a JSON array");
    assert!(!rows.is_empty(), "empty dump");

    let mut errors: Vec<String> = Vec::new();
    let mut per_kind: BTreeMap<&'static str, usize> = BTreeMap::new();
    // Key paths with indices removed, so one key counts once however often
    // it appears; values are never printed.
    let mut unexpected: BTreeMap<String, usize> = BTreeMap::new();
    let mut masks = MaskCounts::default();
    let mut process_versions: BTreeMap<String, usize> = BTreeMap::new();
    let mut ranges = RangeCheck::default();

    // Rows are named by their position only: their ids are private photo ids.
    for (i, row) in rows.iter().enumerate() {
        let meta: J = match row["metadata"].as_str().map(serde_json::from_str) {
            Some(Ok(m)) => m,
            _ => {
                errors.push(format!("row #{i}: metadata is not a JSON string"));
                continue;
            }
        };
        let Some(blob) = meta["develop_settings"].as_str() else {
            errors.push(format!("row #{i}: no develop_settings string"));
            continue;
        };
        let hint = FileKindHint::from(meta["is_raw"].as_bool());
        let (settings, warnings) = match from_lua_str(blob, hint) {
            Ok(r) => r,
            Err(e) => {
                errors.push(format!("row #{i}: {e}"));
                continue;
            }
        };
        ranges.settings(&settings);
        for w in &warnings {
            *per_kind.entry(w.kind.name()).or_default() += 1;
            *unexpected
                .entry(format!("{} {}", w.kind.name(), generic_path(&w.path)))
                .or_default() += 1;
        }
        *process_versions
            .entry(
                settings
                    .process_version()
                    .map_or_else(|| "none".into(), |pv| pv.to_string()),
            )
            .or_default() += 1;
        masks.add(&settings);
    }

    eprintln!(
        "training dump: {} rows, {} parse errors",
        rows.len(),
        errors.len()
    );
    eprintln!("warnings per kind: {per_kind:?}");
    eprintln!("process versions: {process_versions:?}");
    eprintln!(
        "corrections: {}; mask tools: {:?}",
        masks.corrections, masks.tools
    );
    eprintln!("combinations: {:?}", masks.combines);
    eprintln!("values outside their registry range: {:?}", ranges.outside);
    for (what, n) in &unexpected {
        eprintln!("  {n:>5}  {what}");
    }

    assert!(errors.is_empty(), "parse errors: {errors:#?}");
    // The dump is Lightroom's own output: any warning (an unknown key, a
    // type mismatch, a file-kind or process-version finding, a kind added
    // later) means the registry or the reader is wrong, not the row. An
    // allow-list, empty today, so a new warning kind fails here too.
    assert!(
        per_kind.is_empty(),
        "warnings (extend the registry or fix the reader): {unexpected:#?}"
    );
    assert_eq!(masks.combines.get("unrecognised"), None);
    assert!(
        ranges.outside.is_empty(),
        "values outside the registry range (widen the range in registry/table.rs): {:#?}",
        ranges.outside
    );
}
