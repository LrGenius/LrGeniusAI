//! Runs the evaluation harness over `testdata/cull_eval` in CI.
//!
//! The fixtures are synthetic and encode the failure modes the signal-quality
//! pass fixed (see `testdata/cull_eval/README.md`), so this is a regression
//! guard, not a quality benchmark: passing means those bugs have not come back,
//! and says nothing about accuracy on real photographs.
//!
//! Thresholds are floors rather than exact values on purpose. Pinning the
//! numbers would mean every deliberate tuning change breaks a test that has no
//! opinion about whether the change was good, which is how the old golden
//! became a brake instead of a check.

use lrg_analysis::culling_config::{get_culling_config, CullingConfig};
use lrg_analysis::eval::{evaluate, merge, EvalFixture, EvalReport};

fn fixtures() -> Vec<EvalFixture> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/cull_eval");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .expect("testdata/cull_eval must exist")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no fixtures found in {dir}");
    paths
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap();
            serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{} is not a valid fixture: {e}", p.display()))
        })
        .collect()
}

fn score(cfg: impl Fn(&EvalFixture) -> CullingConfig) -> EvalReport {
    let all: Vec<EvalReport> = fixtures().iter().map(|f| evaluate(f, &cfg(f))).collect();
    merge(&all, "all")
}

fn current() -> EvalReport {
    score(|f| get_culling_config(&f.culling_preset))
}

/// The one number with no acceptable trade-off attached. A bracket or focus
/// stack losing frames is a destroyed photograph, and no gain elsewhere buys it
/// back.
#[test]
fn no_intentional_set_ever_loses_a_frame() {
    let report = current();
    assert!(
        report.intentional_sets_labelled > 0,
        "fixtures must contain at least one labelled set for this to mean anything"
    );
    assert_eq!(
        report.set_preservation(),
        1.0,
        "{} of {} labelled sets had frames nominated for rejection",
        report.intentional_sets_labelled - report.intentional_sets_preserved,
        report.intentional_sets_labelled
    );
    // ...and survived on purpose, not because the grouper split them apart.
    assert_eq!(
        report.set_recognition(),
        1.0,
        "{} of {} labelled sets were not recognised as sets",
        report.intentional_sets_labelled - report.intentional_sets_recognised,
        report.intentional_sets_labelled
    );
}

#[test]
fn the_photographers_pick_wins_every_labelled_group() {
    let report = current();
    assert!(
        report.ranked_groups >= 5,
        "expected a handful of ranked groups, got {}",
        report.ranked_groups
    );
    assert_eq!(
        report.winner_accuracy(),
        1.0,
        "winner accuracy {:.1}% ({}/{})",
        report.winner_accuracy() * 100.0,
        report.winner_hits,
        report.ranked_groups
    );
}

/// Grouping errors invalidate every ranking number above them, so they are
/// checked separately rather than folded into the accuracy assertions.
#[test]
fn the_grouper_reproduces_the_labelled_partition() {
    let report = current();
    assert_eq!(report.grouping_recall(), 1.0, "grouping recall");
    assert_eq!(report.grouping_precision(), 1.0, "grouping precision");
}

/// Guards the harness itself. If ranking as it stood before this pass scored
/// the same as ranking now, the fixtures are not exercising anything and every
/// assertion above is vacuous.
#[test]
fn the_fixtures_can_tell_the_old_ranking_apart_from_the_new() {
    let before = score(|_| CullingConfig::python_parity());
    let after = current();
    assert!(
        after.winner_accuracy() > before.winner_accuracy(),
        "winner accuracy did not improve: {:.3} -> {:.3}",
        before.winner_accuracy(),
        after.winner_accuracy()
    );
    // Recognition, not preservation: the old code scored a *perfect*
    // preservation on these fixtures, because it failed to group the bracket
    // frames at all and singletons are never nominated for rejection. That is
    // the metric certifying the bug it exists to catch, which is why the two
    // numbers are tracked separately.
    assert!(
        after.set_recognition() > before.set_recognition(),
        "set recognition did not improve: {:.3} -> {:.3}",
        before.set_recognition(),
        after.set_recognition()
    );
    assert!(
        after.reject_precision() > before.reject_precision(),
        "reject precision did not improve: {:.3} -> {:.3}",
        before.reject_precision(),
        after.reject_precision()
    );
}

/// Each knob is checked to be load-bearing on its own, so a future refactor
/// that quietly disables one shows up here rather than in a user's catalog.
#[test]
fn each_new_signal_is_load_bearing() {
    let after = current();

    let without_peak = score(|f| {
        let mut c = get_culling_config(&f.culling_preset);
        c.ranking.sharpness_peak_weight = 0.0;
        c.ranking.recompute_technical_score = false;
        c
    });
    assert!(
        without_peak.winner_accuracy() < after.winner_accuracy(),
        "peak sharpness changed nothing: {:.3}",
        without_peak.winner_accuracy()
    );

    let without_sets = score(|f| {
        let mut c = get_culling_config(&f.culling_preset);
        c.sets.enabled = false;
        c
    });
    assert!(
        without_sets.set_recognition() < after.set_recognition(),
        "set detection changed nothing: {:.3}",
        without_sets.set_recognition()
    );

    let without_bracket_edges = score(|f| {
        let mut c = get_culling_config(&f.culling_preset);
        c.grouping.bracket_edges = false;
        c
    });
    assert!(
        without_bracket_edges.set_recognition() < after.set_recognition(),
        "bracket grouping edges changed nothing: {:.3}",
        without_bracket_edges.set_recognition()
    );
}
