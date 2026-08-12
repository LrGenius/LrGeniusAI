//! Port of `config.py`'s `BASE_CULLING_CONFIG` + `CULLING_PRESETS` +
//! `get_culling_config` (deep-merge preset overrides onto the base).
//! Centralizes every tunable weight/threshold used by grouping and
//! ranking so behavior stays identical to Python without hand-copying
//! magic numbers into each algorithm module.
//!
//! [`ImageMetricsConfig`] and [`FaceMetricsConfig`] are re-exported from
//! `lrg-imaging`, which is where the code that reads them lives (that crate
//! sits below both `lrg-ml` and this one). They used to be declared here *and*
//! duplicated as private `const`s next to each algorithm; only the `const`s
//! were ever read, so every preset override of an image or face metric was
//! silently discarded. There is now one definition and the presets reach it.
//!
//! Note which knobs a preset can actually move. Image metrics are computed once
//! at index time and stored, so the *threshold-shaped* fields (denominators,
//! exposure target, clip thresholds) are baked into the stored sub-scores and
//! only change on a re-index. The *weight-shaped* fields
//! (`technical_weight_*`, `aesthetic_*_weight`) are re-applied at rank time by
//! [`crate::grouping::rank_group_records`], so presets do move those per run.

pub use lrg_imaging::cull_config::{FaceMetricsConfig, ImageMetricsConfig};

#[derive(Debug, Clone, Copy)]
pub struct GroupingConfig {
    pub time_window_default_seconds: i64,
    pub phash_hamming_auto: f64,
    pub burst_distance_auto: f64,
    pub duplicate_distance_auto: f64,
    pub duplicate_distance_min: f64,
    pub duplicate_distance_span: f64,
    pub phash_max: f64,
    pub duplicate_time_window_multiplier: i64,
    pub duplicate_time_window_min_seconds: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct RankingConfig {
    pub face_group_weight_technical: f64,
    pub face_group_weight_face: f64,
    pub face_group_weight_aesthetic: f64,
    pub face_group_blink_penalty_weight: f64,
    pub face_group_occlusion_penalty_weight: f64,
    pub face_missing_technical_weight: f64,
    pub face_missing_penalty: f64,
    pub no_face_group_weight_aesthetic: f64,
    pub reason_blur_threshold: f64,
    pub reason_exposure_threshold: f64,
    pub reason_low_aesthetic_threshold: f64,
    pub reason_occlusion_threshold: f64,
    pub reason_sharpest_delta: f64,
    pub reason_best_face_delta: f64,
    pub reason_weak_face_delta: f64,
    pub reason_eyes_open_delta: f64,
    pub reason_possible_blink_threshold: f64,
    pub reject_score_delta: f64,
    pub reject_exposure_threshold: f64,
    pub reject_face_score_threshold: f64,
    pub reject_blink_penalty_threshold: f64,
    pub reject_occlusion_threshold: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct CullingConfig {
    pub grouping: GroupingConfig,
    pub image_metrics: ImageMetricsConfig,
    pub face_metrics: FaceMetricsConfig,
    pub ranking: RankingConfig,
}

const BASE: CullingConfig = CullingConfig {
    grouping: GroupingConfig {
        time_window_default_seconds: 1,
        phash_hamming_auto: 10.0,
        burst_distance_auto: 0.12,
        duplicate_distance_auto: 0.05,
        duplicate_distance_min: 0.02,
        duplicate_distance_span: 0.06,
        phash_max: 64.0,
        duplicate_time_window_multiplier: 4,
        duplicate_time_window_min_seconds: 10,
    },
    image_metrics: ImageMetricsConfig::defaults(),
    face_metrics: FaceMetricsConfig::defaults(),
    ranking: RankingConfig {
        face_group_weight_technical: 0.55,
        face_group_weight_face: 0.45,
        face_group_weight_aesthetic: 0.10,
        face_group_blink_penalty_weight: 0.10,
        face_group_occlusion_penalty_weight: 0.08,
        face_missing_technical_weight: 0.70,
        face_missing_penalty: 0.20,
        no_face_group_weight_aesthetic: 0.08,
        reason_blur_threshold: 0.20,
        reason_exposure_threshold: 0.35,
        reason_low_aesthetic_threshold: 0.35,
        reason_occlusion_threshold: 0.55,
        reason_sharpest_delta: 0.02,
        reason_best_face_delta: 0.03,
        reason_weak_face_delta: 0.10,
        reason_eyes_open_delta: 0.05,
        reason_possible_blink_threshold: 0.55,
        reject_score_delta: 0.18,
        reject_exposure_threshold: 0.28,
        reject_face_score_threshold: 0.30,
        reject_blink_penalty_threshold: 0.75,
        reject_occlusion_threshold: 0.75,
    },
};

pub fn get_culling_config(preset: &str) -> CullingConfig {
    let mut cfg = BASE;
    match preset.trim().to_lowercase().as_str() {
        "portrait" => {
            cfg.ranking.face_group_weight_technical = 0.34;
            cfg.ranking.face_group_weight_face = 0.66;
            cfg.ranking.face_group_weight_aesthetic = 0.18;
            cfg.ranking.face_group_blink_penalty_weight = 0.20;
            cfg.ranking.face_group_occlusion_penalty_weight = 0.18;
            cfg.ranking.reason_possible_blink_threshold = 0.40;
            cfg.ranking.reason_occlusion_threshold = 0.45;
            cfg.ranking.reason_low_aesthetic_threshold = 0.42;
            cfg.ranking.reject_blink_penalty_threshold = 0.55;
            cfg.ranking.reject_face_score_threshold = 0.35;
            cfg.ranking.reject_occlusion_threshold = 0.55;
        }
        "street" => {
            cfg.ranking.face_group_weight_technical = 0.70;
            cfg.ranking.face_group_weight_face = 0.30;
            cfg.ranking.face_group_weight_aesthetic = 0.14;
            cfg.ranking.face_group_blink_penalty_weight = 0.06;
            cfg.ranking.face_group_occlusion_penalty_weight = 0.04;
            cfg.ranking.reason_possible_blink_threshold = 0.65;
            cfg.ranking.reject_blink_penalty_threshold = 0.85;
            cfg.ranking.reject_score_delta = 0.22;
        }
        "event" => {
            cfg.grouping.time_window_default_seconds = 2;
            cfg.grouping.burst_distance_auto = 0.14;
            cfg.ranking.face_group_weight_technical = 0.48;
            cfg.ranking.face_group_weight_face = 0.52;
            cfg.ranking.face_group_weight_aesthetic = 0.14;
            cfg.ranking.face_group_blink_penalty_weight = 0.14;
            cfg.ranking.face_group_occlusion_penalty_weight = 0.10;
            cfg.ranking.reason_possible_blink_threshold = 0.50;
            cfg.ranking.reason_occlusion_threshold = 0.50;
            cfg.ranking.reason_low_aesthetic_threshold = 0.38;
            cfg.ranking.reject_blink_penalty_threshold = 0.62;
            cfg.ranking.reject_face_score_threshold = 0.33;
            cfg.ranking.reject_occlusion_threshold = 0.62;
            cfg.ranking.reject_score_delta = 0.20;
        }
        "sports" => {
            cfg.grouping.time_window_default_seconds = 3;
            cfg.grouping.burst_distance_auto = 0.16;
            cfg.ranking.face_group_weight_technical = 0.75;
            cfg.ranking.face_group_weight_face = 0.25;
            cfg.ranking.face_group_weight_aesthetic = 0.10;
            cfg.ranking.face_group_blink_penalty_weight = 0.04;
            cfg.ranking.face_group_occlusion_penalty_weight = 0.04;
            cfg.ranking.reason_blur_threshold = 0.15;
            cfg.ranking.reject_score_delta = 0.24;
            cfg.ranking.reason_possible_blink_threshold = 0.75;
            cfg.ranking.reject_blink_penalty_threshold = 0.92;
        }
        _ => {}
    }
    cfg
}

pub fn available_presets() -> Vec<&'static str> {
    vec!["default", "event", "portrait", "sports", "street"]
}
