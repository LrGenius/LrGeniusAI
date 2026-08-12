//! CLIP-IQA: an aesthetic / quality score read straight off the SigLIP2
//! embedding that indexing already computed and stored.
//!
//! What this replaces is `cull_aesthetic`, a weighted sum of contrast,
//! colourfulness and exposure. That heuristic rewards punchy, saturated frames
//! and floors muted fine-art portraiture, fog, snow, low-key work and every
//! desaturated editorial look — it measures how loud a picture is, not how good
//! it is, and no amount of re-weighting fixes a signal that is asking the wrong
//! question.
//!
//! The method (Wang, Chan & Loy, *Exploring CLIP for Assessing the Look and
//! Feel of Images*) is to ask the model directly, with pairs of opposed
//! prompts: embed "a good photo" and "a bad photo", and score an image by which
//! it sits closer to. It costs one text-tower pass for the whole prompt set,
//! once per server lifetime — after that every photo is a handful of dot
//! products against a vector the database already holds. There is no per-photo
//! image inference, which is what makes this affordable in the culling hot
//! path where a learned IQA model would not be.
//!
//! It applies only to photos that carry an embedding, so the fast `tasks=cull`
//! ingest — which deliberately skips SigLIP2 — degrades to the old heuristic
//! rather than failing.

use crate::siglip::{l2_normalize, SiglipError, SiglipModel};

/// Opposed prompt pairs, `(positive, negative)`.
///
/// Kept few and plainly worded on purpose. CLIP-IQA's own ablation finds short
/// unadorned antonyms beat elaborate prompts, and every pair added is another
/// text-tower row plus another axis along which the score can be diluted.
///
/// The set spans the three judgements culling's other signals cannot make:
/// whether the frame reads as a *good photograph* at all, whether its light is
/// working, and whether it is composed. Sharpness and exposure are deliberately
/// *not* asked about — those are measured directly and far more reliably from
/// pixels, and duplicating them here would just add a noisy second opinion.
pub const DEFAULT_PROMPT_PAIRS: &[(&str, &str)] = &[
    ("a good photo.", "a bad photo."),
    ("a beautiful photo.", "an ugly photo."),
    (
        "a well-composed photograph.",
        "a badly composed photograph.",
    ),
    (
        "a photo with beautiful lighting.",
        "a photo with bad lighting.",
    ),
    ("an interesting photo.", "a boring photo."),
];

/// Prompt pairs for *moment* rather than quality: is this the frame where
/// something is happening?
///
/// This exists because of a concrete complaint about a soccer series. In a
/// faceless action burst the score reduces to roughly 46% sharpness, 32%
/// exposure, 14% low-noise and 7% aesthetics — and within one burst exposure
/// and noise barely move, so the pick is decided by which frame carries the
/// most high-frequency detail. That tracks crowd texture, grass and how far the
/// player's limbs are spread. It does not track whether the ball is at their
/// foot. Nothing in the technical signals can express "peak action", which is
/// what actually decides a sports pick.
///
/// **Calibrate expectations.** CLIP-family models are reliable at "is there a
/// ball", "is this a kick", "are the feet off the ground" — object presence and
/// coarse pose. They are much weaker at fine temporal ordering, so do not
/// expect this to identify *the* peak frame out of five adjacent ones at 20fps.
/// It should reliably separate the frames where the action is from the frames
/// between plays, which is the larger part of the problem.
///
/// The pairs are written as concrete, visible facts rather than judgements for
/// that reason: "the ball is in the frame" is something the model can see,
/// "this is the decisive moment" is not. They are also deliberately sport-
/// agnostic in wording — "athlete", "ball", "in mid-air" — since one preset
/// covers every field sport.
pub const ACTION_PROMPT_PAIRS: &[(&str, &str)] = &[
    (
        "an athlete in the middle of the action.",
        "an athlete standing still between plays.",
    ),
    (
        "a photo of a player about to strike the ball.",
        "a photo of a player waiting with no ball nearby.",
    ),
    (
        "an athlete with both feet off the ground.",
        "an athlete standing upright on the ground.",
    ),
    (
        "the ball is clearly visible in the frame.",
        "there is no ball in the frame.",
    ),
    (
        "an athlete's body fully extended in effort.",
        "an athlete in a relaxed, neutral pose.",
    ),
];

/// Prompt pairs for facial expression, for portrait and family work.
///
/// The genre table says a portrait is decided by micro-expression, and nothing
/// measures it — `cull_face_score` is sharpness, size, position and a crude
/// eye-openness proxy, none of which can tell a warm smile from a rictus.
///
/// Expression is one of the things CLIP-family models genuinely read well: it is
/// coarse, global, and heavily represented in image-caption training data. It is
/// still a *whole-frame* judgement, so on a group shot it reports the mood of
/// the picture rather than of any one face — useful, but not a substitute for
/// per-face analysis.
pub const EXPRESSION_PROMPT_PAIRS: &[(&str, &str)] = &[
    (
        "a person with a warm, genuine smile.",
        "a person with a flat, awkward expression.",
    ),
    (
        "a natural, relaxed expression.",
        "a stiff, self-conscious expression.",
    ),
    (
        "a portrait where the subject connects with the viewer.",
        "a portrait where the subject looks disengaged.",
    ),
];

/// Prompt pairs for the candid moment, for event, wedding and street work.
///
/// The genre table gives "the moment" as what decides a wedding or street pick,
/// and "moment and gesture over technical" for documentary — the one genre where
/// grain and motion blur are explicitly legitimate. The technical signals cannot
/// express any of that; at best they rank the frames by how tidy they are, which
/// is close to the opposite of what the work is about.
///
/// Written as observable relations between people rather than as judgements,
/// for the same reason as the action set: "people are looking at each other" is
/// something a model can see, "this is the decisive moment" is not.
pub const CANDID_PROMPT_PAIRS: &[(&str, &str)] = &[
    (
        "people caught in a genuine, unposed moment.",
        "people posing stiffly for the camera.",
    ),
    (
        "people reacting to each other.",
        "people standing around waiting.",
    ),
    (
        "a photograph with a story in it.",
        "a photograph where nothing is happening.",
    ),
];

/// Which question a [`IqaPrompts`] set is asking.
///
/// Kept as separate sets rather than one long list because they are scored and
/// weighted separately: quality applies to every photograph, the others are
/// genre-specific, and averaging them would let a technically pretty frame of
/// nothing outrank a slightly softer frame of the goal.
///
/// **What is deliberately not here**, because CLIP would answer badly and
/// something else already answers well:
///
/// - *Sharpness and exposure.* Measured directly from pixels, far more
///   reliably. A prompt would add a noisy second opinion to a settled question.
/// - *Eyes open / blink.* Fine facial detail at small scale is exactly where
///   these models are weakest, and the analysis doc is explicit that this needs
///   a real eye-state classifier — a prompt pair here would look like progress
///   while changing nothing.
/// - *Horizon level, verticals, corner-to-corner sharpness.* Geometric and
///   spatial relations are a known blind spot for contrastive image-text models.
///   These are measurable directly and should be.
/// - *Animal eye sharpness.* Needs detection, not a global scene judgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptSet {
    /// Is this a good photograph? [`DEFAULT_PROMPT_PAIRS`].
    Quality,
    /// Is something happening in it? [`ACTION_PROMPT_PAIRS`].
    Action,
    /// Is the face working? [`EXPRESSION_PROMPT_PAIRS`].
    Expression,
    /// Is this a real moment or a pose? [`CANDID_PROMPT_PAIRS`].
    Candid,
}

impl PromptSet {
    pub fn pairs(self) -> &'static [(&'static str, &'static str)] {
        match self {
            PromptSet::Quality => DEFAULT_PROMPT_PAIRS,
            PromptSet::Action => ACTION_PROMPT_PAIRS,
            PromptSet::Expression => EXPRESSION_PROMPT_PAIRS,
            PromptSet::Candid => CANDID_PROMPT_PAIRS,
        }
    }

    /// Stable name, used as the cache key and as the axis label a preset names.
    ///
    /// A string rather than the enum because `lrg-analysis` selects the axis and
    /// cannot depend on this crate.
    pub fn as_str(self) -> &'static str {
        match self {
            PromptSet::Quality => "quality",
            PromptSet::Action => "action",
            PromptSet::Expression => "expression",
            PromptSet::Candid => "candid",
        }
    }

    /// Inverse of [`Self::as_str`]. Not `FromStr`: an unknown axis name is a
    /// config typo the caller should ignore and log, not an error to propagate.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "quality" => Some(PromptSet::Quality),
            "action" => Some(PromptSet::Action),
            "expression" => Some(PromptSet::Expression),
            "candid" => Some(PromptSet::Candid),
            _ => None,
        }
    }

    /// Every set except [`PromptSet::Quality`], which is not genre-selectable.
    pub const SEMANTIC: [PromptSet; 3] =
        [PromptSet::Action, PromptSet::Expression, PromptSet::Candid];
}

/// The logit scale CLIP-style contrastive models are trained with. The softmax
/// over two cosine similarities is meaningless without it: raw cosines for a
/// prompt pair typically differ by a few hundredths, which a plain softmax
/// would flatten to ~0.5 for every image on earth.
const LOGIT_SCALE: f64 = 100.0;

/// L2-normalized text embeddings for [`DEFAULT_PROMPT_PAIRS`], computed once.
#[derive(Clone)]
pub struct IqaPrompts {
    pairs: Vec<(Vec<f32>, Vec<f32>)>,
}

impl IqaPrompts {
    /// Runs the text tower over every prompt in one batch.
    ///
    /// This is the only expensive call in the module, and it also loads the
    /// SigLIP2 model if it has idle-unloaded. Callers should treat failure as
    /// "no IQA available" rather than an error — the aesthetic heuristic is
    /// still there.
    pub fn compute(model: &SiglipModel) -> Result<Self, SiglipError> {
        Self::compute_set(model, PromptSet::Quality)
    }

    /// [`Self::compute`] for a named set. One text-tower call per set; callers
    /// cache the result for the process lifetime.
    pub fn compute_set(model: &SiglipModel, set: PromptSet) -> Result<Self, SiglipError> {
        let texts: Vec<String> = set
            .pairs()
            .iter()
            .flat_map(|(pos, neg)| [pos.to_string(), neg.to_string()])
            .collect();
        let mut embeddings = model.embed_text(&texts)?;
        for e in embeddings.iter_mut() {
            l2_normalize(e);
        }
        let pairs = embeddings
            .chunks_exact(2)
            .map(|c| (c[0].clone(), c[1].clone()))
            .collect();
        Ok(IqaPrompts { pairs })
    }

    pub fn pair_count(&self) -> usize {
        self.pairs.len()
    }

    /// Scores one image embedding in `0..1`, higher being better.
    ///
    /// `image` need not be normalized; a copy is normalized here, because the
    /// vectors coming back out of `IMAGE_TABLE` are stored normalized but
    /// nothing in the schema enforces it.
    ///
    /// Returns `None` for a dimension mismatch or an all-zero vector — the
    /// nullable-vector convention for "this photo has no embedding".
    pub fn score(&self, image: &[f32]) -> Option<f64> {
        if self.pairs.is_empty() || image.is_empty() {
            return None;
        }
        let mut v = image.to_vec();
        l2_normalize(&mut v);
        if v.iter().all(|x| *x == 0.0) {
            return None;
        }

        let mut total = 0.0f64;
        for (pos, neg) in &self.pairs {
            if pos.len() != v.len() {
                return None;
            }
            let sim = |t: &[f32]| -> f64 {
                v.iter()
                    .zip(t)
                    .map(|(a, b)| *a as f64 * *b as f64)
                    .sum::<f64>()
                    * LOGIT_SCALE
            };
            let (sp, sn) = (sim(pos), sim(neg));
            // Two-way softmax, computed as a logistic on the difference so a
            // large negative exponent cannot overflow.
            total += 1.0 / (1.0 + (sn - sp).exp());
        }
        Some((total / self.pairs.len() as f64).clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds prompts directly rather than through the text tower, so the
    /// scoring maths can be tested without a model on disk.
    fn prompts(pairs: Vec<(Vec<f32>, Vec<f32>)>) -> IqaPrompts {
        let mut pairs = pairs;
        for (a, b) in pairs.iter_mut() {
            l2_normalize(a);
            l2_normalize(b);
        }
        IqaPrompts { pairs }
    }

    #[test]
    fn an_image_aligned_with_the_positive_prompt_scores_high() {
        let p = prompts(vec![(vec![1.0, 0.0], vec![0.0, 1.0])]);
        let score = p.score(&[1.0, 0.0]).unwrap();
        assert!(score > 0.99, "got {score}");
    }

    #[test]
    fn an_image_aligned_with_the_negative_prompt_scores_low() {
        let p = prompts(vec![(vec![1.0, 0.0], vec![0.0, 1.0])]);
        let score = p.score(&[0.0, 1.0]).unwrap();
        assert!(score < 0.01, "got {score}");
    }

    /// Equidistant from both prompts is genuinely no information, and must come
    /// back as the midpoint rather than drifting to one side.
    #[test]
    fn an_equidistant_image_scores_one_half() {
        let p = prompts(vec![(vec![1.0, 0.0], vec![0.0, 1.0])]);
        let score = p.score(&[1.0, 1.0]).unwrap();
        assert!((score - 0.5).abs() < 1e-9, "got {score}");
    }

    /// The logit scale is what makes the score usable: without it, cosine
    /// differences of a few hundredths — the realistic range — collapse to
    /// ~0.5 and the signal disappears.
    #[test]
    fn small_cosine_differences_still_separate() {
        // Two prompts 0.02 apart in cosine from the image.
        let p = prompts(vec![(vec![1.0, 0.0], vec![0.98, 0.199])]);
        let score = p.score(&[1.0, 0.0]).unwrap();
        assert!(
            score > 0.8,
            "a 0.02 cosine edge should be decisive, got {score}"
        );
    }

    #[test]
    fn a_zero_vector_has_no_score() {
        let p = prompts(vec![(vec![1.0, 0.0], vec![0.0, 1.0])]);
        assert!(p.score(&[0.0, 0.0]).is_none());
        assert!(p.score(&[]).is_none());
    }

    #[test]
    fn a_dimension_mismatch_has_no_score() {
        let p = prompts(vec![(vec![1.0, 0.0], vec![0.0, 1.0])]);
        assert!(p.score(&[1.0, 0.0, 0.0]).is_none());
    }

    #[test]
    fn every_pair_in_every_set_is_distinct_and_non_empty() {
        for set in [PromptSet::Quality, PromptSet::Action] {
            let pairs = set.pairs();
            assert!(!pairs.is_empty(), "{set:?} has no pairs");
            for (pos, neg) in pairs {
                assert!(!pos.is_empty() && !neg.is_empty(), "{set:?}");
                assert_ne!(pos, neg, "{set:?}");
            }
        }
    }

    /// The two sets must ask different questions. Sharing a prompt would mean
    /// the same evidence is counted twice under two weights, which is exactly
    /// the double-counting the split exists to avoid.
    #[test]
    fn the_prompt_sets_do_not_overlap() {
        let quality: Vec<&str> = DEFAULT_PROMPT_PAIRS
            .iter()
            .flat_map(|(a, b)| [*a, *b])
            .collect();
        for (a, b) in ACTION_PROMPT_PAIRS {
            assert!(!quality.contains(a), "{a:?} appears in both sets");
            assert!(!quality.contains(b), "{b:?} appears in both sets");
        }
    }
}
