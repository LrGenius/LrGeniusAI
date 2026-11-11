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
//!
//! Two kinds of fixture feed it. A hand cull (*Export Culling Fixture* in
//! Lightroom) labels a ranking and the frames to delete. The public benchmark
//! (`scripts/cull_bench.py`, `testdata/cull_bench/`) makes the defects itself on
//! free-licence photographs, so it knows more than a ranking: which frame must
//! survive any cull (`must_keep`), which defect each frame carries (`defect`),
//! and what the explanation should and should not say about it
//! (`expect_reasons`, `forbid_reasons`). Those labels are optional, and a
//! fixture without them scores exactly as before.

use std::collections::{BTreeMap, HashMap, HashSet};

use base64::Engine;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::culling_config::CullingConfig;
use crate::grouping::{group_and_sort_images_with_config, Group, GroupingInput, RankedPhoto};

/// One labelled photo.
#[derive(Debug, Clone, Default, Deserialize)]
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
    /// The embedding as [`quantize_embedding`] packs it, which is how
    /// `/v1/cull/grade` hands it out (`include_embeddings`). Used when
    /// `embedding` is absent.
    ///
    /// Without an embedding the harness groups on pHash and capture time
    /// alone, and a frame two stops brighter or a soft frame of the same burst
    /// then lands in a group of its own: its score is computed against nothing
    /// and no longer says how it compares with its siblings. A full `f32`
    /// vector is 1152 numbers; this is a fifth of the size and moves a cosine
    /// distance by about 1e-4, far inside every grouping threshold.
    #[serde(default)]
    pub embedding_q8: Option<String>,
    /// The `cull_*` fields exactly as `IMAGE_TABLE` holds them.
    #[serde(default)]
    pub metadata: Map<String, Value>,
    /// The photographer's own ranking, 1 = best. Photos left unranked are
    /// scored as ties below every ranked one. Several frames may share a rank:
    /// any of them is a correct winner.
    #[serde(default)]
    pub rank: Option<usize>,
    /// True for frames the photographer would delete.
    #[serde(default)]
    pub reject: bool,
    /// Deleting this frame is a mistake however hard the cull: a shallow depth
    /// of field, a panning shot. Nominating it is counted on its own, as the
    /// error a user never forgives.
    #[serde(default)]
    pub must_keep: bool,
    /// Whether this frame goes is a matter of cull strength, not of a defect: a
    /// stop darker than its twin, noisier than it. A reject candidate here is
    /// neither right nor wrong, so it counts for neither reject precision nor
    /// recall.
    #[serde(default)]
    pub reject_optional: bool,
    /// What is wrong with the frame (`blur_strong`, `eyes_soft`, `panning`…),
    /// for the breakdown by defect. Free text; the benchmark's names are listed
    /// in `testdata/cull_bench/README.md`.
    #[serde(default)]
    pub defect: Option<String>,
    /// Reason codes the explanation should carry. A hit when any one of them
    /// is there, since the same defect may be named more than one way.
    #[serde(default)]
    pub expect_reasons: Vec<String>,
    /// Reason codes that would misdescribe the frame — `blurred` on a frame
    /// whose subject is sharp and only its background is not.
    #[serde(default)]
    pub forbid_reasons: Vec<String>,
}

/// Packs an embedding as base64 of a little-endian `f32` scale followed by one
/// signed byte per dimension, each value ≈ byte × scale.
///
/// One scale per vector, symmetric about zero. For the unit-length SigLIP2
/// embeddings that loses less than 1e-4 of a cosine distance, which is what
/// the grouper compares.
pub fn quantize_embedding(v: &[f32]) -> String {
    let max = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let scale = if max > 0.0 { max / 127.0 } else { 1.0 };
    let mut bytes = Vec::with_capacity(4 + v.len());
    bytes.extend_from_slice(&scale.to_le_bytes());
    bytes.extend(
        v.iter()
            .map(|x| (x / scale).round().clamp(-127.0, 127.0) as i8 as u8),
    );
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The inverse of [`quantize_embedding`]; `None` for anything it did not make.
pub fn dequantize_embedding(packed: &str) -> Option<Vec<f32>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(packed)
        .ok()?;
    if bytes.len() < 5 {
        return None;
    }
    let scale = f32::from_le_bytes(bytes[..4].try_into().ok()?);
    if !scale.is_finite() {
        return None;
    }
    Some(bytes[4..].iter().map(|b| *b as i8 as f32 * scale).collect())
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

/// What happened to the frames of one kind of defect, summed over a fixture.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DefectStats {
    pub photos: usize,
    /// Had the best score of its labelled group: which defect takes the top
    /// spot when the winner is wrong.
    pub won: usize,
    /// Nominated for deletion.
    pub rejected: usize,
    /// Of `photos`, labelled `reject`: the frames this defect should cost.
    pub reject_labelled: usize,
    /// Of `reject_labelled`, nominated. The recall for this defect.
    pub reject_found: usize,
    pub must_keep: usize,
    /// Of `must_keep`, nominated anyway.
    pub must_keep_rejected: usize,
    /// Pairs of this frame against a better-ranked frame of its group, and how
    /// many of them came out in that order (a tie counts half).
    pub pairs: usize,
    pub pairs_correct: f64,
    /// Frames with an `expect_reasons` label, and how many explanations
    /// named one of them.
    pub reasons_expected: usize,
    pub reasons_hit: usize,
    /// Frames with a `forbid_reasons` label, and how many explanations used a
    /// forbidden one anyway.
    pub forbid_checked: usize,
    pub forbid_seen: usize,
}

impl DefectStats {
    fn add(&mut self, other: &DefectStats) {
        self.photos += other.photos;
        self.won += other.won;
        self.rejected += other.rejected;
        self.reject_labelled += other.reject_labelled;
        self.reject_found += other.reject_found;
        self.must_keep += other.must_keep;
        self.must_keep_rejected += other.must_keep_rejected;
        self.pairs += other.pairs;
        self.pairs_correct += other.pairs_correct;
        self.reasons_expected += other.reasons_expected;
        self.reasons_hit += other.reasons_hit;
        self.forbid_checked += other.forbid_checked;
        self.forbid_seen += other.forbid_seen;
    }
}

/// A frame the benchmark's labels say was judged wrong in the way a
/// photographer forgives least, for `cull_eval --mistakes`.
#[derive(Debug, Clone, PartialEq)]
pub struct Mistake {
    pub photo_id: String,
    pub defect: Option<String>,
    /// `"must-keep lost"` or `"wrong reason"`.
    pub kind: &'static str,
    /// What the explanation said.
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvalReport {
    pub fixture: String,
    /// Groups that had a labelled best frame.
    pub ranked_groups: usize,
    pub winner_hits: usize,
    /// Mean NDCG over `ranked_groups`.
    pub ndcg: f64,
    /// Pairs of frames inside one labelled group whose ranks differ, and how
    /// many of them the scores put in the same order (a tie counts half).
    ///
    /// Finer than the winner: it sees every comparison inside a burst, not just
    /// the top one, and it is the measure the published work on photo series
    /// reports (Chang et al. 2016, "Automatic Triage for a Photo Series").
    pub pairs_labelled: usize,
    pub pairs_correct: f64,
    /// Frames labelled `must_keep`, and how many were nominated for deletion.
    pub must_keep_labelled: usize,
    pub must_keep_rejected: usize,
    /// Frames with `expect_reasons`, and how many explanations named one.
    pub reasons_expected: usize,
    pub reasons_hit: usize,
    /// Frames with `forbid_reasons`, and how many explanations used one.
    pub forbid_checked: usize,
    pub forbid_seen: usize,
    /// The same counts per `defect` label, for `cull_eval --by-defect`.
    pub by_defect: BTreeMap<String, DefectStats>,
    /// Every must-keep frame nominated and every forbidden reason given.
    pub mistakes: Vec<Mistake>,
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
    pub fn pair_accuracy(&self) -> f64 {
        ratio_f(self.pairs_correct, self.pairs_labelled)
    }
    /// The share of `must_keep` frames that were nominated for deletion. Lower
    /// is better, and the target is zero.
    pub fn must_keep_reject_rate(&self) -> f64 {
        ratio(self.must_keep_rejected, self.must_keep_labelled)
    }
    pub fn reason_hit_rate(&self) -> f64 {
        ratio(self.reasons_hit, self.reasons_expected)
    }
    /// The share of frames whose explanation used a reason it must not. Lower
    /// is better.
    pub fn forbidden_reason_rate(&self) -> f64 {
        ratio(self.forbid_seen, self.forbid_checked)
    }

    /// A fixed-width block, one fixture per call. The benchmark's lines
    /// (pairs, must-keep, reasons) appear only when the fixture labels them.
    pub fn to_text(&self) -> String {
        let pct = |v: f64| format!("{:>6.1}%", v * 100.0);
        let mut text = format!(
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
        );
        if self.pairs_labelled > 0 {
            text.push_str(&format!(
                "\n  pair order       {} ({:.1}/{})",
                pct(self.pair_accuracy()),
                self.pairs_correct,
                self.pairs_labelled
            ));
        }
        if self.must_keep_labelled > 0 {
            text.push_str(&format!(
                "\n  must-keep lost   {} ({}/{})",
                pct(self.must_keep_reject_rate()),
                self.must_keep_rejected,
                self.must_keep_labelled
            ));
        }
        if self.reasons_expected > 0 {
            text.push_str(&format!(
                "\n  reason named     {} ({}/{})",
                pct(self.reason_hit_rate()),
                self.reasons_hit,
                self.reasons_expected
            ));
        }
        if self.forbid_checked > 0 {
            text.push_str(&format!(
                "\n  wrong reason     {} ({}/{})",
                pct(self.forbidden_reason_rate()),
                self.forbid_seen,
                self.forbid_checked
            ));
        }
        text
    }

    /// One row per `defect` label: how often it was rejected, how often a
    /// must-keep frame of it was lost, how often it ranked below the better
    /// frames of its group, and whether the explanation named it right.
    pub fn defect_table(&self) -> String {
        let cell = |n: f64, d: usize| -> String {
            if d == 0 {
                "     -".to_string()
            } else {
                format!("{:>5.0}%", 100.0 * n / d as f64)
            }
        };
        let mut out = format!(
            "  {:<18}{:>6} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}\n",
            "defect",
            "frames",
            "won",
            "rejected",
            "found",
            "kept-ok",
            "ordered",
            "named",
            "misnamed"
        );
        for (defect, s) in &self.by_defect {
            out.push_str(&format!(
                "  {:<18}{:>6} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}\n",
                defect,
                s.photos,
                s.won,
                cell(s.rejected as f64, s.photos),
                cell(s.reject_found as f64, s.reject_labelled),
                cell((s.must_keep - s.must_keep_rejected) as f64, s.must_keep),
                cell(s.pairs_correct, s.pairs),
                cell(s.reasons_hit as f64, s.reasons_expected),
                cell(s.forbid_seen as f64, s.forbid_checked),
            ));
        }
        out
    }
}

fn ratio_f(numerator: f64, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator / denominator as f64
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
        out.pairs_labelled += r.pairs_labelled;
        out.pairs_correct += r.pairs_correct;
        out.must_keep_labelled += r.must_keep_labelled;
        out.must_keep_rejected += r.must_keep_rejected;
        out.reasons_expected += r.reasons_expected;
        out.reasons_hit += r.reasons_hit;
        out.forbid_checked += r.forbid_checked;
        out.forbid_seen += r.forbid_seen;
        for (defect, stats) in &r.by_defect {
            out.by_defect.entry(defect.clone()).or_default().add(stats);
        }
        out.mistakes.extend(r.mistakes.iter().cloned());
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
        embedding: p
            .embedding
            .clone()
            .or_else(|| p.embedding_q8.as_deref().and_then(dequantize_embedding)),
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
    let ranked_by_id: HashMap<&str, &RankedPhoto> = produced
        .iter()
        .flat_map(|g| g.photos.iter().map(|p| (p.photo_id.as_str(), p)))
        .collect();
    let score_of = |id: &str| ranked_by_id.get(id).map_or(0.0, |r| r.cull_score);

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

        // Every pair whose ranks differ, in the order the labels give. The
        // worse frame's defect gets the credit when the better one is a best
        // frame of the group (rank 1): that is the comparison the defect is
        // about, where a pair of two flawed frames is a matter of degree.
        for (i, a) in group.photos.iter().enumerate() {
            for b in &group.photos[i + 1..] {
                let (Some(ra), Some(rb)) = (a.rank, b.rank) else {
                    continue;
                };
                if ra == rb {
                    continue;
                }
                let (better, worse) = if ra < rb { (a, b) } else { (b, a) };
                let (sb, sw) = (score_of(&better.photo_id), score_of(&worse.photo_id));
                let credit = if sb > sw {
                    1.0
                } else if sb == sw {
                    0.5
                } else {
                    0.0
                };
                report.pairs_labelled += 1;
                report.pairs_correct += credit;
                if better.rank == Some(1) {
                    if let Some(defect) = &worse.defect {
                        let stats = report.by_defect.entry(defect.clone()).or_default();
                        stats.pairs += 1;
                        stats.pairs_correct += credit;
                    }
                }
            }
        }

        let has_ranking = group.photos.iter().any(|p| p.rank == Some(1));
        if has_ranking && group.photos.len() > 1 {
            report.ranked_groups += 1;
            // The produced score for each labelled member, highest first.
            let mut by_score: Vec<(&EvalPhoto, f64)> = group
                .photos
                .iter()
                .map(|p| (p, score_of(&p.photo_id)))
                .collect();
            by_score.sort_by(|a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap()
                    .then_with(|| a.0.photo_id.cmp(&b.0.photo_id))
            });
            if by_score[0].0.rank == Some(1) {
                report.winner_hits += 1;
            }
            if let Some(defect) = &by_score[0].0.defect {
                report.by_defect.entry(defect.clone()).or_default().won += 1;
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
    //
    // A frame whose deletion is a matter of cull strength counts neither way:
    // nominating it is not a mistake, and missing it is not timidity.
    for group in &produced {
        for id in &group.reject_candidate_photo_ids {
            let labelled = label.get(id.as_str());
            if labelled.is_some_and(|p| p.reject_optional) {
                continue;
            }
            report.rejects_predicted += 1;
            if labelled.is_some_and(|p| p.reject) {
                report.rejects_correct += 1;
            }
        }
    }
    report.rejects_labelled = fixture
        .groups
        .iter()
        .filter(|g| g.intentional_set.is_none())
        .flat_map(|g| g.photos.iter())
        .filter(|p| p.reject && !p.reject_optional)
        .count();

    // --- the benchmark's labels, per photo ---------------------------------
    for photo in fixture.groups.iter().flat_map(|g| g.photos.iter()) {
        let produced = ranked_by_id.get(photo.photo_id.as_str());
        let rejected = produced.is_some_and(|r| r.reject_candidate);
        let reasons: &[String] = produced.map_or(&[], |r| r.reason_codes.as_slice());
        let named = !photo.expect_reasons.is_empty()
            && photo.expect_reasons.iter().any(|c| reasons.contains(c));
        let misnamed = photo.forbid_reasons.iter().any(|c| reasons.contains(c));

        if photo.must_keep {
            report.must_keep_labelled += 1;
            report.must_keep_rejected += usize::from(rejected);
        }
        for (wrong, kind) in [
            (photo.must_keep && rejected, "must-keep lost"),
            (misnamed, "wrong reason"),
        ] {
            if wrong {
                report.mistakes.push(Mistake {
                    photo_id: photo.photo_id.clone(),
                    defect: photo.defect.clone(),
                    kind,
                    reasons: reasons.to_vec(),
                });
            }
        }
        if !photo.expect_reasons.is_empty() {
            report.reasons_expected += 1;
            report.reasons_hit += usize::from(named);
        }
        if !photo.forbid_reasons.is_empty() {
            report.forbid_checked += 1;
            report.forbid_seen += usize::from(misnamed);
        }

        let Some(defect) = &photo.defect else {
            continue;
        };
        let stats = report.by_defect.entry(defect.clone()).or_default();
        stats.photos += 1;
        stats.rejected += usize::from(rejected);
        if photo.reject && !photo.reject_optional {
            stats.reject_labelled += 1;
            stats.reject_found += usize::from(rejected);
        }
        if photo.must_keep {
            stats.must_keep += 1;
            stats.must_keep_rejected += usize::from(rejected);
        }
        if !photo.expect_reasons.is_empty() {
            stats.reasons_expected += 1;
            stats.reasons_hit += usize::from(named);
        }
        if !photo.forbid_reasons.is_empty() {
            stats.forbid_checked += 1;
            stats.forbid_seen += usize::from(misnamed);
        }
    }

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
            ..Default::default()
        }
    }

    #[test]
    fn a_quantized_embedding_keeps_cosine_distances() {
        let a: Vec<f32> = (0..1152)
            .map(|i| ((i * 37 % 101) as f32 - 50.0) / 900.0)
            .collect();
        let b: Vec<f32> = a
            .iter()
            .enumerate()
            .map(|(i, x)| x + (i % 7) as f32 / 4000.0)
            .collect();
        let cosine = |x: &[f32], y: &[f32]| {
            let dot: f32 = x.iter().zip(y).map(|(p, q)| p * q).sum();
            let n = |v: &[f32]| v.iter().map(|p| p * p).sum::<f32>().sqrt();
            1.0 - dot / (n(x) * n(y))
        };
        let qa = dequantize_embedding(&quantize_embedding(&a)).unwrap();
        let qb = dequantize_embedding(&quantize_embedding(&b)).unwrap();
        assert_eq!(qa.len(), 1152);
        assert!((cosine(&a, &b) - cosine(&qa, &qb)).abs() < 1e-3);
        assert!(dequantize_embedding("not base64!").is_none());
        assert!(dequantize_embedding("").is_none());
    }

    /// A must-keep frame that is nominated, a reason that is named and one
    /// that is forbidden all land in their own counters, overall and per
    /// defect, and a frame whose deletion is optional counts for nothing.
    #[test]
    fn the_benchmark_labels_are_counted_per_defect() {
        let mut sharp = photo("sharp", 1000.0, 0.9, Some(1), false);
        sharp.defect = Some("original".into());
        sharp.forbid_reasons = vec!["blurred".into()];
        let mut soft = photo("soft", 1000.3, 0.05, Some(3), true);
        soft.defect = Some("blur_strong".into());
        soft.expect_reasons = vec!["blurred".into(), "motion_blur".into()];
        let mut keep = photo("keep", 1000.6, 0.05, Some(1), false);
        keep.defect = Some("background_soft".into());
        keep.must_keep = true;
        keep.forbid_reasons = vec!["blurred".into()];
        let mut optional = photo("dark", 1000.9, 0.88, Some(2), false);
        optional
            .metadata
            .insert("cull_exposure".into(), Value::from(0.1));
        optional.reject_optional = true;
        optional.defect = Some("under".into());
        let fixture = EvalFixture {
            name: "labels".into(),
            notes: String::new(),
            groups_are_authoritative: true,
            culling_preset: "default".into(),
            time_delta_seconds: Some(1),
            groups: vec![EvalGroup {
                group_id: "g1".into(),
                intentional_set: None,
                photos: vec![sharp, soft, keep, optional],
            }],
        };
        let report = evaluate(&fixture, &get_culling_config("default"));
        // The background-soft frame reads as soft frame-wide here, which is
        // exactly the mistake must_keep exists to count.
        assert_eq!(report.must_keep_labelled, 1);
        assert_eq!(report.must_keep_rejected, 1);
        assert_eq!(report.forbid_checked, 2);
        assert_eq!(
            report.forbid_seen, 1,
            "only the background-soft frame is misnamed"
        );
        assert_eq!(report.reasons_expected, 1);
        assert_eq!(report.reasons_hit, 1);
        let soft = &report.by_defect["blur_strong"];
        assert_eq!((soft.reject_labelled, soft.reject_found), (1, 1));
        assert_eq!(soft.pairs, 2, "against both rank-1 frames");
        assert!(report.pairs_labelled >= 4);
        // The darker frame's fate is a matter of cull strength: whatever
        // happened to it, it is not among the predicted or labelled rejects.
        assert_eq!(report.rejects_labelled, 1);
        assert!(!report.to_text().is_empty());
        assert!(report.defect_table().contains("background_soft"));
        let merged = merge(&[report.clone(), report.clone()], "twice");
        assert_eq!(merged.by_defect["blur_strong"].pairs, 4);
        assert_eq!(merged.must_keep_rejected, 2);
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
