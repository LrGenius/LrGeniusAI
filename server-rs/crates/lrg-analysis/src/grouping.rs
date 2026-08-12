//! Port of `services/chroma.py::group_and_sort_images` +
//! `_rank_group_records` + `_derive_grouping_thresholds`: groups photos
//! into near-duplicate/burst clusters (pHash + CLIP cosine + capture-time
//! adjacency, connected components) and ranks each group (winner /
//! alternates / reject candidates) with human-readable reason codes.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use crate::culling_config::{get_culling_config, CullingConfig};

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
}

struct Scored<'a> {
    record: &'a GroupingInput,
    cull_sharpness: f64,
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

fn rank_group_records(
    records: &[&GroupingInput],
    group_type: &str,
    cfg: &CullingConfig,
) -> Vec<RankedPhoto> {
    let fm = &cfg.face_metrics;
    let mut scored: Vec<Scored> = records
        .iter()
        .map(|record| {
            let m = &record.metadata;
            let sharpness = metric(m, "cull_sharpness", 0.0);
            let exposure = metric(m, "cull_exposure", 0.0);
            let noise_penalty = metric(m, "cull_noise", 1.0);
            let highlight_clip = metric(m, "cull_highlight_clip", 0.0);
            let shadow_clip = metric(m, "cull_shadow_clip", 0.0);
            let clipping_penalty = unit(highlight_clip + shadow_clip);
            let technical_default = 0.5 * sharpness
                + 0.3 * exposure
                + 0.1 * (1.0 - noise_penalty)
                + 0.1 * (1.0 - clipping_penalty);
            let technical_score = metric(m, "cull_technical_score", technical_default);
            let aesthetic_score = metric(m, "cull_aesthetic", 0.0);
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
        .map(|s| s.cull_sharpness)
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
        if item.cull_sharpness < rk.reason_blur_threshold {
            reason_codes.push("blurred".to_string());
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
            && item.cull_sharpness >= max_sharpness - rk.reason_sharpest_delta
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
        if index > 1 && group_type != "single" {
            reason_codes.push("near_duplicate_weaker".to_string());
        }

        let reject_candidate = scored.len() > 1
            && (item.cull_score <= (winner_score - rk.reject_score_delta).max(0.0)
                || item.cull_sharpness < rk.reason_blur_threshold
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
        let reject_candidate = reject_candidate && index != 1;

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
    mut records: Vec<GroupingInput>,
    phash_threshold: Option<f64>,
    clip_threshold: Option<f64>,
    time_delta: Option<i64>,
    culling_preset: &str,
) -> Vec<Group> {
    if records.is_empty() {
        return Vec::new();
    }
    let cfg = get_culling_config(culling_preset);
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

        let group_type: &'static str = if group_photo_ids.len() == 1 {
            "single"
        } else if group_edge_types.contains("near_duplicate") && !group_edge_types.contains("burst")
        {
            "near_duplicate"
        } else if time_span_seconds <= th.time_window_seconds {
            "burst"
        } else {
            "near_duplicate"
        };

        let ranked = rank_group_records(&component, group_type, &cfg);
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
