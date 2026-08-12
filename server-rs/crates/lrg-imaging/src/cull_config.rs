//! Tunable parameters for the per-image and per-face culling metrics.
//!
//! These live here, below both `lrg-ml` and `lrg-analysis`, because the code
//! that consumes them is split across those two crates: `metrics::culling_metrics`
//! reads [`ImageMetricsConfig`], while `lrg_ml::face_quality` and
//! `lrg_analysis::face_aggregate` both read [`FaceMetricsConfig`]. Neither of
//! those crates can depend on the other, and `lrg-analysis`'s `CullingConfig`
//! (which owns grouping/ranking on top of these two) sits above both.
//!
//! History: every field here previously existed *twice* — once in
//! `lrg_analysis::culling_config`, where the culling presets set it, and again
//! as a private `const` next to each algorithm, which is what actually got
//! used. The preset copies were dead, so `portrait`/`sports`/`event` could not
//! move any image or face metric no matter what they set. These are now the
//! single source of truth and the presets reach them for real.
//!
//! [`Default`] reproduces `BASE_CULLING_CONFIG` exactly, so the golden tests
//! that pin metric output stay bit-identical.

/// Parameters for [`crate::metrics::culling_metrics`].
#[derive(Debug, Clone, Copy, PartialEq)]
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

impl ImageMetricsConfig {
    /// `const` so `lrg_analysis`'s `BASE` culling config can stay a `const`.
    pub const fn defaults() -> Self {
        ImageMetricsConfig {
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
        }
    }
}

impl Default for ImageMetricsConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Parameters for `lrg_ml::face_quality` and `lrg_analysis::face_aggregate`.
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// Which faces count as "prominent" when rolling per-face eye, blink and
    /// occlusion scores up to the photo, as a fraction of the largest face's
    /// area. A face below this is treated as background and cannot veto the
    /// frame.
    ///
    /// Photo-level eyes-open is the *worst* prominent face, not the best. A
    /// ten-person group shot where nine people blink and one has their eyes
    /// open is a reject, and taking the max — which is what this used to do —
    /// scored it as perfect. Without the size gate, though, any half-visible
    /// bystander at the edge of the frame would sink every frame equally and
    /// the signal would stop discriminating.
    pub prominent_face_area_fraction: f64,
}

impl FaceMetricsConfig {
    /// `const` so `lrg_analysis`'s `BASE` culling config can stay a `const`.
    pub const fn defaults() -> Self {
        FaceMetricsConfig {
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
            prominent_face_area_fraction: 0.25,
        }
    }
}

impl Default for FaceMetricsConfig {
    fn default() -> Self {
        Self::defaults()
    }
}
