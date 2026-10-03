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

use lrg_develop::model::{WbFamily, WbMode, WbSetting};
use lrg_develop::registry::{self, Level};
use lrg_develop::{FileKind, Finite};
use serde_json::{json, Map, Value};

use crate::training::{canonical_key, canonical_keys, number_json, white_balance_json};

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
    /// The example's white balance (mode, key family, and the numbers when
    /// the mode is Custom); `None` when its settings carry no
    /// `WhiteBalance`. The family is the example's file kind: see
    /// [`blend_white_balance`] for why it matters.
    pub white_balance: Option<WbSetting>,
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

impl StyleQuery {
    /// The target's file kind, which decides the white-balance family.
    pub fn file_kind(&self) -> Option<FileKind> {
        self.is_raw
            .map(|raw| if raw { FileKind::Raw } else { FileKind::NonRaw })
    }
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

/// Port of `interpolate_recipes`: weighted blend of the canonical settings
/// across the winners, weights normalized by composite score **per key**,
/// over the winners that have the key (an example that never touched a
/// slider must not pull it towards 0), rounded to the key's registry
/// precision (integers for integer sliders, `Exposure2012` to 2 decimals).
///
/// Only keys of [`canonical_keys`] are blended; white balance is not a
/// canonical number (see [`blend_white_balance`]).
pub fn interpolate_recipes(winners: &[(TrainingCandidate, f64)]) -> Map<String, Value> {
    let mut blended = Map::new();
    for key in canonical_keys() {
        let mut weight = 0.0;
        let mut sum = 0.0;
        for (example, score) in winners {
            if let Some(v) = example
                .canonical_settings
                .get(key.name)
                .and_then(Value::as_f64)
            {
                weight += score;
                sum += score * v;
            }
        }
        if weight > 0.0 {
            blended.insert(key.name.to_string(), number_json(key.spec, sum / weight));
        }
    }
    blended
}

/// Whether the style engine may send a white-balance *mode* without numbers
/// (`Auto`, `Daylight`, ...), for Lightroom to resolve per photo.
///
/// Off until experiment E1 shows that `applyDevelopSettings{WhiteBalance =
/// "Daylight"}` makes Lightroom recompute the temperature for the target
/// photo. If it does not, a mode alone would leave the photo's old Kelvin in
/// place under a new label. Until then an Auto/preset majority transfers
/// nothing and says so ([`WbOutcome::NotTransferred`]).
pub const EMIT_NAMED_WB_MODES: bool = false;

/// At most this many Custom examples (the best-scored ones of the pool) give
/// the transferred temperature and tint.
pub const WB_CUSTOM_EXAMPLES: usize = 3;

/// At least this many of them must exist: one example's Kelvin belongs to
/// the light of its own scene.
pub const WB_MIN_CUSTOM_EXAMPLES: usize = 2;

/// Their raw temperatures may be at most this far apart (Kelvin); further
/// apart, they do not describe one habit.
pub const WB_MAX_KELVIN_SPREAD: f64 = 500.0;

/// The same agreement limit for non-raw examples, whose
/// `IncrementalTemperature` is an offset on -100..100. An assumption like the
/// Kelvin one (every example so far is raw), to revisit with data.
pub const WB_MAX_INCREMENTAL_SPREAD: f64 = 20.0;

/// What [`blend_white_balance`] decided.
#[derive(Debug, Clone, PartialEq)]
pub enum WbOutcome {
    /// Leave the photo's own white balance alone; nothing to report (the
    /// examples keep theirs as shot, or carry none).
    Unchanged,
    /// Send this white balance.
    Transfer(WbSetting),
    /// Nothing is sent, and the user should know why.
    NotTransferred(String),
}

fn family_label(family: WbFamily) -> &'static str {
    match family {
        WbFamily::Raw => "raw files",
        WbFamily::NonRaw => "JPEG/TIFF (non-raw) files",
    }
}

/// Learns the white balance to send from the scored candidate pool (all of
/// it, best first — not only the [`TOP_K_BLEND`] winners: a 2-of-3 vote says
/// nothing).
///
/// 1. The pool votes for a mode, weighted by composite score. Examples
///    without a `WhiteBalance` do not vote. Ties go to the more conservative
///    mode (As Shot, then Auto, then a preset, then Custom).
/// 2. As Shot wins: nothing is sent; the photo keeps its own, which is what
///    the examples show.
/// 3. Auto or a preset wins: only the mode would be sent, without numbers —
///    and only once [`EMIT_NAMED_WB_MODES`] is on.
/// 4. Custom wins: temperature and tint from up to [`WB_CUSTOM_EXAMPLES`] of
///    the best-scored Custom examples **of the target's family** (raw
///    examples' Kelvin only for raw targets, non-raw offsets only for non-raw
///    targets), only when at least [`WB_MIN_CUSTOM_EXAMPLES`] exist and their
///    temperatures agree within the spread limit, weighted over exactly
///    those examples (renormalised per number: a missing tint does not pull
///    towards 0).
///
/// An unknown target file kind sends nothing: Kelvin on a JPEG, or an offset
/// on a raw file, would be a scale error.
pub fn blend_white_balance(
    pool: &[(TrainingCandidate, f64)],
    target: Option<FileKind>,
) -> WbOutcome {
    fn rank(mode: WbMode) -> u8 {
        match mode {
            WbMode::AsShot => 0,
            WbMode::Auto => 1,
            WbMode::Named(_) => 2,
            WbMode::Custom => 3,
        }
    }
    let mut votes: Vec<(WbMode, f64)> = Vec::new();
    for (c, score) in pool {
        let Some(wb) = c.white_balance else { continue };
        match votes.iter_mut().find(|(m, _)| *m == wb.mode) {
            Some((_, w)) => *w += score.max(0.0),
            None => votes.push((wb.mode, score.max(0.0))),
        }
    }
    let Some(&(winner, _)) = votes.iter().min_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(rank(a.0).cmp(&rank(b.0)))
    }) else {
        return WbOutcome::Unchanged;
    };

    match winner {
        WbMode::AsShot => WbOutcome::Unchanged,
        WbMode::Auto | WbMode::Named(_) => {
            // Decision (plan step 1, 6.1): a mode without numbers is only
            // sent after E1; see EMIT_NAMED_WB_MODES. Reported rather than
            // silent, because the examples do show a white-balance habit
            // that this photo will not get.
            if !EMIT_NAMED_WB_MODES {
                return WbOutcome::NotTransferred(format!(
                    "White balance was not transferred: most of your matching examples use the \"{winner}\" white balance, which AI Edit does not apply yet."
                ));
            }
            let Some(target) = target else {
                return WbOutcome::NotTransferred(
                    "White balance was not transferred: it is not known whether this photo is a raw file.".to_string(),
                );
            };
            // Lightroom offers only As Shot, Auto and Custom for non-raw files.
            if matches!(winner, WbMode::Named(_)) && target == FileKind::NonRaw {
                return WbOutcome::NotTransferred(format!(
                    "White balance was not transferred: the \"{winner}\" preset exists only for raw files, and this photo is not raw."
                ));
            }
            WbOutcome::Transfer(WbSetting {
                mode: winner,
                family: target.into(),
                temperature: None,
                tint: None,
            })
        }
        WbMode::Custom => blend_custom_white_balance(pool, target),
    }
}

fn blend_custom_white_balance(
    pool: &[(TrainingCandidate, f64)],
    target: Option<FileKind>,
) -> WbOutcome {
    let Some(target) = target else {
        return WbOutcome::NotTransferred(
            "White balance was not transferred: it is not known whether this photo is a raw file, so the temperature scale is unknown.".to_string(),
        );
    };
    let family = WbFamily::from(target);
    let mut customs: Vec<(WbSetting, f64)> = pool
        .iter()
        .filter_map(|(c, s)| {
            let wb = c.white_balance?;
            (wb.mode == WbMode::Custom && wb.temperature.is_some()).then_some((wb, *s))
        })
        .collect();
    // Stable: equal scores keep pool order.
    customs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let other_family = customs.iter().any(|(wb, _)| wb.family != family);
    customs.retain(|(wb, _)| wb.family == family);
    customs.truncate(WB_CUSTOM_EXAMPLES);

    if customs.is_empty() {
        return if other_family {
            WbOutcome::NotTransferred(match family {
                // No "save JPEG examples" advice: training accepts raw and DNG
                // files only, so the user could not follow it.
                WbFamily::NonRaw => "White balance was not transferred: your training examples are raw files and this photo is not, so their Kelvin temperature does not apply. The photo keeps its own white balance; set it by hand if needed.".to_string(),
                WbFamily::Raw => "White balance was not transferred: your matching examples are JPEG/TIFF (non-raw) files and this photo is a raw file, so their temperature offset does not apply. Save a few edited raw photos as training examples to learn their white balance.".to_string(),
            })
        } else {
            WbOutcome::Unchanged
        };
    }
    if customs.len() < WB_MIN_CUSTOM_EXAMPLES {
        return WbOutcome::NotTransferred(format!(
            "White balance was not transferred: only {} of your matching examples sets a custom white balance on {}; at least {WB_MIN_CUSTOM_EXAMPLES} are needed.",
            customs.len(),
            family_label(family),
        ));
    }

    let temps: Vec<f64> = customs
        .iter()
        .filter_map(|(wb, _)| wb.temperature.map(Finite::get))
        .collect();
    let spread = temps.iter().cloned().fold(f64::MIN, f64::max)
        - temps.iter().cloned().fold(f64::MAX, f64::min);
    let (limit, unit) = match family {
        WbFamily::Raw => (WB_MAX_KELVIN_SPREAD, " K"),
        WbFamily::NonRaw => (WB_MAX_INCREMENTAL_SPREAD, ""),
    };
    if spread > limit {
        // The measured spread differs per photo, so it goes to the log and
        // the text stays constant per family: the plugin merges warnings by
        // exact text, and a per-photo number would give every photo its own
        // line and push run-wide causes out of the end-of-run dialog.
        log::info!(
            "White balance not transferred: custom temperatures spread {spread:.0}{unit} > {limit:.0}{unit}"
        );
        return WbOutcome::NotTransferred(format!(
            "White balance was not transferred: your matching examples disagree (their custom temperatures are more than {limit:.0}{unit} apart). Set it by hand for this photo."
        ));
    }

    let blend = |pick: fn(&WbSetting) -> Option<Finite>, key: &str| -> Option<Finite> {
        let spec = registry::lookup(Level::Global, key)?.spec();
        let (mut sum, mut weight) = (0.0, 0.0);
        for (wb, s) in &customs {
            if let Some(v) = pick(wb) {
                sum += s.max(0.0) * v.get();
                weight += s.max(0.0);
            }
        }
        if weight <= 0.0 {
            return None;
        }
        let mut v = sum / weight;
        if let Some((lo, hi)) = spec.range {
            v = v.clamp(lo.get(), hi.get());
        }
        number_json(spec, v)
            .as_f64()
            .and_then(|x| Finite::new(x).ok())
    };
    let temperature = blend(|wb| wb.temperature, family.temperature_key());
    let Some(temperature) = temperature else {
        // Every chosen example scored 0: nothing to weight by.
        return WbOutcome::Unchanged;
    };
    WbOutcome::Transfer(WbSetting {
        mode: WbMode::Custom,
        family,
        temperature: Some(temperature),
        tint: blend(|wb| wb.tint, family.tint_key()),
    })
}

/// `x` in the precision of the canonical key `name`.
fn rounded_like(name: &str, x: f64) -> Value {
    match canonical_key(name) {
        Some(key) => number_json(key.spec, x),
        None => json!(round_to(x, 2)),
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
            rounded_like("exposure", current + exposure_correction),
        );
    }
    if contrast_correction.abs() > 1.0 {
        let current = recipe
            .get("contrast")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        recipe.insert(
            "contrast".into(),
            rounded_like("contrast", current + contrast_correction),
        );
    }
}

/// Port of `_canonical_to_edit_recipe`: each canonical value goes to its
/// registry recipe alias below `global` (`tone_curve_shadows` →
/// `global.tone_curve.shadows`).
///
/// White balance goes to the **recipe level**, `white_balance: {mode,
/// family, temperature?, tint?}` next to `global`/`masks`, never into
/// `global`: `global.white_balance` is already the LLM schema's
/// `{temperature, tint}` alias, and installed plugins print every unknown
/// `global` key (a table shows as `table: 0x...`). Those plugins read only
/// `summary`/`global`/`masks`/`warnings` and ignore this field.
pub fn canonical_to_edit_recipe(
    canonical: &Map<String, Value>,
    summary: &str,
    white_balance: Option<&WbSetting>,
) -> Value {
    let mut global_settings = Map::new();
    for key in canonical_keys() {
        let Some(v) = canonical.get(key.name) else {
            continue;
        };
        let (leaf, parents) = key
            .recipe_path
            .split_last()
            .expect("a recipe alias has at least one segment");
        let mut node = &mut global_settings;
        for parent in parents {
            node = match node
                .entry(parent.to_string())
                .or_insert_with(|| Value::Object(Map::new()))
            {
                Value::Object(m) => m,
                _ => unreachable!("recipe alias paths never end where another continues"),
            };
        }
        node.insert(leaf.to_string(), v.clone());
    }

    let summary = if summary.is_empty() {
        "Style-matched edit by LrGeniusAI Style Engine"
    } else {
        summary
    };
    let mut recipe = json!({
        "summary": summary,
        "global": global_settings,
        "masks": [],
        "warnings": [],
    });
    if let Some(wb) = white_balance {
        recipe["white_balance"] = white_balance_json(wb);
    }
    recipe
}

#[derive(Debug, Clone)]
pub struct StyleEngineResult {
    pub recipe: Value,
    pub confidence: f64,
    pub matched_count: usize,
    pub engine: &'static str,
    /// Everything the user should know about this result, in order (a
    /// list: the confidence note and a white-balance note can both apply).
    pub warnings: Vec<String>,
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
            warnings: vec![format!(
                "Style engine inactive: only {training_count} training example(s) available (minimum {MIN_TRAINING_EXAMPLES} required). Please save more AI training examples."
            )],
            matched_filenames: Vec::new(),
        };
    }
    if candidates.is_empty() {
        return StyleEngineResult {
            recipe: json!({}),
            confidence: 0.0,
            matched_count: 0,
            engine: "none",
            warnings: vec!["No training examples could be retrieved from the database.".to_string()],
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

    // Before the pool is cut to the winners: the mode vote needs all of it.
    let white_balance = blend_white_balance(&scored, query.file_kind());
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

    let (wb_setting, wb_warning) = match white_balance {
        WbOutcome::Transfer(wb) => (Some(wb), None),
        WbOutcome::Unchanged => (None, None),
        WbOutcome::NotTransferred(w) => (None, Some(w)),
    };
    let recipe = canonical_to_edit_recipe(&blended, &summary, wb_setting.as_ref());

    // Both can apply at once, and the user needs both: one is about how well
    // the style matched, the other about a setting that was left out of it.
    let warnings: Vec<String> = confidence_warning(confidence)
        .into_iter()
        .chain(wb_warning)
        .collect();

    StyleEngineResult {
        recipe,
        confidence,
        matched_count: winners.len(),
        engine: "style",
        warnings,
        matched_filenames,
    }
}

/// The note for a match below [`CONFIDENCE_GOOD`], or `None`.
///
/// The text carries no percentage on purpose: the number is already in the
/// response's `confidence` and in the recipe summary, and the plugin merges
/// warnings by exact text into one "(N photos)" line per cause. A per-photo
/// number would make every photo its own line and crowd run-wide causes (a
/// white balance left out on every JPEG) out of the end-of-run dialog.
pub fn confidence_warning(confidence: f64) -> Option<String> {
    if confidence < CONFIDENCE_LOW {
        Some(
            "Low style match confidence. Results may not match your editing style precisely. Consider adding more training examples for this type of photo."
                .to_string(),
        )
    } else if confidence < CONFIDENCE_GOOD {
        Some("Moderate style match confidence. Review the result before applying.".to_string())
    } else {
        None
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

    /// A candidate from a real `develop_settings` blob (the plugin's
    /// JSON.lua form), canonicalised the way the route does it.
    fn from_blob(blob: Value, distance: f64) -> TrainingCandidate {
        let c = crate::training::canonicalize_develop_settings(
            &blob,
            lrg_develop::FileKindHint::Unknown,
        )
        .unwrap();
        TrainingCandidate {
            distance,
            canonical_settings: c.settings,
            white_balance: c.white_balance,
            ..Default::default()
        }
    }

    /// A raw example with the given white balance (`None` Kelvin for the
    /// non-Custom modes, which carry the camera's reading).
    fn raw_wb(mode: &str, kelvin: f64, tint: f64) -> TrainingCandidate {
        from_blob(
            json!({
                "ProcessVersion": "15.4",
                "WhiteBalance": mode,
                "Temperature": kelvin,
                "Tint": tint,
                "Contrast2012": 10,
            }),
            0.0,
        )
    }

    fn non_raw_wb(mode: &str, offset: f64, tint: f64) -> TrainingCandidate {
        from_blob(
            json!({
                "ProcessVersion": "15.4",
                "WhiteBalance": mode,
                "IncrementalTemperature": offset,
                "IncrementalTint": tint,
            }),
            0.0,
        )
    }

    fn transferred(outcome: WbOutcome) -> WbSetting {
        match outcome {
            WbOutcome::Transfer(wb) => wb,
            other => panic!("expected a transfer, got {other:?}"),
        }
    }

    fn not_transferred(outcome: WbOutcome) -> String {
        match outcome {
            WbOutcome::NotTransferred(w) => w,
            other => panic!("expected a reported non-transfer, got {other:?}"),
        }
    }

    #[test]
    fn interpolate_recipes_weighted_blend() {
        let winners = vec![
            (from_blob(json!({"Exposure2012": 1.0}), 0.0), 2.0),
            (from_blob(json!({"Exposure2012": 0.0}), 0.0), 1.0),
        ];
        let blended = interpolate_recipes(&winners);
        // 2/3 * 1.0 + 1/3 * 0.0, to Exposure2012's 2 decimals
        assert_eq!(blended["exposure"], json!(0.67));
    }

    #[test]
    fn interpolate_renormalises_per_key_over_the_winners_that_have_it() {
        // The second example never touched Clarity; it must not pull the
        // first one's +30 halfway to 0.
        let winners = vec![
            (
                from_blob(json!({"Clarity2012": 30, "Contrast2012": 10}), 0.0),
                1.0,
            ),
            (from_blob(json!({"Contrast2012": 21}), 0.0), 1.0),
        ];
        let blended = interpolate_recipes(&winners);
        assert_eq!(blended["clarity"], json!(30));
        assert_eq!(
            blended["contrast"],
            json!(16),
            "15.5 to even, as an integer"
        );
    }

    #[test]
    fn interpolate_ignores_keys_that_are_not_canonical() {
        let mut c = from_blob(json!({"Contrast2012": 10}), 0.0);
        // A version-1 row still carrying the old tint number.
        c.canonical_settings.insert("tint".into(), json!(40.0));
        let blended = interpolate_recipes(&[(c, 1.0)]);
        assert!(blended.get("tint").is_none());
        assert_eq!(blended["contrast"], json!(10));
    }

    #[test]
    fn generate_style_edit_reports_insufficient_examples() {
        let query = StyleQuery::default();
        let result = generate_style_edit(2, &[], &query);
        assert_eq!(result.engine, "none");
        assert_eq!(result.confidence, 0.0);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("only 2 training example"));
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
        let mut c = from_blob(
            json!({"Exposure2012": 1.0, "Contrast2012": 10, "ParametricDarks": -5}),
            0.1,
        );
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
        assert_eq!(result.recipe["global"]["contrast"], json!(10));
        assert_eq!(result.recipe["global"]["tone_curve"]["darks"], json!(-5));
        assert!(result.recipe.get("white_balance").is_none());
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert_eq!(result.matched_filenames, vec!["photo1.jpg".to_string()]);
    }

    #[test]
    fn a_custom_majority_with_agreeing_examples_transfers_its_white_balance() {
        let p = vec![
            (raw_wb("Custom", 5600.0, 10.0), 0.9),
            (raw_wb("Custom", 5400.0, 4.0), 0.8),
            (raw_wb("As Shot", 5000.0, 0.0), 0.7),
            (raw_wb("Custom", 5500.0, 6.0), 0.6),
            (raw_wb("Custom", 9000.0, 50.0), 0.1),
        ];
        let wb = transferred(blend_white_balance(&p, Some(FileKind::Raw)));
        assert_eq!((wb.mode, wb.family), (WbMode::Custom, WbFamily::Raw));
        // The best three Custom examples only; 9000 K is the fourth.
        // (0.9*5600 + 0.8*5400 + 0.6*5500) / 2.3 = 5504.3
        assert_eq!(wb.temperature.map(Finite::get), Some(5504.0));
        // (0.9*10 + 0.8*4 + 0.6*6) / 2.3 = 6.87
        assert_eq!(wb.tint.map(Finite::get), Some(7.0));
    }

    #[test]
    fn the_custom_numbers_are_renormalised_over_exactly_the_chosen_examples() {
        // A missing value must not count as 0: one 5600 K example among three
        // equally weighted ones would otherwise come out near 1867 K.
        let mut no_tint = raw_wb("Custom", 5600.0, 0.0);
        no_tint.white_balance.as_mut().unwrap().tint = None;
        let p = vec![
            (no_tint, 1.0),
            (raw_wb("Custom", 5600.0, 12.0), 1.0),
            (raw_wb("Custom", 5600.0, 12.0), 1.0),
        ];
        let wb = transferred(blend_white_balance(&p, Some(FileKind::Raw)));
        assert_eq!(wb.temperature.map(Finite::get), Some(5600.0));
        assert_eq!(wb.tint.map(Finite::get), Some(12.0));
    }

    #[test]
    fn a_single_custom_example_is_not_enough() {
        let p = vec![
            (raw_wb("Custom", 5600.0, 10.0), 0.9),
            (raw_wb("As Shot", 5000.0, 0.0), 0.2),
        ];
        let w = not_transferred(blend_white_balance(&p, Some(FileKind::Raw)));
        assert!(w.contains("only 1"), "{w}");
    }

    #[test]
    fn disagreeing_custom_examples_transfer_nothing_and_say_so() {
        let p = vec![
            (raw_wb("Custom", 5000.0, 0.0), 0.9),
            (raw_wb("Custom", 5600.0, 0.0), 0.8),
        ];
        let w = not_transferred(blend_white_balance(&p, Some(FileKind::Raw)));
        assert!(w.contains("disagree"), "{w}");
        assert!(w.starts_with("White balance was not transferred:"), "{w}");
        assert!(w.contains("more than 500 K apart"), "{w}");

        // The text names the limit, not the measured spread, so two photos
        // whose pools disagree by different amounts merge into one line.
        let wider = vec![
            (raw_wb("Custom", 4000.0, 0.0), 0.9),
            (raw_wb("Custom", 6500.0, 0.0), 0.8),
        ];
        let w2 = not_transferred(blend_white_balance(&wider, Some(FileKind::Raw)));
        assert_eq!(w, w2);

        // 500 K apart is still one habit.
        let p = vec![
            (raw_wb("Custom", 5000.0, 0.0), 0.9),
            (raw_wb("Custom", 5500.0, 0.0), 0.8),
        ];
        assert!(matches!(
            blend_white_balance(&p, Some(FileKind::Raw)),
            WbOutcome::Transfer(_)
        ));
    }

    #[test]
    fn an_as_shot_majority_keeps_the_photos_own_white_balance() {
        let p = vec![
            (raw_wb("Custom", 5600.0, 10.0), 0.9),
            (raw_wb("Custom", 5500.0, 10.0), 0.8),
            (raw_wb("As Shot", 5000.0, 0.0), 0.7),
            (raw_wb("As Shot", 5100.0, 0.0), 0.6),
            (raw_wb("As Shot", 5200.0, 0.0), 0.5),
        ];
        // 1.7 Custom vs 1.8 As Shot
        assert_eq!(
            blend_white_balance(&p, Some(FileKind::Raw)),
            WbOutcome::Unchanged
        );
    }

    #[test]
    fn the_vote_counts_the_whole_pool_not_only_the_blend_winners() {
        // The three best are Custom, but the pool as a whole shoots As Shot.
        let mut items = vec![
            (raw_wb("Custom", 5600.0, 10.0), 0.9),
            (raw_wb("Custom", 5500.0, 10.0), 0.9),
            (raw_wb("Custom", 5550.0, 10.0), 0.9),
        ];
        for _ in 0..6 {
            items.push((raw_wb("As Shot", 5000.0, 0.0), 0.5));
        }
        assert_eq!(
            blend_white_balance(&items, Some(FileKind::Raw)),
            WbOutcome::Unchanged
        );
    }

    #[test]
    fn a_named_mode_majority_is_not_sent_while_the_switch_is_off() {
        const { assert!(!EMIT_NAMED_WB_MODES, "turn on only after experiment E1") };
        let p = vec![
            (raw_wb("Daylight", 5500.0, 10.0), 0.9),
            (raw_wb("Daylight", 5500.0, 10.0), 0.8),
            (raw_wb("Custom", 5600.0, 10.0), 0.2),
        ];
        let w = not_transferred(blend_white_balance(&p, Some(FileKind::Raw)));
        assert!(w.contains("\"Daylight\""), "{w}");
        let p = vec![(raw_wb("Auto", 5500.0, 10.0), 0.9)];
        assert!(not_transferred(blend_white_balance(&p, Some(FileKind::Raw))).contains("Auto"));
    }

    #[test]
    fn raw_examples_give_no_white_balance_to_a_jpeg() {
        let p = vec![
            (raw_wb("Custom", 5600.0, 10.0), 0.9),
            (raw_wb("Custom", 5500.0, 10.0), 0.8),
        ];
        let w = not_transferred(blend_white_balance(&p, Some(FileKind::NonRaw)));
        assert!(
            w.contains("your training examples are raw files and this photo is not"),
            "{w}"
        );
    }

    #[test]
    fn non_raw_examples_transfer_offsets_to_non_raw_targets_only() {
        let p = vec![
            (non_raw_wb("Custom", 12.0, -4.0), 0.9),
            (non_raw_wb("Custom", 8.0, -2.0), 0.9),
        ];
        let wb = transferred(blend_white_balance(&p, Some(FileKind::NonRaw)));
        assert_eq!(wb.family, WbFamily::NonRaw);
        assert_eq!(wb.temperature.map(Finite::get), Some(10.0));
        assert_eq!(wb.tint.map(Finite::get), Some(-3.0));
        let w = not_transferred(blend_white_balance(&p, Some(FileKind::Raw)));
        assert!(w.contains("non-raw"), "{w}");
    }

    #[test]
    fn an_unknown_target_gets_no_white_balance() {
        let p = vec![
            (raw_wb("Custom", 5600.0, 10.0), 0.9),
            (raw_wb("Custom", 5500.0, 10.0), 0.8),
        ];
        let w = not_transferred(blend_white_balance(&p, None));
        assert!(w.contains("not known"), "{w}");
    }

    #[test]
    fn examples_without_a_white_balance_do_not_vote() {
        let p = vec![
            (from_blob(json!({"Exposure2012": 0.5}), 0.0), 5.0),
            (from_blob(json!([]), 0.0), 5.0),
        ];
        assert_eq!(
            blend_white_balance(&p, Some(FileKind::Raw)),
            WbOutcome::Unchanged
        );
    }

    #[test]
    fn the_style_recipe_carries_white_balance_at_recipe_level() {
        let query = StyleQuery {
            is_raw: Some(true),
            ..Default::default()
        };
        let cands = vec![
            raw_wb("Custom", 5600.0, 10.0),
            raw_wb("Custom", 5400.0, 4.0),
            raw_wb("Custom", 5500.0, 6.0),
        ];
        let result = generate_style_edit(5, &cands, &query);
        assert_eq!(
            result.recipe["white_balance"],
            json!({"mode": "Custom", "family": "raw", "temperature": 5500, "tint": 7})
        );
        let global = result.recipe["global"].as_object().unwrap();
        for key in ["temperature", "tint", "white_balance"] {
            assert!(!global.contains_key(key), "{key} must not be in global");
        }
    }

    #[test]
    fn a_jpeg_target_gets_the_white_balance_warning_next_to_the_confidence_one() {
        let query = StyleQuery {
            is_raw: Some(false),
            ..Default::default()
        };
        let cands: Vec<TrainingCandidate> = [5600.0, 5500.0, 5550.0]
            .iter()
            .map(|&k| {
                let mut c = raw_wb("Custom", k, 5.0);
                c.distance = 1.0;
                c
            })
            .collect();
        let result = generate_style_edit(5, &cands, &query);
        assert!(result.recipe.get("white_balance").is_none());
        assert_eq!(result.warnings.len(), 2, "{:?}", result.warnings);
        assert!(result.warnings[0].contains("confidence"));
        assert!(!result.warnings[0].contains('%'), "{}", result.warnings[0]);
        assert!(result.warnings[1].contains("raw files and this photo is not"));
    }

    #[test]
    fn confidence_notes_are_constant_per_band() {
        // Same band, different scores: identical text, so the plugin's
        // per-text tally merges them into one "(N photos)" line.
        assert_eq!(confidence_warning(0.62), confidence_warning(0.50));
        assert_eq!(confidence_warning(0.30), confidence_warning(0.10));
        assert_ne!(confidence_warning(0.62), confidence_warning(0.30));
        for c in [0.10, 0.30, 0.50, 0.62] {
            let w = confidence_warning(c).unwrap();
            assert!(!w.contains('%'), "{w}");
            assert!(!w.chars().any(|ch| ch.is_ascii_digit()), "{w}");
        }
        assert_eq!(confidence_warning(CONFIDENCE_GOOD), None);
    }
}
