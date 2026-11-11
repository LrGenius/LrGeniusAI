//! Runs the public culling benchmark (`testdata/cull_bench`) in CI.
//!
//! The fixtures hold what the backend measured on free-licence photographs
//! with defects made on purpose (see `testdata/cull_bench/README.md`), so the
//! right answer is known without a hand cull. Unlike `cull_eval_fixtures.rs`
//! this is a benchmark, not a regression list: the numbers are far from
//! perfect, and the floors below are where ranking stands today. A change may
//! raise them and should then raise the floor with it; one that lowers a
//! number has to say in its own words why that is the right trade.
//!
//! What it cannot see: taste and the moment (every frame of a series shows the
//! same moment), and anything that needs pixels, which only re-running
//! `scripts/cull_bench.py run` measures.

use lrg_analysis::culling_config::get_culling_config;
use lrg_analysis::eval::{
    evaluate, evaluate_by_preset, evaluate_selection, merge, EvalFixture, EvalReport,
    SelectionReport,
};
use lrg_analysis::selection::Target;

fn fixtures() -> Vec<EvalFixture> {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/cull_bench/fixtures"
    );
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .expect("testdata/cull_bench/fixtures must exist")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    assert!(paths.len() >= 5, "one fixture per genre expected in {dir}");
    paths
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap();
            serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{} is not a valid fixture: {e}", p.display()))
        })
        .collect()
}

fn current() -> EvalReport {
    let all: Vec<EvalReport> = fixtures()
        .iter()
        .map(|f| evaluate(f, &get_culling_config(&f.culling_preset)))
        .collect();
    merge(&all, "bench")
}

/// The series are made to be grouped: each must come back as one group, or
/// every number below compares the wrong frames.
#[test]
fn every_series_is_one_group() {
    let r = current();
    assert_eq!(r.grouping_recall(), 1.0, "grouping recall");
    assert_eq!(r.grouping_precision(), 1.0, "grouping precision");
}

/// Floors at today's level; see the module comment for what to do when a
/// change moves one.
#[test]
fn ranking_holds_its_level_on_the_benchmark() {
    let r = current();
    let floor = |name: &str, value: f64, min: f64| {
        assert!(
            value >= min,
            "{name} fell to {:.1}% (floor {:.1}%)",
            value * 100.0,
            min * 100.0
        );
    };
    floor("winner top-1", r.winner_accuracy(), 0.73);
    floor("pair order", r.pair_accuracy(), 0.85);
    floor("reject precision", r.reject_precision(), 0.92);
    floor("reject recall", r.reject_recall(), 0.70);
    assert!(
        r.must_keep_reject_rate() <= 0.10,
        "{:.1}% of the frames that must survive any cull were nominated (ceiling 10%)",
        r.must_keep_reject_rate() * 100.0
    );
    assert!(
        r.forbidden_reason_rate() <= 0.02,
        "{:.1}% of the frames were misnamed — a sharp subject called blurred (ceiling 2%)",
        r.forbidden_reason_rate() * 100.0
    );
}

/// The defects the benchmark makes most clearly: a frame blurred all over, a
/// frame whose eyes missed focus and one whose focus missed the subject must
/// be found as often as today, and the last named for what it is.
#[test]
fn the_clear_defects_are_found() {
    let r = current();
    let found = |defect: &str| {
        let s = &r.by_defect[defect];
        s.reject_found as f64 / s.reject_labelled.max(1) as f64
    };
    let named = |defect: &str| {
        let s = &r.by_defect[defect];
        s.reasons_hit as f64 / s.reasons_expected.max(1) as f64
    };
    assert!(
        found("subject_soft") >= 0.7,
        "subject_soft {:.2}",
        found("subject_soft")
    );
    assert!(
        named("subject_soft") >= 0.8,
        "subject_soft named {:.2}",
        named("subject_soft")
    );
    assert!(
        found("blur_strong") >= 0.7,
        "blur_strong {:.2}",
        found("blur_strong")
    );
    assert!(
        found("eyes_soft") >= 0.8,
        "eyes_soft {:.2}",
        found("eyes_soft")
    );
}

/// A copy at about ISO 12 800 used to take the series 58 times in 120: grain
/// reads as detail. It must stay the exception.
#[test]
fn the_noisy_twin_does_not_win() {
    let r = current();
    let won = r.by_defect["noise"].won;
    assert!(won <= 5, "the noisy copy won {won} series");
}

/// `culling_preset: "auto"` chooses each series' preset from its photos'
/// genre. Not knowing the genre a series was picked for, it must rank at least
/// as well as the preset chosen by hand for it.
#[test]
fn auto_ranks_as_well_as_the_preset_chosen_by_hand() {
    let fixtures = fixtures();
    assert!(
        fixtures.iter().all(|f| f
            .groups
            .iter()
            .flat_map(|g| &g.photos)
            .all(|p| p.metadata.get("genre").is_some())),
        "every frame of the benchmark carries the genre the backend read"
    );
    let auto = merge(
        &fixtures
            .iter()
            .map(|f| evaluate_by_preset(f, "auto", get_culling_config))
            .collect::<Vec<_>>(),
        "auto",
    );
    let by_hand = current();
    let at_least = |name: &str, auto: f64, by_hand: f64| {
        assert!(
            auto + 1e-9 >= by_hand,
            "{name}: auto {:.1}% below the preset chosen by hand ({:.1}%)",
            auto * 100.0,
            by_hand * 100.0
        );
    };
    at_least(
        "winner top-1",
        auto.winner_accuracy(),
        by_hand.winner_accuracy(),
    );
    at_least("pair order", auto.pair_accuracy(), by_hand.pair_accuracy());
    at_least(
        "reject precision",
        auto.reject_precision(),
        by_hand.reject_precision(),
    );
    at_least(
        "reject recall",
        auto.reject_recall(),
        by_hand.reject_recall(),
    );
    at_least(
        "must-keep kept",
        1.0 - auto.must_keep_reject_rate(),
        1.0 - by_hand.must_keep_reject_rate(),
    );
}

/// A target of one photo per series: every series is in it (each is a scene
/// of its own, an hour from the next), mostly with its best frame, and almost
/// never with a labelled reject.
#[test]
fn a_target_of_one_per_series_covers_every_series() {
    let mut all = SelectionReport::default();
    for f in &fixtures() {
        let target = Target::Count(f.groups.len());
        all.add(&evaluate_selection(
            f,
            "auto",
            get_culling_config,
            target,
            0,
        ));
    }
    assert_eq!(all.series_covered, all.series, "{all:?}");
    let share = |a: usize| a as f64 / all.selected.max(1) as f64;
    assert!(
        share(all.best) >= 0.76,
        "best of its series {:.3}",
        share(all.best)
    );
    assert!(
        share(all.rejects) <= 0.01,
        "rejects {:.3}",
        share(all.rejects)
    );
}
