//! Where a frame's [`EditBudget`] meets a generated edit recipe.
//!
//! `lrg-analysis` derives what a photograph can absorb and `lrg-providers`
//! defines the recipe's shape; neither depends on the other, so the place the
//! two meet is here, in the crate that has both.
//!
//! The rule this enforces, in one sentence: a style profile says what a
//! photographer likes, and it cannot know what *this* frame can take. The
//! canonical case is contrast on harshly lit material — a habitual `+25`,
//! formed on frames shot in kind light, destroys a midday frame that is already
//! split into blown highlights and blocked shadows. See
//! [`lrg_analysis::edit_guardrails`] for the measurements behind each ceiling.
//!
//! Ceilings only ever remove. A recipe that asks for less than the budget
//! allows comes back untouched, and a negative move — pulling contrast *down*,
//! which is what a harsh frame actually wants — is never scaled.

use lrg_analysis::edit_guardrails::{
    derive_budget, fit_contrast_moves, CaptureConditions, EditBudget, GuardrailConfig,
};
use lrg_imaging::scene::{scene_state, SceneState, SceneStateConfig};
use serde_json::{json, Map, Value};

/// The plain recipe fields that add contrast, in the order
/// [`fit_contrast_moves`] sees them. The tone curve is budgeted alongside them
/// but is not a single field, so it rides in a fourth slot as a derived
/// strength and is scaled back afterwards.
const CONTRAST_FIELDS: [&str; 3] = ["contrast", "clarity", "dehaze"];

/// The parametric tone-curve fields, in the order they lift or drop the range.
/// The first two raise the top half, the last two drop the bottom.
const CURVE_FIELDS: [&str; 4] = ["highlights", "lights", "darks", "shadows"];

fn round4(v: f64) -> f64 {
    (v * 10000.0).round_ties_even() / 10000.0
}

fn number(map: &Map<String, Value>, key: &str) -> f64 {
    map.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// How much contrast a parametric tone curve adds, in slider points.
///
/// An S-curve lifts the top half and drops the bottom, so the four moves are
/// combined in the contrast-increasing direction and averaged. Averaged rather
/// than summed because the budget is expressed in the same units as `contrast`
/// itself, and a curve built from four ±25 moves is worth about as much as
/// `+25 contrast`, not `+100`.
fn curve_strength(curve: &Map<String, Value>) -> f64 {
    (number(curve, "highlights") + number(curve, "lights")
        - number(curve, "darks")
        - number(curve, "shadows"))
        / 2.0
}

/// Whether this recipe contains anything a budget could act on.
///
/// Checked before the frame is measured, because measuring means decoding the
/// image — pure waste for a recipe that only sets a white balance. Negative
/// moves do not count: they spend nothing, so there is nothing to cap.
pub fn recipe_needs_budget(recipe: &Value) -> bool {
    let Some(global) = recipe.get("global").and_then(Value::as_object) else {
        return false;
    };
    if global
        .get("tone_curve")
        .and_then(Value::as_object)
        .is_some_and(|curve| curve_strength(curve) > 0.0)
    {
        return true;
    }
    ["contrast", "clarity", "dehaze", "shadows", "whites"]
        .iter()
        .any(|field| number(global, field) > 0.0)
}

/// Cap one positive move. Returns whether it actually moved.
///
/// Negative values pass through: the budget limits how much damage an edit may
/// do, and pulling a slider down is never the damage it guards against.
fn cap_field(global: &mut Map<String, Value>, field: &str, ceiling: f64) -> bool {
    if let Some(value) = global.get(field).and_then(Value::as_f64) {
        if value > ceiling {
            global.insert(field.to_string(), json!(round4(ceiling)));
            return true;
        }
    }
    false
}

/// Apply a budget already derived for this frame.
///
/// Returns the reason codes, **or nothing when the recipe was left as it was**.
/// The distinction matters for what the user ends up reading: the codes explain
/// the frame, and `derive_budget` produces them for every rule that fires
/// whether or not the recipe happened to ask for the move it limits. Reporting
/// "contrast was not increased, since the frame is already split between lit
/// and shadowed areas" next to an edit that never wanted contrast is noise, and
/// noise in an explanation is how an explanation stops being read.
///
/// Split from [`measure_and_apply`] so the rules can be tested against a
/// constructed [`SceneState`] without going through a decode.
pub fn apply_budget(recipe: &mut Value, budget: &EditBudget) -> Vec<String> {
    let Some(global) = recipe.get_mut("global").and_then(Value::as_object_mut) else {
        return Vec::new();
    };

    // The contrast-increasing moves are budgeted *together*, because they do
    // the same thing to a histogram: capping each one separately would be
    // defeated by spending the same increase across all four.
    let curve_before = global
        .get("tone_curve")
        .and_then(Value::as_object)
        .map(curve_strength)
        .unwrap_or(0.0);
    let mut moves = [
        number(global, "contrast"),
        number(global, "clarity"),
        number(global, "dehaze"),
        curve_before,
    ];
    let mut changed = fit_contrast_moves(&mut moves, budget.contrast_budget);
    if changed {
        for (i, field) in CONTRAST_FIELDS.iter().enumerate() {
            if global.contains_key(*field) {
                global.insert((*field).to_string(), json!(round4(moves[i])));
            }
        }
        // The curve has no single field to write back to, so it is scaled by
        // the factor the fit applied to its strength. That keeps its shape —
        // someone whose profile reaches for a curve over the contrast slider
        // still gets a curve, just a shallower one.
        if curve_before > 0.0 {
            let scale = moves[3] / curve_before;
            if let Some(curve) = global.get_mut("tone_curve").and_then(Value::as_object_mut) {
                for field in CURVE_FIELDS {
                    if let Some(value) = curve.get(field).and_then(Value::as_f64) {
                        curve.insert(field.to_string(), json!(round4(value * scale)));
                    }
                }
            }
        }
    }

    // Then the per-field ceilings, which are harsher than the shared one for
    // the moves that damage a specific thing: clarity on faces and texture,
    // shadow lift on noise, whites on highlights with nothing behind them.
    // Written as separate statements rather than `||`-chained so that
    // short-circuiting cannot skip a cap once an earlier one has fired.
    changed |= cap_field(global, "clarity", budget.clarity_budget);
    changed |= cap_field(global, "shadows", budget.shadow_lift_budget);
    changed |= cap_field(global, "whites", budget.whites_budget);

    if !changed {
        return Vec::new();
    }
    budget
        .reason_codes()
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Measure the frame and constrain the recipe to what it can absorb.
///
/// Returns the reason codes for whatever fired, empty when nothing did. A
/// decode failure is not an error: the recipe is returned unconstrained, which
/// is exactly how the edit path behaved before guardrails existed.
pub fn measure_and_apply(
    recipe: &mut Value,
    image_bytes: &[u8],
    capture: &CaptureConditions,
) -> Vec<String> {
    if !recipe_needs_budget(recipe) {
        return Vec::new();
    }
    let Ok(decoded) = image::load_from_memory(image_bytes) else {
        log::warn!("Edit guardrails: could not decode the image; recipe left unconstrained.");
        return Vec::new();
    };
    let rgb = decoded.to_rgb8();
    let (width, height) = (rgb.width() as usize, rgb.height() as usize);
    let pixels = rgb.into_raw();
    let scene = scene_state(&pixels, width, height, &SceneStateConfig::defaults());
    apply_with_scene(recipe, &scene, capture)
}

/// [`measure_and_apply`] with the measurement already done.
pub fn apply_with_scene(
    recipe: &mut Value,
    scene: &SceneState,
    capture: &CaptureConditions,
) -> Vec<String> {
    let budget = derive_budget(scene, capture, &GuardrailConfig::defaults());
    apply_budget(recipe, &budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(hardness: f64, dynamic_range: f64) -> SceneState {
        let mut s = SceneState::unknown();
        s.light_hardness = hardness;
        s.dynamic_range_stops = dynamic_range;
        s
    }

    fn harsh() -> SceneState {
        scene(0.8, 7.0)
    }

    fn ordinary() -> SceneState {
        scene(0.3, 5.0)
    }

    fn recipe_with(global: Value) -> Value {
        json!({ "summary": "test", "global": global })
    }

    fn global_of(recipe: &Value) -> &Map<String, Value> {
        recipe["global"].as_object().unwrap()
    }

    /// The case the whole module exists for.
    #[test]
    fn harsh_light_removes_added_contrast() {
        let mut recipe = recipe_with(json!({"contrast": 40.0, "exposure": 0.3}));
        let reasons = apply_with_scene(&mut recipe, &harsh(), &CaptureConditions::default());

        assert_eq!(global_of(&recipe)["contrast"].as_f64().unwrap(), 0.0);
        assert!(reasons.contains(&"hard_light_no_added_contrast".to_string()));
        // Untouched fields stay untouched: the budget is not a rewrite.
        assert_eq!(global_of(&recipe)["exposure"].as_f64().unwrap(), 0.3);
    }

    #[test]
    fn a_recipe_within_budget_is_returned_unchanged() {
        let mut recipe = recipe_with(json!({"contrast": 10.0, "clarity": 5.0}));
        let before = recipe.clone();
        let reasons = apply_with_scene(&mut recipe, &ordinary(), &CaptureConditions::default());
        assert_eq!(recipe, before);
        assert!(
            reasons.is_empty(),
            "nothing was constrained, so nothing to explain"
        );
    }

    /// A frame can be harshly lit without the recipe ever asking for contrast.
    /// Explaining that contrast "was not increased" there would be noise next
    /// to an edit that was returned exactly as generated.
    #[test]
    fn a_frame_that_constrains_nothing_reports_no_reasons() {
        let mut recipe = recipe_with(json!({"exposure": 0.4, "vibrance": 10.0}));
        let reasons = apply_with_scene(&mut recipe, &harsh(), &CaptureConditions::default());
        assert!(reasons.is_empty(), "got {reasons:?}");
    }

    /// Scaling rather than clamping: someone who reaches for clarity over
    /// contrast still gets that, just less of it.
    #[test]
    fn over_budget_moves_are_scaled_in_proportion() {
        let mut recipe = recipe_with(json!({"contrast": 30.0, "clarity": 10.0}));
        apply_with_scene(&mut recipe, &ordinary(), &CaptureConditions::default());

        let g = global_of(&recipe);
        let (contrast, clarity) = (
            g["contrast"].as_f64().unwrap(),
            g["clarity"].as_f64().unwrap(),
        );
        // Default budget is 25; 3:1 going in, 3:1 coming out.
        assert!(
            (contrast + clarity - 25.0).abs() < 0.01,
            "{contrast}+{clarity}"
        );
        assert!((contrast / clarity - 3.0).abs() < 0.01);
    }

    /// Capping the four fields separately would be defeated by spending the
    /// same increase across all of them, so a tone curve counts too.
    #[test]
    fn a_tone_curve_spends_from_the_same_budget_and_keeps_its_shape() {
        let mut recipe = recipe_with(json!({
            "contrast": 20.0,
            "tone_curve": {"highlights": 30.0, "lights": 10.0, "darks": -10.0, "shadows": -30.0},
        }));
        apply_with_scene(&mut recipe, &ordinary(), &CaptureConditions::default());

        let g = global_of(&recipe);
        let curve = g["tone_curve"].as_object().unwrap();
        // Strength was (30+10+10+30)/2 = 40, plus 20 contrast = 60 against a
        // budget of 25, so everything scales by 25/60.
        let scale = 25.0 / 60.0;
        assert!((g["contrast"].as_f64().unwrap() - 20.0 * scale).abs() < 0.01);
        assert!((curve["highlights"].as_f64().unwrap() - 30.0 * scale).abs() < 0.01);
        // The S keeps its sign: the bottom half is still pulled down.
        assert!(curve["shadows"].as_f64().unwrap() < 0.0);
        assert!(
            (curve["shadows"].as_f64().unwrap() / curve["highlights"].as_f64().unwrap() + 1.0)
                .abs()
                < 0.01
        );
    }

    /// A harsh frame needs contrast pulled *down*, and a rule reading magnitude
    /// rather than direction would block exactly that.
    #[test]
    fn negative_moves_are_never_scaled() {
        let mut recipe = recipe_with(json!({"contrast": -30.0, "clarity": -10.0}));
        let before = recipe.clone();
        let reasons = apply_with_scene(&mut recipe, &harsh(), &CaptureConditions::default());
        assert_eq!(recipe, before);
        assert!(reasons.is_empty());
    }

    #[test]
    fn blown_highlights_block_the_white_point_on_a_rendered_source() {
        let mut s = ordinary();
        s.specular_fraction = 0.05;
        let mut recipe = recipe_with(json!({"whites": 40.0}));
        let reasons = apply_with_scene(
            &mut recipe,
            &s,
            &CaptureConditions {
                is_raw: false,
                ..Default::default()
            },
        );

        assert_eq!(global_of(&recipe)["whites"].as_f64().unwrap(), 0.0);
        assert!(reasons.contains(&"highlights_unrecoverable".to_string()));
    }

    /// Raw holds a stop or more above what its default rendering shows, so the
    /// same measurement means something different per source.
    #[test]
    fn the_same_frame_keeps_some_white_headroom_when_it_is_raw() {
        let mut s = ordinary();
        s.specular_fraction = 0.05;
        let mut recipe = recipe_with(json!({"whites": 40.0}));
        apply_with_scene(
            &mut recipe,
            &s,
            &CaptureConditions {
                is_raw: true,
                ..Default::default()
            },
        );

        assert!(global_of(&recipe)["whites"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn high_iso_limits_the_shadow_lift() {
        let mut recipe = recipe_with(json!({"shadows": 80.0}));
        let reasons = apply_with_scene(
            &mut recipe,
            &ordinary(),
            &CaptureConditions {
                iso: Some(6400.0),
                is_raw: true,
            },
        );

        assert!(global_of(&recipe)["shadows"].as_f64().unwrap() < 80.0);
        assert!(reasons.contains(&"high_iso_shadows_limited".to_string()));
    }

    /// An unknown ISO must not be read as a low one — it leaves the ceiling
    /// alone, which is the right direction for a rule that prevents damage.
    #[test]
    fn an_unknown_iso_does_not_limit_anything() {
        let mut recipe = recipe_with(json!({"shadows": 55.0}));
        apply_with_scene(&mut recipe, &ordinary(), &CaptureConditions::default());
        assert_eq!(global_of(&recipe)["shadows"].as_f64().unwrap(), 55.0);
    }

    #[test]
    fn a_recipe_with_nothing_to_cap_skips_the_measurement() {
        // No decode is attempted, so deliberately invalid image bytes are fine.
        let mut recipe = recipe_with(json!({"temperature": 5600.0, "contrast": -10.0}));
        assert!(!recipe_needs_budget(&recipe));
        let reasons =
            measure_and_apply(&mut recipe, b"not an image", &CaptureConditions::default());
        assert!(reasons.is_empty());
        assert_eq!(global_of(&recipe)["temperature"].as_f64().unwrap(), 5600.0);
    }

    /// Encode an RGB8 buffer as JPEG, so the tests below go through the same
    /// decode the request path does.
    fn jpeg(pixels: Vec<u8>, width: u32, height: u32) -> Vec<u8> {
        let buffer = image::RgbImage::from_raw(width, height, pixels).unwrap();
        let mut out = std::io::Cursor::new(Vec::new());
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95);
        buffer.write_with_encoder(encoder).unwrap();
        out.into_inner()
    }

    /// A frame split into a lit and a shadowed half by one hard edge — the
    /// shape `light_hardness` is built to find, and the situation the whole
    /// guardrail module exists for.
    fn harshly_lit_jpeg() -> Vec<u8> {
        const SIZE: u32 = 256;
        let mut pixels = Vec::with_capacity((SIZE * SIZE * 3) as usize);
        for _ in 0..SIZE {
            for x in 0..SIZE {
                let v = if x < SIZE / 2 { 8u8 } else { 250u8 };
                pixels.extend_from_slice(&[v, v, v]);
            }
        }
        jpeg(pixels, SIZE, SIZE)
    }

    /// Even, soft light with fine texture and no large-scale step: the frame
    /// that genuinely wants more contrast, not less.
    fn flat_soft_jpeg() -> Vec<u8> {
        const SIZE: u32 = 256;
        let mut pixels = Vec::with_capacity((SIZE * SIZE * 3) as usize);
        for y in 0..SIZE {
            for x in 0..SIZE {
                // Texture at the pixel scale only — it averages away in the
                // coarse pass, which is precisely what separates subject
                // matter from lighting.
                let v = 118 + u8::from((x + y) % 2 == 0) * 6;
                pixels.extend_from_slice(&[v, v, v]);
            }
        }
        jpeg(pixels, SIZE, SIZE)
    }

    /// The seam every other test in this module skips.
    ///
    /// The unit tests above construct a `SceneState` directly, so all of them
    /// would still pass if the measurement were never wired to the recipe —
    /// which is exactly the state this module was written to fix. This one
    /// starts from encoded pixels and asserts the edit came back changed.
    #[test]
    fn real_pixels_from_a_harsh_frame_constrain_the_recipe() {
        let mut recipe = recipe_with(json!({"contrast": 60.0, "clarity": 30.0}));
        let reasons = measure_and_apply(
            &mut recipe,
            &harshly_lit_jpeg(),
            &CaptureConditions::default(),
        );

        let g = global_of(&recipe);
        assert_eq!(g["contrast"].as_f64().unwrap(), 0.0, "reasons: {reasons:?}");
        assert_eq!(g["clarity"].as_f64().unwrap(), 0.0);
        assert!(
            reasons.contains(&"hard_light_no_added_contrast".to_string())
                || reasons.contains(&"no_tonal_headroom".to_string()),
            "expected a contrast-blocking reason, got {reasons:?}"
        );
    }

    /// The negative control. Without it the test above would still pass if the
    /// budget capped everything unconditionally, which would be its own bug —
    /// a flat frame is the one that most needs the contrast it asked for.
    #[test]
    fn real_pixels_from_a_flat_frame_leave_a_moderate_recipe_alone() {
        let mut recipe = recipe_with(json!({"contrast": 30.0}));
        let before = recipe.clone();
        let reasons = measure_and_apply(
            &mut recipe,
            &flat_soft_jpeg(),
            &CaptureConditions::default(),
        );

        assert_eq!(recipe, before, "flat light should widen, never narrow");
        assert!(reasons.is_empty(), "got {reasons:?}");
    }

    #[test]
    fn an_undecodable_image_leaves_the_recipe_alone() {
        let mut recipe = recipe_with(json!({"contrast": 90.0}));
        let before = recipe.clone();
        let reasons =
            measure_and_apply(&mut recipe, b"not an image", &CaptureConditions::default());
        assert!(reasons.is_empty());
        assert_eq!(recipe, before, "a decode failure must not change the edit");
    }

    #[test]
    fn a_recipe_without_a_global_block_is_handled() {
        let mut recipe = json!({"summary": "masks only", "masks": []});
        assert!(!recipe_needs_budget(&recipe));
        assert!(apply_with_scene(&mut recipe, &harsh(), &CaptureConditions::default()).is_empty());
    }
}
