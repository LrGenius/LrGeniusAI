//! Scores culling against labelled fixtures.
//!
//! ```text
//! cargo run --release -p lrg-analysis --example cull_eval -- testdata/cull_eval
//! cargo run --release -p lrg-analysis --example cull_eval -- testdata/cull_eval --preset portrait
//! cargo run --release -p lrg-analysis --example cull_eval -- testdata/cull_eval --compare
//! ```
//!
//! `--compare` is the mode that answers the question this harness exists for.
//! It runs the current configuration against `CullingConfig::python_parity` —
//! the ranking as it was before this pass — and prints both, so a proposed
//! signal change can be shown to be an improvement rather than asserted to be
//! one. Use `--ablate <knob>` to isolate a single change.
//!
//! Fixture format: see `lrg_analysis::eval::EvalFixture`, and
//! `testdata/cull_eval/README.md` for how to build one from a real shoot.

use std::path::{Path, PathBuf};

use lrg_analysis::culling_config::{get_culling_config, CullingConfig};
use lrg_analysis::eval::{evaluate, merge, EvalFixture, EvalReport};

fn load_fixtures(root: &Path) -> Vec<(PathBuf, EvalFixture)> {
    let mut paths: Vec<PathBuf> = if root.is_dir() {
        std::fs::read_dir(root)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", root.display()))
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect()
    } else {
        vec![root.to_path_buf()]
    };
    paths.sort();

    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            let fixture: EvalFixture = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{} is not a valid fixture: {e}", path.display()));
            (path, fixture)
        })
        .collect()
}

fn run(
    fixtures: &[(PathBuf, EvalFixture)],
    config_for: impl Fn(&EvalFixture) -> CullingConfig,
) -> Vec<EvalReport> {
    fixtures
        .iter()
        .map(|(_, f)| evaluate(f, &config_for(f)))
        .collect()
}

/// Named single-knob rollbacks, for attributing a delta to one change.
fn ablate(mut cfg: CullingConfig, knob: &str) -> CullingConfig {
    match knob {
        "sharpness-peak" => {
            cfg.ranking.sharpness_peak_weight = 0.0;
            cfg.ranking.recompute_technical_score = false;
        }
        "relative" => cfg.ranking.relative_normalization_weight = 0.0,
        "iqa" => cfg.ranking.aesthetic_iqa_weight = 0.0,
        "semantic" => cfg.ranking.semantic_weight = 0.0,
        "sets" => cfg.sets.enabled = false,
        other => panic!(
            "unknown --ablate knob {other:?}; expected one of: \
             sharpness-peak, relative, iqa, semantic, sets"
        ),
    }
    cfg
}

fn print_block(title: &str, reports: &[EvalReport]) {
    println!("\n=== {title} ===");
    for r in reports {
        println!("{}", r.to_text());
    }
    if reports.len() > 1 {
        println!("\n{}", merge(reports, "ALL FIXTURES").to_text());
    }
}

/// The comparison table. Deltas are in percentage points, and the sign
/// convention is "positive is better" for every row including precision —
/// there is no row here where more is worse.
fn print_delta(before: &EvalReport, after: &EvalReport) {
    let row = |label: &str, a: f64, b: f64| {
        let delta = (b - a) * 100.0;
        let mark = if delta.abs() < 0.05 {
            "   ="
        } else if delta > 0.0 {
            "  ++"
        } else {
            "  --"
        };
        println!(
            "  {label:<18}{:>7.1}% -> {:>7.1}%   {:>+6.1}pp{mark}",
            a * 100.0,
            b * 100.0,
            delta
        );
    };
    println!("\n=== delta ===");
    row(
        "winner top-1",
        before.winner_accuracy(),
        after.winner_accuracy(),
    );
    row("NDCG", before.ndcg, after.ndcg);
    row(
        "reject precision",
        before.reject_precision(),
        after.reject_precision(),
    );
    row(
        "reject recall",
        before.reject_recall(),
        after.reject_recall(),
    );
    row(
        "set preservation",
        before.set_preservation(),
        after.set_preservation(),
    );
    row(
        "set recognition",
        before.set_recognition(),
        after.set_recognition(),
    );
    row(
        "grouping recall",
        before.grouping_recall(),
        after.grouping_recall(),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!(
            "usage: cull_eval <fixture.json|fixture-dir> [--preset NAME] \
             [--compare] [--ablate KNOB]\n\n\
             --preset NAME   override each fixture's own culling_preset\n\
             --compare       also run CullingConfig::python_parity and show the delta\n\
             --ablate KNOB   also run with one knob rolled back, and show the delta\n\
             \x20               (sharpness-peak | relative | iqa | semantic | sets)"
        );
        std::process::exit(if args.is_empty() { 2 } else { 0 });
    }

    let root = PathBuf::from(&args[0]);
    let value_after = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let preset_override = value_after("--preset");
    let ablate_knob = value_after("--ablate");
    let compare = args.iter().any(|a| a == "--compare");

    let fixtures = load_fixtures(&root);
    if fixtures.is_empty() {
        eprintln!("no fixtures found under {}", root.display());
        std::process::exit(1);
    }
    println!(
        "Loaded {} fixture(s), {} group(s), {} photo(s)",
        fixtures.len(),
        fixtures.iter().map(|(_, f)| f.groups.len()).sum::<usize>(),
        fixtures
            .iter()
            .flat_map(|(_, f)| f.groups.iter())
            .map(|g| g.photos.len())
            .sum::<usize>(),
    );

    let current = |f: &EvalFixture| {
        get_culling_config(preset_override.as_deref().unwrap_or(&f.culling_preset))
    };
    let after = run(&fixtures, current);
    print_block("current", &after);
    let after_all = merge(&after, "current");

    if compare {
        let before = run(&fixtures, |_| CullingConfig::python_parity());
        print_block("python_parity (ranking before this pass)", &before);
        print_delta(&merge(&before, "parity"), &after_all);
    }

    if let Some(knob) = ablate_knob {
        let before = run(&fixtures, |f| ablate(current(f), &knob));
        print_block(&format!("ablated: {knob}"), &before);
        print_delta(&merge(&before, "ablated"), &after_all);
    }
}
