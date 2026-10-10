//! Local-only: the style engine's canonical form over every row of the
//! maintainer's private training dump.
//!
//! Same dump and gate as `lrg-develop/tests/training_dump_local.rs` (see
//! there for the format and `server-rs/scripts/develop_registry/
//! dump_training.py`):
//!
//! ```text
//! LRG_DEVELOP_TRAINING_DUMP=/path/to/training_rows.json \
//!     cargo test -p lrg-analysis --test training_dump_canonical_local -- --nocapture
//! ```
//!
//! Without the variable the test skips through the shared golden gate
//! (family `training-dump`, local-only). With it, every row must
//! canonicalise without an error, every canonical key must come out of some
//! row, and blending all rows must give the recipe groups the installed
//! plugin applies. Output is counts per key only, never values or photo ids.

#[path = "../../lrg-ml/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use lrg_analysis::style_engine::{
    canonical_to_edit_recipe, interpolate_recipes, TrainingCandidate,
};
use lrg_analysis::training::{canonical_keys, canonicalize_develop_settings_str};
use lrg_develop::FileKindHint;
use serde_json::Value as J;

const VAR: &str = "LRG_DEVELOP_TRAINING_DUMP";

#[test]
fn every_training_row_yields_the_style_keys() {
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

    let mut errors = 0usize;
    let mut empty = 0usize;
    let mut per_key: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut stale_hues = 0usize;
    let mut pool: Vec<(TrainingCandidate, f64)> = Vec::new();
    for row in &rows {
        let Some(meta) = row["metadata"]
            .as_str()
            .and_then(|m| serde_json::from_str::<J>(m).ok())
        else {
            errors += 1;
            continue;
        };
        let Some(blob) = meta["develop_settings"].as_str() else {
            errors += 1;
            continue;
        };
        let hint = FileKindHint::from(meta["is_raw"].as_bool());
        let Ok(c) = canonicalize_develop_settings_str(blob, hint) else {
            errors += 1;
            continue;
        };
        if c.settings.is_empty() {
            empty += 1;
        }
        for key in canonical_keys() {
            if c.settings.contains_key(key.name) {
                *per_key.entry(key.name).or_default() += 1;
            }
        }
        for zone in ["shadows", "highlights"] {
            let get = |f: &str| {
                c.settings
                    .get(&format!("color_grading_{zone}_{f}"))
                    .and_then(J::as_f64)
            };
            if get("saturation") == Some(0.0) && get("hue").is_some_and(|h| h != 0.0) {
                stale_hues += 1;
            }
        }
        pool.push((
            TrainingCandidate {
                canonical_settings: c.settings,
                ..Default::default()
            },
            1.0,
        ));
    }

    eprintln!(
        "training dump: {} rows, {errors} unreadable, {empty} with no canonical key",
        rows.len()
    );
    eprintln!("rows per canonical key ({} keys):", canonical_keys().len());
    for key in canonical_keys() {
        eprintln!("  {:<40} {}", key.name, per_key.get(key.name).unwrap_or(&0));
    }
    eprintln!("zones with a stale hue at saturation 0: {stale_hues}");

    assert_eq!(errors, 0, "rows that could not be canonicalised");
    let missing: Vec<&str> = canonical_keys()
        .iter()
        .map(|k| k.name)
        .filter(|n| !per_key.contains_key(n))
        .collect();
    assert!(missing.is_empty(), "keys no row yields: {missing:?}");

    let blend = interpolate_recipes(&pool);
    let recipe = canonical_to_edit_recipe(&blend.settings, "", None);
    let global = recipe["global"].as_object().unwrap();
    let groups: Vec<String> = global
        .iter()
        .map(|(k, v)| match v.as_object() {
            Some(m) => format!("{k}({})", m.len()),
            None => k.clone(),
        })
        .collect();
    eprintln!(
        "blend of all rows: {} global fields: {groups:?}",
        global.len()
    );
    eprintln!("blend notes: {}", blend.warnings.len());
    for group in ["hsl", "color_grading", "tone_curve"] {
        assert!(global.get(group).is_some_and(J::is_object), "no {group}");
    }
    let hsl = global["hsl"].as_object().unwrap();
    assert_eq!(hsl.len(), 8, "HSL channels");
    let curve = global["tone_curve"].as_object().unwrap();
    for split in ["shadow_split", "midtone_split", "highlight_split"] {
        assert!(curve.contains_key(split), "tone_curve.{split}");
    }
}
