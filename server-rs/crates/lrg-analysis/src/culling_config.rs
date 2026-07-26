//! Port of `config.py`'s `BASE_CULLING_CONFIG` + `CULLING_PRESETS` +
//! `get_culling_config` (deep-merge preset overrides onto the base).
//! Centralizes every tunable weight/threshold used by grouping and
//! ranking so behavior stays identical to Python without hand-copying
//! magic numbers into each algorithm module.

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
pub struct ImageMetricsConfig {
    pub sharpness_denominator: f64,
    pub highlight_threshold: f64,
    pub shadow_threshold: f64,
    pub highlight_clip_weight: f64,
    pub shadow_clip_weight: f64,
    pub exposure_target: f64,
    pub exposure_tolerance: f64,
    pub exposure_balance_weight: f64,
    pub exposure_clip_weight: f64,
    pub noise_denominator: f64,
    pub technical_weight_sharpness: f64,
    pub technical_weight_exposure: f64,
    pub technical_weight_noise: f64,
    pub aesthetic_contrast_weight: f64,
    pub aesthetic_colorfulness_weight: f64,
    pub aesthetic_exposure_weight: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct FaceMetricsConfig {
    pub face_sharpness_denominator: f64,
    pub eye_patch_ratio: f64,
    pub eye_patch_radius_min: i64,
    pub eye_patch_radius_max: i64,
    pub eye_openness_denominator: f64,
    pub prominence_normalizer: f64,
    pub visibility_det_weight: f64,
    pub visibility_center_weight: f64,
    pub score_weight_sharpness: f64,
    pub score_weight_prominence: f64,
    pub score_weight_visibility: f64,
    pub score_weight_eye_openness: f64,
    pub score_weight_occlusion: f64,
    pub occlusion_det_weight: f64,
    pub occlusion_center_weight: f64,
    pub occlusion_eye_weight: f64,
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
    image_metrics: ImageMetricsConfig {
        sharpness_denominator: 0.015,
        highlight_threshold: 0.98,
        shadow_threshold: 0.02,
        highlight_clip_weight: 2.5,
        shadow_clip_weight: 2.0,
        exposure_target: 0.5,
        exposure_tolerance: 0.35,
        exposure_balance_weight: 0.75,
        exposure_clip_weight: 0.25,
        noise_denominator: 0.08,
        technical_weight_sharpness: 0.5,
        technical_weight_exposure: 0.35,
        technical_weight_noise: 0.15,
        aesthetic_contrast_weight: 0.45,
        aesthetic_colorfulness_weight: 0.35,
        aesthetic_exposure_weight: 0.20,
    },
    face_metrics: FaceMetricsConfig {
        face_sharpness_denominator: 0.02,
        eye_patch_ratio: 0.08,
        eye_patch_radius_min: 2,
        eye_patch_radius_max: 8,
        eye_openness_denominator: 0.07,
        prominence_normalizer: 0.12,
        visibility_det_weight: 0.5,
        visibility_center_weight: 0.5,
        score_weight_sharpness: 0.35,
        score_weight_prominence: 0.25,
        score_weight_visibility: 0.20,
        score_weight_eye_openness: 0.20,
        score_weight_occlusion: 0.15,
        occlusion_det_weight: 0.55,
        occlusion_center_weight: 0.20,
        occlusion_eye_weight: 0.25,
    },
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
