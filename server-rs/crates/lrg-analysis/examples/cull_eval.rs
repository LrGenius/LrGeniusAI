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
//! `--by-defect` adds a table per defect for fixtures that label them (the
//! public benchmark, `testdata/cull_bench`). `--tune <knob>[,<knob>…]` tries
//! each knob over a range of values, one at a time, and prints what each value
//! scores; it suggests and changes nothing.
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
        "aesthetic-model" => cfg.ranking.aesthetic_model_weight = 0.0,
        "semantic" => cfg.ranking.semantic_weight = 0.0,
        "emotion" => cfg.ranking.emotion_prompt_set = None,
        "usable-gate" => cfg.ranking.usable_gate = false,
        "kiss-gate" => cfg.ranking.eyes_closed_intent_threshold = 0.0,
        "single-rejects" => cfg.ranking.reject_blurred_without_alternative = false,
        "sets" => cfg.sets.enabled = false,
        "eye-focus" => {
            cfg.ranking.eye_focus_weight = 0.0;
            cfg.ranking.reason_eye_focus_threshold = 0.0;
            cfg.ranking.reject_eye_focus_threshold = 0.0;
            cfg.ranking.reason_face_focus_threshold = 0.0;
            cfg.ranking.reject_face_focus_threshold = 0.0;
        }
        other => panic!(
            "unknown --ablate knob {other:?}; expected one of: \
             sharpness-peak, relative, aesthetic-model, semantic, emotion, usable-gate, \
             kiss-gate, single-rejects, sets, eye-focus"
        ),
    }
    cfg
}

/// A numeric knob `--tune` can sweep, and the values it tries. A switch is a
/// knob with the values 0 and 1.
struct Knob {
    name: &'static str,
    values: &'static [f64],
    set: fn(&mut CullingConfig, f64),
}

const KNOBS: &[Knob] = &[
    Knob {
        name: "reason_blur_threshold",
        values: &[0.08, 0.12, 0.16, 0.20, 0.24, 0.28],
        set: |c, v| c.ranking.reason_blur_threshold = v,
    },
    Knob {
        name: "relative_blur_rule",
        values: &[0.0, 1.0],
        set: |c, v| c.ranking.relative_blur_rule = v > 0.5,
    },
    Knob {
        name: "relative_defect_margin",
        values: &[0.04, 0.07, 0.10, 0.14, 0.20],
        set: |c, v| c.ranking.relative_defect_margin = v,
    },
    Knob {
        name: "relative_occlusion_margin",
        values: &[0.10, 0.15, 0.20, 0.25],
        set: |c, v| c.ranking.relative_occlusion_margin = v,
    },
    Knob {
        name: "sharpness_peak_weight",
        values: &[0.0, 0.25, 0.4, 0.55, 0.7, 0.85],
        set: |c, v| c.ranking.sharpness_peak_weight = v,
    },
    Knob {
        name: "reject_score_delta",
        values: &[0.10, 0.14, 0.18, 0.22, 0.26],
        set: |c, v| c.ranking.reject_score_delta = v,
    },
    Knob {
        name: "eye_focus_weight",
        values: &[0.0, 0.2, 0.35, 0.5, 0.65],
        set: |c, v| c.ranking.eye_focus_weight = v,
    },
    Knob {
        name: "reject_eye_focus_threshold",
        values: &[0.35, 0.40, 0.45, 0.50, 0.55],
        set: |c, v| c.ranking.reject_eye_focus_threshold = v,
    },
    Knob {
        name: "reason_eye_focus_threshold",
        values: &[0.42, 0.47, 0.52, 0.57, 0.62],
        set: |c, v| c.ranking.reason_eye_focus_threshold = v,
    },
    Knob {
        name: "relative_focus_margin",
        values: &[0.03, 0.05, 0.08, 0.11, 0.15],
        set: |c, v| c.ranking.relative_focus_margin = v,
    },
    Knob {
        name: "aesthetic_model_weight",
        values: &[0.0, 0.3, 0.6, 0.9, 1.0],
        set: |c, v| c.ranking.aesthetic_model_weight = v,
    },
    Knob {
        name: "relative_normalization_weight",
        values: &[0.0, 0.25, 0.5, 0.65, 0.8],
        set: |c, v| c.ranking.relative_normalization_weight = v,
    },
    Knob {
        name: "semantic_weight",
        values: &[0.0, 0.15, 0.3, 0.45, 0.6],
        set: |c, v| c.ranking.semantic_weight = v,
    },
    Knob {
        name: "reject_exposure_threshold",
        values: &[0.0, 0.15, 0.22, 0.28, 0.34],
        set: |c, v| c.ranking.reject_exposure_threshold = v,
    },
];

/// One number for `--tune` to compare values by, and only for that: the
/// report itself keeps every measure apart, because they trade against each
/// other. Ordering counts most, a lost must-keep frame costs twice what a
/// found reject is worth, and a wrong reject costs as much as a missed one.
fn tune_objective(r: &EvalReport) -> f64 {
    let order = if r.pairs_labelled > 0 {
        r.pair_accuracy()
    } else {
        r.ndcg
    };
    order + r.winner_accuracy() + 0.5 * r.reject_recall() + 0.5 * r.reject_precision()
        - 2.0 * r.must_keep_reject_rate()
        - 0.5 * r.forbidden_reason_rate()
}

fn print_block(title: &str, reports: &[EvalReport], by_defect: bool) {
    println!("\n=== {title} ===");
    for r in reports {
        println!("{}", r.to_text());
    }
    let all = merge(reports, "ALL FIXTURES");
    if reports.len() > 1 {
        println!("\n{}", all.to_text());
    }
    if by_defect && !all.by_defect.is_empty() {
        println!("\n{}", all.defect_table());
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
    if before.pairs_labelled > 0 {
        row("pair order", before.pair_accuracy(), after.pair_accuracy());
    }
    // Inverted so that "positive is better" holds here too.
    if before.must_keep_labelled > 0 {
        row(
            "must-keep kept",
            1.0 - before.must_keep_reject_rate(),
            1.0 - after.must_keep_reject_rate(),
        );
    }
    if before.reasons_expected > 0 {
        row(
            "reason named",
            before.reason_hit_rate(),
            after.reason_hit_rate(),
        );
    }
    if before.forbid_checked > 0 {
        row(
            "no wrong reason",
            1.0 - before.forbidden_reason_rate(),
            1.0 - after.forbidden_reason_rate(),
        );
    }
}

/// `--tune`: each named knob alone, over its values, on top of each fixture's
/// own preset. Prints the measures per value and marks the best by
/// [`tune_objective`]. A suggestion to look at, not a setting.
fn tune(
    fixtures: &[(PathBuf, EvalFixture)],
    current: &dyn Fn(&EvalFixture) -> CullingConfig,
    names: &str,
) {
    for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let Some(knob) = KNOBS.iter().find(|k| k.name == name) else {
            let known: Vec<&str> = KNOBS.iter().map(|k| k.name).collect();
            panic!(
                "unknown --tune knob {name:?}; expected one of: {}",
                known.join(", ")
            );
        };
        println!("\n=== tune: {name} ===");
        println!(
            "  {:>7}  {:>7} {:>7} {:>7} {:>7} {:>8} {:>7}  {:>7}",
            "value", "top-1", "pairs", "rej-P", "rej-R", "kept-ok", "named", "score"
        );
        let mut best: Option<(f64, f64)> = None;
        let mut rows = Vec::new();
        for &value in knob.values {
            let reports = run(fixtures, |f| {
                let mut cfg = current(f);
                (knob.set)(&mut cfg, value);
                cfg
            });
            let all = merge(&reports, name);
            let score = tune_objective(&all);
            if best.is_none_or(|(_, s)| score > s + 1e-9) {
                best = Some((value, score));
            }
            rows.push((value, all, score));
        }
        for (value, all, score) in rows {
            let mark = if best.is_some_and(|(v, _)| v == value) {
                "  <- best"
            } else {
                ""
            };
            println!(
                "  {:>7.3}  {:>6.1}% {:>6.1}% {:>6.1}% {:>6.1}% {:>7.1}% {:>6.1}%  {:>7.3}{mark}",
                value,
                all.winner_accuracy() * 100.0,
                all.pair_accuracy() * 100.0,
                all.reject_precision() * 100.0,
                all.reject_recall() * 100.0,
                (1.0 - all.must_keep_reject_rate()) * 100.0,
                all.reason_hit_rate() * 100.0,
                score,
            );
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!(
            "usage: cull_eval <fixture.json|fixture-dir> [--preset NAME] \
             [--compare] [--ablate KNOB] [--by-defect] [--tune KNOB[,KNOB…]]\n\n\
             --preset NAME   override each fixture's own culling_preset\n\
             --compare       also run CullingConfig::python_parity and show the delta\n\
             --ablate KNOB   also run with one knob rolled back, and show the delta\n\
             \x20               (sharpness-peak | relative | aesthetic-model | semantic |\n\
             \x20                emotion | usable-gate | kiss-gate | single-rejects | sets |\n\
             \x20                eye-focus)\n\
             --by-defect     a table per labelled defect (the public benchmark)\n\
             --tune KNOBS    sweep each knob alone and show what each value scores:\n\
             \x20               {}",
            KNOBS.iter().map(|k| k.name).collect::<Vec<_>>().join(", ")
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
    let tune_knobs = value_after("--tune");
    let compare = args.iter().any(|a| a == "--compare");
    let by_defect = args.iter().any(|a| a == "--by-defect");

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
    print_block("current", &after, by_defect);
    let after_all = merge(&after, "current");

    if compare {
        let before = run(&fixtures, |_| CullingConfig::python_parity());
        print_block(
            "python_parity (ranking before this pass)",
            &before,
            by_defect,
        );
        print_delta(&merge(&before, "parity"), &after_all);
    }

    if let Some(knob) = ablate_knob {
        let before = run(&fixtures, |f| ablate(current(f), &knob));
        print_block(&format!("ablated: {knob}"), &before, by_defect);
        print_delta(&merge(&before, "ablated"), &after_all);
    }

    if let Some(knobs) = tune_knobs {
        tune(&fixtures, &current, &knobs);
    }
}
