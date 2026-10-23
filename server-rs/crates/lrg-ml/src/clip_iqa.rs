//! CLIP-IQA: zero-shot scores read straight off the SigLIP2 embedding that
//! indexing already computed and stored.
//!
//! The method (Wang, Chan & Loy, *Exploring CLIP for Assessing the Look and
//! Feel of Images*) is to ask the model directly, with pairs of opposed
//! prompts: embed "a photo of an athlete in mid-air" and "a photo of an
//! athlete standing still", and score an image by which it sits closer to. The
//! prompts cost one text-tower pass, once per model: the backend keeps their
//! vectors in the database's `text_embeddings` table (`lrg-api`'s
//! `prompt_embeddings`), so after the first run nothing loads the text tower for
//! them at all. Every photo is then a handful of dot products against a vector
//! the database already holds. There is no per-photo image inference, which is
//! what makes this affordable in the culling hot path.
//!
//! **What this no longer does is judge overall quality.** A "good photo / bad
//! photo" pair set used to feed the aesthetic term of culling; that is now the
//! AestheticSigLIP score stored with every photo (`crate::aesthetic`), a model
//! trained for exactly that question. What remains here are the questions a
//! general aesthetic model cannot answer: the *moment* (action, expression,
//! candid, emotion), whether closed eyes are meant, and whether an organism is
//! in the frame.
//!
//! It applies only to photos that carry an embedding, so the fast `tasks=cull`
//! ingest — which deliberately skips SigLIP2 — degrades to ranking without it
//! rather than failing.

use crate::siglip::l2_normalize;

/// Opposed prompt pairs are `(positive, negative)`; every set below follows the
/// CLIP-IQA recipe. Kept few and plainly worded on purpose: its own ablation
/// finds short unadorned antonyms beat elaborate prompts, and every pair added
/// is another text-tower row plus another axis along which the score can be
/// diluted.
///
/// Prompt pairs for the *moment*: is this the frame where
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

/// Prompt pairs for emotion in sport: is somebody in this frame *feeling*
/// something strongly?
///
/// The action set asks whether play is happening; it cannot see the goal
/// celebration that follows, the striker on his knees after the miss, or the
/// keeper screaming at his defence. In sport those are often the frames that
/// get published, and none of them has a ball near anybody's foot. So emotion
/// is asked separately, and ranking takes the *stronger* of the two answers as
/// the frame's moment rather than their average.
///
/// What counts is *intensity*, not whether the emotion is happy. Despair and
/// anger are as valuable to a sports photographer as joy, so every pair names
/// several emotions on the positive side — the organism set's lesson applies:
/// a pair that only covers joy drags a frame of despair down with it, because
/// [`IqaPrompts::score`] averages over pairs.
pub const SPORT_EMOTION_PROMPT_PAIRS: &[(&str, &str)] = &[
    (
        "an athlete showing intense emotion, such as joy, anger or despair.",
        "an athlete with a calm, neutral face.",
    ),
    (
        "players celebrating, protesting or consoling each other with big gestures.",
        "players standing apart, waiting calmly.",
    ),
    (
        "an athlete screaming, cheering or crying.",
        "an athlete looking relaxed and expressionless.",
    ),
    (
        "a dramatic, emotional moment in a sports game.",
        "an uneventful moment in a sports game.",
    ),
];

/// Prompt pairs for emotion at events: weddings, parties, company events,
/// concerts.
///
/// Here the emotion that counts is warm — laughter, happy tears, a hug, a
/// guest's delighted reaction — and the failure that counts is the
/// unflattering frame: mid-sentence, mid-bite, a blank stare. The last pair
/// asks about that directly, because an event photographer's first cull is
/// largely throwing those out.
///
/// Same construction as the sport set: each pair is a complete question on its
/// own, naming several emotions, so averaging cannot punish a frame for showing
/// the "wrong" kind of joy.
pub const EVENT_EMOTION_PROMPT_PAIRS: &[(&str, &str)] = &[
    (
        "people laughing, crying with joy or hugging.",
        "people with blank, neutral faces.",
    ),
    (
        "a heartfelt, emotional moment between people.",
        "an ordinary moment where nobody shows any emotion.",
    ),
    (
        "guests reacting with delight or surprise.",
        "guests looking bored or distracted.",
    ),
    (
        "people with natural, flattering expressions.",
        "a person caught mid-sentence or mid-bite with an awkward face.",
    ),
];

/// Prompt pairs asking whether closed eyes in this frame are *meant*: a kiss,
/// or laughter hard enough to squeeze the eyes shut.
///
/// Not a grading axis, a gate, like [`ORGANISM_PROMPT_PAIRS`]. The blink proxy
/// cannot tell a kiss from a blink — both are closed eyes — so without this the
/// kiss at a wedding is the frame most reliably marked as a reject candidate.
/// The ranking skips the blink penalty for a frame that clears the gate.
///
/// The last pair contrasts the two causes head-on, which is the question
/// actually being asked; the first two keep a frame where nobody is kissing or
/// laughing firmly below the threshold.
pub const EYES_CLOSED_INTENT_PROMPT_PAIRS: &[(&str, &str)] = &[
    (
        "people kissing or laughing so hard their eyes are closed.",
        "people with their eyes open, neither kissing nor laughing.",
    ),
    (
        "a kiss or a burst of laughter.",
        "a calm moment without a kiss or laughter.",
    ),
    (
        "eyes closed in a kiss or in laughter.",
        "eyes closed in an accidental blink.",
    ),
];

/// Prompt pairs asking whether there is an *organism* in the frame at all.
///
/// Unlike every other set here this is not a grading axis — nothing weights it
/// into a cull score. It is a gate: BioCLIP 2 is a ViT-L/14 and costs a few
/// hundred milliseconds per photo on CPU, which is not something to spend on a
/// wedding portrait or a street scene. Scoring the SigLIP2 embedding the
/// database already holds costs nothing extra and answers "is this worth
/// asking BioCLIP about" well enough to skip most of a mixed catalog.
///
/// Written to separate *subject*, not *setting*. "A photo taken outdoors" would
/// pass every landscape; "an organism fills the frame" is the actual
/// precondition for a useful species call. The negatives name the things a
/// general photo library is mostly made of, so a portrait scores low even when
/// it was shot in a forest.
///
/// **Every pair names both animals and plants**, and that is not redundancy —
/// it is the fix for a measured bug. An earlier version had one animal pair,
/// one plant pair and one animal-leaning pair. Because [`IqaPrompts::score`]
/// *averages* over pairs, a frame-filling flower scored high on the one plant
/// pair and low on the other two, landing under the threshold: a live run over
/// 38 photos rejected a macro of an Allium and a bed of roses while correctly
/// rejecting a bronze sculpture. A pair that only half the organisms can
/// answer drags down the mean for the other half, so each one has to be a
/// complete "is there an organism here" question.
///
/// Deliberately generous rather than precise: a false positive costs one
/// wasted BioCLIP pass and a low-confidence result the rank floor discards, a
/// false negative silently loses a species the user wanted. The default
/// threshold in the indexing route is set with that asymmetry in mind.
pub const ORGANISM_PROMPT_PAIRS: &[(&str, &str)] = &[
    (
        "a photo of a living animal or plant.",
        "a photo of people, buildings, or objects.",
    ),
    (
        "an animal, bird, insect, flower, or fungus is the subject of this photo.",
        "the subject of this photo is a person or something man-made.",
    ),
    (
        "a close-up photo of a living thing in nature.",
        "there is no animal or plant in this photo.",
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptSet {
    /// Is something happening in it? [`ACTION_PROMPT_PAIRS`].
    Action,
    /// Is the face working? [`EXPRESSION_PROMPT_PAIRS`].
    Expression,
    /// Is this a real moment or a pose? [`CANDID_PROMPT_PAIRS`].
    Candid,
    /// Is an athlete feeling something strongly? [`SPORT_EMOTION_PROMPT_PAIRS`].
    SportEmotion,
    /// Are people laughing, moved, reacting? [`EVENT_EMOTION_PROMPT_PAIRS`].
    EventEmotion,
    /// Are closed eyes meant — a kiss, a laugh? [`EYES_CLOSED_INTENT_PROMPT_PAIRS`].
    ///
    /// A gate on the blink penalty, not a grading axis — deliberately absent
    /// from [`PromptSet::SEMANTIC`].
    EyesClosedIntent,
    /// Is there an organism in the frame? [`ORGANISM_PROMPT_PAIRS`].
    ///
    /// A gate for the species task, not a grading axis — deliberately absent
    /// from [`PromptSet::SEMANTIC`].
    Organism,
}

impl PromptSet {
    pub fn pairs(self) -> &'static [(&'static str, &'static str)] {
        match self {
            PromptSet::Action => ACTION_PROMPT_PAIRS,
            PromptSet::Expression => EXPRESSION_PROMPT_PAIRS,
            PromptSet::Candid => CANDID_PROMPT_PAIRS,
            PromptSet::SportEmotion => SPORT_EMOTION_PROMPT_PAIRS,
            PromptSet::EventEmotion => EVENT_EMOTION_PROMPT_PAIRS,
            PromptSet::EyesClosedIntent => EYES_CLOSED_INTENT_PROMPT_PAIRS,
            PromptSet::Organism => ORGANISM_PROMPT_PAIRS,
        }
    }

    /// Every prompt of the set, positive before negative, pair by pair — the
    /// texts whose embeddings [`IqaPrompts::from_lookup`] asks for.
    pub fn texts(self) -> impl Iterator<Item = &'static str> {
        self.pairs().iter().flat_map(|(pos, neg)| [*pos, *neg])
    }

    /// Stable name, used as the cache key and as the axis label a preset names.
    ///
    /// A string rather than the enum because `lrg-analysis` selects the axis and
    /// cannot depend on this crate.
    pub fn as_str(self) -> &'static str {
        match self {
            PromptSet::Action => "action",
            PromptSet::Expression => "expression",
            PromptSet::Candid => "candid",
            PromptSet::SportEmotion => "sport_emotion",
            PromptSet::EventEmotion => "event_emotion",
            PromptSet::EyesClosedIntent => "eyes_closed_intent",
            PromptSet::Organism => "organism",
        }
    }

    /// Inverse of [`Self::as_str`]. Not `FromStr`: an unknown axis name is a
    /// config typo the caller should ignore and log, not an error to propagate.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "action" => Some(PromptSet::Action),
            "expression" => Some(PromptSet::Expression),
            "candid" => Some(PromptSet::Candid),
            "sport_emotion" => Some(PromptSet::SportEmotion),
            "event_emotion" => Some(PromptSet::EventEmotion),
            "eyes_closed_intent" => Some(PromptSet::EyesClosedIntent),
            "organism" => Some(PromptSet::Organism),
            _ => None,
        }
    }

    /// The genre-selectable grading axes: every set except the two gates —
    /// [`PromptSet::Organism`] for the species task and
    /// [`PromptSet::EyesClosedIntent`] for the blink penalty — which decide
    /// whether something runs rather than grading anything.
    pub const SEMANTIC: [PromptSet; 5] = [
        PromptSet::Action,
        PromptSet::Expression,
        PromptSet::Candid,
        PromptSet::SportEmotion,
        PromptSet::EventEmotion,
    ];

    /// Every set, so the invariant tests cannot silently skip a new one.
    pub const ALL: [PromptSet; 7] = [
        PromptSet::Action,
        PromptSet::Expression,
        PromptSet::Candid,
        PromptSet::SportEmotion,
        PromptSet::EventEmotion,
        PromptSet::EyesClosedIntent,
        PromptSet::Organism,
    ];
}

/// The logit scale CLIP-style contrastive models are trained with. The softmax
/// over two cosine similarities is meaningless without it: raw cosines for a
/// prompt pair typically differ by a few hundredths, which a plain softmax
/// would flatten to ~0.5 for every image on earth.
const LOGIT_SCALE: f64 = 100.0;

/// L2-normalized text embeddings for one [`PromptSet`]'s pairs.
#[derive(Clone)]
pub struct IqaPrompts {
    pairs: Vec<(Vec<f32>, Vec<f32>)>,
}

impl IqaPrompts {
    /// Builds the set from embeddings the caller already has, `None` when any
    /// of its texts is missing.
    ///
    /// Takes a lookup rather than a model because the text tower is the
    /// expensive way to get these: they are a pure function of the weights
    /// and a compile-time list of strings, so the backend stores them once
    /// per model and reads them back. A half-built set is refused rather than
    /// scored, since [`Self::score`] averages over pairs and a missing pair
    /// would quietly change what the number means.
    ///
    /// The vectors are normalized here whatever the lookup hands over.
    pub fn from_lookup<'a>(
        set: PromptSet,
        lookup: impl Fn(&str) -> Option<&'a [f32]>,
    ) -> Option<Self> {
        let pairs = set
            .pairs()
            .iter()
            .map(|(pos, neg)| {
                let mut pos = lookup(pos)?.to_vec();
                let mut neg = lookup(neg)?.to_vec();
                l2_normalize(&mut pos);
                l2_normalize(&mut neg);
                Some((pos, neg))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(IqaPrompts { pairs })
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
    fn from_lookup_builds_a_complete_set_and_refuses_a_partial_one() {
        let vectors: std::collections::HashMap<&str, Vec<f32>> = PromptSet::Organism
            .texts()
            .enumerate()
            .map(|(i, text)| (text, vec![i as f32 + 1.0, 1.0]))
            .collect();
        let full =
            IqaPrompts::from_lookup(PromptSet::Organism, |t| vectors.get(t).map(Vec::as_slice))
                .unwrap();
        assert_eq!(full.pair_count(), ORGANISM_PROMPT_PAIRS.len());

        let missing = ORGANISM_PROMPT_PAIRS[1].1;
        let partial = IqaPrompts::from_lookup(PromptSet::Organism, |t| {
            (t != missing)
                .then(|| vectors.get(t).map(Vec::as_slice))
                .flatten()
        });
        assert!(partial.is_none());
    }

    /// The lookup's vectors need not be normalized; scoring assumes they are.
    #[test]
    fn from_lookup_normalizes_what_it_is_given() {
        let pos = vec![10.0, 0.0];
        let neg = vec![0.0, 0.5];
        let p = IqaPrompts::from_lookup(PromptSet::Candid, |t| {
            if PromptSet::Candid.pairs().iter().any(|(a, _)| *a == t) {
                Some(pos.as_slice())
            } else {
                Some(neg.as_slice())
            }
        })
        .unwrap();
        for (a, b) in &p.pairs {
            assert_eq!(a, &vec![1.0, 0.0]);
            assert_eq!(b, &vec![0.0, 1.0]);
        }
    }

    #[test]
    fn every_pair_in_every_set_is_distinct_and_non_empty() {
        for set in PromptSet::ALL {
            let pairs = set.pairs();
            assert!(!pairs.is_empty(), "{set:?} has no pairs");
            for (pos, neg) in pairs {
                assert!(!pos.is_empty() && !neg.is_empty(), "{set:?}");
                assert_ne!(pos, neg, "{set:?}");
            }
        }
    }

    #[test]
    fn every_set_name_round_trips() {
        for set in PromptSet::ALL {
            assert_eq!(PromptSet::from_name(set.as_str()), Some(set));
        }
    }

    /// The sets must ask different questions. Sharing a prompt would mean the
    /// same evidence is counted twice under two weights, which is exactly the
    /// double-counting the split exists to avoid.
    #[test]
    fn the_prompt_sets_do_not_overlap() {
        for (i, set) in PromptSet::ALL.iter().enumerate() {
            let own: Vec<&str> = set.pairs().iter().flat_map(|(a, b)| [*a, *b]).collect();
            for other in &PromptSet::ALL[i + 1..] {
                for (a, b) in other.pairs() {
                    assert!(!own.contains(a), "{a:?} appears in {set:?} and {other:?}");
                    assert!(!own.contains(b), "{b:?} appears in {set:?} and {other:?}");
                }
            }
        }
    }
}
