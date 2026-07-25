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
    time_delta: i64,
    cfg: &CullingConfig,
) -> Thresholds {
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
    time_delta: i64,
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

    for i in 0..n {
        for j in (i + 1)..n {
            let (left, right) = (&records[i], &records[j]);
            let distance = cosine_distance(
                effective_embedding(&left.embedding),
                effective_embedding(&right.embedding),
            );
            let phash_distance = phash_hamming(left.phash, right.phash);

            let mut time_gap = None;
            if let (Some(lt), Some(rt)) = (left.capture_time, right.capture_time) {
                let gap = (rt - lt).abs();
                time_gap = Some(gap);
                if gap > th.duplicate_time_window_seconds
                    && distance.is_none()
                    && phash_distance.is_none()
                {
                    break;
                }
            }

            let is_near_duplicate = (phash_distance
                .is_some_and(|d| d as f64 <= th.phash_hamming_threshold as f64)
                || distance.is_some_and(|d| d <= th.duplicate_distance_threshold))
                && time_gap.is_none_or(|g| g <= th.duplicate_time_window_seconds);
            let is_burst = distance.is_some_and(|d| d <= th.burst_distance_threshold)
                && time_gap.is_some_and(|g| g <= th.time_window_seconds);

            if !is_near_duplicate && !is_burst {
                continue;
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
