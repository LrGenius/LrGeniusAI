//! Limits on an edit that come from the photograph rather than from taste.
//!
//! A style profile says what a photographer likes. It cannot say what *this*
//! frame can absorb, and applying a liked value to a frame that cannot take it
//! is how an automatic edit ruins a picture. The canonical case, and the one
//! that motivated this module: raising contrast on harshly lit material. The
//! photographer's own average edit may well carry `+25 contrast`, because most
//! of their frames were shot in kind light — applied to a midday frame already
//! split into blown highlights and blocked shadows, the same number destroys it.
//!
//! So these rules sit *above* whatever the profile wants, they are derived from
//! measurements rather than from preference, and every one that fires reports
//! why. That last part is deliberate and mirrors culling's `reason_codes`: a
//! surprising edit that explains itself is debuggable, and one that does not is
//! indistinguishable from a bug.
//!
//! The rules about faces read [`FaceConditions`], which the caller measures on
//! the same frame. Every one of them compares a face with itself or with its own
//! frame, and none calls a face too dark: skin ranges over several stops between
//! people, so a dark face in bright surroundings is just as often exposed
//! perfectly. All such a reading may do here is hold contrast back, which costs
//! that face nothing.
//!
//! What is **not** here yet, because it needs signals this crate cannot reach:
//! mixed-light detection (needs a regional illuminant estimate). [`EditBudget`]
//! is shaped so it slots in without changing callers.

use lrg_imaging::scene::SceneState;

/// What the camera was doing, as far as it changes what an edit may safely do.
///
/// Separate from [`SceneState`] because none of it is measurable from pixels:
/// two frames with identical histograms want different shadow treatment at
/// ISO 100 and ISO 6400, and only the metadata knows which is which.
#[derive(Debug, Clone, Copy, Default)]
pub struct CaptureConditions {
    pub iso: Option<f64>,
    /// Whether the source is a raw file. Decides whether clipped highlights are
    /// recoverable — `SceneState::highlight_clip` measures the rendering and
    /// deliberately does not guess at this.
    pub is_raw: bool,
}

/// What was measured about the faces in the frame, as far as it limits an edit.
/// The worst of the faces that matter in the frame speaks for it, and `None` is
/// "not measured": no faces, or no face model on this computer.
#[derive(Debug, Clone, Copy, Default)]
pub struct FaceConditions {
    /// Between the two cheeks of the hardest-lit face, in stops.
    pub across_face_stops: Option<f64>,
    /// The darkest face against the frame around it, in stops; negative is
    /// darker. A backlit face reads low, and so does dark skin in bright
    /// surroundings, which is why this only ever holds contrast back.
    pub against_surround_stops: Option<f64>,
    /// The largest share of a face's skin that is blown out, `0..1`.
    pub blown: Option<f64>,
}

/// Tunables for [`derive_budget`].
///
/// Every threshold here is a starting guess, chosen to be defensible rather
/// than fitted: no labelled set of real photographs exists yet to calibrate
/// against. They are configuration rather than constants so that calibrating
/// them later is a data change, not a rewrite — and so a preset can move them.
#[derive(Debug, Clone, Copy)]
pub struct GuardrailConfig {
    /// Above this `light_hardness`, the frame is treated as harshly lit.
    pub hard_light_threshold: f64,
    /// Rendered dynamic range, in stops, above which there is no tonal room
    /// left to spend on added contrast.
    pub high_dynamic_range_stops: f64,
    /// ...and below which contrast and clarity are the right tools, so their
    /// budget opens up.
    pub flat_dynamic_range_stops: f64,
    /// Fraction of specular/blown pixels above which the white point must not
    /// be pushed further.
    pub specular_fraction_threshold: f64,
    /// At or below this ISO, lifting shadows costs nothing.
    pub iso_shadow_free: f64,
    /// At or above this ISO, lifting shadows is mostly lifting noise.
    pub iso_shadow_exhausted: f64,
    /// Largest shadow lift, in Lightroom slider points, at low ISO.
    pub shadow_lift_max: f64,
    /// Largest total of contrast-increasing moves on an ordinary frame.
    pub contrast_budget_default: f64,
    /// ...and on a flat, softly lit one.
    pub contrast_budget_flat: f64,
    /// Largest clarity value on an ordinary frame.
    pub clarity_budget_default: f64,
    /// Light this many stops apart between the two cheeks is hard light on the
    /// face. 10% of faces in a sample of 600 portraits reach 2, nearly all of
    /// them real side light.
    pub face_hard_light_stops: f64,
    /// A face this many stops below the frame around it. 2.4% of that sample.
    pub face_below_surround_stops: f64,
    /// This share of a face's skin blown out; below it sit surgical masks and
    /// white headbands.
    pub face_blown_share: f64,
    /// Largest total of contrast-increasing moves on a face that needs it.
    pub contrast_budget_face: f64,
}

impl GuardrailConfig {
    pub const fn defaults() -> Self {
        GuardrailConfig {
            hard_light_threshold: 0.55,
            high_dynamic_range_stops: 6.5,
            flat_dynamic_range_stops: 3.5,
            specular_fraction_threshold: 0.02,
            iso_shadow_free: 400.0,
            iso_shadow_exhausted: 6400.0,
            shadow_lift_max: 60.0,
            contrast_budget_default: 25.0,
            contrast_budget_flat: 40.0,
            clarity_budget_default: 20.0,
            face_hard_light_stops: 2.0,
            face_below_surround_stops: 2.0,
            face_blown_share: 0.15,
            contrast_budget_face: 10.0,
        }
    }
}

impl Default for GuardrailConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Why a budget came out the way it did. One variant per rule, so a caller can
/// render an explanation without re-deriving the reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardrailReason {
    /// Harsh light: contrast-increasing moves are capped at zero.
    HardLightNoAddedContrast,
    /// The rendered range is already full; contrast would only clip further.
    NoTonalHeadroom,
    /// Flat, soft light: contrast and clarity are what this frame wants.
    FlatLightContrastAllowed,
    /// Blown or specular highlights with no raw headroom behind them.
    HighlightsUnrecoverable,
    /// High ISO: lifting shadows mostly lifts noise.
    HighIsoShadowsLimited,
    /// Shadows are already blocked; lifting them cannot recover detail.
    ShadowsAlreadyClipped,
    /// Hard light across a face: clarity and contrast would deepen the split.
    FaceHardLight,
    /// A face well below the frame around it: contrast would push it further.
    FaceBelowSurroundings,
    /// Blown skin on a face: the white point must not push it further.
    FaceBlown,
}

impl GuardrailReason {
    /// Stable identifier for logs, the API and the plugin, in the same style as
    /// culling's reason codes.
    pub fn code(self) -> &'static str {
        match self {
            GuardrailReason::HardLightNoAddedContrast => "hard_light_no_added_contrast",
            GuardrailReason::NoTonalHeadroom => "no_tonal_headroom",
            GuardrailReason::FlatLightContrastAllowed => "flat_light_contrast_allowed",
            GuardrailReason::HighlightsUnrecoverable => "highlights_unrecoverable",
            GuardrailReason::HighIsoShadowsLimited => "high_iso_shadows_limited",
            GuardrailReason::ShadowsAlreadyClipped => "shadows_already_clipped",
            GuardrailReason::FaceHardLight => "face_hard_light",
            GuardrailReason::FaceBelowSurroundings => "face_below_surroundings",
            GuardrailReason::FaceBlown => "face_blown",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            GuardrailReason::HardLightNoAddedContrast => {
                "Harsh light: contrast was not increased, since the frame is already split between lit and shadowed areas."
            }
            GuardrailReason::NoTonalHeadroom => {
                "The frame already spans the full tonal range, so added contrast would only clip it further."
            }
            GuardrailReason::FlatLightContrastAllowed => {
                "Flat, soft light: contrast and clarity were allowed a wider range than usual."
            }
            GuardrailReason::HighlightsUnrecoverable => {
                "Highlights are blown with no raw headroom behind them, so the white point was left alone."
            }
            GuardrailReason::HighIsoShadowsLimited => {
                "High ISO: shadows were lifted conservatively to avoid raising noise with them."
            }
            GuardrailReason::ShadowsAlreadyClipped => {
                "Shadows are already blocked, so lifting them further would not recover detail."
            }
            GuardrailReason::FaceHardLight => {
                "Hard light falls across a face, so no clarity was added and contrast was limited: both would deepen the split."
            }
            GuardrailReason::FaceBelowSurroundings => {
                "A face sits well below the frame around it, so contrast was limited to keep it from going darker."
            }
            GuardrailReason::FaceBlown => {
                "Skin on a face is blown out, so the white point was held back."
            }
        }
    }
}

/// How much of each destructive move this frame can take.
///
/// Ceilings, not targets. A profile asking for less than a budget allows gets
/// what it asked for; the budget only ever removes.
#[derive(Debug, Clone)]
pub struct EditBudget {
    /// Ceiling on the sum of contrast-increasing moves — contrast, clarity,
    /// dehaze and the S-strength of a tone curve. They are budgeted together
    /// because they do the same thing to a histogram, and a rule that capped
    /// each one separately would be trivially defeated by spending the same
    /// increase across all four.
    pub contrast_budget: f64,
    /// Ceiling on clarity specifically, which is harsher than contrast on faces
    /// and on already-textured material.
    pub clarity_budget: f64,
    /// Ceiling on lifting shadows.
    pub shadow_lift_budget: f64,
    /// Ceiling on raising the white point.
    pub whites_budget: f64,
    pub reasons: Vec<GuardrailReason>,
}

impl EditBudget {
    /// The budget of a frame nothing is known about: generous enough not to
    /// interfere, which is the right failure direction for a rule that exists
    /// to prevent damage rather than to create a look.
    pub fn unconstrained() -> Self {
        EditBudget {
            contrast_budget: 100.0,
            clarity_budget: 100.0,
            shadow_lift_budget: 100.0,
            whites_budget: 100.0,
            reasons: Vec::new(),
        }
    }

    pub fn reason_codes(&self) -> Vec<&'static str> {
        self.reasons.iter().map(|r| r.code()).collect()
    }
}

/// Linear interpolation of `t` from `[a, b]` onto `[0, 1]`, clamped.
fn ramp(t: f64, a: f64, b: f64) -> f64 {
    if (b - a).abs() < f64::EPSILON {
        return if t >= b { 1.0 } else { 0.0 };
    }
    ((t - a) / (b - a)).clamp(0.0, 1.0)
}

/// Derive what this frame can absorb, from what was measured about it.
pub fn derive_budget(
    scene: &SceneState,
    capture: &CaptureConditions,
    cfg: &GuardrailConfig,
) -> EditBudget {
    let mut budget = EditBudget {
        contrast_budget: cfg.contrast_budget_default,
        clarity_budget: cfg.clarity_budget_default,
        shadow_lift_budget: cfg.shadow_lift_max,
        whites_budget: cfg.contrast_budget_default,
        reasons: Vec::new(),
    };

    // Harsh light. The rule the whole module exists for: the frame is already
    // carrying the contrast, and the photographer's habitual +contrast was
    // formed on frames that were not.
    if scene.light_hardness >= cfg.hard_light_threshold {
        budget.contrast_budget = 0.0;
        budget.clarity_budget = 0.0;
        budget
            .reasons
            .push(GuardrailReason::HardLightNoAddedContrast);
    }

    // Full rendered range, whatever the light was like. Distinct from hardness:
    // a backlit frame can span everything without a hard edge anywhere.
    if scene.dynamic_range_stops >= cfg.high_dynamic_range_stops {
        budget.contrast_budget = budget.contrast_budget.min(0.0);
        if !budget
            .reasons
            .contains(&GuardrailReason::HardLightNoAddedContrast)
        {
            budget.reasons.push(GuardrailReason::NoTonalHeadroom);
        }
    } else if scene.dynamic_range_stops <= cfg.flat_dynamic_range_stops
        && scene.light_hardness < cfg.hard_light_threshold
    {
        // The mirror image, and the reason this is a budget rather than a set of
        // prohibitions: a flat frame in soft light genuinely wants more contrast
        // than an average one, and a rule that could only ever subtract would
        // leave it looking muddy.
        budget.contrast_budget = cfg.contrast_budget_flat;
        budget
            .reasons
            .push(GuardrailReason::FlatLightContrastAllowed);
    }

    // Blown highlights. Raw usually holds something above the rendering, a JPEG
    // never does, so the same measurement means different things per source.
    if scene.specular_fraction >= cfg.specular_fraction_threshold
        || scene.highlight_clip >= cfg.specular_fraction_threshold
    {
        if capture.is_raw {
            budget.whites_budget = budget.whites_budget.min(cfg.contrast_budget_default * 0.5);
        } else {
            budget.whites_budget = 0.0;
            budget
                .reasons
                .push(GuardrailReason::HighlightsUnrecoverable);
        }
    }

    // ISO. Lifting shadows is free on a clean file and mostly lifts noise on a
    // dirty one, so the ceiling ramps down rather than switching off: there is
    // no ISO at which shadow recovery becomes worthless, only ISOs at which it
    // costs more than it returns.
    if let Some(iso) = capture.iso {
        let exhaustion = ramp(iso, cfg.iso_shadow_free, cfg.iso_shadow_exhausted);
        if exhaustion > 0.0 {
            budget.shadow_lift_budget = cfg.shadow_lift_max * (1.0 - 0.75 * exhaustion);
            budget.clarity_budget = budget
                .clarity_budget
                .min(cfg.clarity_budget_default * (1.0 - 0.5 * exhaustion));
            if exhaustion > 0.5 {
                budget.reasons.push(GuardrailReason::HighIsoShadowsLimited);
            }
        }
    }

    // Shadows that are already blocked hold nothing to recover; lifting them
    // raises the black point and greys the frame out.
    if scene.shadow_clip >= 0.25 {
        budget.shadow_lift_budget = budget.shadow_lift_budget.min(cfg.shadow_lift_max * 0.25);
        budget.reasons.push(GuardrailReason::ShadowsAlreadyClipped);
    }

    budget
}

/// What this frame can absorb, faces included. See [`FaceConditions`] for why
/// these rules only ever remove, and never call a face too dark.
pub fn derive_budget_with_faces(
    scene: &SceneState,
    capture: &CaptureConditions,
    faces: &FaceConditions,
    cfg: &GuardrailConfig,
) -> EditBudget {
    let mut budget = derive_budget(scene, capture, cfg);
    if faces
        .across_face_stops
        .is_some_and(|s| s >= cfg.face_hard_light_stops)
    {
        budget.clarity_budget = 0.0;
        budget.contrast_budget = budget.contrast_budget.min(cfg.contrast_budget_face);
        budget.reasons.push(GuardrailReason::FaceHardLight);
    }
    if faces
        .against_surround_stops
        .is_some_and(|s| s <= -cfg.face_below_surround_stops)
    {
        budget.contrast_budget = budget.contrast_budget.min(cfg.contrast_budget_face);
        budget.reasons.push(GuardrailReason::FaceBelowSurroundings);
    }
    // The same split as highlights in general: raw usually holds something above
    // the rendering, a JPEG never does.
    if faces.blown.is_some_and(|b| b >= cfg.face_blown_share) {
        budget.whites_budget = if capture.is_raw {
            budget.whites_budget.min(cfg.contrast_budget_default * 0.5)
        } else {
            0.0
        };
        budget.reasons.push(GuardrailReason::FaceBlown);
    }
    budget
}

/// The contrast-increasing moves in a recipe, summed.
///
/// Only the increases count. A profile that pulls contrast down and raises
/// clarity slightly is not spending from this budget on net, and treating the
/// sum of absolute values as spend would block edits that reduce contrast — the
/// exact edits a harshly lit frame needs.
pub fn contrast_spend(contrast: f64, clarity: f64, dehaze: f64, curve_strength: f64) -> f64 {
    contrast.max(0.0) + clarity.max(0.0) + dehaze.max(0.0) + curve_strength.max(0.0)
}

/// Scale a set of contrast-increasing moves so their total fits the budget.
///
/// Scaling rather than clamping each value keeps the profile's *proportions* —
/// someone who reaches for clarity over contrast still gets that, just less of
/// it. Negative values pass through untouched, since they spend nothing.
pub fn fit_contrast_moves(values: &mut [f64], budget: f64) -> bool {
    let spend: f64 = values.iter().map(|v| v.max(0.0)).sum();
    if spend <= budget || spend <= 0.0 {
        return false;
    }
    let scale = (budget / spend).clamp(0.0, 1.0);
    for v in values.iter_mut() {
        if *v > 0.0 {
            *v *= scale;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(hardness: f64, dr: f64) -> SceneState {
        let mut s = SceneState::unknown();
        s.light_hardness = hardness;
        s.dynamic_range_stops = dr;
        s
    }

    fn cfg() -> GuardrailConfig {
        GuardrailConfig::defaults()
    }

    #[test]
    fn harsh_light_forbids_added_contrast() {
        // The case this module exists for.
        let b = derive_budget(&scene(0.8, 7.0), &CaptureConditions::default(), &cfg());

        assert_eq!(b.contrast_budget, 0.0);
        assert_eq!(b.clarity_budget, 0.0);
        assert!(b
            .reasons
            .contains(&GuardrailReason::HardLightNoAddedContrast));
    }

    #[test]
    fn flat_soft_light_gets_a_wider_budget_than_average() {
        // A guardrail that could only subtract would leave flat frames muddy.
        let flat = derive_budget(&scene(0.1, 2.5), &CaptureConditions::default(), &cfg());
        let ordinary = derive_budget(&scene(0.3, 5.0), &CaptureConditions::default(), &cfg());

        assert!(flat.contrast_budget > ordinary.contrast_budget);
        assert!(flat
            .reasons
            .contains(&GuardrailReason::FlatLightContrastAllowed));
    }

    #[test]
    fn a_full_tonal_range_blocks_contrast_even_without_a_hard_edge() {
        // Backlight spans everything without producing a hard shadow edge, so
        // hardness alone would miss it.
        let b = derive_budget(&scene(0.2, 7.5), &CaptureConditions::default(), &cfg());

        assert_eq!(b.contrast_budget, 0.0);
        assert!(b.reasons.contains(&GuardrailReason::NoTonalHeadroom));
    }

    #[test]
    fn the_two_contrast_rules_do_not_both_report() {
        // Harsh light usually also fills the range; reporting both would read as
        // two independent findings when it is one situation.
        let b = derive_budget(&scene(0.9, 7.5), &CaptureConditions::default(), &cfg());
        assert!(b
            .reasons
            .contains(&GuardrailReason::HardLightNoAddedContrast));
        assert!(!b.reasons.contains(&GuardrailReason::NoTonalHeadroom));
    }

    #[test]
    fn blown_highlights_are_recoverable_on_raw_and_not_on_jpeg() {
        let mut s = scene(0.3, 5.0);
        s.specular_fraction = 0.05;

        let raw = derive_budget(
            &s,
            &CaptureConditions {
                is_raw: true,
                ..Default::default()
            },
            &cfg(),
        );
        let jpeg = derive_budget(
            &s,
            &CaptureConditions {
                is_raw: false,
                ..Default::default()
            },
            &cfg(),
        );

        assert!(
            raw.whites_budget > 0.0,
            "raw holds headroom above the render"
        );
        assert_eq!(jpeg.whites_budget, 0.0);
        assert!(jpeg
            .reasons
            .contains(&GuardrailReason::HighlightsUnrecoverable));
    }

    #[test]
    fn shadow_lift_shrinks_as_iso_rises() {
        let low = derive_budget(
            &scene(0.3, 5.0),
            &CaptureConditions {
                iso: Some(100.0),
                ..Default::default()
            },
            &cfg(),
        );
        let high = derive_budget(
            &scene(0.3, 5.0),
            &CaptureConditions {
                iso: Some(6400.0),
                ..Default::default()
            },
            &cfg(),
        );

        assert!(
            high.shadow_lift_budget < low.shadow_lift_budget,
            "ISO 6400 {} should allow less lift than ISO 100 {}",
            high.shadow_lift_budget,
            low.shadow_lift_budget
        );
        assert!(high
            .reasons
            .contains(&GuardrailReason::HighIsoShadowsLimited));
        assert!(high.shadow_lift_budget > 0.0, "never switches off entirely");
    }

    #[test]
    fn blocked_shadows_cannot_be_lifted_back() {
        let mut s = scene(0.3, 5.0);
        s.shadow_clip = 0.4;
        let b = derive_budget(&s, &CaptureConditions::default(), &cfg());

        assert!(b.shadow_lift_budget <= cfg().shadow_lift_max * 0.25);
        assert!(b.reasons.contains(&GuardrailReason::ShadowsAlreadyClipped));
    }

    #[test]
    fn an_unknown_frame_is_not_constrained() {
        let b = EditBudget::unconstrained();
        assert!(b.reasons.is_empty());
        assert!(b.contrast_budget >= 100.0);
    }

    #[test]
    fn only_increases_count_against_the_contrast_budget() {
        // A harshly lit frame needs contrast pulled *down*, and a rule reading
        // magnitude rather than direction would block exactly that.
        assert_eq!(contrast_spend(-30.0, 0.0, 0.0, 0.0), 0.0);
        assert_eq!(contrast_spend(10.0, 5.0, 0.0, 0.0), 15.0);

        let mut moves = [-30.0, 0.0];
        assert!(!fit_contrast_moves(&mut moves, 0.0));
        assert_eq!(moves, [-30.0, 0.0]);
    }

    #[test]
    fn fitting_preserves_the_proportions_a_profile_asked_for() {
        let mut moves = [30.0, 10.0];
        assert!(fit_contrast_moves(&mut moves, 20.0));

        let total: f64 = moves.iter().sum();
        assert!(
            (total - 20.0).abs() < 1e-9,
            "total {total} should hit budget"
        );
        // 3:1 going in, 3:1 coming out.
        assert!((moves[0] / moves[1] - 3.0).abs() < 1e-9);
    }

    #[test]
    fn fitting_is_a_no_op_when_the_recipe_is_already_within_budget() {
        let mut moves = [5.0, 5.0];
        assert!(!fit_contrast_moves(&mut moves, 25.0));
        assert_eq!(moves, [5.0, 5.0]);
    }

    #[test]
    fn reason_codes_are_stable_and_distinct() {
        let all = [
            GuardrailReason::HardLightNoAddedContrast,
            GuardrailReason::NoTonalHeadroom,
            GuardrailReason::FlatLightContrastAllowed,
            GuardrailReason::HighlightsUnrecoverable,
            GuardrailReason::HighIsoShadowsLimited,
            GuardrailReason::ShadowsAlreadyClipped,
        ];
        let codes: std::collections::HashSet<_> = all.iter().map(|r| r.code()).collect();
        assert_eq!(codes.len(), all.len());
        for reason in all {
            assert!(!reason.explanation().is_empty());
        }
    }

    fn with_faces(faces: FaceConditions, is_raw: bool) -> EditBudget {
        derive_budget_with_faces(
            &scene(0.3, 5.0),
            &CaptureConditions { iso: None, is_raw },
            &faces,
            &cfg(),
        )
    }

    #[test]
    fn no_face_reading_leaves_the_budget_as_it_was() {
        let plain = derive_budget(&scene(0.3, 5.0), &CaptureConditions::default(), &cfg());
        let budget = with_faces(FaceConditions::default(), false);
        assert_eq!(budget.contrast_budget, plain.contrast_budget);
        assert_eq!(budget.clarity_budget, plain.clarity_budget);
        assert_eq!(budget.whites_budget, plain.whites_budget);
        assert!(budget.reasons.is_empty());
    }

    #[test]
    fn hard_light_across_a_face_takes_clarity_and_most_of_the_contrast() {
        let budget = with_faces(
            FaceConditions {
                across_face_stops: Some(3.2),
                ..Default::default()
            },
            false,
        );
        assert_eq!(budget.clarity_budget, 0.0);
        assert_eq!(budget.contrast_budget, cfg().contrast_budget_face);
        assert_eq!(budget.reasons, vec![GuardrailReason::FaceHardLight]);

        let soft = with_faces(
            FaceConditions {
                across_face_stops: Some(0.8),
                ..Default::default()
            },
            false,
        );
        assert!(soft.reasons.is_empty());
    }

    /// This reading is as low on dark skin in bright surroundings as on a
    /// backlit face, so it may only ever hold contrast back: never the shadows a
    /// dim face could use, never the clarity.
    #[test]
    fn a_face_below_its_surroundings_only_holds_contrast_back() {
        let plain = with_faces(FaceConditions::default(), false);
        let budget = with_faces(
            FaceConditions {
                against_surround_stops: Some(-3.1),
                ..Default::default()
            },
            false,
        );
        assert_eq!(budget.contrast_budget, cfg().contrast_budget_face);
        assert_eq!(budget.shadow_lift_budget, plain.shadow_lift_budget);
        assert_eq!(budget.clarity_budget, plain.clarity_budget);
        assert_eq!(budget.reasons, vec![GuardrailReason::FaceBelowSurroundings]);

        let brighter = with_faces(
            FaceConditions {
                against_surround_stops: Some(1.5),
                ..Default::default()
            },
            false,
        );
        assert!(brighter.reasons.is_empty());
    }

    #[test]
    fn blown_skin_holds_the_white_point_back_by_what_the_source_can_recover() {
        let blown = FaceConditions {
            blown: Some(0.4),
            ..Default::default()
        };
        assert_eq!(with_faces(blown, false).whites_budget, 0.0);
        assert!(with_faces(blown, true).whites_budget > 0.0);
        assert_eq!(
            with_faces(blown, false).reasons,
            vec![GuardrailReason::FaceBlown]
        );
    }

    /// A ceiling never rises: a flat frame's wider contrast still gives way.
    #[test]
    fn face_rules_only_ever_remove() {
        let flat = derive_budget_with_faces(
            &scene(0.1, 3.0),
            &CaptureConditions::default(),
            &FaceConditions {
                across_face_stops: Some(3.0),
                ..Default::default()
            },
            &cfg(),
        );
        assert_eq!(flat.contrast_budget, cfg().contrast_budget_face);
    }
}
