//! Learning a photographer's taste from their own culls.
//!
//! A Lightroom catalog holds years of the photographer's decisions: which frame
//! of a burst got the flag, which got three stars, which was rejected. No
//! stand-alone culling tool can read them; this plugin can. From them it learns
//! how much each signal ranking measures — sharpness, eyes, smile, the moment,
//! the aesthetic score — counts *for this photographer*, and reorders the
//! frames of a burst by it.
//!
//! - **Decisions**: inside a group of frames the photographer culled (at least
//!   one mark of theirs, not one the cull wrote), a frame flagged pick beats
//!   one unflagged, which beats one rejected; with equal flags, more stars win.
//!   Each such pair is one decision.
//! - **Model**: a pairwise logistic regression on the differences of the
//!   signals between the two frames, each scaled by its spread — no bias, so it
//!   only ever compares. The default ranking's own score is one of the signals,
//!   so the model learns a correction to it rather than starting over.
//! - **Proof**: five-fold cross-validation by group, against the default
//!   ranking on the same held-out pairs. The model is used only where it agrees
//!   with the photographer more often than the default does, and only from
//!   [`MIN_PAIRS`] decisions.
//! - **Use**: it reorders the frames that are not reject candidates; a reject
//!   stays a reject, since a blink or a blur is not a matter of taste.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::grouping::{Group, RankedPhoto};

/// The signals the model weighs, by their key in a ranked photo's `metrics`,
/// plus `cull_score`, the default ranking itself.
pub const FEATURES: &[&str] = &[
    "cull_score",
    "sharpness_effective",
    "exposure",
    "highlight_clip",
    "noise",
    "aesthetic",
    "face_score",
    "eye_openness",
    "eye_focus",
    "eye_closed",
    "smile",
    "gaze_off_lens",
    "mouth_open",
    "moment",
    "subject_focus",
];

/// Fewer decisions than this teach nothing that cross-validation could tell
/// from chance.
pub const MIN_PAIRS: usize = 30;

/// Bump when [`FEATURES`] or the way they are read changes: a stored model is
/// only comparable with the features it was trained on.
pub const MODEL_VERSION: u32 = 1;

/// What the photographer decided about one photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct Label {
    /// `1` pick, `0` none, `-1` reject.
    pub flag: i8,
    /// Stars, `0..=5`.
    pub rating: u8,
    /// The mark is the photographer's: not one a cull wrote and left.
    pub own: bool,
}

impl Label {
    fn order(&self) -> (i8, u8) {
        (self.flag, self.rating)
    }
}

/// A learned taste.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TasteModel {
    pub version: u32,
    pub features: Vec<String>,
    pub weights: Vec<f64>,
    /// Each feature's spread over the training photos; differences are
    /// divided by it.
    pub scales: Vec<f64>,
    /// Decisions it was learned from, and the groups they came from.
    pub pairs: usize,
    pub groups: usize,
    /// Cross-validated: how often it agrees with the photographer, and how
    /// often the default ranking does, on the same held-out decisions.
    pub agreement: f64,
    pub default_agreement: f64,
}

impl TasteModel {
    /// Whether it earns its place: enough decisions, and more agreement than
    /// the default ranking.
    pub fn useful(&self) -> bool {
        self.version == MODEL_VERSION
            && self.pairs >= MIN_PAIRS
            && self.agreement > self.default_agreement
    }

    fn score(&self, f: &[Option<f64>], fill: &[f64]) -> f64 {
        self.weights
            .iter()
            .zip(&self.scales)
            .enumerate()
            .map(|(k, (w, s))| w * f[k].unwrap_or(fill[k]) / s)
            .sum()
    }
}

/// Why nothing was learned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LearnError {
    /// Fewer than [`MIN_PAIRS`] decisions in the photos given.
    TooFewDecisions(usize),
}

/// A ranked photo's signals, in [`FEATURES`] order.
pub fn features_of(photo: &RankedPhoto) -> Vec<Option<f64>> {
    FEATURES
        .iter()
        .map(|&key| {
            if key == "cull_score" {
                Some(photo.cull_score)
            } else {
                photo.metrics.get(key).and_then(Value::as_f64)
            }
        })
        .collect()
}

/// One decision: the photo preferred and the one it was preferred to, with
/// the group they share.
struct Pair {
    group: usize,
    better: Vec<Option<f64>>,
    worse: Vec<Option<f64>>,
    default_says: f64,
}

fn decisions(groups: &[Group], labels: &HashMap<String, Label>) -> Vec<Pair> {
    let mut out = Vec::new();
    for (g, group) in groups.iter().enumerate() {
        if group.keep_all() {
            continue;
        }
        let labelled: Vec<(&RankedPhoto, Label)> = group
            .photos
            .iter()
            .filter_map(|p| labels.get(&p.photo_id).map(|l| (p, *l)))
            .collect();
        // A group counts only where the photographer made a mark of their own:
        // a cull's pick left untouched says nothing about taste.
        if !labelled.iter().any(|(_, l)| l.own) {
            continue;
        }
        for (i, (a, la)) in labelled.iter().enumerate() {
            for (b, lb) in &labelled[i + 1..] {
                let (better, worse) = match la.order().cmp(&lb.order()) {
                    std::cmp::Ordering::Greater => (a, b),
                    std::cmp::Ordering::Less => (b, a),
                    std::cmp::Ordering::Equal => continue,
                };
                out.push(Pair {
                    group: g,
                    better: features_of(better),
                    worse: features_of(worse),
                    default_says: (better.cull_score - worse.cull_score).signum(),
                });
            }
        }
    }
    out
}

fn difference(p: &Pair, scales: &[f64]) -> Vec<f64> {
    p.better
        .iter()
        .zip(&p.worse)
        .zip(scales)
        .map(|((a, b), s)| match (a, b) {
            (Some(a), Some(b)) => (a - b) / s,
            _ => 0.0,
        })
        .collect()
}

/// Gradient descent on the regularised pairwise logistic loss.
fn fit(diffs: &[Vec<f64>]) -> Vec<f64> {
    const STEPS: usize = 400;
    const RATE: f64 = 0.5;
    const L2: f64 = 0.01;
    let k = FEATURES.len();
    let mut w = vec![0.0f64; k];
    if diffs.is_empty() {
        return w;
    }
    let n = diffs.len() as f64;
    for _ in 0..STEPS {
        let mut grad: Vec<f64> = w.iter().map(|wi| L2 * wi).collect();
        for d in diffs {
            let z: f64 = w.iter().zip(d).map(|(a, b)| a * b).sum();
            // d/dw of log(1 + e^-z) is -d * sigmoid(-z).
            let s = 1.0 / (1.0 + z.exp());
            for (g, x) in grad.iter_mut().zip(d) {
                *g -= s * x / n;
            }
        }
        for (wi, g) in w.iter_mut().zip(&grad) {
            *wi -= RATE * g;
        }
    }
    w
}

fn agrees(score: f64) -> f64 {
    if score > 0.0 {
        1.0
    } else if score == 0.0 {
        0.5
    } else {
        0.0
    }
}

/// Learns the photographer's taste from their decisions in `groups` (ranked
/// as the cull would rank them) and `labels`, by photo id.
pub fn learn(groups: &[Group], labels: &HashMap<String, Label>) -> Result<TasteModel, LearnError> {
    let pairs = decisions(groups, labels);
    if pairs.len() < MIN_PAIRS {
        return Err(LearnError::TooFewDecisions(pairs.len()));
    }
    // Spread per feature over the photos in the decisions.
    let k = FEATURES.len();
    let scales: Vec<f64> = (0..k)
        .map(|j| {
            let values: Vec<f64> = pairs
                .iter()
                .flat_map(|p| [p.better[j], p.worse[j]])
                .flatten()
                .collect();
            if values.len() < 2 {
                return 1.0;
            }
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
            var.sqrt().max(1e-3)
        })
        .collect();
    let diffs: Vec<Vec<f64>> = pairs.iter().map(|p| difference(p, &scales)).collect();

    // Five folds by group, so a burst's decisions are never on both sides.
    const FOLDS: usize = 5;
    let (mut model_hits, mut default_hits) = (0.0, 0.0);
    for fold in 0..FOLDS {
        let train: Vec<Vec<f64>> = pairs
            .iter()
            .zip(&diffs)
            .filter(|(p, _)| p.group % FOLDS != fold)
            .map(|(_, d)| d.clone())
            .collect();
        let w = fit(&train);
        for (p, d) in pairs
            .iter()
            .zip(&diffs)
            .filter(|(p, _)| p.group % FOLDS == fold)
        {
            model_hits += agrees(w.iter().zip(d).map(|(a, b)| a * b).sum());
            default_hits += agrees(p.default_says);
        }
    }
    let n = pairs.len() as f64;
    let mut groups_seen: Vec<usize> = pairs.iter().map(|p| p.group).collect();
    groups_seen.sort_unstable();
    groups_seen.dedup();
    Ok(TasteModel {
        version: MODEL_VERSION,
        features: FEATURES.iter().map(|s| s.to_string()).collect(),
        weights: fit(&diffs),
        scales,
        pairs: pairs.len(),
        groups: groups_seen.len(),
        agreement: model_hits / n,
        default_agreement: default_hits / n,
    })
}

/// Reorders the frames of each group that are not reject candidates by the
/// learned taste; reject candidates keep their places after them. Where it
/// changes a group's pick, the new pick says so. Returns how many picks
/// changed.
pub fn apply(groups: &mut [Group], model: &TasteModel) -> usize {
    let mut changed = 0;
    for group in groups.iter_mut().filter(|g| !g.keep_all()) {
        let features: Vec<Vec<Option<f64>>> = group.photos.iter().map(features_of).collect();
        // A signal a frame lacks counts as the group's mean, so it neither
        // helps nor hurts that frame.
        let fill: Vec<f64> = (0..FEATURES.len())
            .map(|k| {
                let v: Vec<f64> = features.iter().filter_map(|f| f[k]).collect();
                if v.is_empty() {
                    0.0
                } else {
                    v.iter().sum::<f64>() / v.len() as f64
                }
            })
            .collect();
        let scores: Vec<f64> = features.iter().map(|f| model.score(f, &fill)).collect();
        let before = group.winner_photo_id.clone();

        let mut order: Vec<usize> = (0..group.photos.len()).collect();
        order.sort_by(|&a, &b| {
            let (pa, pb) = (&group.photos[a], &group.photos[b]);
            pa.reject_candidate.cmp(&pb.reject_candidate).then_with(|| {
                if pa.reject_candidate {
                    pa.rank.cmp(&pb.rank)
                } else {
                    scores[b].total_cmp(&scores[a]).then(pa.rank.cmp(&pb.rank))
                }
            })
        });
        let mut photos: Vec<RankedPhoto> = order.iter().map(|&i| group.photos[i].clone()).collect();
        for (place, (photo, &i)) in photos.iter_mut().zip(&order).enumerate() {
            photo.rank = place + 1;
            photo.winner = false;
            photo
                .metrics
                .insert("taste".into(), Value::from((scores[i] * 1e4).round() / 1e4));
        }
        if let Some(first) = photos.iter_mut().find(|p| !p.reject_candidate) {
            first.winner = true;
            group.winner_photo_id = Some(first.photo_id.clone());
            if before.as_deref() != Some(first.photo_id.as_str()) {
                changed += 1;
                first.reason_codes.push("preferred_by_your_style".into());
                first
                    .explanation
                    .push_str("; your culling style prefers it");
            }
        }
        group.alternate_photo_ids = photos
            .iter()
            .filter(|p| !p.winner && !p.reject_candidate)
            .map(|p| p.photo_id.clone())
            .collect();
        group.photos = photos;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grouping::{group_and_sort_images, GroupingInput};
    use serde_json::Map;

    /// A burst of four, one per hour: the sharpest frame and the broadest
    /// smile are never the same one.
    fn bursts(n: usize) -> Vec<GroupingInput> {
        let mut out = Vec::new();
        for b in 0..n {
            for i in 0..4 {
                let mut metadata = Map::new();
                let sharp = 0.9 - 0.1 * i as f64;
                let smile = 0.2 + 0.2 * i as f64;
                for (k, v) in [
                    ("cull_sharpness", sharp),
                    ("cull_sharpness_peak", sharp),
                    ("cull_exposure", 0.7),
                    ("cull_noise", 0.1),
                    ("cull_face_score", 0.6),
                    ("cull_face_sharpness", 0.6),
                    ("cull_eye_openness", 0.9),
                    ("cull_blink_penalty", 0.1),
                    ("cull_smile", smile),
                    ("cull_eye_closed", 0.05),
                ] {
                    metadata.insert(k.into(), Value::from(v));
                }
                metadata.insert("cull_face_count".into(), Value::from(1));
                metadata.insert("cull_faces_present".into(), Value::from(true));
                metadata.insert(
                    crate::grouping::FACE_STATE_VERSION_KEY.into(),
                    Value::from(crate::grouping::FACE_STATE_VERSION),
                );
                out.push(GroupingInput {
                    photo_id: format!("b{b}_{i}"),
                    filename: format!("b{b}_{i}.jpg"),
                    capture_time: Some(b as f64 * 3600.0 + i as f64),
                    embedding: None,
                    phash: Some(0xffff_0000_ffff_0000 ^ b as u64),
                    metadata,
                });
            }
        }
        out
    }

    /// A photographer who always flags the broadest smile, where the default
    /// ranking picks the sharpest frame: learned, proven better on held-out
    /// bursts, and used, it moves the pick.
    #[test]
    fn a_taste_for_smiles_is_learned_and_moves_the_pick() {
        let mut groups = group_and_sort_images(bursts(15), None, None, Some(2), "default");
        assert_eq!(groups.len(), 15);
        let mut labels = HashMap::new();
        for b in 0..15 {
            for i in 0..4 {
                labels.insert(
                    format!("b{b}_{i}"),
                    Label {
                        flag: i8::from(i == 3),
                        rating: i as u8,
                        own: true,
                    },
                );
            }
        }
        let model = learn(&groups, &labels).expect("enough decisions");
        assert_eq!(model.pairs, 15 * 6);
        assert!(model.agreement > model.default_agreement, "{model:?}");
        assert!(model.useful());
        let smile = FEATURES.iter().position(|f| *f == "smile").unwrap();
        assert!(model.weights[smile] > 0.0, "{model:?}");

        let changed = apply(&mut groups, &model);
        assert!(changed > 0);
        let first = &groups[0];
        assert_eq!(first.winner_photo_id.as_deref(), Some("b0_3"));
        assert!(first.photos[0]
            .reason_codes
            .contains(&"preferred_by_your_style".to_string()));
    }

    /// Marks a cull wrote and the photographer left say nothing; too few
    /// decisions are refused.
    #[test]
    fn untouched_cull_marks_teach_nothing() {
        let groups = group_and_sort_images(bursts(15), None, None, Some(2), "default");
        let labels: HashMap<String, Label> = (0..15)
            .flat_map(|b| (0..4).map(move |i| (b, i)))
            .map(|(b, i)| {
                (
                    format!("b{b}_{i}"),
                    Label {
                        flag: i8::from(i == 0),
                        rating: 0,
                        own: false,
                    },
                )
            })
            .collect();
        assert_eq!(learn(&groups, &labels), Err(LearnError::TooFewDecisions(0)));
    }
}
