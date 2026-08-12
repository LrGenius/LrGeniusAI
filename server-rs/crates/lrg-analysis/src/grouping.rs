//! Port of `services/chroma.py::group_and_sort_images` +
//! `_rank_group_records` + `_derive_grouping_thresholds`: groups photos
//! into near-duplicate/burst clusters (pHash + CLIP cosine + capture-time
//! adjacency, connected components) and ranks each group (winner /
//! alternates / reject candidates) with human-readable reason codes.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use crate::culling_config::{get_culling_config, CullingConfig, IntentionalSet};
use crate::sets::{detect_intentional_set, SetFrame};

#[derive(Debug, Clone)]
pub struct GroupingInput {
    pub photo_id: String,
    pub filename: String,
    pub capture_time: Option<f64>,
    /// `None` for metadata-only records (nullable-vector convention);
    /// an explicit all-zero vector is also treated as absent, matching
    /// `_embedding_to_array`'s `np.allclose(arr, 0.0)` check.
    pub embedding: Option<Vec<f32>>,
    pub phash: Option<u64>,
    pub metadata: Map<String, Value>,
}

fn effective_embedding(v: &Option<Vec<f32>>) -> Option<&[f32]> {
    let v = v.as_ref()?;
    if v.iter().all(|x| *x == 0.0) {
        None
    } else {
        Some(v.as_slice())
    }
}

fn cosine_distance(a: Option<&[f32]>, b: Option<&[f32]>) -> Option<f64> {
    let (a, b) = (a?, b?);
    let dot: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
    let norm_a: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return None;
    }
    let similarity = (dot / (norm_a * norm_b)).clamp(-1.0, 1.0);
    Some(1.0 - similarity)
}

/// [`cosine_distance`] with the L2 norms supplied by the caller instead of
/// recomputed per pair. Arithmetic is otherwise identical, so results match
/// bit-for-bit; the point is to pay O(n) norms instead of O(n^2).
fn cosine_distance_pre(a: Option<(&[f32], f64)>, b: Option<(&[f32], f64)>) -> Option<f64> {
    let ((va, norm_a), (vb, norm_b)) = (a?, b?);
    let dot: f64 = va.iter().zip(vb).map(|(x, y)| *x as f64 * *y as f64).sum();
    let similarity = (dot / (norm_a * norm_b)).clamp(-1.0, 1.0);
    Some(1.0 - similarity)
}

fn phash_hamming(a: Option<u64>, b: Option<u64>) -> Option<u32> {
    Some((a? ^ b?).count_ones())
}

fn round4(v: f64) -> f64 {
    (v * 10000.0).round_ties_even() / 10000.0
}

fn unit(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

struct Thresholds {
    phash_hamming_threshold: u32,
    duplicate_distance_threshold: f64,
    burst_distance_threshold: f64,
    duplicate_time_window_seconds: f64,
    time_window_seconds: f64,
}

/// `phash_threshold`/`clip_threshold`: `None` means "auto".
fn derive_grouping_thresholds(
    phash_threshold: Option<f64>,
    clip_threshold: Option<f64>,
    time_delta: Option<i64>,
    cfg: &CullingConfig,
) -> Thresholds {
    // `None` means the caller did not specify a burst window, so the preset's
    // own default applies. That field existed since the Python port but nothing
    // ever read it: the window was taken from the request unconditionally, with
    // a hardcoded fallback of 1, so `event`'s 2s and `sports`'s 3s were dead.
    let time_delta = time_delta.unwrap_or(cfg.grouping.time_window_default_seconds);
    let time_window_seconds = time_delta.max(0) as f64;

    let burst_distance_threshold = clip_threshold
        .map(|v| v.max(0.0))
        .unwrap_or(cfg.grouping.burst_distance_auto);

    let phash_hamming_threshold = match phash_threshold {
        Some(v) => v.max(0.0).min(cfg.grouping.phash_max) as u32,
        None => cfg.grouping.phash_hamming_auto as u32,
    };

    let phash_max = cfg.grouping.phash_max;
    let normalized = (phash_hamming_threshold as f64).clamp(0.0, phash_max) / phash_max;
    let duplicate_distance_threshold =
        cfg.grouping.duplicate_distance_min + normalized * cfg.grouping.duplicate_distance_span;

    let duplicate_time_window_seconds = (time_window_seconds
        * cfg.grouping.duplicate_time_window_multiplier as f64)
        .max(cfg.grouping.duplicate_time_window_min_seconds as f64);

    Thresholds {
        phash_hamming_threshold,
        duplicate_distance_threshold,
        burst_distance_threshold,
        duplicate_time_window_seconds,
        time_window_seconds,
    }
}

/// `(capture_time_is_none, capture_time_or_inf, filename, photo_id)` ascending.
fn record_sort_key(r: &GroupingInput) -> (bool, f64, String, String) {
    (
        r.capture_time.is_none(),
        r.capture_time.unwrap_or(f64::INFINITY),
        r.filename.clone(),
        r.photo_id.clone(),
    )
}

fn metric(metadata: &Map<String, Value>, key: &str, default: f64) -> f64 {
    let v = metadata.get(key).and_then(Value::as_f64).unwrap_or(default);
    unit(v)
}

struct UnionFind {
    parent: Vec<usize>,
}
impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind {
            parent: (0..n).collect(),
        }
    }
    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

#[derive(Debug, Clone)]
pub struct RankedPhoto {
    pub photo_id: String,
    pub rank: usize,
    pub cull_score: f64,
    pub winner: bool,
    pub reject_candidate: bool,
    pub reason_codes: Vec<String>,
    pub explanation: String,
    pub metrics: Map<String, Value>,
}

#[derive(Debug, Clone)]
pub struct Group {
    pub group_id: String,
    pub group_type: &'static str,
    pub group_size: usize,
    pub primary_photo_id: String,
    pub photo_ids: Vec<String>,
    pub winner_photo_id: String,
    pub alternate_photo_ids: Vec<String>,
    pub reject_candidate_photo_ids: Vec<String>,
    pub photos: Vec<RankedPhoto>,
    pub min_capture_time: Option<f64>,
    pub max_capture_time: Option<f64>,
    pub time_span_seconds: f64,
    pub pairwise_distances: Vec<f64>,
    pub pairwise_phash_distances: Vec<u32>,
    pub edge_types: Vec<&'static str>,
    pub thresholds: (u32, f64, f64, f64, f64),
    /// `Some` when the group is a bracket, focus stack or panorama. Ranking
    /// still ran — `winner_photo_id` names the set's representative frame — but
    /// `reject_candidate_photo_ids` is guaranteed empty.
    pub intentional_set: Option<IntentionalSet>,
}

impl Group {
    /// Every frame in this group is wanted; nothing in it should be offered for
    /// deletion. The plugin routes these to their own collection.
    pub fn keep_all(&self) -> bool {
        self.intentional_set.is_some()
    }
}

struct Scored<'a> {
    record: &'a GroupingInput,
    cull_sharpness: f64,
    /// The sharpness the score actually uses: a blend of the frame-wide value
    /// and the sharpest tile, per `ranking.sharpness_peak_weight`. Equal to
    /// `cull_sharpness` for photos indexed before tiles existed.
    sharpness_effective: f64,
    cull_motion_anisotropy: f64,
    /// The genre's zero-shot semantic score, injected by the API layer from
    /// whichever prompt set `ranking.semantic_prompt_set` names. `None` when
    /// there is no embedding, no text tower, or the preset has no axis.
    cull_semantic: Option<f64>,
    cull_exposure: f64,
    cull_noise: f64,
    cull_highlight_clip: f64,
    cull_shadow_clip: f64,
    cull_technical_score: f64,
    cull_aesthetic: f64,
    cull_face_count: i64,
    cull_face_sharpness: f64,
    cull_face_prominence: f64,
    cull_face_visibility: f64,
    cull_face_score: f64,
    cull_occlusion: f64,
    cull_eye_openness: f64,
    cull_blink_penalty: f64,
    cull_score: f64,
}

fn explanation_from_reason_codes(codes: &[String]) -> String {
    if codes.is_empty() {
        return "single image in group".to_string();
    }
    let label = |code: &str| -> String {
        match code {
            "sharpest_in_group" => "sharpest in group".to_string(),
            "blurred" => "noticeably blurred".to_string(),
            "motion_blur" => "softness looks like camera shake or subject motion".to_string(),
            "peak_action" => "strongest moment in the sequence".to_string(),
            "best_expression" => "best expression in the group".to_string(),
            "strongest_moment" => "the most genuine, unposed moment in the group".to_string(),
            "bracket_frame_kept" => "part of an exposure bracket — kept, not culled".to_string(),
            "focus_stack_frame_kept" => "part of a focus stack — kept, not culled".to_string(),
            "panorama_frame_kept" => "part of a panorama sweep — kept, not culled".to_string(),
            "underexposed" => "darker than stronger alternatives".to_string(),
            "overexposed" => "brighter than stronger alternatives".to_string(),
            "low_aesthetic" => "weaker aesthetic impression than alternatives".to_string(),
            "best_face_quality" => "best face quality in group".to_string(),
            "weak_face_quality" => "weaker face quality than alternatives".to_string(),
            "no_face_detected_in_group" => {
                "no clear face detected while alternatives have faces".to_string()
            }
            "possible_occlusion" => "possible facial occlusion or weak visibility".to_string(),
            "eyes_open_best" => "best eyes-open result in group".to_string(),
            "possible_blink" => "possible blink or eyes less open".to_string(),
            "near_duplicate_weaker" => "weaker duplicate or burst alternative".to_string(),
            other => other.replace('_', " "),
        }
    };
    codes
        .iter()
        .map(|c| label(c))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Blends `value` towards where it sits between the group's own worst and best.
///
/// Polarity carries over from the input and needs no flag: for exposure, where
/// high is good, the frame nearest the top of the group's range lands near 1;
/// for the noise *penalty*, where high is bad, the noisiest frame lands near 1.
/// Either way the metric keeps meaning what it meant.
///
/// Returns `value` untouched when the group has one member — "relative to the
/// group" means nothing there — and when the group agrees to within
/// `relative_min_spread`, because stretching a two-percent spread across the
/// full range manufactures a ranking out of measurement noise.
fn relative_to_group(values: &[f64], value: f64, cfg: &CullingConfig) -> f64 {
    let w = cfg.ranking.relative_normalization_weight;
    if w <= 0.0 || values.len() < 2 {
        return value;
    }
    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let spread = max - min;
    if spread < cfg.ranking.relative_min_spread {
        return value;
    }
    unit((1.0 - w) * value + w * ((value - min) / spread))
}

fn rank_group_records(
    records: &[&GroupingInput],
    group_type: &str,
    intentional_set: Option<IntentionalSet>,
    cfg: &CullingConfig,
) -> Vec<RankedPhoto> {
    let fm = &cfg.face_metrics;
    let im = &cfg.image_metrics;
    let rk_peak_weight = cfg.ranking.sharpness_peak_weight.clamp(0.0, 1.0);

    // Group-wide vectors for the relative pass. Collected before scoring
    // because every member's exposure and noise are judged against all of them.
    let exposures: Vec<f64> = records
        .iter()
        .map(|r| metric(&r.metadata, "cull_exposure", 0.0))
        .collect();

    let mut scored: Vec<Scored> = records
        .iter()
        .map(|record| {
            let m = &record.metadata;
            let sharpness = metric(m, "cull_sharpness", 0.0);
            // Absent for photos indexed before the tiled pass shipped, in which
            // case the frame-wide value carries the full weight and this knob
            // is inert rather than wrong.
            let sharpness_effective = match m.get("cull_sharpness_peak").and_then(Value::as_f64) {
                Some(peak) => (1.0 - rk_peak_weight) * sharpness + rk_peak_weight * unit(peak),
                None => sharpness,
            };
            let motion_anisotropy = metric(m, "cull_motion_anisotropy", 0.0);
            let exposure_absolute = metric(m, "cull_exposure", 0.0);
            let exposure = relative_to_group(&exposures, exposure_absolute, cfg);
            // Noise is deliberately *not* normalized against the group, though
            // the plan grouped it with exposure. Measured on two frames of one
            // scene differing only in focus, the raw estimator already ranks
            // the sharp frame dirtier than the blurred one — 0.082 against
            // 0.012 — because it is a high-pass residual and real detail looks
            // like grain to it. Rescaling that to the group's range stretched
            // the gap to 0.535, an eightfold amplification of a signal that is
            // pointing the wrong way.
            //
            // The stated reason for normalizing it does not survive contact
            // either: a whole group shot at the same high ISO is penalised
            // *equally*, and a constant offset cannot change a within-group
            // ranking. Relative noise buys nothing and costs this. Fixing the
            // estimator so it stops competing with sharpness is real work and
            // stays on the outstanding list.
            let noise_penalty = metric(m, "cull_noise", 1.0);
            let highlight_clip = metric(m, "cull_highlight_clip", 0.0);
            let shadow_clip = metric(m, "cull_shadow_clip", 0.0);
            let clipping_penalty = unit(highlight_clip + shadow_clip);
            let technical_default = 0.5 * sharpness_effective
                + 0.3 * exposure
                + 0.1 * (1.0 - noise_penalty)
                + 0.1 * (1.0 - clipping_penalty);
            // The stored composite was frozen at index time under the default
            // config and cannot see the preset's weights, the peak-sharpness
            // blend or the relative pass. Re-derive it from the sub-scores that
            // *can*, and fall back to the stored value only when they are gone.
            let technical_score = if cfg.ranking.recompute_technical_score {
                let weight_sum = im.technical_weight_sharpness
                    + im.technical_weight_exposure
                    + im.technical_weight_noise;
                unit(
                    (im.technical_weight_sharpness * sharpness_effective
                        + im.technical_weight_exposure * exposure
                        + im.technical_weight_noise * (1.0 - noise_penalty))
                        / weight_sum.max(1e-6),
                )
            } else {
                metric(m, "cull_technical_score", technical_default)
            };
            // `cull_aesthetic_iqa` is injected by the API layer from a CLIP-IQA
            // pass over the stored SigLIP2 embedding. Absent for photos with no
            // embedding, or when the text tower could not be reached.
            let aesthetic_heuristic = metric(m, "cull_aesthetic", 0.0);
            let aesthetic_score = match m.get("cull_aesthetic_iqa").and_then(Value::as_f64) {
                Some(iqa) => {
                    let w = cfg.ranking.aesthetic_iqa_weight.clamp(0.0, 1.0);
                    unit((1.0 - w) * aesthetic_heuristic + w * unit(iqa))
                }
                None => aesthetic_heuristic,
            };
            let face_count = m
                .get("cull_face_count")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let face_sharpness = metric(m, "cull_face_sharpness", 0.0);
            let face_prominence = metric(m, "cull_face_prominence", 0.0);
            let face_visibility = metric(m, "cull_face_visibility", 0.0);
            let occlusion_penalty = metric(m, "cull_occlusion", 0.0);
            let eye_openness = metric(m, "cull_eye_openness", 0.0);
            let face_weight_sum = fm.score_weight_sharpness
                + fm.score_weight_prominence
                + fm.score_weight_visibility
                + fm.score_weight_eye_openness
                + fm.score_weight_occlusion;
            let face_default = (fm.score_weight_sharpness * face_sharpness
                + fm.score_weight_prominence * face_prominence
                + fm.score_weight_visibility * face_visibility
                + fm.score_weight_eye_openness * eye_openness
                + fm.score_weight_occlusion * (1.0 - occlusion_penalty))
                / face_weight_sum.max(1e-6);
            let face_score = metric(m, "cull_face_score", face_default);
            let blink_penalty = metric(m, "cull_blink_penalty", 1.0);

            Scored {
                record,
                cull_sharpness: sharpness,
                sharpness_effective,
                cull_motion_anisotropy: motion_anisotropy,
                cull_semantic: m.get("cull_semantic_iqa").and_then(Value::as_f64).map(unit),
                cull_exposure: exposure,
                cull_noise: noise_penalty,
                cull_highlight_clip: highlight_clip,
                cull_shadow_clip: shadow_clip,
                cull_technical_score: technical_score,
                cull_aesthetic: aesthetic_score,
                cull_face_count: face_count,
                cull_face_sharpness: face_sharpness,
                cull_face_prominence: face_prominence,
                cull_face_visibility: face_visibility,
                cull_face_score: face_score,
                cull_occlusion: occlusion_penalty,
                cull_eye_openness: eye_openness,
                cull_blink_penalty: blink_penalty,
                cull_score: 0.0,
            }
        })
        .collect();

    let group_has_faces = scored.iter().any(|s| s.cull_face_count > 0);
    let rk = &cfg.ranking;
    for item in scored.iter_mut() {
        item.cull_score = if group_has_faces {
            if item.cull_face_count > 0 {
                let weight_sum = rk.face_group_weight_technical
                    + rk.face_group_weight_face
                    + rk.face_group_weight_aesthetic;
                let weighted = rk.face_group_weight_technical * item.cull_technical_score
                    + rk.face_group_weight_face * item.cull_face_score
                    + rk.face_group_weight_aesthetic * item.cull_aesthetic;
                let base = unit(weighted / weight_sum.max(1e-6));
                unit(
                    base - (rk.face_group_blink_penalty_weight * item.cull_blink_penalty
                        + rk.face_group_occlusion_penalty_weight * item.cull_occlusion),
                )
            } else {
                (rk.face_missing_technical_weight * item.cull_technical_score
                    - rk.face_missing_penalty)
                    .max(0.0)
            }
        } else {
            let weight_sum = 1.0 + rk.no_face_group_weight_aesthetic;
            let weighted =
                item.cull_technical_score + rk.no_face_group_weight_aesthetic * item.cull_aesthetic;
            unit(weighted / weight_sum.max(1e-6))
        };

        // The genre's semantic axis, blended over whatever the quality signals
        // concluded.
        //
        // A convex blend rather than another weighted term, because it answers
        // a different question from everything above it: those rank *how well
        // made* a frame is, this ranks *what it is of* — the ball at the foot,
        // the genuine smile, the unposed reaction. Inside one burst the quality
        // signals are near-constant and this is the only term with range left,
        // which is the point; across a mixed selection it must not be able to
        // promote a badly blurred frame just because the subject is right, and
        // capping it at `semantic_weight` is what prevents that.
        //
        // Absent — no embedding, no text tower, or a preset with no axis —
        // leaves the score exactly as it was.
        if let (true, Some(semantic)) = (rk.semantic_weight > 0.0, item.cull_semantic) {
            let w = rk.semantic_weight.clamp(0.0, 1.0);
            item.cull_score = unit((1.0 - w) * item.cull_score + w * semantic);
        }
    }

    scored.sort_by(|a, b| {
        b.cull_score
            .partial_cmp(&a.cull_score)
            .unwrap()
            .then_with(|| b.cull_face_score.partial_cmp(&a.cull_face_score).unwrap())
            .then_with(|| b.cull_sharpness.partial_cmp(&a.cull_sharpness).unwrap())
            .then_with(|| b.cull_exposure.partial_cmp(&a.cull_exposure).unwrap())
            .then_with(|| a.cull_noise.partial_cmp(&b.cull_noise).unwrap())
            .then_with(|| a.record.photo_id.cmp(&b.record.photo_id))
    });

    if scored.is_empty() {
        return Vec::new();
    }

    let max_sharpness = scored
        .iter()
        .map(|s| s.sharpness_effective)
        .fold(f64::MIN, f64::max);
    let max_face_score = scored
        .iter()
        .map(|s| s.cull_face_score)
        .fold(f64::MIN, f64::max);
    let max_eye_openness = scored
        .iter()
        .map(|s| s.cull_eye_openness)
        .fold(f64::MIN, f64::max);
    let max_aesthetic = scored
        .iter()
        .map(|s| s.cull_aesthetic)
        .fold(f64::MIN, f64::max);
    let winner_score = scored[0].cull_score;

    let mut out = Vec::with_capacity(scored.len());
    for (i, item) in scored.iter().enumerate() {
        let index = i + 1;
        let mut reason_codes = Vec::new();
        let is_soft = item.sharpness_effective < rk.reason_blur_threshold;
        if is_soft {
            // Same threshold, different diagnosis. A soft frame whose gradients
            // all point one way is camera shake; one whose gradients point
            // everywhere is missed focus. The score does not read this — only
            // the label the user sees does, because directional *content*
            // (architecture, horizons) raises the same measure.
            reason_codes.push(
                if item.cull_motion_anisotropy > rk.reason_motion_anisotropy_threshold {
                    "motion_blur".to_string()
                } else {
                    "blurred".to_string()
                },
            );
        }
        if item.cull_exposure < rk.reason_exposure_threshold {
            reason_codes.push(if item.cull_shadow_clip >= item.cull_highlight_clip {
                "underexposed".to_string()
            } else {
                "overexposed".to_string()
            });
        }
        if item.cull_aesthetic < rk.reason_low_aesthetic_threshold
            && item.cull_aesthetic < (max_aesthetic - 0.08).max(0.0)
        {
            reason_codes.push("low_aesthetic".to_string());
        }
        if index == 1
            && scored.len() > 1
            && item.sharpness_effective >= max_sharpness - rk.reason_sharpest_delta
        {
            reason_codes.push("sharpest_in_group".to_string());
        }
        if group_has_faces {
            if item.cull_face_count == 0 {
                reason_codes.push("no_face_detected_in_group".to_string());
            } else if item.cull_face_score >= max_face_score - rk.reason_best_face_delta
                && index == 1
            {
                reason_codes.push("best_face_quality".to_string());
            } else if item.cull_face_score < (max_face_score - rk.reason_weak_face_delta).max(0.0) {
                reason_codes.push("weak_face_quality".to_string());
            }
            if item.cull_occlusion > rk.reason_occlusion_threshold {
                reason_codes.push("possible_occlusion".to_string());
            }
            if item.cull_eye_openness >= (max_eye_openness - rk.reason_eyes_open_delta).max(0.0)
                && index == 1
            {
                reason_codes.push("eyes_open_best".to_string());
            } else if item.cull_blink_penalty > rk.reason_possible_blink_threshold {
                reason_codes.push("possible_blink".to_string());
            }
        }
        // Explains the pick a technical reading would not have made. Emitted
        // for the winner only, and only when the semantic axis is what carried
        // it — a user who disagrees with a pick needs to see that the ranking
        // was arguing about the moment or the expression, not about sharpness.
        //
        // The code names the axis rather than saying "semantic", because
        // "strongest moment in the sequence" is actionable feedback and
        // "semantic score 0.83" is not.
        if index == 1 && scored.len() > 1 && rk.semantic_weight > 0.0 {
            if let (Some(semantic), Some(axis)) = (item.cull_semantic, rk.semantic_prompt_set) {
                let best_elsewhere = scored
                    .iter()
                    .skip(1)
                    .filter_map(|s| s.cull_semantic)
                    .fold(f64::MIN, f64::max);
                if semantic >= best_elsewhere && semantic > 0.5 {
                    reason_codes.push(match axis {
                        "action" => "peak_action".to_string(),
                        "expression" => "best_expression".to_string(),
                        "candid" => "strongest_moment".to_string(),
                        other => format!("strongest_{other}"),
                    });
                }
            }
        }
        if index > 1 && group_type != "single" && intentional_set.is_none() {
            reason_codes.push("near_duplicate_weaker".to_string());
        }
        if let Some(kind) = intentional_set {
            reason_codes.push(format!("{}_frame_kept", kind.as_str()));
        }

        let reject_candidate = scored.len() > 1
            && (item.cull_score <= (winner_score - rk.reject_score_delta).max(0.0)
                || item.sharpness_effective < rk.reason_blur_threshold
                || item.cull_exposure < rk.reject_exposure_threshold
                || (group_has_faces
                    && item.cull_face_count > 0
                    && item.cull_face_score < rk.reject_face_score_threshold)
                || (group_has_faces
                    && item.cull_face_count > 0
                    && item.cull_blink_penalty > rk.reject_blink_penalty_threshold)
                || (group_has_faces
                    && item.cull_face_count > 0
                    && item.cull_occlusion > rk.reject_occlusion_threshold));
        // A bracket's dark frame *is* clipped shadows and its bright frame *is*
        // clipped highlights; a focus stack's near frame *is* soft at the back.
        // Every reject rule above fires on exactly the frames these sets exist
        // to capture, so for an intentional set none of them apply. Ordering is
        // still produced — the top frame becomes the set's representative — but
        // nothing is ever nominated for deletion.
        let reject_candidate = reject_candidate && index != 1 && intentional_set.is_none();

        let mut metrics = Map::new();
        metrics.insert("sharpness".into(), Value::from(round4(item.cull_sharpness)));
        metrics.insert("exposure".into(), Value::from(round4(item.cull_exposure)));
        metrics.insert("noise".into(), Value::from(round4(item.cull_noise)));
        metrics.insert(
            "highlight_clip".into(),
            Value::from(round4(item.cull_highlight_clip)),
        );
        metrics.insert(
            "shadow_clip".into(),
            Value::from(round4(item.cull_shadow_clip)),
        );
        metrics.insert(
            "technical_score".into(),
            Value::from(round4(item.cull_technical_score)),
        );
        metrics.insert("aesthetic".into(), Value::from(round4(item.cull_aesthetic)));
        metrics.insert("face_count".into(), Value::from(item.cull_face_count));
        metrics.insert(
            "face_sharpness".into(),
            Value::from(round4(item.cull_face_sharpness)),
        );
        metrics.insert(
            "face_prominence".into(),
            Value::from(round4(item.cull_face_prominence)),
        );
        metrics.insert(
            "face_visibility".into(),
            Value::from(round4(item.cull_face_visibility)),
        );
        metrics.insert(
            "face_score".into(),
            Value::from(round4(item.cull_face_score)),
        );
        metrics.insert("occlusion".into(), Value::from(round4(item.cull_occlusion)));
        metrics.insert(
            "eye_openness".into(),
            Value::from(round4(item.cull_eye_openness)),
        );
        metrics.insert(
            "blink_penalty".into(),
            Value::from(round4(item.cull_blink_penalty)),
        );
        // Emitted alongside `sharpness` rather than replacing it: when the two
        // disagree, that disagreement *is* the diagnosis, and a user asking why
        // a frame won needs to see both.
        metrics.insert(
            "sharpness_effective".into(),
            Value::from(round4(item.sharpness_effective)),
        );
        metrics.insert(
            "motion_anisotropy".into(),
            Value::from(round4(item.cull_motion_anisotropy)),
        );
        // Emitted only when it was actually computed, so an absent key means
        // "not scored" rather than "scored zero" — the difference between "no
        // embedding" and "nothing is happening here". The axis rides along so
        // the number is self-describing: 0.83 means nothing without knowing
        // whether the question was about action or expression.
        if let Some(semantic) = item.cull_semantic {
            metrics.insert("semantic".into(), Value::from(round4(semantic)));
            if let Some(axis) = rk.semantic_prompt_set {
                metrics.insert("semantic_axis".into(), Value::from(axis));
            }
        }

        out.push(RankedPhoto {
            photo_id: item.record.photo_id.clone(),
            rank: index,
            cull_score: round4(item.cull_score),
            winner: index == 1,
            reject_candidate,
            explanation: explanation_from_reason_codes(&reason_codes),
            reason_codes,
            metrics,
        });
    }
    out
}

/// `phash_threshold`/`clip_threshold` of `None` mean "auto" (Python's
/// string sentinel). `time_delta` seconds; `culling_preset` name.
pub fn group_and_sort_images(
    records: Vec<GroupingInput>,
    phash_threshold: Option<f64>,
    clip_threshold: Option<f64>,
    time_delta: Option<i64>,
    culling_preset: &str,
) -> Vec<Group> {
    let cfg = get_culling_config(culling_preset);
    group_and_sort_images_with_config(records, phash_threshold, clip_threshold, time_delta, &cfg)
}

/// [`group_and_sort_images`] with the config supplied directly rather than
/// looked up by preset name.
///
/// Exists for two callers that need a config no preset name maps to: the
/// Python-parity golden test (see [`CullingConfig::python_parity`]) and the
/// evaluation harness, which sweeps individual knobs to measure what each one
/// is worth.
pub fn group_and_sort_images_with_config(
    mut records: Vec<GroupingInput>,
    phash_threshold: Option<f64>,
    clip_threshold: Option<f64>,
    time_delta: Option<i64>,
    cfg: &CullingConfig,
) -> Vec<Group> {
    if records.is_empty() {
        return Vec::new();
    }
    let cfg = *cfg;
    let th = derive_grouping_thresholds(phash_threshold, clip_threshold, time_delta, &cfg);

    records.sort_by(|a, b| {
        let (ka, kb) = (record_sort_key(a), record_sort_key(b));
        ka.0.cmp(&kb.0)
            .then(ka.1.partial_cmp(&kb.1).unwrap())
            .then(ka.2.cmp(&kb.2))
            .then(ka.3.cmp(&kb.3))
    });
    let n = records.len();

    let mut uf = UnionFind::new(n);
    // sorted-pair (i,j) with i<j -> "near_duplicate" | "burst"
    let mut edge_kinds: HashMap<(usize, usize), &'static str> = HashMap::new();

    // Hoist the L2 norms out of the pair loop. `cosine_distance` recomputed
    // both of them on every call, which is two thirds of the work in an
    // O(n^2) sweep over 1152-dimensional vectors. The arithmetic is unchanged,
    // so distances stay bit-identical.
    let normed: Vec<Option<(&[f32], f64)>> = records
        .iter()
        .map(|r| {
            let v = effective_embedding(&r.embedding)?;
            let norm: f64 = v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
            if norm == 0.0 {
                None
            } else {
                Some((v, norm))
            }
        })
        .collect();

    // `record_sort_key` puts records that *have* a capture time first (the key
    // leads with `capture_time.is_none()`), ascending. So the timed records are
    // a sorted prefix and the undated ones are a suffix.
    let timed = records
        .iter()
        .take_while(|r| r.capture_time.is_some())
        .count();

    // Neither edge type can survive a time gap wider than this:
    //   - near-duplicate requires `gap <= duplicate_time_window_seconds`
    //   - burst requires `gap <= time_window_seconds`
    // so once a pair exceeds the wider of the two, every later `j` (sorted
    // ascending) is further still and can be skipped. Taking the max rather
    // than assuming `duplicate >= burst` keeps this correct even if a config
    // sets `duplicate_time_window_multiplier` below 1.
    let max_edge_gap = th.duplicate_time_window_seconds.max(th.time_window_seconds);

    let consider = |i: usize,
                    j: usize,
                    uf: &mut UnionFind,
                    edge_kinds: &mut HashMap<(usize, usize), &'static str>| {
        let (left, right) = (&records[i], &records[j]);
        let time_gap = match (left.capture_time, right.capture_time) {
            (Some(lt), Some(rt)) => Some((rt - lt).abs()),
            _ => None,
        };

        let phash_distance = phash_hamming(left.phash, right.phash);
        let time_ok_for_duplicate = time_gap.is_none_or(|g| g <= th.duplicate_time_window_seconds);
        let phash_says_duplicate =
            phash_distance.is_some_and(|d| d as f64 <= th.phash_hamming_threshold as f64);

        // Bracket edge: neither similarity signal can see that these frames go
        // together, because changing the exposure is what breaks both of them.
        // See `GroupingConfig::bracket_edges`. Being in one group does not make
        // them a bracket — `sets::detect_intentional_set` decides that.
        if cfg.grouping.bracket_edges {
            let exposure_step = match (
                left.metadata.get("exposure_bias").and_then(Value::as_f64),
                right.metadata.get("exposure_bias").and_then(Value::as_f64),
            ) {
                (Some(a), Some(b)) => (a - b).abs(),
                _ => 0.0,
            };
            if exposure_step > cfg.sets.uniform_ev_tolerance
                && time_gap.is_some_and(|g| g <= th.time_window_seconds)
            {
                uf.union(i, j);
                edge_kinds.entry((i, j)).or_insert("bracket_candidate");
                return;
            }
        }

        // A matching pHash already settles it: the pair is a near-duplicate and
        // the edge is labelled `near_duplicate` whatever the cosine says, so
        // skip the embedding comparison entirely. This is the common case
        // inside a burst, where consecutive frames hash identically.
        let (is_near_duplicate, is_burst) = if phash_says_duplicate && time_ok_for_duplicate {
            (true, false)
        } else {
            let distance = cosine_distance_pre(normed[i], normed[j]);
            let is_near_duplicate = (phash_says_duplicate
                || distance.is_some_and(|d| d <= th.duplicate_distance_threshold))
                && time_ok_for_duplicate;
            let is_burst = distance.is_some_and(|d| d <= th.burst_distance_threshold)
                && time_gap.is_some_and(|g| g <= th.time_window_seconds);
            (is_near_duplicate, is_burst)
        };

        if !is_near_duplicate && !is_burst {
            return;
        }
        uf.union(i, j);
        edge_kinds.insert(
            (i, j),
            if is_near_duplicate {
                "near_duplicate"
            } else {
                "burst"
            },
        );
    };

    // Timed records against the sliding window of timed records that follow.
    let times: Vec<f64> = records
        .iter()
        .take(timed)
        .map(|r| r.capture_time.unwrap_or(f64::INFINITY))
        .collect();
    for (i, &ti) in times.iter().enumerate() {
        for (offset, &tj) in times[(i + 1)..].iter().enumerate() {
            if (tj - ti).abs() > max_edge_gap {
                break;
            }
            consider(i, i + 1 + offset, &mut uf, &mut edge_kinds);
        }
    }
    // Undated records still need the full sweep: with no capture time the gap
    // is unknown, and `is_near_duplicate` treats that as "not disqualifying".
    for i in 0..n {
        for j in (i + 1).max(timed)..n {
            consider(i, j, &mut uf, &mut edge_kinds);
        }
    }

    // Assign group numbers in global-sort discovery order (this is what
    // Python's group_XXXX numbering is actually keyed on, independent of
    // the final min-capture-time sort applied below).
    let mut root_to_group: HashMap<usize, usize> = HashMap::new();
    let mut group_counter = 0usize;
    let mut group_of = vec![0usize; n];
    for (i, slot) in group_of.iter_mut().enumerate() {
        let root = uf.find(i);
        let gid = *root_to_group.entry(root).or_insert_with(|| {
            let g = group_counter;
            group_counter += 1;
            g
        });
        *slot = gid;
    }

    let mut members: Vec<Vec<usize>> = vec![Vec::new(); group_counter];
    for (i, &g) in group_of.iter().enumerate() {
        members[g].push(i);
    }

    let mut groups: Vec<Group> = Vec::with_capacity(group_counter);
    for (g, idxs) in members.into_iter().enumerate() {
        // idxs are already in ascending original-index order, which is
        // global-sort order, so component_records is pre-sorted.
        let component: Vec<&GroupingInput> = idxs.iter().map(|&i| &records[i]).collect();
        let group_photo_ids: Vec<String> = component.iter().map(|r| r.photo_id.clone()).collect();

        let capture_times: Vec<f64> = component.iter().filter_map(|r| r.capture_time).collect();
        let time_span_seconds = if capture_times.len() >= 2 {
            capture_times.iter().cloned().fold(f64::MIN, f64::max)
                - capture_times.iter().cloned().fold(f64::MAX, f64::min)
        } else {
            0.0
        };

        let mut pairwise_distances = Vec::new();
        let mut pairwise_phash_distances = Vec::new();
        let mut group_edge_types: HashSet<&'static str> = HashSet::new();
        for a in 0..idxs.len() {
            for b in (a + 1)..idxs.len() {
                let (gi, gj) = (idxs[a].min(idxs[b]), idxs[a].max(idxs[b]));
                let (left, right) = (&records[gi], &records[gj]);
                if let Some(d) = cosine_distance(
                    effective_embedding(&left.embedding),
                    effective_embedding(&right.embedding),
                ) {
                    pairwise_distances.push(round4(d));
                }
                if let Some(d) = phash_hamming(left.phash, right.phash) {
                    pairwise_phash_distances.push(d);
                }
                if let Some(&kind) = edge_kinds.get(&(gi, gj)) {
                    group_edge_types.insert(kind);
                }
            }
        }

        // `component` is in global-sort order, which for timed records is
        // ascending capture time — the adjacency order the panorama rule needs.
        let set_frames: Vec<SetFrame> = component
            .iter()
            .map(|r| SetFrame {
                capture_time: r.capture_time,
                exposure_bias: r.metadata.get("exposure_bias").and_then(Value::as_f64),
                phash: r.phash,
                embedding: effective_embedding(&r.embedding),
                sharp_region: match (
                    r.metadata
                        .get("cull_sharp_region_x")
                        .and_then(Value::as_f64),
                    r.metadata
                        .get("cull_sharp_region_y")
                        .and_then(Value::as_f64),
                ) {
                    (Some(x), Some(y)) => Some((x, y)),
                    _ => None,
                },
            })
            .collect();
        let intentional_set = detect_intentional_set(&set_frames, &cfg.sets);

        let group_type: &'static str = if let Some(kind) = intentional_set {
            kind.as_str()
        } else if group_photo_ids.len() == 1 {
            "single"
        } else if group_edge_types.contains("near_duplicate") && !group_edge_types.contains("burst")
        {
            "near_duplicate"
        } else if time_span_seconds <= th.time_window_seconds {
            "burst"
        } else {
            "near_duplicate"
        };

        let ranked = rank_group_records(&component, group_type, intentional_set, &cfg);
        let group_id = format!("group_{:04}", g + 1);
        let winner_photo_id = ranked
            .first()
            .map(|r| r.photo_id.clone())
            .unwrap_or_else(|| group_photo_ids[0].clone());
        let alternate_photo_ids: Vec<String> = ranked
            .iter()
            .skip(1)
            .filter(|r| !r.reject_candidate)
            .map(|r| r.photo_id.clone())
            .collect();
        let reject_candidate_photo_ids: Vec<String> = ranked
            .iter()
            .filter(|r| r.reject_candidate)
            .map(|r| r.photo_id.clone())
            .collect();

        let mut edge_types: Vec<&'static str> = group_edge_types.into_iter().collect();
        edge_types.sort_unstable();

        groups.push(Group {
            group_id,
            group_type,
            group_size: group_photo_ids.len(),
            primary_photo_id: group_photo_ids[0].clone(),
            photo_ids: group_photo_ids,
            winner_photo_id,
            alternate_photo_ids,
            reject_candidate_photo_ids,
            intentional_set,
            photos: ranked,
            min_capture_time: capture_times
                .iter()
                .cloned()
                .fold(None, |acc: Option<f64>, v| {
                    Some(acc.map_or(v, |a| a.min(v)))
                }),
            max_capture_time: capture_times
                .iter()
                .cloned()
                .fold(None, |acc: Option<f64>, v| {
                    Some(acc.map_or(v, |a| a.max(v)))
                }),
            time_span_seconds: (time_span_seconds * 1000.0).round_ties_even() / 1000.0,
            pairwise_distances,
            pairwise_phash_distances,
            edge_types,
            thresholds: (
                th.phash_hamming_threshold,
                round4(th.duplicate_distance_threshold),
                round4(th.burst_distance_threshold),
                th.duplicate_time_window_seconds,
                th.time_window_seconds,
            ),
        });
    }

    groups.sort_by(|a, b| {
        a.min_capture_time
            .is_none()
            .cmp(&b.min_capture_time.is_none())
            .then(
                a.min_capture_time
                    .unwrap_or(f64::INFINITY)
                    .partial_cmp(&b.min_capture_time.unwrap_or(f64::INFINITY))
                    .unwrap(),
            )
            .then(a.primary_photo_id.cmp(&b.primary_photo_id))
    });

    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, capture_time: Option<f64>, phash: Option<u64>) -> GroupingInput {
        GroupingInput {
            photo_id: id.to_string(),
            filename: format!("{id}.jpg"),
            capture_time,
            embedding: None,
            phash,
            metadata: Map::new(),
        }
    }

    /// Group id per photo, for comparing partitions regardless of ordering.
    fn partition(groups: &[Group]) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for g in groups {
            for id in &g.photo_ids {
                out.insert(id.clone(), g.group_id.clone());
            }
        }
        out
    }

    fn run(records: Vec<GroupingInput>) -> Vec<Group> {
        group_and_sort_images(records, None, None, Some(1), "default")
    }

    /// The sliding window skips pairs beyond the duplicate window. That is only
    /// sound because no edge can form there — this pins the behaviour.
    #[test]
    fn identical_hashes_far_apart_in_time_do_not_group() {
        let hash = Some(0xffff_ffff_ffff_ffff);
        // Default duplicate window is max(1*4, 10) = 10s; 60s apart is well out.
        let groups = run(vec![
            rec("a", Some(1000.0), hash),
            rec("b", Some(1060.0), hash),
            rec("c", Some(1120.0), hash),
        ]);
        assert_eq!(groups.len(), 3, "each frame should stand alone");
        assert!(groups.iter().all(|g| g.group_type == "single"));
    }

    #[test]
    fn identical_hashes_inside_the_window_group() {
        let hash = Some(0xffff_ffff_ffff_ffff);
        let groups = run(vec![
            rec("a", Some(1000.0), hash),
            rec("b", Some(1002.0), hash),
            rec("c", Some(1004.0), hash),
        ]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group_size, 3);
    }

    /// A chain of frames each within the window of the next spans further than
    /// the window overall. Union-find must still connect them transitively —
    /// the window prunes *pair* checks, not connectivity.
    #[test]
    fn window_pruning_preserves_transitive_chains() {
        let hash = Some(0xffff_ffff_ffff_ffff);
        let records: Vec<GroupingInput> = (0..12)
            .map(|i| rec(&format!("p{i}"), Some(1000.0 + i as f64 * 3.0), hash))
            .collect();
        // Span is 33s, far wider than the 10s window, but consecutive gaps are 3s.
        let groups = run(records);
        assert_eq!(groups.len(), 1, "chain must stay connected");
        assert_eq!(groups[0].group_size, 12);
    }

    /// Records with no capture time cannot be pruned by the window, because
    /// `is_near_duplicate` treats an unknown gap as "not disqualifying".
    #[test]
    fn undated_records_still_match_across_the_whole_set() {
        let hash = Some(0xffff_ffff_ffff_ffff);
        let groups = run(vec![
            rec("timed", Some(1000.0), hash),
            rec("undated_a", None, hash),
            rec("undated_b", None, hash),
        ]);
        assert_eq!(groups.len(), 1, "undated frames join on pHash alone");
        assert_eq!(groups[0].group_size, 3);
    }

    /// Undated records sort last, so they are exactly the suffix the second
    /// loop sweeps. A dated frame far from another dated frame must still not
    /// join, even when undated frames are present in the same request.
    #[test]
    fn undated_suffix_does_not_leak_edges_between_distant_dated_frames() {
        let groups = run(vec![
            rec("a", Some(1000.0), Some(0x0000_0000_0000_0000)),
            rec("b", Some(9000.0), Some(0xffff_ffff_ffff_ffff)),
            rec("c", None, Some(0x0f0f_0f0f_0f0f_0f0f)),
        ]);
        let p = partition(&groups);
        assert_ne!(p["a"], p["b"], "distinct hashes, 8000s apart");
        assert_eq!(groups.len(), 3);
    }

    /// Differing pHashes beyond the threshold must not group even when the
    /// frames are adjacent in time, otherwise the pHash short-circuit would be
    /// masking a real comparison.
    #[test]
    fn distinct_hashes_close_in_time_do_not_group_without_embeddings() {
        let groups = run(vec![
            rec("a", Some(1000.0), Some(0x0000_0000_0000_0000)),
            rec("b", Some(1001.0), Some(0xffff_ffff_ffff_ffff)),
        ]);
        assert_eq!(groups.len(), 2);
    }

    /// Embeddings and pHashes must agree with the pre-normed distance path.
    #[test]
    fn embedding_only_records_group_by_cosine() {
        let mut a = rec("a", Some(1000.0), None);
        let mut b = rec("b", Some(1001.0), None);
        let mut far = rec("far", Some(1002.0), None);
        a.embedding = Some(vec![1.0, 0.0, 0.0]);
        b.embedding = Some(vec![1.0, 0.001, 0.0]); // cosine distance ~5e-7
        far.embedding = Some(vec![0.0, 1.0, 0.0]); // orthogonal
        let groups = run(vec![a, b, far]);
        let p = partition(&groups);
        assert_eq!(p["a"], p["b"], "near-identical vectors group");
        assert_ne!(p["a"], p["far"], "orthogonal vector stays separate");
    }

    /// Attaches culling metrics to a record.
    fn with_metrics(mut r: GroupingInput, pairs: &[(&str, f64)]) -> GroupingInput {
        for (k, v) in pairs {
            r.metadata.insert((*k).into(), Value::from(*v));
        }
        r
    }

    fn winner_of(groups: &[Group]) -> String {
        assert_eq!(groups.len(), 1, "expected one group");
        groups[0].winner_photo_id.clone()
    }

    /// The shallow-depth-of-field case. `shallow` has a razor-sharp subject on
    /// smooth bokeh: low frame-wide variance, high peak. `busy` is soft
    /// everywhere over detailed foliage: higher frame-wide variance, no peak.
    /// Frame-wide sharpness alone crowns `busy`, which is the bug.
    #[test]
    fn a_sharp_subject_on_bokeh_beats_a_uniformly_soft_but_busy_frame() {
        let hash = Some(0xffff_ffff_ffff_ffff);
        let shallow = with_metrics(
            rec("shallow", Some(1000.0), hash),
            &[
                ("cull_sharpness", 0.25),
                ("cull_sharpness_peak", 0.97),
                ("cull_exposure", 0.7),
                ("cull_noise", 0.1),
            ],
        );
        let busy = with_metrics(
            rec("busy", Some(1000.5), hash),
            &[
                ("cull_sharpness", 0.45),
                ("cull_sharpness_peak", 0.47),
                ("cull_exposure", 0.7),
                ("cull_noise", 0.1),
            ],
        );
        assert_eq!(
            winner_of(&run(vec![shallow.clone(), busy.clone()])),
            "shallow"
        );

        // ...and with the peak blend disabled it is the other way round, which
        // is exactly the behaviour this change exists to fix.
        let mut cfg = get_culling_config("default");
        cfg.ranking.sharpness_peak_weight = 0.0;
        let groups =
            group_and_sort_images_with_config(vec![shallow, busy], None, None, Some(1), &cfg);
        assert_eq!(winner_of(&groups), "busy");
    }

    /// A three-stop bracket must come back with every frame kept. Before set
    /// detection the dark and bright frames tripped the exposure reject rules
    /// and the sequence was offered up for deletion.
    #[test]
    fn a_bracket_group_nominates_nothing_for_rejection() {
        let hash = Some(0xabcd_ef01_2345_6789);
        let frame = |id: &str, t: f64, ev: f64, exposure: f64, shadow: f64, highlight: f64| {
            with_metrics(
                rec(id, Some(t), hash),
                &[
                    ("exposure_bias", ev),
                    ("cull_sharpness", 0.8),
                    ("cull_sharpness_peak", 0.85),
                    ("cull_exposure", exposure),
                    ("cull_noise", 0.1),
                    ("cull_shadow_clip", shadow),
                    ("cull_highlight_clip", highlight),
                ],
            )
        };
        let groups = run(vec![
            frame("under", 1000.0, -2.0, 0.15, 0.30, 0.0),
            frame("normal", 1000.5, 0.0, 0.85, 0.0, 0.0),
            frame("over", 1001.0, 2.0, 0.18, 0.0, 0.28),
        ]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group_type, "bracket");
        assert!(groups[0].keep_all());
        assert!(
            groups[0].reject_candidate_photo_ids.is_empty(),
            "a bracket must never nominate rejects, got {:?}",
            groups[0].reject_candidate_photo_ids
        );
        assert_eq!(
            groups[0].alternate_photo_ids.len(),
            2,
            "the non-representative frames stay alternates, not rejects"
        );
        for photo in &groups[0].photos {
            assert!(
                photo.reason_codes.iter().any(|c| c == "bracket_frame_kept"),
                "{}: {:?}",
                photo.photo_id,
                photo.reason_codes
            );
        }
    }

    /// The same three frames without exposure compensation are an ordinary
    /// burst, and the badly exposed ones must still be nominated. This is the
    /// guard against set detection swallowing real culls.
    #[test]
    fn the_same_frames_without_exposure_bias_still_get_culled() {
        let hash = Some(0xabcd_ef01_2345_6789);
        let frame = |id: &str, t: f64, exposure: f64, shadow: f64| {
            with_metrics(
                rec(id, Some(t), hash),
                &[
                    ("cull_sharpness", 0.8),
                    ("cull_sharpness_peak", 0.85),
                    ("cull_exposure", exposure),
                    ("cull_noise", 0.1),
                    ("cull_shadow_clip", shadow),
                ],
            )
        };
        let groups = run(vec![
            frame("under", 1000.0, 0.15, 0.30),
            frame("normal", 1000.5, 0.85, 0.0),
            frame("over", 1001.0, 0.18, 0.0),
        ]);
        assert_eq!(groups.len(), 1);
        assert_ne!(groups[0].group_type, "bracket");
        assert!(!groups[0].keep_all());
        assert!(
            !groups[0].reject_candidate_photo_ids.is_empty(),
            "badly exposed burst frames are still reject candidates"
        );
    }

    /// A deliberately low-key set: every frame sits far from the absolute
    /// mid-grey target, so the absolute score cannot separate them by much.
    /// Relative normalisation restores the ordering within the group.
    #[test]
    fn relative_normalisation_separates_a_uniformly_dark_group() {
        let hash = Some(0x1234_5678_9abc_def0);
        let frame = |id: &str, t: f64, exposure: f64| {
            with_metrics(
                rec(id, Some(t), hash),
                &[
                    ("cull_sharpness", 0.6),
                    ("cull_sharpness_peak", 0.6),
                    ("cull_exposure", exposure),
                    ("cull_noise", 0.2),
                ],
            )
        };
        let records = vec![
            frame("darkest", 1000.0, 0.10),
            frame("best", 1000.5, 0.28),
            frame("dark", 1001.0, 0.18),
        ];

        let mut relative = get_culling_config("default");
        relative.ranking.relative_normalization_weight = 0.8;
        let with_relative =
            group_and_sort_images_with_config(records.clone(), None, None, Some(1), &relative);
        assert_eq!(winner_of(&with_relative), "best");

        // The absolute-only run agrees on the winner here — ordering by a
        // monotone transform of one metric cannot disagree. What changes is the
        // *spread*: relative scoring pulls the group apart so the reject rules
        // and `reject_score_delta` can act on it at all.
        let mut absolute = get_culling_config("default");
        absolute.ranking.relative_normalization_weight = 0.0;
        let with_absolute =
            group_and_sort_images_with_config(records, None, None, Some(1), &absolute);
        let spread = |groups: &[Group]| {
            let scores: Vec<f64> = groups[0].photos.iter().map(|p| p.cull_score).collect();
            scores.iter().cloned().fold(f64::MIN, f64::max)
                - scores.iter().cloned().fold(f64::MAX, f64::min)
        };
        assert!(
            spread(&with_relative) > spread(&with_absolute) * 1.5,
            "relative {} should clearly exceed absolute {}",
            spread(&with_relative),
            spread(&with_absolute)
        );
    }

    /// A soft frame with strongly directional gradients is reported as shake,
    /// not as missed focus. Both still count as blurred for the score.
    #[test]
    fn directional_softness_is_labelled_motion_blur() {
        let hash = Some(0x0f0f_0f0f_0f0f_0f0f);
        let shaken = with_metrics(
            rec("shaken", Some(1000.0), hash),
            &[
                ("cull_sharpness", 0.05),
                ("cull_sharpness_peak", 0.06),
                ("cull_motion_anisotropy", 0.9),
                ("cull_exposure", 0.7),
            ],
        );
        let defocused = with_metrics(
            rec("defocused", Some(1000.5), hash),
            &[
                ("cull_sharpness", 0.05),
                ("cull_sharpness_peak", 0.06),
                ("cull_motion_anisotropy", 0.1),
                ("cull_exposure", 0.7),
            ],
        );
        let groups = run(vec![shaken, defocused]);
        let code_for = |id: &str| -> Vec<String> {
            groups[0]
                .photos
                .iter()
                .find(|p| p.photo_id == id)
                .unwrap()
                .reason_codes
                .clone()
        };
        assert!(code_for("shaken").contains(&"motion_blur".to_string()));
        assert!(code_for("defocused").contains(&"blurred".to_string()));
    }

    /// The IQA score, when the API layer supplies one, must move the aesthetic
    /// term — otherwise the whole CLIP-IQA path is decorative.
    #[test]
    fn an_injected_iqa_score_overrides_the_heuristic() {
        let hash = Some(0x5555_5555_5555_5555);
        // Identical technically; the heuristic prefers `punchy`, IQA prefers
        // `muted`. IQA carries 0.8 of the aesthetic term by default.
        let punchy = with_metrics(
            rec("punchy", Some(1000.0), hash),
            &[
                ("cull_sharpness", 0.7),
                ("cull_exposure", 0.7),
                ("cull_noise", 0.1),
                ("cull_aesthetic", 0.9),
                ("cull_aesthetic_iqa", 0.2),
            ],
        );
        let muted = with_metrics(
            rec("muted", Some(1000.5), hash),
            &[
                ("cull_sharpness", 0.7),
                ("cull_exposure", 0.7),
                ("cull_noise", 0.1),
                ("cull_aesthetic", 0.2),
                ("cull_aesthetic_iqa", 0.9),
            ],
        );
        assert_eq!(winner_of(&run(vec![punchy, muted])), "muted");
    }

    /// The soccer case. Two frames from one burst that are technically
    /// indistinguishable except that the *softer* one is where the ball is.
    /// Sharpness alone crowns the empty frame; that is the complaint this
    /// signal exists to answer.
    #[test]
    fn the_moment_score_can_outrank_a_marginally_sharper_empty_frame() {
        let hash = Some(0x0f0f_0f0f_0f0f_0f0f);
        let action = with_metrics(
            rec("action", Some(1000.0), hash),
            &[
                ("cull_sharpness", 0.60),
                ("cull_sharpness_peak", 0.62),
                ("cull_exposure", 0.7),
                ("cull_noise", 0.2),
                ("cull_semantic_iqa", 0.92),
            ],
        );
        let empty = with_metrics(
            rec("empty", Some(1000.2), hash),
            &[
                ("cull_sharpness", 0.66),
                ("cull_sharpness_peak", 0.68),
                ("cull_exposure", 0.7),
                ("cull_noise", 0.2),
                ("cull_semantic_iqa", 0.12),
            ],
        );

        let sports = get_culling_config("sports");
        let groups = group_and_sort_images_with_config(
            vec![action.clone(), empty.clone()],
            None,
            None,
            Some(3),
            &sports,
        );
        assert_eq!(winner_of(&groups), "action");
        assert!(
            groups[0].photos[0]
                .reason_codes
                .iter()
                .any(|c| c == "peak_action"),
            "the pick must say why: {:?}",
            groups[0].photos[0].reason_codes
        );

        // `default` names no axis at all — it spans every genre, and there is
        // no semantic question that is right for all of them — so the same two
        // frames rank on sharpness alone.
        let groups = group_and_sort_images_with_config(
            vec![action, empty],
            None,
            None,
            Some(3),
            &get_culling_config("default"),
        );
        assert_eq!(winner_of(&groups), "empty");
    }

    /// Each genre preset must name an axis the API layer can actually resolve,
    /// and give it weight. A typo here would silently disable the signal for a
    /// whole genre, which is precisely the class of bug that left every preset
    /// knob dead before the config unification.
    #[test]
    fn every_preset_axis_is_a_known_name_with_a_weight() {
        // Mirrors `lrg_ml::clip_iqa::PromptSet::from_name`, which this crate
        // cannot reach. If a set is added there, add it here.
        const KNOWN: [&str; 3] = ["action", "expression", "candid"];
        let mut with_axis = 0;
        for preset in crate::culling_config::available_presets() {
            let rk = get_culling_config(preset).ranking;
            match rk.semantic_prompt_set {
                Some(axis) => {
                    assert!(KNOWN.contains(&axis), "{preset}: unknown axis {axis:?}");
                    assert!(
                        rk.semantic_weight > 0.0,
                        "{preset}: names {axis:?} but weights it 0, so it does nothing"
                    );
                    with_axis += 1;
                }
                None => assert_eq!(
                    rk.semantic_weight, 0.0,
                    "{preset}: weights a semantic axis it does not name"
                ),
            }
        }
        assert!(with_axis >= 3, "expected several genre presets to use one");
    }

    /// The reason code names the axis, not the machinery. "Best expression in
    /// the group" is feedback a photographer can argue with; "semantic 0.83" is
    /// not.
    #[test]
    fn each_axis_explains_itself_in_its_own_words() {
        let hash = Some(0x0f0f_0f0f_0f0f_0f0f);
        let strong = with_metrics(
            rec("strong", Some(1000.0), hash),
            &[
                ("cull_sharpness", 0.6),
                ("cull_sharpness_peak", 0.6),
                ("cull_exposure", 0.7),
                ("cull_noise", 0.2),
                ("cull_semantic_iqa", 0.95),
            ],
        );
        let weak = with_metrics(
            rec("weak", Some(1000.2), hash),
            &[
                ("cull_sharpness", 0.62),
                ("cull_sharpness_peak", 0.62),
                ("cull_exposure", 0.7),
                ("cull_noise", 0.2),
                ("cull_semantic_iqa", 0.10),
            ],
        );
        for (preset, expected) in [
            ("sports", "peak_action"),
            ("portrait", "best_expression"),
            ("street", "strongest_moment"),
            ("event", "strongest_moment"),
        ] {
            let groups = group_and_sort_images_with_config(
                vec![strong.clone(), weak.clone()],
                None,
                None,
                Some(3),
                &get_culling_config(preset),
            );
            let winner = &groups[0].photos[0];
            assert_eq!(winner.photo_id, "strong", "{preset}");
            assert!(
                winner.reason_codes.iter().any(|c| c == expected),
                "{preset}: expected {expected:?}, got {:?}",
                winner.reason_codes
            );
            assert_eq!(
                winner.metrics.get("semantic_axis").and_then(Value::as_str),
                get_culling_config(preset).ranking.semantic_prompt_set,
                "{preset}: the emitted axis must match the preset's"
            );
        }
    }

    /// The blend is capped at `moment_weight`, so a frame that is genuinely
    /// unusable cannot be promoted just because the ball is in it.
    #[test]
    fn a_high_moment_score_cannot_rescue_an_unusable_frame() {
        let hash = Some(0x0f0f_0f0f_0f0f_0f0f);
        let ruined = with_metrics(
            rec("ruined", Some(1000.0), hash),
            &[
                ("cull_sharpness", 0.01),
                ("cull_sharpness_peak", 0.01),
                ("cull_exposure", 0.05),
                ("cull_noise", 0.9),
                ("cull_semantic_iqa", 0.99),
            ],
        );
        let clean = with_metrics(
            rec("clean", Some(1000.2), hash),
            &[
                ("cull_sharpness", 0.75),
                ("cull_sharpness_peak", 0.85),
                ("cull_exposure", 0.72),
                ("cull_noise", 0.15),
                ("cull_semantic_iqa", 0.40),
            ],
        );
        let groups = group_and_sort_images_with_config(
            vec![ruined, clean],
            None,
            None,
            Some(3),
            &get_culling_config("sports"),
        );
        assert_eq!(winner_of(&groups), "clean");
    }

    /// Photos with no embedding carry no moment score, and must rank exactly as
    /// they did before the signal existed — that is every photo on the fast
    /// `tasks=cull` path.
    #[test]
    fn a_missing_moment_score_leaves_the_ranking_untouched() {
        let hash = Some(0x0f0f_0f0f_0f0f_0f0f);
        let make = |id: &str, t: f64, sharp: f64| {
            with_metrics(
                rec(id, Some(t), hash),
                &[
                    ("cull_sharpness", sharp),
                    ("cull_sharpness_peak", sharp),
                    ("cull_exposure", 0.7),
                    ("cull_noise", 0.2),
                ],
            )
        };
        let records = vec![make("a", 1000.0, 0.4), make("b", 1000.2, 0.8)];
        let sports = get_culling_config("sports");
        let with_moment =
            group_and_sort_images_with_config(records.clone(), None, None, Some(3), &sports);
        let mut without = sports;
        without.ranking.semantic_weight = 0.0;
        let no_moment = group_and_sort_images_with_config(records, None, None, Some(3), &without);
        assert_eq!(
            with_moment[0].photos[0].cull_score,
            no_moment[0].photos[0].cull_score
        );
        assert_eq!(winner_of(&with_moment), "b");
    }

    /// Hoisting the L2 norms out of the pair loop must not change any distance.
    #[test]
    fn pre_normed_cosine_matches_the_original() {
        let a: Vec<f32> = (0..64).map(|i| (i as f32 * 0.37).sin()).collect();
        let b: Vec<f32> = (0..64).map(|i| (i as f32 * 0.11).cos()).collect();
        let norm = |v: &[f32]| -> f64 { v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt() };
        let want = cosine_distance(Some(&a), Some(&b)).unwrap();
        let got = cosine_distance_pre(Some((&a, norm(&a))), Some((&b, norm(&b)))).unwrap();
        assert_eq!(want.to_bits(), got.to_bits(), "must be bit-identical");
    }
}
