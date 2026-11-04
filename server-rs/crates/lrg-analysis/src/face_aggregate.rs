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
    /// Focus of the face's sharper eye and of the face as a whole, `0..1`,
    /// measured at full resolution (`lrg_ml::face_quality::face_focus`).
    /// `None` when the face is too small to judge, or was indexed before the
    /// measurement existed.
    pub eye_focus: Option<f64>,
    pub face_focus: Option<f64>,
    /// How closed the eyes are, `0..1`, from MediaPipe's blendshapes
    /// (`lrg_ml::face_state`). `None` when the face state did not run for this
    /// face or could not judge the eyes (turned away, too small).
    pub eye_closed: Option<f64>,
    /// The lids are down on a lowered head (`EyeState::Down`). Such a face's
    /// `eye_closed` is `None`: looking down is not a blink.
    pub looking_down: bool,
    /// `mouthSmile`, `0..1`; `None` like `eye_closed`.
    pub smile: Option<f64>,
    /// `eyeSquint`, `0..1`; `None` like `eye_closed`.
    pub eye_squint: Option<f64>,
    /// How far from the lens the eyes point, in degrees, head turn included;
    /// `None` like `eye_closed`.
    pub gaze_off_lens: Option<f64>,
    /// How far the mouth is open, `0..1`; `None` like `eye_closed`.
    pub mouth_open: Option<f64>,
    /// The light on the skin (`lrg_ml::face_state::FaceLight`); `None` like
    /// `eye_closed`, or when no skin patch fell inside the image.
    pub luminance: Option<f64>,
    pub across_face_stops: Option<f64>,
    pub blown: Option<f64>,
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
    /// Eye focus of the *least* sharp prominent face whose eyes could be
    /// measured, `None` when no prominent face's could.
    pub cull_eye_focus: Option<f64>,
    /// The same for the whole face.
    pub cull_face_focus: Option<f64>,
    /// Eyes of the prominent face whose eyes are the most closed, from the
    /// face state model; `None` when no prominent face's eyes could be judged.
    pub cull_eye_closed: Option<f64>,
    /// That reading as `open`/`half`/`closed`, at the face-state thresholds, or
    /// `down` when eyes that would read open are next to a face looking down, or
    /// no other face could be judged.
    pub cull_eye_state: Option<&'static str>,
    /// A prominent face is looking down.
    pub cull_looking_down: bool,
    /// The weakest smile among the prominent faces: a group only smiles when
    /// everyone in it does.
    pub cull_smile: Option<f64>,
    /// The strongest squint among the prominent faces that are not smiling; a
    /// broad smile narrows the eyes too, and that is not a squint.
    pub cull_eye_squint: Option<f64>,
    /// The prominent face looking furthest from the lens, in degrees.
    pub cull_gaze_off_lens: Option<f64>,
    /// The most open mouth among the prominent faces not smiling.
    pub cull_mouth_open: Option<f64>,
    /// The darkest prominent face. Only ever compared with this same frame's
    /// siblings: skin ranges over several stops between people.
    pub cull_face_luminance: Option<f64>,
    /// The hardest light across a prominent face, in stops.
    pub cull_face_across_stops: Option<f64>,
    /// The largest share of a prominent face's skin that is blown out.
    pub cull_face_blown: Option<f64>,
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
            cull_eye_focus: None,
            cull_face_focus: None,
            cull_eye_closed: None,
            cull_eye_state: None,
            cull_looking_down: false,
            cull_smile: None,
            cull_eye_squint: None,
            cull_gaze_off_lens: None,
            cull_mouth_open: None,
            cull_face_luminance: None,
            cull_face_across_stops: None,
            cull_face_blown: None,
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
    // A missing per-face occlusion used to be reconstructed here from
    // `det_score`, `center_proximity` and `eye_openness`. That was possible
    // only because the stored value was itself just those three numbers
    // recombined. It is now a measurement over the aligned face crop
    // (`lrg_ml::face_quality::occlusion_from_crop`) and nothing in this record
    // can stand in for it.
    //
    // So absent means absent, and absent scores 0. The alternative — guessing
    // high — would let an old `FACE_TABLE` row trip `reject_occlusion_threshold`
    // on no evidence, and a reject nominated from missing data is worse than a
    // reject missed. Photos indexed before this shipped therefore rank on their
    // other face signals until they are re-indexed.
    let occlusion: Vec<f64> = faces
        .iter()
        .map(|f| f.occlusion.map(unit).unwrap_or(0.0))
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
    // Focus answers both questions at once, and the second one decides: a
    // couple or a group is only sharp when every prominent face is, so the
    // weakest measured one speaks for the frame. Faces too small to measure
    // abstain rather than count as soft — a bystander's 20-pixel eyes say
    // nothing about where the focus went.
    let least_focused = |pick: fn(&FaceMetricsInput) -> Option<f64>| {
        prominent
            .iter()
            .filter_map(|&i| pick(&faces[i]).map(unit))
            .fold(None, |acc: Option<f64>, v| {
                Some(acc.map_or(v, |a| a.min(v)))
            })
    };
    let eye_focus_agg = least_focused(|f| f.eye_focus);
    let face_focus_agg = least_focused(|f| f.face_focus);
    // The face state answers the "is anyone spoiling the frame" questions too:
    // the most closed eyes and the face looking furthest away speak for it, the
    // weakest smile says whether the group is smiling. Faces it could not judge
    // abstain, exactly as for focus.
    let measured = |pick: fn(&FaceMetricsInput) -> Option<f64>| {
        prominent
            .iter()
            .filter_map(move |&i| pick(&faces[i]).map(unit))
    };
    let eye_closed_agg = measured(|f| f.eye_closed).reduce(f64::max);
    let smile_agg = measured(|f| f.smile).reduce(f64::min);
    let eye_squint_agg = prominent
        .iter()
        .map(|&i| &faces[i])
        .filter(|f| f.smile.is_some_and(|s| s < cfg.face_state_squint_max_smile))
        .filter_map(|f| f.eye_squint.map(unit))
        .reduce(f64::max);
    // Degrees, so not through `measured`'s clamp to `0..1`. Shut eyes look
    // nowhere: their `eyeLook*` is noise (31° on a portrait with the eyes shut),
    // so such a face abstains.
    let gaze_off_lens_agg = prominent
        .iter()
        .map(|&i| &faces[i])
        .filter(|f| {
            f.eye_closed
                .is_none_or(|c| c < cfg.face_state_closed_threshold)
        })
        .filter_map(|f| f.gaze_off_lens)
        .reduce(f64::max);
    let mouth_open_agg = prominent
        .iter()
        .map(|&i| &faces[i])
        // A lowered head foreshortens the lips, and a closed mouth reads open.
        .filter(|f| !f.looking_down)
        .filter(|f| f.smile.is_some_and(|s| s < cfg.face_state_mouth_max_smile))
        .filter_map(|f| f.mouth_open.map(unit))
        .reduce(f64::max);
    let luminance_agg = measured(|f| f.luminance).reduce(f64::min);
    let blown_agg = measured(|f| f.blown).reduce(f64::max);
    // Stops, so not through `measured`'s clamp to `0..1`.
    let across_agg = prominent
        .iter()
        .filter_map(|&i| faces[i].across_face_stops)
        .reduce(f64::max);
    let looking_down_agg = prominent.iter().any(|&i| faces[i].looking_down);
    let eye_state_agg = match eye_closed_agg {
        Some(v) if v >= cfg.face_state_closed_threshold => Some("closed"),
        Some(v) if v >= cfg.face_state_half_threshold => Some("half"),
        // Someone's eyes are not on the frame, so "everyone's eyes open" is out.
        _ if looking_down_agg => Some("down"),
        Some(_) => Some("open"),
        None => None,
    };

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
        cull_eye_focus: eye_focus_agg.map(round4),
        cull_face_focus: face_focus_agg.map(round4),
        cull_eye_closed: eye_closed_agg.map(round4),
        cull_eye_state: eye_state_agg,
        cull_looking_down: looking_down_agg,
        cull_smile: smile_agg.map(round4),
        cull_eye_squint: eye_squint_agg.map(round4),
        cull_gaze_off_lens: gaze_off_lens_agg.map(round4),
        cull_mouth_open: mouth_open_agg.map(round4),
        cull_face_luminance: luminance_agg.map(round4),
        cull_face_across_stops: across_agg.map(round4),
        cull_face_blown: blown_agg.map(round4),
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
            eye_focus: Some(0.7),
            face_focus: Some(0.55),
            eye_closed: Some(0.03),
            looking_down: false,
            smile: Some(0.9),
            eye_squint: Some(0.4),
            gaze_off_lens: Some(4.0),
            mouth_open: Some(0.02),
            luminance: Some(0.3),
            across_face_stops: Some(0.4),
            blown: Some(0.0),
        }];
        let m = aggregate_face_culling_metrics(&faces, &fm());
        assert_eq!(m.cull_face_count, 1);
        assert!(m.cull_face_score > 0.95, "score {}", m.cull_face_score);
        assert_eq!(m.cull_blink_penalty, 0.0);
        assert!(m.cull_faces_present);
    }

    /// This used to assert the opposite: a missing occlusion was reconstructed
    /// as `1 - (0.55·det + 0.20·center + 0.25·eye)`. That only worked because
    /// the stored value *was* those three numbers, and it is now a measurement
    /// over the aligned face crop that nothing here can stand in for.
    ///
    /// Absent must therefore score 0, not high. A row indexed before the
    /// measurement existed carries no evidence of occlusion, and inventing some
    /// would trip `reject_occlusion_threshold` and nominate the photo for
    /// deletion on the strength of a missing field.
    #[test]
    fn a_missing_occlusion_field_scores_zero_rather_than_being_guessed() {
        let inputs = |occlusion| {
            vec![FaceMetricsInput {
                det_score: 0.5,
                center_proximity: 0.5,
                eye_openness: 0.5,
                occlusion,
                ..Default::default()
            }]
        };
        let absent = aggregate_face_culling_metrics(&inputs(None), &fm());
        let zero = aggregate_face_culling_metrics(&inputs(Some(0.0)), &fm());
        assert_eq!(absent.cull_occlusion, 0.0);
        assert_eq!(absent.cull_occlusion, zero.cull_occlusion);

        // A real stored measurement still comes through untouched.
        let measured = aggregate_face_culling_metrics(&inputs(Some(0.8)), &fm());
        assert!((measured.cull_occlusion - 0.8).abs() < 1e-9);
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

    /// One person blinking spoils a couple's portrait; the smile of the
    /// group is its weakest; a bystander and an unjudged face abstain.
    #[test]
    fn face_state_takes_the_worst_judged_prominent_face() {
        let faces = vec![
            FaceMetricsInput {
                area_ratio: 0.20,
                eye_closed: Some(0.04),
                smile: Some(0.9),
                gaze_off_lens: Some(32.5),
                ..Default::default()
            },
            FaceMetricsInput {
                area_ratio: 0.18,
                eye_closed: Some(0.81),
                smile: Some(0.6),
                gaze_off_lens: Some(40.0),
                ..Default::default()
            },
            // Turned away: the model could not judge the eyes.
            FaceMetricsInput {
                area_ratio: 0.19,
                ..Default::default()
            },
            // A bystander far smaller: does not get a say.
            FaceMetricsInput {
                area_ratio: 0.01,
                eye_closed: Some(0.99),
                smile: Some(0.0),
                gaze_off_lens: Some(70.0),
                ..Default::default()
            },
        ];
        let m = aggregate_face_culling_metrics(&faces, &fm());
        assert_eq!(m.cull_eye_closed, Some(0.81));
        assert_eq!(m.cull_eye_state, Some("closed"));
        assert_eq!(m.cull_smile, Some(0.6));
        assert_eq!(m.cull_gaze_off_lens, Some(32.5));

        let open = aggregate_face_culling_metrics(&faces[..1], &fm());
        assert_eq!(open.cull_eye_state, Some("open"));
        let unjudged = aggregate_face_culling_metrics(&faces[2..3], &fm());
        assert_eq!(
            (unjudged.cull_eye_closed, unjudged.cull_eye_state),
            (None, None)
        );
    }

    #[test]
    fn a_face_looking_down_is_no_blink_but_the_eyes_are_not_all_open() {
        let down = FaceMetricsInput {
            area_ratio: 0.2,
            looking_down: true,
            ..Default::default()
        };
        let open = FaceMetricsInput {
            area_ratio: 0.2,
            eye_closed: Some(0.05),
            ..Default::default()
        };
        let shut = FaceMetricsInput {
            area_ratio: 0.2,
            eye_closed: Some(0.7),
            ..Default::default()
        };
        let alone = aggregate_face_culling_metrics(std::slice::from_ref(&down), &fm());
        assert!(alone.cull_looking_down);
        assert_eq!(alone.cull_eye_closed, None);
        assert_eq!(alone.cull_eye_state, Some("down"));

        let with_open = aggregate_face_culling_metrics(&[down, open], &fm());
        assert_eq!(with_open.cull_eye_closed, Some(0.05));
        assert_eq!(with_open.cull_eye_state, Some("down"));

        // A real blink next to it still speaks.
        let with_shut = aggregate_face_culling_metrics(&[down, shut], &fm());
        assert_eq!(with_shut.cull_eye_state, Some("closed"));
    }

    /// A laugh is open too, and not what the mouth reading is after.
    #[test]
    fn an_open_mouth_only_counts_on_a_face_that_is_not_smiling() {
        let face = |open, smile| FaceMetricsInput {
            area_ratio: 0.2,
            mouth_open: Some(open),
            smile: Some(smile),
            ..Default::default()
        };
        let laugh = aggregate_face_culling_metrics(&[face(0.6, 0.8)], &fm());
        assert_eq!(laugh.cull_mouth_open, None);
        let word = aggregate_face_culling_metrics(&[face(0.3, 0.1)], &fm());
        assert_eq!(word.cull_mouth_open, Some(0.3));
        let lowered = FaceMetricsInput {
            looking_down: true,
            ..face(0.44, 0.0)
        };
        let lowered = aggregate_face_culling_metrics(&[lowered], &fm());
        assert_eq!(lowered.cull_mouth_open, None);
    }

    #[test]
    fn the_light_takes_the_worst_prominent_face() {
        let face = |luminance, across, blown| FaceMetricsInput {
            area_ratio: 0.2,
            luminance: Some(luminance),
            across_face_stops: Some(across),
            blown: Some(blown),
            ..Default::default()
        };
        let group = [face(0.4, 0.5, 0.0), face(0.2, 3.1, 0.3)];
        let m = aggregate_face_culling_metrics(&group, &fm());
        assert_eq!(m.cull_face_luminance, Some(0.2));
        assert_eq!(m.cull_face_across_stops, Some(3.1));
        assert_eq!(m.cull_face_blown, Some(0.3));
    }

    #[test]
    fn eyes_that_are_shut_do_not_look_away() {
        let face = |closed, off| FaceMetricsInput {
            area_ratio: 0.2,
            eye_closed: Some(closed),
            gaze_off_lens: Some(off),
            ..Default::default()
        };
        let shut = aggregate_face_culling_metrics(&[face(0.66, 31.0)], &fm());
        assert_eq!(shut.cull_gaze_off_lens, None);
        let group = [face(0.66, 31.0), face(0.05, 12.0)];
        let group = aggregate_face_culling_metrics(&group, &fm());
        assert_eq!(group.cull_gaze_off_lens, Some(12.0));
    }

    /// `eyeSquint` runs high on a broad smile, so only an unsmiling face's
    /// counts as a squint; a face the smile was not read on says nothing.
    #[test]
    fn a_squint_only_counts_on_a_face_that_is_not_smiling() {
        let face = |squint, smile: Option<f64>| FaceMetricsInput {
            area_ratio: 0.2,
            eye_squint: Some(squint),
            smile,
            ..Default::default()
        };
        let smiling = aggregate_face_culling_metrics(&[face(0.9, Some(0.8))], &fm());
        assert_eq!(smiling.cull_eye_squint, None);
        let squinting = aggregate_face_culling_metrics(&[face(0.84, Some(0.1))], &fm());
        assert_eq!(squinting.cull_eye_squint, Some(0.84));
        let group = [face(0.9, Some(0.8)), face(0.6, Some(0.1))];
        let group = aggregate_face_culling_metrics(&group, &fm());
        assert_eq!(group.cull_eye_squint, Some(0.6));
        let unread = aggregate_face_culling_metrics(&[face(0.9, None)], &fm());
        assert_eq!(unread.cull_eye_squint, None);
    }

    #[test]
    fn eye_focus_is_the_least_sharp_measured_prominent_face() {
        let faces = vec![
            FaceMetricsInput {
                area_ratio: 0.20,
                eye_focus: Some(0.70),
                face_focus: Some(0.55),
                ..Default::default()
            },
            // The partner, as large, focus behind her eyes.
            FaceMetricsInput {
                area_ratio: 0.18,
                eye_focus: Some(0.48),
                face_focus: Some(0.50),
                ..Default::default()
            },
            // A bystander far smaller: does not get a say.
            FaceMetricsInput {
                area_ratio: 0.01,
                eye_focus: Some(0.20),
                face_focus: Some(0.10),
                ..Default::default()
            },
        ];
        let m = aggregate_face_culling_metrics(&faces, &fm());
        assert_eq!(m.cull_eye_focus, Some(0.48));
        assert_eq!(m.cull_face_focus, Some(0.50));
    }

    /// A face too small to measure abstains instead of reading as soft, and a
    /// frame where no prominent face could be measured has no eye focus.
    #[test]
    fn unmeasured_faces_abstain() {
        let faces = vec![
            FaceMetricsInput {
                area_ratio: 0.20,
                eye_focus: None,
                face_focus: Some(0.40),
                ..Default::default()
            },
            FaceMetricsInput {
                area_ratio: 0.19,
                eye_focus: Some(0.66),
                face_focus: None,
                ..Default::default()
            },
        ];
        let m = aggregate_face_culling_metrics(&faces, &fm());
        assert_eq!(m.cull_eye_focus, Some(0.66));
        assert_eq!(m.cull_face_focus, Some(0.40));

        let none = aggregate_face_culling_metrics(
            &[FaceMetricsInput {
                area_ratio: 0.2,
                ..Default::default()
            }],
            &fm(),
        );
        assert_eq!((none.cull_eye_focus, none.cull_face_focus), (None, None));
        assert_eq!(
            aggregate_face_culling_metrics(&[], &fm()).cull_eye_focus,
            None
        );
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
