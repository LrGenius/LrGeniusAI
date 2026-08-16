//! Port of `services/style_engine.py`: the LLM-free "Photographer Style
//! Engine" that reproduces the user's own editing style by retrieving
//! CLIP-similar training examples, re-scoring them on exposure/scene/
//! time-of-day proximity, interpolating their develop settings, and
//! applying a small RAW-adaptive exposure/contrast compensation.
//!
//! Deliberately pure: candidate retrieval (CLIP similarity query against
//! the `edit_training` table) is I/O and happens at the `lrg-api` route
//! layer, which builds a `TrainingCandidate` list and calls
//! [`generate_style_edit`] here.

use serde_json::{json, Map, Value};

pub const WEIGHT_CLIP: f64 = 0.50;
pub const WEIGHT_EXPOSURE: f64 = 0.25;
pub const WEIGHT_SCENE: f64 = 0.15;
pub const WEIGHT_TIME_OF_DAY: f64 = 0.10;

pub const CONFIDENCE_GOOD: f64 = 0.70;
pub const CONFIDENCE_LOW: f64 = 0.45;

pub const TOP_K_BLEND: usize = 3;
pub const CANDIDATE_POOL: usize = 20;
pub const MIN_TRAINING_EXAMPLES: usize = 5;

const TOD_ORDER: [&str; 6] = [
    "dawn",
    "morning",
    "afternoon",
    "evening",
    "night",
    "unknown",
];

fn round_to(v: f64, decimals: i32) -> f64 {
    let factor = 10f64.powi(decimals);
    (v * factor).round() / factor
}

/// Port of `_clip_distance_to_similarity`. `distance` is a Chroma-style
/// squared-L2 distance over unit vectors (`||a-b||^2 = 2 - 2*cos(theta)`),
/// i.e. `2.0 * cosine_distance` when the caller only has a plain cosine
/// distance (`1 - cos`) on hand.
pub fn clip_distance_to_similarity(distance: f64) -> f64 {
    (1.0 - distance / 2.0).clamp(0.0, 1.0)
}

/// Per-photo exposure features used for proximity scoring. `None` fields
/// are simply excluded from the average delta, matching Python's
/// `dict.get()` + skip-if-either-missing behavior.
#[derive(Debug, Clone, Default)]
pub struct ExposureFeatures {
    pub luminance_mean: Option<f64>,
    pub contrast: Option<f64>,
    pub warmth_proxy: Option<f64>,
}

/// Port of `_exposure_proximity`.
pub fn exposure_proximity(query: &ExposureFeatures, candidate: &ExposureFeatures) -> f64 {
    let pairs = [
        (query.luminance_mean, candidate.luminance_mean),
        (query.contrast, candidate.contrast),
        (query.warmth_proxy, candidate.warmth_proxy),
    ];
    let deltas: Vec<f64> = pairs
        .iter()
        .filter_map(|(q, c)| Some((q.as_ref()? - c.as_ref()?).abs()))
        .collect();
    if deltas.is_empty() {
        return 0.5;
    }
    let mean_delta = deltas.iter().sum::<f64>() / deltas.len() as f64;
    (1.0 - mean_delta / 0.3).max(0.0)
}

/// Port of `_scene_overlap`: Jaccard overlap between two tag sets.
pub fn scene_overlap(query_tags: &[String], candidate_tags: &[String]) -> f64 {
    use std::collections::HashSet;
    let q: HashSet<&str> = query_tags.iter().map(String::as_str).collect();
    let c: HashSet<&str> = candidate_tags.iter().map(String::as_str).collect();
    if q.is_empty() && c.is_empty() {
        return 0.5;
    }
    if q.is_empty() || c.is_empty() {
        return 0.3;
    }
    let intersection = q.intersection(&c).count();
    let union = q.union(&c).count();
    intersection as f64 / union as f64
}

/// Port of `_tod_proximity`.
pub fn tod_proximity(query_tod: &str, candidate_tod: &str) -> f64 {
    if query_tod == "unknown" || candidate_tod == "unknown" {
        return 0.5;
    }
    if query_tod == candidate_tod {
        return 1.0;
    }
    let Some(q_idx) = TOD_ORDER.iter().position(|&t| t == query_tod) else {
        return 0.5;
    };
    let Some(c_idx) = TOD_ORDER.iter().position(|&t| t == candidate_tod) else {
        return 0.5;
    };
    let diff = (q_idx as i64 - c_idx as i64).unsigned_abs() as usize;
    let diff = diff.min(5 - diff);
    (1.0 - diff as f64 * 0.35).max(0.0)
}

/// One retrieved+scored training example, already carrying everything
/// `generate_style_edit` needs (CLIP retrieval + parsing of stored JSON
/// metadata happens at the route layer).
#[derive(Debug, Clone, Default)]
pub struct TrainingCandidate {
    pub photo_id: String,
    pub filename: Option<String>,
    pub label: Option<String>,
    pub summary: Option<String>,
    /// Chroma-style squared-L2 distance (see [`clip_distance_to_similarity`]).
    pub distance: f64,
    pub canonical_settings: Map<String, Value>,
    pub scene_tags: Vec<String>,
    pub exposure: ExposureFeatures,
    pub time_of_day_bucket: String,
    /// Whether the photo this example was saved from is a raw file.
    ///
    /// `None` for examples saved before the flag was recorded. See
    /// [`restrict_temperature_to_matching_raw_status`] for why it matters.
    pub is_raw: Option<bool>,
}

/// The new photo's own query-side features.
#[derive(Debug, Clone, Default)]
pub struct StyleQuery {
    pub exposure: ExposureFeatures,
    pub scene_tags: Vec<String>,
    pub time_of_day_bucket: String,
    /// Whether the photo being edited is a raw file. `None` means unknown.
    pub is_raw: Option<bool>,
}

/// Port of `calculate_composite_score`.
pub fn calculate_composite_score(
    clip_sim: f64,
    query: &StyleQuery,
    candidate: &TrainingCandidate,
) -> f64 {
    let exp_score = exposure_proximity(&query.exposure, &candidate.exposure);
    let scene_score = scene_overlap(&query.scene_tags, &candidate.scene_tags);
    let tod_score = tod_proximity(&query.time_of_day_bucket, &candidate.time_of_day_bucket);
    WEIGHT_CLIP * clip_sim
        + WEIGHT_EXPOSURE * exp_score
        + WEIGHT_SCENE * scene_score
        + WEIGHT_TIME_OF_DAY * tod_score
}

/// Port of `interpolate_recipes`: weighted blend of numeric canonical
/// settings across the winners, weights normalized by composite score.
pub fn interpolate_recipes(winners: &[(TrainingCandidate, f64)]) -> Map<String, Value> {
    let total_weight: f64 = winners.iter().map(|(_, s)| s).sum();
    if total_weight <= 0.0 {
        return Map::new();
    }
    let mut blended: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for (example, score) in winners {
        let weight = score / total_weight;
        for (key, value) in &example.canonical_settings {
            if let Some(v) = value.as_f64() {
                *blended.entry(key.clone()).or_insert(0.0) += weight * v;
            }
        }
    }
    blended
        .into_iter()
        .map(|(k, v)| (k, json!(round_to(v, 1))))
        .collect()
}

/// Recompute `temperature` from only the winners whose raw status matches the
/// photo being edited, or drop it.
///
/// Every other canonical field means the same thing on a raw file and on a
/// JPEG. `temperature` does not: Lightroom exposes it as Kelvin for raw and as
/// a relative -100..100 for everything else. Averaging 5600 and +12 produces a
/// number that is meaningless on either scale, and the result is applied to a
/// real photo — so the blend is restricted rather than corrected afterwards.
///
/// `tint` is deliberately left alone. Its *range* differs (-150..150 raw,
/// -100..100 otherwise) but its meaning does not, so a mixed blend is a scale
/// distortion the clamp already handles, not a category error.
///
/// Examples with an unknown raw status (saved before the flag was recorded)
/// count as matching, since excluding them would quietly shrink every
/// long-standing user's pool to nothing.
///
/// Returns a warning when the field had to be dropped entirely.
pub fn restrict_temperature_to_matching_raw_status(
    blended: &mut Map<String, Value>,
    winners: &[(TrainingCandidate, f64)],
    query_is_raw: Option<bool>,
) -> Option<String> {
    if !blended.contains_key("temperature") {
        return None;
    }
    let compatible = |candidate: &TrainingCandidate| match (query_is_raw, candidate.is_raw) {
        (Some(q), Some(c)) => q == c,
        _ => true,
    };
    if winners.iter().all(|(c, _)| compatible(c)) {
        return None;
    }

    let matching: Vec<&(TrainingCandidate, f64)> =
        winners.iter().filter(|(c, _)| compatible(c)).collect();
    let total_weight: f64 = matching.iter().map(|(_, s)| s).sum();
    let recomputed: Option<f64> = if total_weight > 0.0 {
        let sum: f64 = matching
            .iter()
            .filter_map(|(c, s)| {
                c.canonical_settings
                    .get("temperature")
                    .and_then(Value::as_f64)
                    .map(|v| v * (s / total_weight))
            })
            .sum();
        Some(round_to(sum, 1))
    } else {
        None
    };

    match recomputed {
        Some(v) => {
            blended.insert("temperature".into(), json!(v));
            Some(format!(
                "White balance was blended from {} of {} matched examples — the others were saved from {} files, where Lightroom's temperature is on a different scale.",
                matching.len(),
                winners.len(),
                if query_is_raw == Some(true) { "non-raw" } else { "raw" }
            ))
        }
        None => {
            blended.remove("temperature");
            Some(
                "White balance was left unchanged: every matched training example was saved from a file whose temperature is on a different scale than this photo's."
                    .to_string(),
            )
        }
    }
}

/// Port of `adaptive_compensation`.
pub fn adaptive_compensation(
    recipe: &mut Map<String, Value>,
    query_exposure: &ExposureFeatures,
    winners: &[(TrainingCandidate, f64)],
) {
    if winners.is_empty() {
        return;
    }
    let total_weight: f64 = winners.iter().map(|(_, s)| s).sum();
    if total_weight <= 0.0 {
        return;
    }

    let avg_train_lum: f64 = winners
        .iter()
        .map(|(ex, s)| ex.exposure.luminance_mean.unwrap_or(0.5) * (s / total_weight))
        .sum();
    let avg_train_contrast: f64 = winners
        .iter()
        .map(|(ex, s)| ex.exposure.contrast.unwrap_or(0.5) * (s / total_weight))
        .sum();

    let query_lum = query_exposure.luminance_mean.unwrap_or(0.5);
    let query_contrast = query_exposure.contrast.unwrap_or(0.5);

    let lum_delta = query_lum - avg_train_lum;
    let exposure_correction = (-lum_delta * 5.0).clamp(-1.5, 1.5);

    let contrast_delta = query_contrast - avg_train_contrast;
    let contrast_correction = (-contrast_delta * 20.0).clamp(-15.0, 15.0);

    if exposure_correction.abs() > 0.05 {
        let current = recipe
            .get("exposure")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        recipe.insert(
            "exposure".into(),
            json!(round_to(current + exposure_correction, 2)),
        );
    }
    if contrast_correction.abs() > 1.0 {
        let current = recipe
            .get("contrast")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        recipe.insert(
            "contrast".into(),
            json!(round_to(current + contrast_correction, 1)),
        );
    }
}

const CANONICAL_TO_RECIPE_FIELDS: &[&str] = &[
    "exposure",
    "contrast",
    "highlights",
    "shadows",
    "whites",
    "blacks",
    "temperature",
    "tint",
    "texture",
    "clarity",
    "dehaze",
    "vibrance",
    "saturation",
    "sharpening",
    "noise_reduction",
    "color_noise_reduction",
    "vignette",
    "grain",
];
const TONE_CURVE_KEYS: &[(&str, &str)] = &[
    ("tone_curve_highlights", "highlights"),
    ("tone_curve_lights", "lights"),
    ("tone_curve_darks", "darks"),
    ("tone_curve_shadows", "shadows"),
];

/// Port of `_canonical_to_edit_recipe`.
pub fn canonical_to_edit_recipe(canonical: &Map<String, Value>, summary: &str) -> Value {
    let mut global_settings = Map::new();
    for &key in CANONICAL_TO_RECIPE_FIELDS {
        if let Some(v) = canonical.get(key) {
            global_settings.insert(key.to_string(), v.clone());
        }
    }
    let mut tone_curve = Map::new();
    for &(canon_key, tc_key) in TONE_CURVE_KEYS {
        if let Some(v) = canonical.get(canon_key) {
            tone_curve.insert(tc_key.to_string(), v.clone());
        }
    }
    if !tone_curve.is_empty() {
        global_settings.insert("tone_curve".into(), Value::Object(tone_curve));
    }

    let summary = if summary.is_empty() {
        "Style-matched edit by LrGeniusAI Style Engine"
    } else {
        summary
    };
    json!({
        "summary": summary,
        "global": global_settings,
        "masks": [],
        "warnings": [],
    })
}

#[derive(Debug, Clone)]
pub struct StyleEngineResult {
    pub recipe: Value,
    pub confidence: f64,
    pub matched_count: usize,
    pub engine: &'static str,
    pub warning: Option<String>,
    pub matched_filenames: Vec<String>,
}

/// Port of `generate_style_edit`'s scoring/blend pipeline. Candidate
/// retrieval (the CLIP-similarity query, or the "no embedding" recent-
/// examples fallback with a neutral 1.0 squared-L2 distance) happens at
/// the caller; `training_count` is a separate cheap `COUNT(*)` the
/// caller already has from the table.
pub fn generate_style_edit(
    training_count: usize,
    candidates: &[TrainingCandidate],
    query: &StyleQuery,
) -> StyleEngineResult {
    if training_count < MIN_TRAINING_EXAMPLES {
        return StyleEngineResult {
            recipe: json!({}),
            confidence: 0.0,
            matched_count: 0,
            engine: "none",
            warning: Some(format!(
                "Style engine inactive: only {training_count} training example(s) available (minimum {MIN_TRAINING_EXAMPLES} required). Please save more AI training examples."
            )),
            matched_filenames: Vec::new(),
        };
    }
    if candidates.is_empty() {
        return StyleEngineResult {
            recipe: json!({}),
            confidence: 0.0,
            matched_count: 0,
            engine: "none",
            warning: Some("No training examples could be retrieved from the database.".to_string()),
            matched_filenames: Vec::new(),
        };
    }

    let mut scored: Vec<(TrainingCandidate, f64)> = candidates
        .iter()
        .map(|c| {
            let clip_sim = clip_distance_to_similarity(c.distance);
            let score = calculate_composite_score(clip_sim, query, c);
            (c.clone(), score)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    let best_score = scored.first().map(|(_, s)| *s).unwrap_or(0.0);
    let confidence = round_to(best_score, 3);

    let winners: Vec<(TrainingCandidate, f64)> = scored.into_iter().take(TOP_K_BLEND).collect();
    let matched_filenames: Vec<String> = winners
        .iter()
        .map(|(ex, _)| {
            ex.filename
                .clone()
                .or_else(|| ex.label.clone())
                .unwrap_or_else(|| ex.photo_id.clone())
        })
        .collect();

    let mut blended = interpolate_recipes(&winners);
    let temperature_warning =
        restrict_temperature_to_matching_raw_status(&mut blended, &winners, query.is_raw);
    adaptive_compensation(&mut blended, &query.exposure, &winners);

    // Python builds this from a `set(...)`, whose iteration order is not
    // guaranteed (and randomized per-process under hash-seed
    // randomization) — display-text only, so we use deterministic
    // first-seen order instead of reproducing that nondeterminism.
    let mut seen_labels = std::collections::HashSet::new();
    let mut labels: Vec<String> = Vec::new();
    for (ex, _) in &winners {
        if let Some(l) = ex.label.clone().or_else(|| ex.summary.clone()) {
            if !l.is_empty() && seen_labels.insert(l.clone()) {
                labels.push(l);
            }
        }
    }
    let mut summary_parts = Vec::new();
    if !labels.is_empty() {
        summary_parts.push(format!(
            "Style: {}",
            labels
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(" / ")
        ));
    }
    summary_parts.push(format!(
        "Matched {} of {} examples (confidence {:.0}%)",
        winners.len(),
        training_count,
        confidence * 100.0
    ));
    let summary = summary_parts.join(" \u{2014} ");

    let recipe = canonical_to_edit_recipe(&blended, &summary);

    let warning = if confidence < CONFIDENCE_LOW {
        Some(format!(
            "Low style match confidence ({:.0}%). Results may not match your editing style precisely. Consider adding more training examples for this type of photo.",
            confidence * 100.0
        ))
    } else if confidence < CONFIDENCE_GOOD {
        Some(format!(
            "Moderate style match confidence ({:.0}%). Review the result before applying.",
            confidence * 100.0
        ))
    } else {
        None
    };
    // Both can be true at once, and the user needs both: one is about how well
    // the style matched, the other about a field that was left out of it.
    let warning = match (warning, temperature_warning) {
        (Some(a), Some(b)) => Some(format!("{a}\n{b}")),
        (a, b) => a.or(b),
    };

    StyleEngineResult {
        recipe,
        confidence,
        matched_count: winners.len(),
        engine: "style",
        warning,
        matched_filenames,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_distance_to_similarity_matches_formula() {
        assert_eq!(clip_distance_to_similarity(0.0), 1.0);
        assert_eq!(clip_distance_to_similarity(2.0), 0.0);
        assert!((clip_distance_to_similarity(1.0) - 0.5).abs() < 1e-9);
        // clamps out-of-range values
        assert_eq!(clip_distance_to_similarity(3.0), 0.0);
        assert_eq!(clip_distance_to_similarity(-1.0), 1.0);
    }

    #[test]
    fn exposure_proximity_neutral_when_no_overlap() {
        let q = ExposureFeatures::default();
        let c = ExposureFeatures::default();
        assert_eq!(exposure_proximity(&q, &c), 0.5);
    }

    #[test]
    fn exposure_proximity_scores_identical_as_one() {
        let f = ExposureFeatures {
            luminance_mean: Some(0.5),
            contrast: Some(0.3),
            warmth_proxy: Some(0.6),
        };
        assert_eq!(exposure_proximity(&f, &f), 1.0);
    }

    #[test]
    fn scene_overlap_jaccard_and_edge_cases() {
        assert_eq!(scene_overlap(&[], &[]), 0.5);
        assert_eq!(scene_overlap(&["a".to_string()], &[]), 0.3);
        let a = vec!["portrait".to_string(), "studio".to_string()];
        let b = vec!["portrait".to_string(), "street".to_string()];
        // intersection={portrait}=1, union={portrait,studio,street}=3
        assert!((scene_overlap(&a, &b) - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn tod_proximity_same_adjacent_and_circular_wrap() {
        assert_eq!(tod_proximity("evening", "evening"), 1.0);
        assert_eq!(tod_proximity("unknown", "morning"), 0.5);
        // dawn(0) vs night(4): diff=4, circular min(4, 5-4=1)=1 -> 1-0.35=0.65
        assert!((tod_proximity("dawn", "night") - 0.65).abs() < 1e-9);
    }

    fn candidate(canonical: &[(&str, f64)], distance: f64) -> TrainingCandidate {
        let mut c = TrainingCandidate {
            distance,
            ..Default::default()
        };
        for &(k, v) in canonical {
            c.canonical_settings.insert(k.to_string(), json!(v));
        }
        c
    }

    #[test]
    fn interpolate_recipes_weighted_blend() {
        let winners = vec![
            (candidate(&[("exposure", 1.0)], 0.0), 2.0),
            (candidate(&[("exposure", 0.0)], 0.0), 1.0),
        ];
        let blended = interpolate_recipes(&winners);
        // weight 2/3 * 1.0 + 1/3 * 0.0 = 0.667 -> rounds to 0.7
        assert_eq!(blended["exposure"], json!(0.7));
    }

    #[test]
    fn generate_style_edit_reports_insufficient_examples() {
        let query = StyleQuery::default();
        let result = generate_style_edit(2, &[], &query);
        assert_eq!(result.engine, "none");
        assert_eq!(result.confidence, 0.0);
        assert!(result.warning.unwrap().contains("only 2 training example"));
    }

    #[test]
    fn generate_style_edit_produces_recipe_from_winners() {
        let query = StyleQuery {
            exposure: ExposureFeatures {
                luminance_mean: Some(0.5),
                contrast: Some(0.4),
                warmth_proxy: Some(0.5),
            },
            scene_tags: vec!["scene_portrait".to_string()],
            time_of_day_bucket: "afternoon".to_string(),
            is_raw: None,
        };
        let mut c = candidate(&[("exposure", 1.0), ("contrast", 10.0)], 0.1);
        c.exposure = ExposureFeatures {
            luminance_mean: Some(0.5),
            contrast: Some(0.4),
            warmth_proxy: Some(0.5),
        };
        c.scene_tags = vec!["scene_portrait".to_string()];
        c.time_of_day_bucket = "afternoon".to_string();
        c.filename = Some("photo1.jpg".to_string());

        let result = generate_style_edit(5, &[c], &query);
        assert_eq!(result.engine, "style");
        assert_eq!(result.matched_count, 1);
        assert!(result.confidence > 0.9); // near-identical query/candidate
        assert_eq!(result.recipe["global"]["exposure"], json!(1.0));
        assert_eq!(result.matched_filenames, vec!["photo1.jpg".to_string()]);
    }

    fn raw_candidate(temperature: f64, is_raw: Option<bool>) -> TrainingCandidate {
        let mut c = candidate(&[("temperature", temperature), ("contrast", 10.0)], 0.0);
        c.is_raw = is_raw;
        c
    }

    #[test]
    fn a_uniform_pool_blends_temperature_as_before() {
        let mut blended = interpolate_recipes(&[
            (raw_candidate(5600.0, Some(true)), 1.0),
            (raw_candidate(5000.0, Some(true)), 1.0),
        ]);
        let winners = vec![
            (raw_candidate(5600.0, Some(true)), 1.0),
            (raw_candidate(5000.0, Some(true)), 1.0),
        ];
        let warning =
            restrict_temperature_to_matching_raw_status(&mut blended, &winners, Some(true));

        assert!(warning.is_none(), "nothing to report when nothing changed");
        assert_eq!(blended["temperature"], json!(5300.0));
    }

    #[test]
    fn a_mixed_pool_blends_only_the_matching_examples() {
        // Averaging 5600 K with a relative +10 produces a number that is
        // meaningless on either scale, and it lands on a real photo.
        let winners = vec![
            (raw_candidate(5600.0, Some(true)), 1.0),
            (raw_candidate(10.0, Some(false)), 1.0),
        ];
        let mut blended = interpolate_recipes(&winners);
        assert_eq!(blended["temperature"], json!(2805.0), "the bad average");

        let warning =
            restrict_temperature_to_matching_raw_status(&mut blended, &winners, Some(true));
        assert_eq!(blended["temperature"], json!(5600.0));
        let warning = warning.expect("the user has to be told the pool was narrowed");
        assert!(warning.contains("1 of 2"), "got {warning:?}");
    }

    #[test]
    fn temperature_is_dropped_when_no_example_matches() {
        let winners = vec![(raw_candidate(5600.0, Some(true)), 1.0)];
        let mut blended = interpolate_recipes(&winners);
        let warning =
            restrict_temperature_to_matching_raw_status(&mut blended, &winners, Some(false));

        assert!(
            blended.get("temperature").is_none(),
            "no white balance beats a wrong-scale one"
        );
        assert_eq!(blended["contrast"], json!(10.0), "other fields survive");
        assert!(warning.unwrap().contains("left unchanged"));
    }

    #[test]
    fn examples_saved_before_the_flag_existed_still_count() {
        // Excluding them would shrink every long-standing user's pool to
        // nothing on the first edit after upgrading.
        let winners = vec![
            (raw_candidate(5600.0, None), 1.0),
            (raw_candidate(5000.0, Some(true)), 1.0),
        ];
        let mut blended = interpolate_recipes(&winners);
        let warning =
            restrict_temperature_to_matching_raw_status(&mut blended, &winners, Some(true));

        assert!(warning.is_none());
        assert_eq!(blended["temperature"], json!(5300.0));
    }

    #[test]
    fn an_unknown_target_accepts_every_example() {
        let winners = vec![
            (raw_candidate(5600.0, Some(true)), 1.0),
            (raw_candidate(5000.0, Some(false)), 1.0),
        ];
        let mut blended = interpolate_recipes(&winners);
        let warning = restrict_temperature_to_matching_raw_status(&mut blended, &winners, None);
        assert!(
            warning.is_none(),
            "nothing is known, so nothing is excluded"
        );
    }
}
