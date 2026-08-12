//! Port of `services/index.py::_aggregate_face_culling_metrics`: rolls a
//! photo's per-face quality scores into one set of photo-level culling
//! fields.
//!
//! Tunables come from [`FaceMetricsConfig`]; they used to be private `const`s
//! here that shadowed the identically-named preset fields, so no culling preset
//! could move them. `FaceMetricsConfig::defaults()` reproduces the old values
//! exactly.

use lrg_imaging::cull_config::FaceMetricsConfig;

fn unit(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

fn round4(v: f64) -> f64 {
    (v * 10000.0).round_ties_even() / 10000.0
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FaceMetricsInput {
    pub sharpness: f64,
    pub area_ratio: f64,
    pub det_score: f64,
    pub center_proximity: f64,
    pub eye_openness: f64,
    pub blink_penalty: f64,
    /// `None` means "absent from the record" (Python's `"occlusion" not
    /// in face`), which recomputes it from det/center/eye instead of
    /// defaulting to 0.0 — distinct from an explicit `Some(0.0)`.
    pub occlusion: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AggregatedFaceMetrics {
    pub cull_face_count: usize,
    pub cull_face_sharpness: f64,
    pub cull_face_prominence: f64,
    pub cull_face_visibility: f64,
    pub cull_face_score: f64,
    pub cull_eye_openness: f64,
    pub cull_blink_penalty: f64,
    pub cull_occlusion: f64,
    pub cull_faces_present: bool,
}

impl AggregatedFaceMetrics {
    fn empty() -> Self {
        AggregatedFaceMetrics {
            cull_face_count: 0,
            cull_face_sharpness: 0.0,
            cull_face_prominence: 0.0,
            cull_face_visibility: 0.0,
            cull_face_score: 0.0,
            cull_eye_openness: 0.0,
            cull_blink_penalty: 1.0,
            cull_occlusion: 0.0,
            cull_faces_present: false,
        }
    }
}

pub fn aggregate_face_culling_metrics(
    faces: &[FaceMetricsInput],
    cfg: &FaceMetricsConfig,
) -> AggregatedFaceMetrics {
    if faces.is_empty() {
        return AggregatedFaceMetrics::empty();
    }

    let sharpness: Vec<f64> = faces.iter().map(|f| unit(f.sharpness)).collect();
    let prominence: Vec<f64> = faces
        .iter()
        .map(|f| unit(f.area_ratio / cfg.prominence_normalizer))
        .collect();
    let visibility: Vec<f64> = faces
        .iter()
        .map(|f| {
            unit(
                cfg.visibility_det_weight * unit(f.det_score)
                    + cfg.visibility_center_weight * unit(f.center_proximity),
            )
        })
        .collect();
    let eye_openness: Vec<f64> = faces.iter().map(|f| unit(f.eye_openness)).collect();
    let blink_penalty: Vec<f64> = faces.iter().map(|f| unit(f.blink_penalty)).collect();
    let occlusion: Vec<f64> = faces
        .iter()
        .map(|f| match f.occlusion {
            Some(v) => unit(v),
            None => unit(
                1.0 - (cfg.occlusion_det_weight * unit(f.det_score)
                    + cfg.occlusion_center_weight * unit(f.center_proximity)
                    + cfg.occlusion_eye_weight * unit(f.eye_openness)),
            ),
        })
        .collect();

    let max_of = |v: &[f64]| v.iter().cloned().fold(f64::MIN, f64::max);
    let min_of = |v: &[f64]| v.iter().cloned().fold(f64::MAX, f64::min);
    let mean_of = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;

    let face_sharpness = max_of(&sharpness);
    let face_prominence = max_of(&prominence);
    let face_visibility = mean_of(&visibility);
    let eye_openness_agg = max_of(&eye_openness);
    let blink_penalty_agg = min_of(&blink_penalty);
    let occlusion_agg = min_of(&occlusion);

    let weight_total = cfg.score_weight_sharpness
        + cfg.score_weight_prominence
        + cfg.score_weight_visibility
        + cfg.score_weight_eye_openness
        + cfg.score_weight_occlusion;
    let face_score_raw = cfg.score_weight_sharpness * face_sharpness
        + cfg.score_weight_prominence * face_prominence
        + cfg.score_weight_visibility * face_visibility
        + cfg.score_weight_eye_openness * eye_openness_agg
        + cfg.score_weight_occlusion * (1.0 - occlusion_agg);
    let face_score = unit(face_score_raw / weight_total.max(1e-6));

    AggregatedFaceMetrics {
        cull_face_count: faces.len(),
        cull_face_sharpness: round4(face_sharpness),
        cull_face_prominence: round4(face_prominence),
        cull_face_visibility: round4(face_visibility),
        cull_face_score: round4(face_score),
        cull_eye_openness: round4(eye_openness_agg),
        cull_blink_penalty: round4(blink_penalty_agg),
        cull_occlusion: round4(occlusion_agg),
        cull_faces_present: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm() -> FaceMetricsConfig {
        FaceMetricsConfig::defaults()
    }

    #[test]
    fn empty_input_returns_documented_defaults() {
        let m = aggregate_face_culling_metrics(&[], &fm());
        assert_eq!(m.cull_face_count, 0);
        assert_eq!(m.cull_blink_penalty, 1.0);
        assert!(!m.cull_faces_present);
    }

    #[test]
    fn single_perfect_face_scores_near_one() {
        let faces = vec![FaceMetricsInput {
            sharpness: 1.0,
            area_ratio: fm().prominence_normalizer, // ratio 1.0 after normalize
            det_score: 1.0,
            center_proximity: 1.0,
            eye_openness: 1.0,
            blink_penalty: 0.0,
            occlusion: Some(0.0),
        }];
        let m = aggregate_face_culling_metrics(&faces, &fm());
        assert_eq!(m.cull_face_count, 1);
        assert!(m.cull_face_score > 0.95, "score {}", m.cull_face_score);
        assert_eq!(m.cull_blink_penalty, 0.0);
        assert!(m.cull_faces_present);
    }

    #[test]
    fn missing_occlusion_field_is_recomputed_not_zeroed() {
        let with_none = vec![FaceMetricsInput {
            det_score: 0.5,
            center_proximity: 0.5,
            eye_openness: 0.5,
            occlusion: None,
            ..Default::default()
        }];
        let with_zero = vec![FaceMetricsInput {
            det_score: 0.5,
            center_proximity: 0.5,
            eye_openness: 0.5,
            occlusion: Some(0.0),
            ..Default::default()
        }];
        let a = aggregate_face_culling_metrics(&with_none, &fm());
        let b = aggregate_face_culling_metrics(&with_zero, &fm());
        assert_ne!(a.cull_occlusion, b.cull_occlusion);
        // recomputed: 1 - (0.55*0.5 + 0.20*0.5 + 0.25*0.5) = 1 - 0.5 = 0.5
        assert!((a.cull_occlusion - 0.5).abs() < 1e-9);
    }

    #[test]
    fn multi_face_uses_max_min_mean_per_field() {
        let faces = vec![
            FaceMetricsInput {
                sharpness: 0.2,
                eye_openness: 0.9,
                blink_penalty: 0.1,
                occlusion: Some(0.1),
                ..Default::default()
            },
            FaceMetricsInput {
                sharpness: 0.8,
                eye_openness: 0.3,
                blink_penalty: 0.6,
                occlusion: Some(0.4),
                ..Default::default()
            },
        ];
        let m = aggregate_face_culling_metrics(&faces, &fm());
        assert_eq!(m.cull_face_sharpness, 0.8); // max
        assert_eq!(m.cull_eye_openness, 0.9); // max
        assert_eq!(m.cull_blink_penalty, 0.1); // min
        assert_eq!(m.cull_occlusion, 0.1); // min
        assert_eq!(m.cull_face_count, 2);
    }
}
