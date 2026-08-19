//! Scoring harness for culling quality.
//!
//! Every signal change so far has been argued from first principles and shipped
//! on the strength of the argument. That is how the aesthetic heuristic
//! survived being contrast plus colourfulness, and how frame-wide sharpness
//! survived punishing shallow depth of field: both are defensible in isolation
//! and wrong in practice, and nothing in the repository could tell the
//! difference. Without a fixture set and a score, "this is an improvement" and
//! "this is a change" are the same sentence.
//!
//! So this module turns a labelled fixture into numbers:
//!
//! - **top-1 winner accuracy** — did the group's winner match the photographer's
//!   pick? The headline number, and the one users actually feel.
//! - **reject precision and recall** — of the frames nominated for deletion, how
//!   many were genuinely unwanted, and how many unwanted frames were found?
//!   Precision matters far more: a missed reject costs a moment of the user's
//!   time, a wrong one costs their trust.
//! - **NDCG** against the labelled ordering — a coarser but more forgiving view
//!   than top-1, which cannot see the difference between "picked the second-best
//!   frame" and "picked the worst".
//! - **set preservation** — the fraction of labelled brackets, focus stacks and
//!   panoramas that came back with no reject candidates. This one should be 1.0;
//!   anything less is a destroyed shot.
//! - **grouping agreement** — pair-counting precision and recall of the
//!   partition against the labelled groups. Ranking accuracy is meaningless if
//!   the groups themselves are wrong, so this frames every number above it.
//!
//! What it deliberately does *not* do is bundle these into one figure of merit.
//! They trade against each other — loosening the reject rules buys recall with
//! precision — and a single number would hide exactly the trade the person
//! tuning needs to see.
//!
//! The fixture format is metrics-level, not pixels: an entry carries the stored
//! `cull_*` metadata for a photo rather than a path to an image. That keeps the
//! harness runnable in CI with no image assets, and keeps it measuring
//! *ranking*, which is what changes. Measuring the metric extraction itself is
//! the goldens' job (`lrg-imaging/tests/goldens.rs`).

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::culling_config::CullingConfig;
use crate::grouping::{group_and_sort_images_with_config, Group, GroupingInput};

/// One labelled photo.
#[derive(Debug, Clone, Deserialize)]
pub struct EvalPhoto {
    pub photo_id: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub capture_time: Option<f64>,
    /// Hex, as stored in `cull_phash`.
    #[serde(default)]
    pub phash: Option<String>,
    #[serde(default)]
    pub embedding: Option<Vec<f32>>,
    /// The `cull_*` fields exactly as `IMAGE_TABLE` holds them.
    #[serde(default)]
    pub metadata: Map<String, Value>,
    /// The photographer's own ranking, 1 = best. Photos left unranked are
    /// scored as ties below every ranked one.
    #[serde(default)]
    pub rank: Option<usize>,
    /// True for frames the photographer would delete.
    #[serde(default)]
    pub reject: bool,
}

/// One labelled group: the frames a human considers one decision.
#[derive(Debug, Clone, Deserialize)]
pub struct EvalGroup {
    pub group_id: String,
    /// `"bracket"`, `"focus_stack"` or `"panorama"` when the group is one
    /// picture across several frames. Present means no frame in it may be
    /// nominated for rejection, and `rank`/`reject` labels are ignored.
    #[serde(default)]
    pub intentional_set: Option<String>,
    pub photos: Vec<EvalPhoto>,
}

/// A fixture file: one shoot, or one genre's worth of groups.
#[derive(Debug, Clone, Deserialize)]
pub struct EvalFixture {
    pub name: String,
    /// Free text — genre, camera, why this fixture exists.
    #[serde(default)]
    pub notes: String,
    /// Preset the fixture is meant to be culled with, e.g. `"portrait"`.
    #[serde(default = "default_preset")]
    pub culling_preset: String,
    #[serde(default)]
    pub time_delta_seconds: Option<i64>,
    /// Whether the group boundaries in this fixture are a human judgement.
    ///
    /// `false` means they were copied from the backend's own output because the
    /// photographer did not stack or otherwise mark them, in which case
    /// comparing the produced partition against them measures nothing — it
    /// would score the grouper against itself and report a perfect 100%. The
    /// grouping counters are simply not accumulated for such a fixture, so the
    /// number is absent rather than flattering.
    ///
    /// Defaults to `false`: a fixture that does not say is assumed derived,
    /// which is the safe direction.
    #[serde(default)]
    pub groups_are_authoritative: bool,
    pub groups: Vec<EvalGroup>,
}

fn default_preset() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvalReport {
    pub fixture: String,
    /// Groups that had a labelled best frame.
    pub ranked_groups: usize,
    pub winner_hits: usize,
    /// Mean NDCG over `ranked_groups`.
    pub ndcg: f64,
    pub rejects_predicted: usize,
    pub rejects_labelled: usize,
    pub rejects_correct: usize,
    pub intentional_sets_labelled: usize,
    /// No frame of the set was nominated for rejection. The harm measure.
    pub intentional_sets_preserved: usize,
    /// The set's frames landed in one group *and* that group was flagged
    /// keep-all. The mechanism measure.
    ///
    /// Tracked separately because "nothing was rejected" is satisfied by a
    /// second, much worse outcome: a grouper that failed to put the frames
    /// together at all leaves each as a singleton, and singletons are never
    /// nominated. Before bracket-aware grouping this repository scored a
    /// perfect 100% preservation for exactly that reason, which is how a metric
    /// ends up certifying the bug it was written to catch. A set that survives
    /// by accident is not protected — widen the burst window and it breaks.
    pub intentional_sets_recognised: usize,
    /// Pair-counting agreement of the produced partition with the labelled one.
    pub grouping_pairs_predicted: usize,
    pub grouping_pairs_labelled: usize,
    pub grouping_pairs_correct: usize,
}

impl EvalReport {
    pub fn winner_accuracy(&self) -> f64 {
        ratio(self.winner_hits, self.ranked_groups)
    }
    pub fn reject_precision(&self) -> f64 {
        ratio(self.rejects_correct, self.rejects_predicted)
    }
    pub fn reject_recall(&self) -> f64 {
        ratio(self.rejects_correct, self.rejects_labelled)
    }
    pub fn set_preservation(&self) -> f64 {
        ratio(
            self.intentional_sets_preserved,
            self.intentional_sets_labelled,
        )
    }
    pub fn set_recognition(&self) -> f64 {
        ratio(
            self.intentional_sets_recognised,
            self.intentional_sets_labelled,
        )
    }
    pub fn grouping_precision(&self) -> f64 {
        ratio(self.grouping_pairs_correct, self.grouping_pairs_predicted)
    }
    pub fn grouping_recall(&self) -> f64 {
        ratio(self.grouping_pairs_correct, self.grouping_pairs_labelled)
    }

    /// A fixed-width block, one fixture per call.
    pub fn to_text(&self) -> String {
        let pct = |v: f64| format!("{:>6.1}%", v * 100.0);
        format!(
            "{name}\n  \
             winner top-1     {winner} ({wh}/{rg})\n  \
             NDCG             {ndcg:>6.3}\n  \
             reject precision {rp} ({rc}/{rpred})\n  \
             reject recall    {rr} ({rc}/{rlab})\n  \
             set preservation {sp} ({sp_n}/{sp_d})\n  \
             set recognition  {sr} ({sr_n}/{sp_d})\n  \
             grouping P/R     {gp} / {gr}",
            name = self.fixture,
            winner = pct(self.winner_accuracy()),
            wh = self.winner_hits,
            rg = self.ranked_groups,
            ndcg = self.ndcg,
            rp = pct(self.reject_precision()),
            rr = pct(self.reject_recall()),
            rc = self.rejects_correct,
            rpred = self.rejects_predicted,
            rlab = self.rejects_labelled,
            sp = pct(self.set_preservation()),
            sp_n = self.intentional_sets_preserved,
            sp_d = self.intentional_sets_labelled,
            sr = pct(self.set_recognition()),
            sr_n = self.intentional_sets_recognised,
            gp = pct(self.grouping_precision()),
            gr = pct(self.grouping_recall()),
        )
    }
}

/// 0 when the denominator is 0, so an empty category reads as "nothing to
/// report" rather than dividing by zero.
fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

/// Sums two reports, so a whole fixture directory can be scored as one corpus.
pub fn merge(reports: &[EvalReport], name: &str) -> EvalReport {
    let mut out = EvalReport {
        fixture: name.to_string(),
        ..Default::default()
    };
    let mut ndcg_sum = 0.0;
    for r in reports {
        out.ranked_groups += r.ranked_groups;
        out.winner_hits += r.winner_hits;
        // Re-weighted by group count so a fixture with three groups does not
        // count as much as one with three hundred.
        ndcg_sum += r.ndcg * r.ranked_groups as f64;
        out.rejects_predicted += r.rejects_predicted;
        out.rejects_labelled += r.rejects_labelled;
        out.rejects_correct += r.rejects_correct;
        out.intentional_sets_labelled += r.intentional_sets_labelled;
        out.intentional_sets_preserved += r.intentional_sets_preserved;
        out.intentional_sets_recognised += r.intentional_sets_recognised;
        out.grouping_pairs_predicted += r.grouping_pairs_predicted;
        out.grouping_pairs_labelled += r.grouping_pairs_labelled;
        out.grouping_pairs_correct += r.grouping_pairs_correct;
    }
    out.ndcg = if out.ranked_groups == 0 {
        0.0
    } else {
        ndcg_sum / out.ranked_groups as f64
    };
    out
}

/// Relevance for NDCG: the best frame is worth the most, unranked frames zero.
///
/// `n + 1 - rank` rather than an exponential gain, because a labelled ordering
/// inside a burst is an *ordering*, not a set of independently meaningful
/// scores — the photographer knows frame 2 beat frame 3, not by how much.
fn relevance(rank: Option<usize>, group_size: usize) -> f64 {
    match rank {
        Some(r) if r >= 1 && r <= group_size => (group_size + 1 - r) as f64,
        _ => 0.0,
    }
}

fn dcg(relevances: &[f64]) -> f64 {
    relevances
        .iter()
        .enumerate()
        .map(|(i, rel)| rel / ((i + 2) as f64).log2())
        .sum()
}

fn ndcg(predicted_order: &[f64]) -> f64 {
    let ideal = {
        let mut v = predicted_order.to_vec();
        v.sort_by(|a, b| b.partial_cmp(a).unwrap());
        dcg(&v)
    };
    if ideal <= 0.0 {
        return 0.0;
    }
    dcg(predicted_order) / ideal
}

fn to_grouping_input(p: &EvalPhoto) -> GroupingInput {
    GroupingInput {
        photo_id: p.photo_id.clone(),
        filename: if p.filename.is_empty() {
            format!("{}.jpg", p.photo_id)
        } else {
            p.filename.clone()
        },
        capture_time: p.capture_time,
        embedding: p.embedding.clone(),
        phash: p
            .phash
            .as_deref()
            .and_then(|s| u64::from_str_radix(s, 16).ok()),
        metadata: p.metadata.clone(),
    }
}

/// Unordered co-membership pairs of a partition, as sorted id pairs.
fn membership_pairs<'a, I>(groups: I) -> HashSet<(&'a str, &'a str)>
where
    I: IntoIterator<Item = Vec<&'a str>>,
{
    let mut pairs = HashSet::new();
    for members in groups {
        for (i, a) in members.iter().enumerate() {
            for b in &members[i + 1..] {
                pairs.insert(if a <= b { (*a, *b) } else { (*b, *a) });
            }
        }
    }
    pairs
}

/// Runs the real grouper over a fixture and scores what comes back.
///
/// Everything is fed through `group_and_sort_images_with_config` rather than
/// through `rank_group_records` directly, so the harness measures the pipeline
/// the user actually gets — including whether grouping put the right frames
/// together in the first place.
pub fn evaluate(fixture: &EvalFixture, cfg: &CullingConfig) -> EvalReport {
    let records: Vec<GroupingInput> = fixture
        .groups
        .iter()
        .flat_map(|g| g.photos.iter().map(to_grouping_input))
        .collect();
    let produced =
        group_and_sort_images_with_config(records, None, None, fixture.time_delta_seconds, cfg);

    let mut report = EvalReport {
        fixture: fixture.name.clone(),
        ..Default::default()
    };

    // --- grouping agreement -------------------------------------------------
    //
    // Skipped entirely for a fixture whose groups came from the backend's own
    // output: scoring the grouper against its own answer reports 100% and means
    // nothing. Leaving the counters at zero makes `grouping_precision()` return
    // 0/0 = 0, and `merge` simply contributes nothing, so the corpus number
    // reflects only the fixtures that can actually speak to it.
    if fixture.groups_are_authoritative {
        let labelled_pairs = membership_pairs(
            fixture
                .groups
                .iter()
                .map(|g| g.photos.iter().map(|p| p.photo_id.as_str()).collect()),
        );
        let predicted_pairs = membership_pairs(
            produced
                .iter()
                .map(|g| g.photo_ids.iter().map(String::as_str).collect()),
        );
        report.grouping_pairs_labelled = labelled_pairs.len();
        report.grouping_pairs_predicted = predicted_pairs.len();
        report.grouping_pairs_correct = predicted_pairs.intersection(&labelled_pairs).count();
    }

    // --- per-photo labels, flattened ---------------------------------------
    let mut label: HashMap<&str, &EvalPhoto> = HashMap::new();
    for group in &fixture.groups {
        for photo in &group.photos {
            label.insert(photo.photo_id.as_str(), photo);
        }
    }
    let produced_by_id: HashMap<&str, &Group> = produced
        .iter()
        .flat_map(|g| g.photo_ids.iter().map(move |id| (id.as_str(), g)))
        .collect();

    // --- ranking, per labelled group ---------------------------------------
    //
    // Scored against the *labelled* grouping, not the produced one: if the
    // grouper split a burst in two, the winner question is still "did the
    // photographer's pick come out on top of the frames it was competing
    // with". Grouping errors are already counted above, and charging them
    // twice would make the ranking numbers unreadable.
    let mut ndcg_sum = 0.0;
    for group in &fixture.groups {
        if group.intentional_set.is_some() {
            report.intentional_sets_labelled += 1;
            let untouched = group.photos.iter().all(|p| {
                produced_by_id
                    .get(p.photo_id.as_str())
                    .is_some_and(|g| !g.reject_candidate_photo_ids.contains(&p.photo_id))
            });
            if untouched {
                report.intentional_sets_preserved += 1;
            }
            // All frames in one produced group, and that group flagged
            // keep-all. See `intentional_sets_recognised` for why "nothing was
            // rejected" is not enough on its own.
            let produced_ids: HashSet<&str> = group
                .photos
                .iter()
                .filter_map(|p| produced_by_id.get(p.photo_id.as_str()))
                .map(|g| g.group_id.as_str())
                .collect();
            let recognised = produced_ids.len() == 1
                && group
                    .photos
                    .first()
                    .and_then(|p| produced_by_id.get(p.photo_id.as_str()))
                    .is_some_and(|g| g.keep_all() && g.photo_ids.len() == group.photos.len());
            if recognised {
                report.intentional_sets_recognised += 1;
            }
            // Rank labels are meaningless for a set: every frame is wanted.
            continue;
        }

        let has_ranking = group.photos.iter().any(|p| p.rank == Some(1));
        if has_ranking && group.photos.len() > 1 {
            report.ranked_groups += 1;
            // The produced score for each labelled member, highest first.
            let mut by_score: Vec<(&EvalPhoto, f64)> = group
                .photos
                .iter()
                .map(|p| {
                    let score = produced_by_id
                        .get(p.photo_id.as_str())
                        .and_then(|g| g.photos.iter().find(|r| r.photo_id == p.photo_id))
                        .map(|r| r.cull_score)
                        .unwrap_or(0.0);
                    (p, score)
                })
                .collect();
            by_score.sort_by(|a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap()
                    .then_with(|| a.0.photo_id.cmp(&b.0.photo_id))
            });
            if by_score[0].0.rank == Some(1) {
                report.winner_hits += 1;
            }
            let order: Vec<f64> = by_score
                .iter()
                .map(|(p, _)| relevance(p.rank, group.photos.len()))
                .collect();
            ndcg_sum += ndcg(&order);
        }
    }
    report.ndcg = if report.ranked_groups == 0 {
        0.0
    } else {
        ndcg_sum / report.ranked_groups as f64
    };

    // --- rejects ------------------------------------------------------------
    for group in &produced {
        for id in &group.reject_candidate_photo_ids {
            report.rejects_predicted += 1;
            if label.get(id.as_str()).is_some_and(|p| p.reject) {
                report.rejects_correct += 1;
            }
        }
    }
    report.rejects_labelled = fixture
        .groups
        .iter()
        .filter(|g| g.intentional_set.is_none())
        .flat_map(|g| g.photos.iter())
        .filter(|p| p.reject)
        .count();

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::culling_config::get_culling_config;
    use serde_json::json;

    fn photo(id: &str, t: f64, sharpness: f64, rank: Option<usize>, reject: bool) -> EvalPhoto {
        EvalPhoto {
            photo_id: id.to_string(),
            filename: format!("{id}.jpg"),
            capture_time: Some(t),
            phash: Some("ffffffffffffffff".to_string()),
            embedding: None,
            metadata: json!({
                "cull_sharpness": sharpness,
                "cull_sharpness_peak": sharpness,
                "cull_exposure": 0.7,
                "cull_noise": 0.1,
            })
            .as_object()
            .unwrap()
            .clone(),
            rank,
            reject,
        }
    }

    #[test]
    fn a_perfectly_ranked_burst_scores_a_clean_sweep() {
        let fixture = EvalFixture {
            name: "sweep".into(),
            notes: String::new(),
            groups_are_authoritative: true,
            culling_preset: "default".into(),
            time_delta_seconds: Some(1),
            groups: vec![EvalGroup {
                group_id: "g1".into(),
                intentional_set: None,
                photos: vec![
                    photo("a", 1000.0, 0.9, Some(1), false),
                    photo("b", 1000.5, 0.6, Some(2), false),
                    photo("c", 1001.0, 0.05, Some(3), true),
                ],
            }],
        };
        let report = evaluate(&fixture, &get_culling_config("default"));
        assert_eq!(report.ranked_groups, 1);
        assert_eq!(report.winner_hits, 1);
        assert!((report.ndcg - 1.0).abs() < 1e-9, "ndcg {}", report.ndcg);
        assert_eq!(report.grouping_pairs_correct, 3, "one group of three");
        assert_eq!(report.reject_precision(), 1.0);
        assert_eq!(report.reject_recall(), 1.0);
    }

    /// The harness has to be able to *fail*, or it measures nothing. Labelling
    /// the softest frame as the photographer's pick must show up as a miss.
    #[test]
    fn a_wrong_winner_is_reported_as_a_miss() {
        let fixture = EvalFixture {
            name: "inverted".into(),
            notes: String::new(),
            groups_are_authoritative: true,
            culling_preset: "default".into(),
            time_delta_seconds: Some(1),
            groups: vec![EvalGroup {
                group_id: "g1".into(),
                intentional_set: None,
                photos: vec![
                    photo("sharp", 1000.0, 0.9, Some(2), false),
                    photo("soft", 1000.5, 0.1, Some(1), false),
                ],
            }],
        };
        let report = evaluate(&fixture, &get_culling_config("default"));
        assert_eq!(report.ranked_groups, 1);
        assert_eq!(report.winner_hits, 0);
        assert!(report.ndcg < 1.0);
    }

    /// The number that must never drop: a labelled bracket coming back with any
    /// reject candidate is a destroyed shot.
    #[test]
    fn a_labelled_bracket_is_preserved() {
        let ev = |id: &str, t: f64, bias: f64, exposure: f64| {
            let mut p = photo(id, t, 0.8, None, false);
            p.metadata.insert("exposure_bias".into(), Value::from(bias));
            p.metadata
                .insert("cull_exposure".into(), Value::from(exposure));
            p
        };
        let fixture = EvalFixture {
            name: "bracket".into(),
            notes: String::new(),
            groups_are_authoritative: true,
            culling_preset: "default".into(),
            time_delta_seconds: Some(1),
            groups: vec![EvalGroup {
                group_id: "g1".into(),
                intentional_set: Some("bracket".into()),
                photos: vec![
                    ev("under", 1000.0, -2.0, 0.15),
                    ev("normal", 1000.5, 0.0, 0.85),
                    ev("over", 1001.0, 2.0, 0.18),
                ],
            }],
        };
        let report = evaluate(&fixture, &get_culling_config("default"));
        assert_eq!(report.intentional_sets_labelled, 1);
        assert_eq!(report.intentional_sets_preserved, 1);
        assert_eq!(report.set_preservation(), 1.0);
        assert_eq!(report.rejects_predicted, 0);
    }

    /// With set detection off, the same bracket is torn apart — which is what
    /// the metric is there to catch, and proves it is not vacuously 1.0.
    #[test]
    fn set_preservation_falls_when_detection_is_disabled() {
        let ev = |id: &str, t: f64, bias: f64, exposure: f64| {
            let mut p = photo(id, t, 0.8, None, false);
            p.metadata.insert("exposure_bias".into(), Value::from(bias));
            p.metadata
                .insert("cull_exposure".into(), Value::from(exposure));
            p
        };
        let fixture = EvalFixture {
            name: "bracket".into(),
            notes: String::new(),
            groups_are_authoritative: true,
            culling_preset: "default".into(),
            time_delta_seconds: Some(1),
            groups: vec![EvalGroup {
                group_id: "g1".into(),
                intentional_set: Some("bracket".into()),
                photos: vec![
                    ev("under", 1000.0, -2.0, 0.15),
                    ev("normal", 1000.5, 0.0, 0.85),
                    ev("over", 1001.0, 2.0, 0.18),
                ],
            }],
        };
        let mut cfg = get_culling_config("default");
        cfg.sets.enabled = false;
        let report = evaluate(&fixture, &cfg);
        assert_eq!(report.set_preservation(), 0.0);
        assert!(report.rejects_predicted > 0);
    }

    /// Two groups the grouper cannot possibly merge: grouping recall must be
    /// perfect and precision must not be inflated by cross-group pairs.
    #[test]
    fn grouping_agreement_counts_pairs_not_groups() {
        let fixture = EvalFixture {
            name: "two".into(),
            notes: String::new(),
            groups_are_authoritative: true,
            culling_preset: "default".into(),
            time_delta_seconds: Some(1),
            groups: vec![
                EvalGroup {
                    group_id: "g1".into(),
                    intentional_set: None,
                    photos: vec![
                        photo("a", 1000.0, 0.9, Some(1), false),
                        photo("b", 1000.5, 0.5, Some(2), false),
                    ],
                },
                EvalGroup {
                    group_id: "g2".into(),
                    intentional_set: None,
                    photos: vec![
                        photo("c", 90000.0, 0.9, Some(1), false),
                        photo("d", 90000.5, 0.5, Some(2), false),
                    ],
                },
            ],
        };
        let report = evaluate(&fixture, &get_culling_config("default"));
        assert_eq!(report.grouping_pairs_labelled, 2);
        assert_eq!(report.grouping_pairs_predicted, 2);
        assert_eq!(report.grouping_pairs_correct, 2);
        assert_eq!(report.grouping_precision(), 1.0);
        assert_eq!(report.grouping_recall(), 1.0);
    }

    #[test]
    fn merging_weights_fixtures_by_group_count() {
        let a = EvalReport {
            fixture: "a".into(),
            ranked_groups: 1,
            winner_hits: 1,
            ndcg: 1.0,
            ..Default::default()
        };
        let b = EvalReport {
            fixture: "b".into(),
            ranked_groups: 3,
            winner_hits: 0,
            ndcg: 0.0,
            ..Default::default()
        };
        let merged = merge(&[a, b], "all");
        assert_eq!(merged.ranked_groups, 4);
        assert_eq!(merged.winner_accuracy(), 0.25);
        assert!((merged.ndcg - 0.25).abs() < 1e-9);
    }

    #[test]
    fn an_empty_category_reports_zero_rather_than_dividing_by_zero() {
        let report = EvalReport::default();
        assert_eq!(report.winner_accuracy(), 0.0);
        assert_eq!(report.reject_precision(), 0.0);
        assert_eq!(report.set_preservation(), 0.0);
        assert!(report.to_text().contains("0.0%"));
    }
}
