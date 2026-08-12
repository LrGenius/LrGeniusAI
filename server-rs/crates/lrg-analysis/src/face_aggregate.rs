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
    let mean_of = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;

    // Which faces get a say in the "is anyone spoiling this frame" signals.
    // Anything much smaller than the biggest face is a bystander, not a
    // subject, and must not be able to veto the shot on its own.
    let largest_area = faces
        .iter()
        .map(|f| f.area_ratio)
        .fold(0.0f64, |a, b| a.max(b));
    let area_cutoff = largest_area * cfg.prominent_face_area_fraction;
    let prominent: Vec<usize> = (0..faces.len())
        .filter(|&i| faces[i].area_ratio >= area_cutoff)
        .collect();
    // `area_cutoff` is 0 when every face reports zero area, so this is only a
    // guard against an empty filter, never the normal path.
    let prominent: Vec<usize> = if prominent.is_empty() {
        (0..faces.len()).collect()
    } else {
        prominent
    };
    let worst_of = |v: &[f64]| prominent.iter().map(|&i| v[i]).fold(f64::MIN, f64::max);
    let best_of_prominent = |v: &[f64]| prominent.iter().map(|&i| v[i]).fold(f64::MAX, f64::min);

    // Sharpness and prominence stay "the best face in the frame": they answer
    // "is the subject sharp / big enough", which the strongest face settles.
    let face_sharpness = max_of(&sharpness);
    let face_prominence = max_of(&prominence);
    let face_visibility = mean_of(&visibility);
    // Eyes, blink and occlusion answer the opposite question — "is anyone
    // ruining this frame" — so they take the worst prominent face. These were
    // max/min/min, i.e. the *best* face, which scored a group shot with one
    // pair of open eyes among nine blinks as flawless.
    let eye_openness_agg = best_of_prominent(&eye_openness);
    let blink_penalty_agg = worst_of(&blink_penalty);
    let occlusion_agg = worst_of(&occlusion);

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

    /// The group-shot case this aggregation exists for: one person with their
    /// eyes open must not redeem a frame where everyone else blinked.
    #[test]
    fn one_open_pair_of_eyes_does_not_rescue_a_group_of_blinks() {
        let mut faces = vec![
            FaceMetricsInput {
                area_ratio: 0.05,
                eye_openness: 0.05,
                blink_penalty: 0.95,
                occlusion: Some(0.1),
                ..Default::default()
            };
            9
        ];
        faces.push(FaceMetricsInput {
            area_ratio: 0.05,
            eye_openness: 0.95,
            blink_penalty: 0.05,
            occlusion: Some(0.1),
            ..Default::default()
        });
        let m = aggregate_face_culling_metrics(&faces, &fm());
        assert_eq!(m.cull_eye_openness, 0.05, "worst prominent face decides");
        assert_eq!(m.cull_blink_penalty, 0.95);
        assert_eq!(m.cull_face_count, 10);
    }

    /// ...but a small bystander at the edge of the frame must not sink an
    /// otherwise good shot, or the signal stops discriminating between frames.
    #[test]
    fn a_tiny_background_face_cannot_veto_the_frame() {
        let faces = vec![
            FaceMetricsInput {
                area_ratio: 0.20, // the subject
                eye_openness: 0.9,
                blink_penalty: 0.1,
                occlusion: Some(0.1),
                ..Default::default()
            },
            FaceMetricsInput {
                area_ratio: 0.01, // 5% of the subject, well under the cutoff
                eye_openness: 0.02,
                blink_penalty: 0.98,
                occlusion: Some(0.9),
                ..Default::default()
            },
        ];
        let m = aggregate_face_culling_metrics(&faces, &fm());
        assert_eq!(m.cull_eye_openness, 0.9, "bystander must not count");
        assert_eq!(m.cull_blink_penalty, 0.1);
        assert_eq!(m.cull_occlusion, 0.1);
    }

    /// Two subjects of comparable size both count, so the weaker one decides.
    #[test]
    fn comparable_sized_faces_both_count() {
        let faces = vec![
            FaceMetricsInput {
                area_ratio: 0.20,
                eye_openness: 0.9,
                blink_penalty: 0.1,
                occlusion: Some(0.1),
                ..Default::default()
            },
            FaceMetricsInput {
                area_ratio: 0.15,
                eye_openness: 0.2,
                blink_penalty: 0.8,
                occlusion: Some(0.6),
                ..Default::default()
            },
        ];
        let m = aggregate_face_culling_metrics(&faces, &fm());
        assert_eq!(m.cull_eye_openness, 0.2);
        assert_eq!(m.cull_blink_penalty, 0.8);
        assert_eq!(m.cull_occlusion, 0.6);
    }

    /// Single-face photos are the common case and must be untouched by the
    /// prominence gate: with one face, worst and best are the same face.
    #[test]
    fn single_face_is_unaffected_by_the_prominence_gate() {
        let faces = vec![FaceMetricsInput {
            area_ratio: 0.3,
            eye_openness: 0.42,
            blink_penalty: 0.58,
            occlusion: Some(0.37),
            ..Default::default()
        }];
        let m = aggregate_face_culling_metrics(&faces, &fm());
        assert_eq!(m.cull_eye_openness, 0.42);
        assert_eq!(m.cull_blink_penalty, 0.58);
        assert_eq!(m.cull_occlusion, 0.37);
    }

    #[test]
    fn sharpness_and_prominence_still_take_the_strongest_face() {
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
        // "Is the subject sharp" — answered by the sharpest face.
        assert_eq!(m.cull_face_sharpness, 0.8);
        // "Is anyone spoiling the frame" — answered by the worst one. Both
        // faces have equal (zero) area here, so both are prominent.
        assert_eq!(m.cull_eye_openness, 0.3);
        assert_eq!(m.cull_blink_penalty, 0.6);
        assert_eq!(m.cull_occlusion, 0.4);
        assert_eq!(m.cull_face_count, 2);
    }
}
