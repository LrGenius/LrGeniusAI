//! The registry as a reviewable JSON file.
//!
//! `server-rs/testdata/develop/registry_snapshot.json` is generated from the
//! registry and compared on every run, so any change to `table.rs` or
//! `patterns.rs` shows up as a readable diff in review. After an intended
//! change, regenerate it:
//!
//! ```text
//! LRG_BLESS=1 cargo test -p lrg-develop --test registry_snapshot
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use lrg_develop::registry::{self, Def, KeySpec, Lit, PatternSpec, Policy, UiScale, PATTERNS};
use serde_json::{json, Value};

fn snapshot_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/develop/registry_snapshot.json")
}

fn default_json(d: Def) -> Value {
    match d {
        Def::Value(lit) => json!({ "value": match lit {
            Lit::Int(i) => json!(i),
            Lit::Real(r) => json!(r.get()),
            Lit::Bool(b) => json!(b),
            Lit::Str(s) => json!(s),
            Lit::IntList(v) => json!(v),
        }}),
        Def::Absent => json!("absent"),
        Def::Unverified => json!("unverified"),
        Def::NoDefault => json!("no_default"),
    }
}

fn policy_json(p: Policy) -> Value {
    match p.gate() {
        Some(g) => json!({ "class": p.class_name(), "gate": format!("{g:?}") }),
        None => json!({ "class": p.class_name() }),
    }
}

fn row_json(s: &KeySpec) -> Value {
    json!({
        "level": s.level.to_string(),
        "name": s.name,
        "kind": format!("{:?}", s.kind),
        "range": s.range.map(|(lo, hi)| json!([lo.get(), hi.get()])),
        "ui": match s.ui {
            UiScale::Identity => json!("identity"),
            UiScale::Div(d) => json!({ "div": d }),
            UiScale::Unknown => json!("unknown"),
        },
        "fmt": format!("{:?}", s.fmt),
        "plus_sign": s.plus_sign,
        "default_raw": default_json(s.default.raw),
        "default_non_raw": default_json(s.default.non_raw),
        "policy": policy_json(s.policy),
        "frame": format!("{:?}", s.frame),
        "file_kind": format!("{:?}", s.file_kind),
        "presence": format!("{:?}", s.presence),
        "min_pv": s.min_pv.map(|pv| pv.to_string()),
        "recipe_alias": s.recipe_alias,
        "note": s.note,
    })
}

fn pattern_json(p: &PatternSpec) -> Value {
    json!({
        "id": p.id,
        "levels": p.levels.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "matcher": format!("{:?}", p.matcher),
        "kind": format!("{:?}", p.kind),
        "policy": policy_json(p.policy),
        "frame": format!("{:?}", p.frame),
        "presence": format!("{:?}", p.presence),
        "note": p.note,
    })
}

/// Rebuilds every object with its keys in sorted order.
///
/// `serde_json` keeps insertion order when its `preserve_order` feature is on
/// and sorts otherwise; the feature is switched on for the whole workspace
/// when a crate that enables it is built alongside (`cargo test --workspace`)
/// and off under `cargo test -p lrg-develop`. Inserting sorted keys gives the
/// same text either way.
fn canonical(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> =
                map.into_iter().map(|(k, v)| (k, canonical(v))).collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        other => other,
    }
}

fn snapshot() -> String {
    let mut counts: BTreeMap<String, BTreeMap<&str, usize>> = BTreeMap::new();
    for s in registry::table() {
        *counts
            .entry(s.level.to_string())
            .or_default()
            .entry(s.policy.class_name())
            .or_default() += 1;
    }
    let doc = json!({
        "description": "Generated from lrg-develop's key registry by tests/registry_snapshot.rs; \
                        do not edit. Regenerate with LRG_BLESS=1 cargo test -p lrg-develop \
                        --test registry_snapshot.",
        "row_count": registry::table().len(),
        "rows_per_level_and_class": counts,
        "rows": registry::table().iter().map(row_json).collect::<Vec<_>>(),
        "patterns": PATTERNS.iter().map(pattern_json).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&canonical(doc)).expect("snapshot serializes") + "\n"
}

#[test]
fn registry_matches_the_committed_snapshot() {
    let path = snapshot_path();
    let fresh = snapshot();
    if std::env::var_os("LRG_BLESS").is_some_and(|v| v == "1") {
        std::fs::write(&path, &fresh).expect("write registry snapshot");
        return;
    }
    // A Windows checkout may turn the committed file's line endings into
    // CRLF (`core.autocrlf`); the content is what must match.
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    if committed == fresh {
        return;
    }
    let first_diff = committed
        .lines()
        .zip(fresh.lines())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| committed.lines().count().min(fresh.lines().count()));
    panic!(
        "{} is out of date (first difference at line {}). If the registry change is intended, \
         run `LRG_BLESS=1 cargo test -p lrg-develop --test registry_snapshot` and commit the file.",
        path.display(),
        first_diff + 1
    );
}
