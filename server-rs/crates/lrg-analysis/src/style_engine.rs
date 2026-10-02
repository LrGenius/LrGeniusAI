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
use lrg_develop::registry::{self, Gate, Level};
use lrg_develop::{FileKind, Finite};
use serde_json::{json, Map, Value};

use crate::training::{
    canonical_key, canonical_keys, number_json, white_balance_json, CanonicalKey,
};

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

/// The blended canonical settings of the winners, and what the blend had
/// to leave out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Blend {
    /// Canonical name → blended value (see [`interpolate_recipes`]).
    pub settings: Map<String, Value>,
    /// One note per setting the winners disagree on too much to average,
    /// constant per cause so the plugin can merge them across photos.
    pub warnings: Vec<String>,
}

/// Registry keys blended as one group because Lightroom keeps them ordered
/// (`ParametricShadowSplit < ParametricMidtoneSplit <
/// ParametricHighlightSplit`). A weighted mean of ordered triples is ordered,
/// so the group is blended only over winners that carry all of its keys.
const ORDERED_GROUPS: &[&[&str]] = &[&[
    "ParametricShadowSplit",
    "ParametricMidtoneSplit",
    "ParametricHighlightSplit",
]];

/// Below this agreement of the toned winners' hues (their mean resultant
/// length: 0 = the tints cancel out, 1 = all the same hue) the blended tint
/// kept less than this share of the toned examples' strength, and the blend
/// says so. The saturation itself is the length of the mean colour vector,
/// so disagreeing tints fade towards neutral continuously; this only decides
/// when that fading is worth a note.
pub const MIN_HUE_AGREEMENT: f64 = 0.5;

/// A zone whose linear mean saturation is below this is too faint for its
/// fading to be worth a note.
const MIN_NOTICEABLE_TONING: f64 = 5.0;

/// The note for a colour-grading zone whose tints faded in the blend.
fn toning_warning(saturation_key: &str) -> String {
    let zone = if saturation_key.contains("Shadow") {
        "shadows"
    } else if saturation_key.contains("Highlight") {
        "highlights"
    } else {
        "photo"
    };
    format!(
        "Color grading of the {zone} was toned down: your matching examples tint the {zone} in clearly different colors."
    )
}

/// The note for curve splits that could not be averaged into a valid order.
const SPLITS_WARNING: &str = "The tone curve's region splits were not transferred: your matching examples' splits could not be averaged into a valid order.";

/// Port of `interpolate_recipes`: weighted blend of the canonical settings
/// across the winners, weights normalized by composite score **per key**,
/// over the winners that have the key (an example that never touched a
/// slider must not pull it towards 0), rounded to the key's registry
/// precision (integers for integer sliders, `Exposure2012` to 2 decimals).
///
/// Two kinds of key are not plain means:
/// - **Hue angles** ([`Gate::CircularHue`], the colour-grading hues) and
///   their saturation: each winner is a colour vector (hue angle, saturation
///   length), and the zone gets the score-weighted mean vector: its angle is
///   the hue, its length the saturation. A hue at saturation 0 does not
///   pull; 350° and 10° average to 0°, not 180°; and tints that disagree
///   fade towards neutral instead of landing at full strength on a hue no
///   example used. When the saturations sum to about 0, or the mean vector
///   rounds to saturation 0, the hue is left out and saturation 0 is sent
///   (the photo keeps its own hue under no toning). When the toned winners
///   agree less than [`MIN_HUE_AGREEMENT`] the blend says so.
/// - **Ordered groups** ([`ORDERED_GROUPS`], the parametric curve splits):
///   blended together over the winners that carry the whole group; left out,
///   with a note, if rounding breaks their order.
///
/// Only keys of [`canonical_keys`] are blended; white balance is not a
/// canonical number (see [`blend_white_balance`]).
///
/// [`Gate::CircularHue`]: lrg_develop::registry::Gate::CircularHue
pub fn interpolate_recipes(winners: &[(TrainingCandidate, f64)]) -> Blend {
    let value_of = |example: &TrainingCandidate, name: &str| {
        example.canonical_settings.get(name).and_then(Value::as_f64)
    };
    let mut blend = Blend::default();
    let mut hues: Vec<(&CanonicalKey, &CanonicalKey)> = Vec::new();
    for key in canonical_keys() {
        if ORDERED_GROUPS.iter().any(|g| g.contains(&key.spec.name)) {
            continue;
        }
        if let Some(Gate::CircularHue(weight)) = key.spec.policy.gate() {
            // A hue whose saturation is not canonical has no weight: skip it.
            if let Some(sat) = canonical_keys().iter().find(|k| k.spec.name == weight) {
                hues.push((key, sat));
            }
            continue;
        }
        let mut weight = 0.0;
        let mut sum = 0.0;
        for (example, score) in winners {
            if let Some(v) = value_of(example, key.name) {
                weight += score;
                sum += score * v;
            }
        }
        if weight > 0.0 {
            blend
                .settings
                .insert(key.name.to_string(), number_json(key.spec, sum / weight));
        }
    }

    for group in ORDERED_GROUPS {
        let keys: Vec<&CanonicalKey> = group
            .iter()
            .filter_map(|name| canonical_keys().iter().find(|k| k.spec.name == *name))
            .collect();
        if keys.len() != group.len() {
            continue;
        }
        let mut weight = 0.0;
        let mut sums = vec![0.0; keys.len()];
        for (example, score) in winners {
            let values: Option<Vec<f64>> = keys.iter().map(|k| value_of(example, k.name)).collect();
            if let Some(values) = values {
                weight += score;
                for (sum, v) in sums.iter_mut().zip(values) {
                    *sum += score * v;
                }
            }
        }
        if weight <= 0.0 {
            continue;
        }
        let blended: Vec<Value> = keys
            .iter()
            .zip(&sums)
            .map(|(k, sum)| number_json(k.spec, sum / weight))
            .collect();
        // Integer rounding can make two neighbours equal (1.5 and 2.5 both
        // round to 2); Lightroom's splits must stay strictly ordered.
        let ordered = blended
            .windows(2)
            .all(|w| match (w[0].as_f64(), w[1].as_f64()) {
                (Some(a), Some(b)) => a < b,
                _ => false,
            });
        if ordered {
            for (k, v) in keys.iter().zip(blended) {
                blend.settings.insert(k.name.to_string(), v);
            }
        } else {
            // Only reachable when the winners' splits are 1 apart and the
            // means land on .5 ties (or a blob is unordered); Lightroom-written
            // triples are far apart. The photo keeps its own splits.
            log::warn!(
                "Style blend: {group:?} lost their order after rounding ({blended:?}); left out"
            );
            if !blend.warnings.iter().any(|w| w == SPLITS_WARNING) {
                blend.warnings.push(SPLITS_WARNING.to_string());
            }
        }
    }

    for (hue, sat) in hues {
        // x, y: the score-weighted sum of the colour vectors; weight: the
        // sum of score x saturation (the length the vectors would have if
        // they all agreed); score_sum: the scores of the winners carrying
        // the pair, which the mean vector is divided by.
        let (mut x, mut y, mut weight, mut score_sum) = (0.0, 0.0, 0.0, 0.0);
        for (example, score) in winners {
            let (Some(h), Some(s)) = (value_of(example, hue.name), value_of(example, sat.name))
            else {
                continue;
            };
            let w = score.max(0.0) * s.max(0.0);
            let rad = h.to_radians();
            x += w * rad.cos();
            y += w * rad.sin();
            weight += w;
            score_sum += score.max(0.0);
        }
        if weight <= 1e-9 {
            // Every winner leaves this zone untoned; the blended saturation
            // (0) says so, and a hue would be noise. A saturation without
            // any hue next to it (a blob missing one of the pair) would tint
            // the photo in whatever hue it has, so it goes too. Silent:
            // Lightroom always writes hue and saturation together, so only a
            // malformed blob gets here and the user has nothing to act on.
            if blend
                .settings
                .get(sat.name)
                .and_then(Value::as_f64)
                .is_some_and(|v| v > 0.0)
            {
                blend.settings.remove(sat.name);
            }
            continue;
        }
        let agreement = x.hypot(y) / weight;
        // The linear mean from the loop above, before the vector replaces it.
        let linear = blend
            .settings
            .get(sat.name)
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let saturation = number_json(sat.spec, x.hypot(y) / score_sum);
        let faded = saturation.as_f64().is_none_or(|v| v <= 0.0);
        blend.settings.insert(sat.name.to_string(), saturation);
        if agreement < MIN_HUE_AGREEMENT && linear >= MIN_NOTICEABLE_TONING {
            log::info!(
                "Style blend: {} hues disagree (agreement {agreement:.3}); saturation {linear:.1} faded",
                hue.spec.name,
            );
            let note = toning_warning(sat.spec.name);
            if !blend.warnings.contains(&note) {
                blend.warnings.push(note);
            }
        }
        if faded {
            // No toning left: the photo keeps its own hue under saturation 0.
            continue;
        }
        let degrees = y.atan2(x).to_degrees().rem_euclid(360.0);
        let mut value = number_json(hue.spec, degrees);
        // 359.6° rounds to 360, which is 0° (the plugin wraps it too).
        if value.as_f64().is_some_and(|v| v >= 360.0) {
            value = number_json(hue.spec, 0.0);
        }
        blend.settings.insert(hue.name.to_string(), value);
    }
    blend
}

/// Whether the style engine may send a white-balance *mode* without numbers
/// (`Auto`, `Daylight`, ...), for Lightroom to resolve per photo.
///
/// Off until experiment E1 shows that `applyDevelopSettings{WhiteBalance =
/// "Daylight"}` makes Lightroom recompute the temperature for the target
/// photo. If it does not, a mode alone would leave the photo's old Kelvin in
/// place under a new label. Until then an Auto/preset majority transfers
/// nothing and says so ([`WbOutcome::NotTransferred`]).
///
/// The same E1 answer is the Lua writer's provisional
/// `lrg_develop::lua::LuaOptions::PROVISIONAL.wb_mode_only` (whether a mode
/// alone is written at all): flip both together, with the const assert in
/// `a_named_mode_majority_is_not_sent_while_the_switch_is_off`. One without
/// the other either sends a mode the writer drops and reports, or enables a
/// form nothing produces.
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
/// `global.tone_curve.shadows`, `hsl_red_hue` → `global.hsl.red.hue`,
/// `color_grading_shadows_hue` → `global.color_grading.shadows.hue`). Those
/// nested objects are exactly what the installed plugin's
/// `DevelopEditManager` reads (`buildHslDevelopSettings`,
/// `buildColorGradingDevelopSettings`, `buildToneCurveSettings`), and they
/// are the only objects in `global`: its review dialog prints any other
/// table as an address.
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

    let Blend {
        settings: mut blended,
        warnings: blend_warnings,
    } = interpolate_recipes(&winners);
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
        .chain(blend_warnings)
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
        let blended = interpolate_recipes(&winners).settings;
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
        let blended = interpolate_recipes(&winners).settings;
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
        let blended = interpolate_recipes(&[(c, 1.0)]).settings;
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

    /// Shadow toning of one example (hue in degrees, saturation 0..100).
    fn toned(hue: f64, sat: f64) -> TrainingCandidate {
        from_blob(
            json!({
                "ProcessVersion": "15.4",
                "SplitToningShadowHue": hue,
                "SplitToningShadowSaturation": sat,
            }),
            0.0,
        )
    }

    #[test]
    fn every_field_the_installed_plugin_applies_is_blended_and_nested() {
        let blob = json!({
            "ProcessVersion": "15.4",
            "SharpenRadius": 1.26,
            "SharpenDetail": 30,
            "SharpenEdgeMasking": 12,
            "LuminanceNoiseReductionDetail": 55,
            "LuminanceNoiseReductionContrast": 5,
            "ColorNoiseReductionDetail": 50,
            "ColorNoiseReductionSmoothness": 60,
            "PostCropVignetteMidpoint": 40,
            "PostCropVignetteRoundness": -10,
            "PostCropVignetteFeather": 70,
            "PostCropVignetteHighlightContrast": 20,
            "GrainSize": 25,
            "GrainFrequency": 50,
            "HueAdjustmentAqua": -8,
            "SaturationAdjustmentBlue": -20,
            "LuminanceAdjustmentOrange": 12,
            "SplitToningShadowHue": 220,
            "SplitToningShadowSaturation": 15,
            "SplitToningHighlightHue": 45,
            "SplitToningHighlightSaturation": 20,
            "SplitToningBalance": -10,
            "ParametricShadowSplit": 20,
            "ParametricMidtoneSplit": 45,
            "ParametricHighlightSplit": 70,
            // Not sent: the installed plugin warns about and drops them.
            "ColorGradeMidtoneHue": 100,
            "ColorGradeMidtoneSat": 10,
            "ColorGradeBlending": 60,
            "ColorGradeShadowLum": 5,
        });
        let blend = interpolate_recipes(&[(from_blob(blob, 0.0), 1.0)]);
        assert!(blend.warnings.is_empty(), "{:?}", blend.warnings);
        let recipe = canonical_to_edit_recipe(&blend.settings, "", None);
        let global = &recipe["global"];
        assert_eq!(global["sharpen_radius"], json!(1.3), "one decimal");
        for (field, v) in [
            ("sharpen_detail", 30),
            ("sharpen_masking", 12),
            ("noise_reduction_detail", 55),
            ("noise_reduction_contrast", 5),
            ("color_noise_reduction_detail", 50),
            ("color_noise_reduction_smoothness", 60),
            ("vignette_midpoint", 40),
            ("vignette_roundness", -10),
            ("vignette_feather", 70),
            ("vignette_highlights", 20),
            ("grain_size", 25),
            ("grain_roughness", 50),
        ] {
            assert_eq!(global[field], json!(v), "{field}");
        }
        assert_eq!(global["hsl"]["aqua"]["hue"], json!(-8));
        assert_eq!(global["hsl"]["blue"]["saturation"], json!(-20));
        assert_eq!(global["hsl"]["orange"]["luminance"], json!(12));
        assert_eq!(
            global["color_grading"],
            json!({
                "shadows": {"hue": 220, "saturation": 15},
                "highlights": {"hue": 45, "saturation": 20},
                "balance": -10,
            })
        );
        assert_eq!(
            global["tone_curve"],
            json!({"shadow_split": 20, "midtone_split": 45, "highlight_split": 70})
        );
    }

    #[test]
    fn hsl_values_are_renormalised_per_key() {
        // The second example's settings carry no colour mixer at all (older
        // Lightroom tables can omit it); it must not pull red towards 0.
        let winners = vec![
            (
                from_blob(json!({"HueAdjustmentRed": 20, "Contrast2012": 0}), 0.0),
                1.0,
            ),
            (from_blob(json!({"Contrast2012": 10}), 0.0), 3.0),
        ];
        let blended = interpolate_recipes(&winners).settings;
        assert_eq!(blended["hsl_red_hue"], json!(20));
        // HueAdjustment* is an offset on -100..100: a plain (linear) mean.
        let winners = vec![
            (from_blob(json!({"HueAdjustmentRed": -90}), 0.0), 1.0),
            (from_blob(json!({"HueAdjustmentRed": 90}), 0.0), 1.0),
        ];
        assert_eq!(
            interpolate_recipes(&winners).settings["hsl_red_hue"],
            json!(0)
        );
    }

    #[test]
    fn split_toning_hues_average_on_the_circle() {
        // 350 and 10 are 20 degrees apart around red, not 180 apart.
        let blend = interpolate_recipes(&[(toned(350.0, 20.0), 1.0), (toned(10.0, 20.0), 1.0)]);
        assert_eq!(blend.settings["color_grading_shadows_hue"], json!(0));
        assert_eq!(
            blend.settings["color_grading_shadows_saturation"],
            json!(20)
        );
        assert!(blend.warnings.is_empty());

        // A mean just below 360 rounds to 360, which is written as 0.
        let blend = interpolate_recipes(&[(toned(359.0, 40.0), 1.0), (toned(0.0, 60.0), 1.0)]);
        assert_eq!(blend.settings["color_grading_shadows_hue"], json!(0));
    }

    #[test]
    fn split_toning_hues_are_weighted_by_score_times_saturation() {
        // Equal scores: 30 x (1, 0) + 10 x (0, 1) -> atan(1/3) = 18.4 degrees.
        let blend = interpolate_recipes(&[(toned(0.0, 30.0), 1.0), (toned(90.0, 10.0), 1.0)]);
        assert_eq!(blend.settings["color_grading_shadows_hue"], json!(18));
        // Equal saturations: the score decides. 3 x (1, 0) + 1 x (0, 1).
        let blend = interpolate_recipes(&[(toned(0.0, 20.0), 3.0), (toned(90.0, 20.0), 1.0)]);
        assert_eq!(blend.settings["color_grading_shadows_hue"], json!(18));
        // A hue at saturation 0 is invisible: it does not pull at all.
        let blend = interpolate_recipes(&[(toned(200.0, 25.0), 1.0), (toned(40.0, 0.0), 5.0)]);
        assert_eq!(blend.settings["color_grading_shadows_hue"], json!(200));
    }

    #[test]
    fn an_untoned_zone_sends_no_hue() {
        // Lightroom keeps a hue around after the saturation went back to 0.
        let blend = interpolate_recipes(&[(toned(218.0, 0.0), 1.0), (toned(33.0, 0.0), 1.0)]);
        assert!(blend.settings.get("color_grading_shadows_hue").is_none());
        assert_eq!(blend.settings["color_grading_shadows_saturation"], json!(0));
        assert!(blend.warnings.is_empty());
        let recipe = canonical_to_edit_recipe(&blend.settings, "", None);
        assert_eq!(
            recipe["global"]["color_grading"]["shadows"],
            json!({"saturation": 0})
        );
    }

    #[test]
    fn opposite_toning_hues_cancel_to_neutral_and_say_so() {
        let blend = interpolate_recipes(&[(toned(30.0, 20.0), 1.0), (toned(210.0, 20.0), 1.0)]);
        assert!(blend.settings.get("color_grading_shadows_hue").is_none());
        assert_eq!(blend.settings["color_grading_shadows_saturation"], json!(0));
        assert_eq!(blend.warnings.len(), 1);
        assert!(
            blend.warnings[0].starts_with("Color grading of the shadows was toned down"),
            "{:?}",
            blend.warnings
        );
    }

    /// Highlight toning of one example.
    fn toned_highlights(hue: f64, sat: f64) -> TrainingCandidate {
        from_blob(
            json!({
                "ProcessVersion": "15.4",
                "SplitToningHighlightHue": hue,
                "SplitToningHighlightSaturation": sat,
            }),
            0.0,
        )
    }

    #[test]
    fn disagreeing_tints_fade_towards_neutral_instead_of_a_third_colour() {
        let sat_hue = |blend: &Blend, zone: &str| {
            (
                blend.settings[&format!("color_grading_{zone}_saturation")].clone(),
                blend
                    .settings
                    .get(&format!("color_grading_{zone}_hue"))
                    .cloned(),
            )
        };
        // Orange and blue shadows: the midpoint (green) only at the length
        // of the mean colour vector, and a note.
        let blend = interpolate_recipes(&[(toned(30.0, 40.0), 1.0), (toned(190.0, 40.0), 1.0)]);
        assert_eq!(sat_hue(&blend, "shadows"), (json!(7), Some(json!(110))));
        assert_eq!(blend.warnings.len(), 1, "{:?}", blend.warnings);
        let blend = interpolate_recipes(&[(toned(47.0, 37.0), 1.0), (toned(216.0, 34.0), 1.0)]);
        assert_eq!(sat_hue(&blend, "shadows"), (json!(4), Some(json!(108))));
        assert_eq!(blend.warnings.len(), 1);
        let blend = interpolate_recipes(&[(toned(30.0, 20.0), 1.0), (toned(190.0, 20.0), 1.0)]);
        assert_eq!(sat_hue(&blend, "shadows"), (json!(3), Some(json!(110))));
        assert_eq!(blend.warnings.len(), 1);

        let blend = interpolate_recipes(&[
            (toned_highlights(37.0, 20.0), 1.0),
            (toned_highlights(213.0, 68.0), 1.0),
            (toned_highlights(16.0, 55.0), 1.0),
        ]);
        assert_eq!(
            blend.settings["color_grading_highlights_saturation"],
            json!(5)
        );
        assert!(
            blend.warnings[0].starts_with("Color grading of the highlights was toned down"),
            "{:?}",
            blend.warnings
        );

        // Hues that agree keep the linear mean: untoned winners dilute, they
        // do not disagree, so there is no note.
        let blend = interpolate_recipes(&[
            (toned(40.0, 35.0), 1.0),
            (toned(200.0, 0.0), 1.0),
            (toned(90.0, 0.0), 1.0),
        ]);
        assert_eq!(sat_hue(&blend, "shadows"), (json!(12), Some(json!(40))));
        assert!(blend.warnings.is_empty(), "{:?}", blend.warnings);
    }

    #[test]
    fn the_curve_splits_are_blended_as_one_ordered_group() {
        let splits = |s: i64, m: i64, h: i64| {
            json!({
                "ParametricShadowSplit": s,
                "ParametricMidtoneSplit": m,
                "ParametricHighlightSplit": h,
            })
        };
        let winners = vec![
            (from_blob(splits(10, 30, 50), 0.0), 1.0),
            (from_blob(splits(40, 60, 90), 0.0), 1.0),
            // Carries only one split: not part of the group's blend.
            (from_blob(json!({"ParametricShadowSplit": 80}), 0.0), 5.0),
        ];
        let blended = interpolate_recipes(&winners).settings;
        assert_eq!(blended["tone_curve_shadow_split"], json!(25));
        assert_eq!(blended["tone_curve_midtone_split"], json!(45));
        assert_eq!(blended["tone_curve_highlight_split"], json!(70));

        // Rounding half to even can collapse neighbours (25.5 and 26.5 both
        // become 26): the group is then left out rather than sent unordered,
        // and the blend says so.
        let winners = vec![
            (from_blob(splits(25, 26, 70), 0.0), 1.0),
            (from_blob(splits(26, 27, 71), 0.0), 1.0),
        ];
        let blend = interpolate_recipes(&winners);
        for key in [
            "tone_curve_shadow_split",
            "tone_curve_midtone_split",
            "tone_curve_highlight_split",
        ] {
            assert!(blend.settings.get(key).is_none(), "{key}: {blend:?}");
        }
        assert_eq!(blend.warnings, [SPLITS_WARNING]);
    }

    #[test]
    fn a_toning_note_reaches_the_style_result() {
        let result = generate_style_edit(
            5,
            &[toned(30.0, 20.0), toned(210.0, 20.0)],
            &StyleQuery::default(),
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.starts_with("Color grading of the shadows")),
            "{:?}",
            result.warnings
        );
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
