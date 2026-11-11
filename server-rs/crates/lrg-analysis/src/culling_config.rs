//! Port of `config.py`'s `BASE_CULLING_CONFIG` + `CULLING_PRESETS` +
//! `get_culling_config` (deep-merge preset overrides onto the base).
//! Centralizes every tunable weight/threshold used by grouping and
//! ranking so behavior stays identical to Python without hand-copying
//! magic numbers into each algorithm module.
//!
//! [`ImageMetricsConfig`] and [`FaceMetricsConfig`] are re-exported from
//! `lrg-imaging`, which is where the code that reads them lives (that crate
//! sits below both `lrg-ml` and this one). They used to be declared here *and*
//! duplicated as private `const`s next to each algorithm; only the `const`s
//! were ever read, so every preset override of an image or face metric was
//! silently discarded. There is now one definition and the presets reach it.
//!
//! Note which knobs a preset can actually move. Image metrics are computed once
//! at index time and stored, so the *threshold-shaped* fields (denominators,
//! exposure target, clip thresholds) are baked into the stored sub-scores and
//! only change on a re-index. The *weight-shaped* fields
//! (`technical_weight_*`, `aesthetic_*_weight`) are re-applied at rank time by
//! [`crate::grouping::rank_group_records`], so presets do move those per run.

pub use lrg_imaging::cull_config::{FaceMetricsConfig, ImageMetricsConfig};

#[derive(Debug, Clone, Copy)]
pub struct GroupingConfig {
    pub time_window_default_seconds: i64,
    pub phash_hamming_auto: f64,
    pub burst_distance_auto: f64,
    pub duplicate_distance_auto: f64,
    pub duplicate_distance_min: f64,
    pub duplicate_distance_span: f64,
    pub phash_max: f64,
    pub duplicate_time_window_multiplier: i64,
    pub duplicate_time_window_min_seconds: i64,
    /// Join frames that are close in time and carry *different* exposure
    /// compensation, without requiring pHash or embedding agreement.
    ///
    /// Without this, a bracket never reaches [`crate::sets`] to be recognised,
    /// because the grouper cannot see that the frames belong together: the
    /// embedding is absent on the fast cull path, and the pHash of a frame two
    /// stops brighter is *near-complemented* — measured at 61 of 64 bits on an
    /// otherwise identical scene, because clipping flips which DCT coefficients
    /// sit above the median. Both of the grouper's similarity signals fail on
    /// exactly the input bracket protection exists to catch.
    ///
    /// The replacement evidence is deliberately narrow: two frames from the
    /// same camera, inside the *burst* window (not the wider duplicate window),
    /// with a deliberate exposure step between them. A group formed this way is
    /// not assumed to be a bracket — [`crate::sets::detect_intentional_set`]
    /// still has to confirm the full pattern, and if it declines the frames are
    /// ranked as an ordinary burst, which for two frames a second apart of the
    /// same scene is the right answer anyway.
    pub bracket_edges: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RankingConfig {
    pub face_group_weight_technical: f64,
    pub face_group_weight_face: f64,
    pub face_group_weight_aesthetic: f64,
    pub face_group_blink_penalty_weight: f64,
    pub face_group_occlusion_penalty_weight: f64,
    pub face_missing_technical_weight: f64,
    pub face_missing_penalty: f64,
    pub no_face_group_weight_aesthetic: f64,
    pub reason_blur_threshold: f64,
    pub reason_exposure_threshold: f64,
    pub reason_low_aesthetic_threshold: f64,
    pub reason_occlusion_threshold: f64,
    pub reason_sharpest_delta: f64,
    pub reason_best_face_delta: f64,
    pub reason_weak_face_delta: f64,
    pub reason_eyes_open_delta: f64,
    pub reason_possible_blink_threshold: f64,
    pub reject_score_delta: f64,
    pub reject_exposure_threshold: f64,
    pub reject_face_score_threshold: f64,
    pub reject_blink_penalty_threshold: f64,
    pub reject_occlusion_threshold: f64,
    /// How much of the effective sharpness comes from the sharpest *tile*
    /// rather than the frame-wide Laplacian variance.
    ///
    /// Frame-wide variance asks "is this frame busy?", which punishes exactly
    /// the frames shallow depth of field is supposed to produce: a sharp
    /// subject on smooth bokeh scores below a mediocre f/8 frame of foliage.
    /// Genres that live at wide apertures and long focal lengths lean harder on
    /// the peak. Photos indexed before `cull_sharpness_peak` existed have no
    /// peak stored and fall back to the global number, so this knob is inert
    /// for them rather than wrong.
    pub sharpness_peak_weight: f64,
    /// Re-derive `cull_technical_score` from the stored sub-scores at rank
    /// time instead of reading the stored composite.
    ///
    /// The stored composite is a snapshot taken at index time under the
    /// *default* config — deliberately so, since a catalog must not end up
    /// bound to whichever preset happened to be active during import. But that
    /// also means nothing computed later can reach it: neither a preset's
    /// `technical_weight_*` (which the indexing code's own comment claims are
    /// re-applied here, and were not) nor [`Self::sharpness_peak_weight`].
    /// Re-deriving makes both live. Falls back to the stored value when the
    /// sub-scores are missing.
    pub recompute_technical_score: bool,
    /// How much of `exposure` is scored *relative to the group* rather than
    /// against an absolute target. 0 restores the absolute-only behaviour.
    ///
    /// The absolute target is mean luminance 0.5, which marks every frame of a
    /// deliberately low-key or high-key set as badly exposed and then cannot
    /// tell them apart. The question that matters during culling is
    /// comparative: which of *these* frames is best exposed.
    ///
    /// Noise was originally to be normalized the same way and is deliberately
    /// not — see the note at its call site in
    /// [`crate::grouping::rank_group_records`]. In short: the estimator is a
    /// high-pass residual that already ranks a sharp frame dirtier than a
    /// blurred one, and rescaling to the group's range amplifies that error
    /// rather than correcting it.
    pub relative_normalization_weight: f64,
    /// Minimum spread between the best and worst value in a group before
    /// relative normalisation engages. Below this the group agrees, and
    /// stretching noise to fill 0..1 would manufacture a ranking out of it.
    pub relative_min_spread: f64,
    /// How much of the technical score a frame loses for highlights blown out
    /// beyond the least clipped frame of its group, at
    /// [`Self::clip_penalty_scale`] and more. 0 leaves clipping out.
    ///
    /// The exposure term asks how close the mean is to mid-grey, which a frame
    /// a stop and a third brighter can answer better than the original while
    /// 14 % of it is blown white. On the public benchmark that frame took the
    /// pick in 21 of 120 series. Lightroom lifts a stop of shadow without a
    /// trace and recovers nothing from a clipped highlight, so clipping is
    /// what costs, and only against a sibling: a high-key photograph clipped
    /// on purpose is clipped in every frame of its burst.
    pub clip_penalty_weight: f64,
    /// The excess of highlight clipping over the group's least clipped frame
    /// (a share of the frame) at which the penalty is full, and from which the
    /// frame is labelled `highlights_clipped`.
    pub clip_penalty_scale: f64,
    /// A frame whose grain at full working size (`cull_noise_fine`) is at
    /// least this many times a sibling's of the same frame-wide sharpness is a
    /// *noisy twin*. 0 switches the rule off.
    ///
    /// Grain reads as detail in every focus measure taken at full size: on the
    /// public benchmark a copy at about ISO 12 800 measured face sharpness
    /// +0.19 and eye focus +0.06 to +0.09 above its clean original, won the
    /// series 58 times in 120, and made the original "eyes out of focus"
    /// beside it. Comparing grain across the whole group instead rewards blur,
    /// which removes grain too, so only frames of equal sharpness are
    /// compared. A noisy twin loses [`Self::noise_twin_penalty`], is named
    /// `noisier_than_sibling`, and does not set the group's best focus. The
    /// penalty orders it below its twin but never makes it a reject candidate
    /// on its own.
    pub noise_twin_ratio: f64,
    /// How close two frames' frame-wide sharpness (`cull_sharpness`, at 512
    /// px, where grain barely moves it) must be to compare their grain. Grain
    /// moved it by 0.005–0.02 on the public benchmark; a pan and the same frame
    /// with its background blurred instead sat 0.023 apart, and at 0.03 the
    /// pan was the "noisier twin" of a keeper.
    pub noise_twin_sharpness_delta: f64,
    /// What a noisy twin loses from its score.
    pub noise_twin_penalty: f64,
    /// How much of the aesthetic signal comes from the AestheticSigLIP score
    /// stored with the photo (`aesthetic_score`, scaled to `0..1` by the API
    /// layer as `cull_aesthetic_model`) rather than the contrast/colourfulness
    /// heuristic. Only applies to photos that have a score from the current
    /// export; otherwise the heuristic carries the full weight.
    ///
    /// High because the heuristic is the weaker judge: it floors muted and
    /// low-key work and cannot tell composition from contrast. It is not 1.0 so
    /// that a frame the model is unsure about is still moved by what the pixels
    /// say about exposure and colour. **Chosen, not tuned**: the fixtures under
    /// `testdata/cull_eval` are synthetic and carry no model scores, so no
    /// number here has been fitted to real picks.
    pub aesthetic_model_weight: f64,
    /// Which zero-shot semantic axis this genre is judged on, if any:
    /// `"action"`, `"expression"` or `"candid"`. `None` disables the pass.
    ///
    /// A string rather than an enum because the prompt sets live in `lrg-ml`,
    /// which this crate cannot depend on. The API layer resolves the name and
    /// logs an unknown one rather than failing the request.
    ///
    /// **One axis per preset, never averaged with another.** Blending two
    /// dilutes both. The one exception is [`Self::emotion_prompt_set`], and it
    /// is not a blend: it is combined by max, so either question being answered
    /// "yes" makes the moment, and neither can water the other down. Its
    /// prompt vectors are embedded once per model and kept in the database
    /// like every other set's, so the cost is a second set of dot products per
    /// photo.
    pub semantic_prompt_set: Option<&'static str>,
    /// How much that axis moves the rank, as a convex blend over the quality
    /// score. Zero for presets with no axis.
    ///
    /// It exists because the technical signals cannot express *what the picture
    /// is of*. In a faceless action burst the score is ~46% sharpness, 32%
    /// exposure, 14% low-noise, 7% aesthetics — and exposure and noise barely
    /// move within one burst, so the winner is whichever frame carries the most
    /// high-frequency detail. That is crowd texture and limb spread, not
    /// whether the ball is at the player's foot. The same argument applies to a
    /// wedding, where the deciding factor is the moment, and to a portrait,
    /// where it is the expression.
    ///
    /// Applies only where a SigLIP2 embedding exists. A bare `tasks=cull`
    /// ingest has none; the plugin adds `embeddings` to it for every preset
    /// that judges the moment. Values are **unvalidated** — score them on a
    /// real fixture before trusting them.
    ///
    /// Despite the name, this weights the frame's whole *moment*: the stronger
    /// of this axis and [`Self::emotion_prompt_set`]. Where
    /// [`Self::usable_gate`] is on, the moment may lead — the gate, not a small
    /// weight, is what keeps a ruined frame from being promoted — so `sports`
    /// and `event` weight it above the quality score. The name stays because
    /// `/cull/grade` accepts it as an override and the plugin and fixtures
    /// already send it.
    pub semantic_weight: f64,
    /// A second zero-shot question for the moment, about *emotion*:
    /// `"sport_emotion"` or `"event_emotion"`. `None` asks nothing.
    ///
    /// Asked separately from [`Self::semantic_prompt_set`] rather than mixed
    /// into it, and combined by **max**, not by average. The goal celebration
    /// has no ball in it and the frame of the shot has nobody cheering yet;
    /// averaging the two questions would score both as half a moment. Either
    /// one happening is the moment.
    pub emotion_prompt_set: Option<&'static str>,
    /// Above this score on the "eyes closed on purpose" gate — a kiss, a laugh
    /// that squeezes the eyes shut — closed eyes stop counting as a blink: no
    /// blink penalty in the score, no blink-based reject. 0 switches the gate
    /// off and skips its prompt pass.
    ///
    /// Exists because the blink proxy sees closed eyes and nothing else, so the
    /// kiss is the wedding frame it most reliably nominates for deletion. Only
    /// the stored `cull_blink_penalty` is lifted; the eye-openness share inside
    /// the stored `cull_face_score` was baked in at index time and stays.
    pub eyes_closed_intent_threshold: f64,
    /// Rank every usable frame above every frame with a clear defect (blurred,
    /// eyes shut, face obstructed) whenever the group is judged on its moment.
    /// Within each half the score decides as usual. Exposure is deliberately
    /// not a defect here: it is judged relative to the group, and Lightroom
    /// lifts a stop without a trace.
    ///
    /// This is "first usable, then the moment" — how a photographer culls. It
    /// is what lets [`Self::semantic_weight`] lead: without it a strong moment
    /// could lift a frame nobody can use above a clean one. Only engages when
    /// every frame of the group carries a moment score; without one the
    /// ranking is exactly what it was.
    pub usable_gate: bool,
    /// Let a noticeably blurred frame be a reject candidate even when nothing
    /// better exists to compare it with: a single frame, or the best of a burst
    /// that is blurred throughout.
    ///
    /// Before this, every single landed in Picks unconditionally, and so did
    /// the top frame of any group however unusable. Only blur counts here,
    /// deliberately: exposure is scored against a mid-grey target that marks a
    /// correctly dark concert frame as underexposed once there is no group to
    /// judge it against, and the blink proxy is too weak to reject a frame
    /// without a better sibling.
    pub reject_blurred_without_alternative: bool,
    /// Above this gradient-direction coherence, a soft frame is reported as
    /// `motion_blur` rather than `blurred`. Purely a labelling threshold — the
    /// score does not read it, because directional *content* raises the same
    /// measure.
    pub reason_motion_anisotropy_threshold: f64,
    /// How far a reject candidate's moment must beat the pick's before it is
    /// reported as `strong_moment_but_unusable`. Measured live on SigLIP2: a
    /// blurred copy of the same frame scored up to 0.09 *higher* on the action
    /// question than the sharp original, so without a margin every blurred
    /// near-copy would claim the stronger moment. A real difference — the
    /// celebration against the empty frame — is several times this.
    pub reason_moment_margin: f64,
    /// Judge the face-proxy defects — blink, occlusion, weak face score —
    /// against the group as well as against their absolute thresholds.
    ///
    /// Measured on a real 797-frame baptism: the eye-openness proxy read
    /// 0.07–0.26 on every frame (its patch is clamped to 8 px and the faces
    /// are small), so `blink_penalty` sat above the Event reject threshold on
    /// 767 of 769 frames with faces, occlusion above its threshold on 515, and
    /// `cull_face_score` below its threshold on nearly all. Every frame that
    /// was not a group's pick was a "clear defect", and 667 of 797 frames were
    /// nominated for deletion at the *gentle* strength. The proxies cannot
    /// tell an open eye from a closed one on a small face, but they can still
    /// tell a frame from its siblings — so a frame is only a blink when its
    /// eyes are visibly less open than the best frame of the group, only
    /// occluded when it is visibly more obstructed than the clearest, and only
    /// weak-faced when it is visibly weaker than the strongest.
    pub relative_defect_rules: bool,
    /// How far below the group's best (eye openness, face score) or above its
    /// best (occlusion) a frame must sit for the proxy to count as a defect
    /// under [`Self::relative_defect_rules`]. Also the margin for blur: a
    /// frame with siblings is a blur defect only when clearly softer than the
    /// sharpest of them, since one burst's frames sit within 0.01–0.10 of each
    /// other and an absolute threshold alone rejects all of them or none.
    pub relative_defect_margin: f64,
    /// The occlusion proxy's own margin. It moves by 0.1–0.35 between frames
    /// of one group of people (head turns, side light; median spread 0.11
    /// measured on a real shoot), roughly half its whole range, so the
    /// shared margin fired in most multi-face groups. 0.20, the candidate
    /// measured on that shoot, since the public benchmark found the same: at
    /// 0.10 five of seven sport originals it nominated for deletion were
    /// "occluded" against a copy of themselves a stop brighter.
    pub relative_occlusion_margin: f64,
    /// Judge blur against the group too (soft *and* clearly softer than the
    /// sharpest sibling). On since the public benchmark
    /// (`testdata/cull_bench`), which agreed with the real baptism shoot it
    /// was first fitted to: reject precision 82.3 % → 82.8 %, must-keep frames
    /// lost 25.4 % → 24.6 %, recall unchanged. Still to be confirmed against a
    /// hand cull across genres.
    pub relative_blur_rule: bool,
    /// How much of the face term in a face group's score comes from the eye
    /// focus (`cull_eye_focus`, scored against the group like exposure) rather
    /// than the stored composite `cull_face_score`. 0 switches eye focus out
    /// of the score.
    ///
    /// The composite's own sharpness share is a Laplacian variance over the
    /// whole face box on the ≤2048 px working copy, which cannot tell a frame
    /// focused on the eyes from one focused on the nose or the hair. Eye focus
    /// is measured on the eyes at full resolution (`lrg_imaging::focus`).
    /// Applies only when every frame of the group with a face has been
    /// measured, all or nothing like the moment, so a burst is never compared
    /// on two different questions.
    pub eye_focus_weight: f64,
    /// Below this eye focus a frame is labelled `eyes_out_of_focus` — when it
    /// has siblings and, under [`Self::relative_defect_rules`], is also
    /// clearly behind the sharpest eyes of its group. 0 switches the label and
    /// the defect off.
    ///
    /// Thresholds are on `1 - blur_effect`. On the reference portrait
    /// (`testdata/astronaut.jpg`) the sharp eye reads 0.69, a Gaussian blur of
    /// sigma 1 0.60, sigma 1.5 0.53, sigma 2 0.47 and sigma 3 0.39. The
    /// absolute level moves with how many pixels the eye has, which is why the
    /// group comparison carries the weight. **Hypotheses until a hand-culled
    /// fixture says otherwise.**
    pub reason_eye_focus_threshold: f64,
    /// Below this eye focus a frame with siblings that is also clearly behind
    /// the group's sharpest eyes is a clear defect — one no edit repairs — and
    /// a reject candidate. Never for a single or the only measurable frame:
    /// without a sibling there is no evidence the shot could have been sharper.
    pub reject_eye_focus_threshold: f64,
    /// The same pair for the whole face (`cull_face_focus`), used only for a
    /// frame whose eyes could not be measured — too small, turned away. On the
    /// reference portrait the face reads 0.55 sharp, 0.43 at sigma 1, 0.36 at
    /// sigma 1.5 and 0.30 at sigma 2.
    pub reason_face_focus_threshold: f64,
    pub reject_face_focus_threshold: f64,
    /// How far behind the group's best eye (or face) focus a frame must be for
    /// the relative focus rules — about the difference between a sharp eye
    /// and a sigma-1 blur of it on the reference portrait.
    pub relative_focus_margin: f64,
    /// A pick within this of the group's best eye focus is reported as having
    /// the sharpest eyes of the group (`eyes_sharpest_in_group`).
    pub reason_eye_focus_delta: f64,
    /// With the face state model (`cull_eye_closed`, MediaPipe's `eyeBlink` of
    /// the more closed eye), above this a frame is labelled `eyes_closed`;
    /// above [`Self::reject_eye_closed_threshold`] it is a blink defect. They
    /// replace `reason_possible_blink_threshold`/`reject_blink_penalty_threshold`,
    /// which were tuned against a proxy that read "closed" on almost every
    /// small face; the relative rules still apply on top. On real photographs
    /// judged by hand, eyes shut read 0.54–0.78 and no open eye reached 0.35.
    /// **Hypotheses until a hand-culled fixture says otherwise.**
    pub reason_eye_closed_threshold: f64,
    pub reject_eye_closed_threshold: f64,
    /// From here up to the closed threshold the eyes read half closed — a
    /// squint, a mid-blink — and the frame is labelled `eyes_half_closed`.
    /// Never a defect. 0 switches the label off.
    pub reason_eyes_half_closed_threshold: f64,
    /// `cull_eye_squint` at or above this gets the same label, for eyes
    /// narrowed against the sun with the lids reading open. Half of all faces
    /// read above 0.43, so it has to sit far up.
    pub reason_eye_squint_threshold: f64,
    /// A smile at least this broad with the eyes shut is a laugh, not a blink
    /// (per face, next to the scene-level CLIP gate). 0 switches it off.
    pub laugh_smile_threshold: f64,
    /// How much of the face term the smile (`cull_smile`, the weakest smile of
    /// the prominent faces) takes, scored against the group. 0 leaves the
    /// smile out of the score.
    pub smile_weight: f64,
    /// A pick within this of the group's broadest smile is reported as having
    /// the best smile (`best_smile`).
    pub reason_smile_delta: f64,
    /// Above this many degrees from the lens (`cull_gaze_off_lens`) a frame is
    /// labelled `looking_away`. 0 switches the label off; it is never a defect.
    pub reason_gaze_off_lens_degrees: f64,
    /// `cull_mouth_open` from here up, on a frame with a sibling whose mouth is
    /// [`Self::reason_mouth_open_delta`] less open, is labelled `mouth_open`.
    /// Measured: 10% of photographs of people speaking reach 0.2, and 0.6% of
    /// portraits. 0 switches the label off; it is never a defect.
    pub reason_mouth_open_threshold: f64,
    pub reason_mouth_open_delta: f64,
    /// A frame whose darkest face is this many stops below the same reading in
    /// its group's brightest frame is labelled `face_darker`. Only within a
    /// group, since skin ranges over several stops between people and this
    /// must never call dark skin underexposed. 0 switches the label off.
    pub reason_face_darker_stops: f64,
    /// `cull_face_blown` from here up is labelled `face_blown`. 0 switches it
    /// off. Measured: 2.6% of faces pass 0.05, where surgical masks and white
    /// headbands still sit, and from 0.15 what is left is blown skin.
    pub reason_face_blown_threshold: f64,
    /// `cull_face_across_stops` from here up, on a frame with a sibling whose
    /// light is [`Self::reason_face_hard_light_delta`] stops softer, is labelled
    /// `face_hard_light`. 10% of faces reach 2 stops, nearly all real side
    /// light. 0 switches the label off.
    pub reason_face_hard_light_stops: f64,
    pub reason_face_hard_light_delta: f64,
    /// How much of the sharpness in the score comes from focus on the subject
    /// (`cull_subject_focus`, scored against the group like eye focus) rather
    /// than from the frame-wide and tiled readings. 0 leaves it out of the
    /// score.
    ///
    /// The frame-wide reading asks whether the frame is busy and the tiled one
    /// whether *anything* is sharp; neither knows what the photo is of. On the
    /// public benchmark a frame whose focus landed on the background read as
    /// sharp as the original. Applies only when every frame of the group went
    /// through the subject pass and has a reading, all or nothing like the
    /// moment.
    ///
    /// 0.75 by default: on the public benchmark pair order rose from 76.6 % at
    /// 0 to 82.9 %, and at 1.0 a landscape's "subject" (a rock, a boat) took
    /// over and cost reject precision. Sports and wildlife take all of it.
    pub subject_focus_weight: f64,
    /// At or above this, the subject counts as in focus: a soft frame-wide
    /// reading is then a shallow depth of field or a pan, not a blur defect,
    /// as sharp eyes already settle it for faces. Below it, with a sibling
    /// whose subject is clearly sharper, the frame is labelled
    /// `subject_out_of_focus` (or `focus_on_background`). 0 switches subject
    /// focus out of the labels and defects.
    pub reason_subject_focus_threshold: f64,
    /// Below this, and [`Self::relative_subject_margin`] behind the group's
    /// sharpest subject, the frame is a clear defect and a reject candidate.
    /// Never for a single: without a sibling there is no evidence the subject
    /// could have been sharper.
    pub reject_subject_focus_threshold: f64,
    /// How far behind the group's sharpest subject a frame must be for the
    /// subject label and defect.
    pub relative_subject_margin: f64,
    /// A soft subject whose background reads at least this much sharper is
    /// labelled `focus_on_background`: the focus did not miss, it landed
    /// behind.
    pub reason_background_sharper_margin: f64,
    /// A frame with its subject in focus and a background streaked at least
    /// this much along one axis is labelled `panning`. 0 switches it off.
    pub reason_panning_streak: f64,
}

/// Which deliberately-captured multi-frame set a group is, if any.
///
/// These are not culls. A bracket, a focus stack and a panorama are all *one
/// picture* spread across several exposures, and nominating a winner while
/// marking the rest reject candidates destroys the shot. Detection exists to
/// suppress ranking's usual output, not to rank them better.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentionalSet {
    /// Exposure-bracketed frames, for HDR merge or exposure choice.
    Bracket,
    /// Same framing and exposure, focus walking through the scene.
    FocusStack,
    /// A sweep of overlapping frames to be stitched.
    Panorama,
}

impl IntentionalSet {
    pub fn as_str(self) -> &'static str {
        match self {
            IntentionalSet::Bracket => "bracket",
            IntentionalSet::FocusStack => "focus_stack",
            IntentionalSet::Panorama => "panorama",
        }
    }
}

/// Thresholds for [`crate::sets::detect_intentional_set`].
#[derive(Debug, Clone, Copy)]
pub struct SetDetectionConfig {
    /// Off entirely when false.
    pub enabled: bool,
    /// Fewest frames that can form any intentional set. Two-frame "brackets"
    /// are indistinguishable from a burst where the photographer rode the
    /// exposure compensation dial, so three is the floor.
    pub min_frames: usize,
    /// Total exposure-compensation span, in EV, required to call a group a
    /// bracket. Below this the variation is drift, not an AEB sequence.
    pub bracket_min_ev_span: f64,
    /// Largest gap between consecutive frames of a bracket, in seconds.
    ///
    /// AEB fires as a burst. Three frames that happen to sit a stop apart
    /// across a minute of shooting are three photographs, not a sequence.
    pub bracket_max_frame_gap_seconds: f64,
    /// Largest EV difference still counted as "the same exposure" when looking
    /// for a focus stack or a panorama.
    ///
    /// Also the smallest difference that makes two frames *candidates* for the
    /// same bracket during grouping — see `GroupingConfig::bracket_edges`.
    pub uniform_ev_tolerance: f64,
    /// A focus stack needs the in-focus region to actually travel: this is the
    /// minimum spread, in normalized frame widths, of `cull_sharp_region_*`
    /// across the group.
    pub focus_stack_min_region_travel: f64,
    /// ...and every frame sharp somewhere: its sharpest part at least this
    /// share of the sharpest frame's. A frame sharp nowhere is a miss, not a
    /// slice. 0 switches the check off.
    pub focus_stack_min_peak_ratio: f64,
    /// ...while the framing stays put. Maximum pHash Hamming distance between
    /// any two frames of a focus stack or bracket.
    pub static_framing_max_phash: u32,
    /// A panorama's consecutive frames overlap but are not duplicates: every
    /// adjacent embedding distance must sit above this.
    pub panorama_min_adjacent_distance: f64,
    /// ...and below this, or the frames are unrelated.
    pub panorama_max_adjacent_distance: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct CullingConfig {
    pub grouping: GroupingConfig,
    pub image_metrics: ImageMetricsConfig,
    pub face_metrics: FaceMetricsConfig,
    pub ranking: RankingConfig,
    pub sets: SetDetectionConfig,
}

impl RankingConfig {
    /// Whether this configuration reads eye or face focus at all — in the
    /// score, a label or a reject. `street` and the Python-parity config do
    /// not, so photos without the measurement cost them nothing.
    pub fn judges_focus(&self) -> bool {
        self.eye_focus_weight > 0.0
            || self.reason_eye_focus_threshold > 0.0
            || self.reject_eye_focus_threshold > 0.0
            || self.reason_face_focus_threshold > 0.0
            || self.reject_face_focus_threshold > 0.0
    }
}

impl CullingConfig {
    /// The configuration as it was when this was a straight port of the Python
    /// backend: every signal added since is switched off.
    ///
    /// This exists for the golden test in `tests/grouping_goldens.rs`, which
    /// pins `cull_score` against output captured from the real Python
    /// implementation before it was deleted. That output cannot be regenerated,
    /// so the choice is between never improving ranking and keeping one
    /// explicit switch that says exactly which knobs diverge. Everything listed
    /// here is a deliberate improvement, not a bug fix:
    ///
    /// - `sharpness_peak_weight` and `recompute_technical_score` — frame-wide
    ///   variance punishes shallow depth of field, and the stored composite
    ///   cannot see the fix (outstanding item 1)
    /// - `relative_normalization_weight` — absolute exposure/noise targets are
    ///   the wrong question inside a group (item 5)
    /// - `aesthetic_model_weight` — the contrast/colourfulness heuristic floors
    ///   muted and low-key work (item 4)
    /// - `sets.enabled` and `grouping.bracket_edges` — brackets and stacks were
    ///   being culled, and could not even be grouped (item 2)
    /// - `usable_gate` and `reject_blurred_without_alternative` — a strong
    ///   moment could otherwise lift an unusable frame, and a blurred single or
    ///   the best of a blurred burst was always a pick
    /// - `relative_defect_rules` — the face proxies fire on nearly every real
    ///   frame, so absolute thresholds rejected 84% of a real shoot
    /// - eye and face focus — the composite face score cannot tell focus on
    ///   the eyes from focus on the nose (outstanding item 3)
    pub fn python_parity() -> Self {
        let mut cfg = BASE;
        cfg.ranking.sharpness_peak_weight = 0.0;
        cfg.ranking.recompute_technical_score = false;
        cfg.ranking.relative_normalization_weight = 0.0;
        cfg.ranking.aesthetic_model_weight = 0.0;
        cfg.sets.enabled = false;
        cfg.grouping.bracket_edges = false;
        cfg.ranking.usable_gate = false;
        cfg.ranking.reject_blurred_without_alternative = false;
        cfg.ranking.relative_defect_rules = false;
        cfg.ranking.relative_blur_rule = false;
        cfg.ranking.clip_penalty_weight = 0.0;
        cfg.ranking.noise_twin_ratio = 0.0;
        cfg.ranking.eye_focus_weight = 0.0;
        cfg.ranking.reason_eye_focus_threshold = 0.0;
        cfg.ranking.reject_eye_focus_threshold = 0.0;
        cfg.ranking.reason_face_focus_threshold = 0.0;
        cfg.ranking.reject_face_focus_threshold = 0.0;
        cfg.ranking.smile_weight = 0.0;
        cfg.ranking.laugh_smile_threshold = 0.0;
        cfg.ranking.reason_eyes_half_closed_threshold = 0.0;
        cfg.ranking.reason_gaze_off_lens_degrees = 0.0;
        cfg.ranking.reason_mouth_open_threshold = 0.0;
        cfg.ranking.reason_face_darker_stops = 0.0;
        cfg.ranking.reason_face_blown_threshold = 0.0;
        cfg.ranking.reason_face_hard_light_stops = 0.0;
        cfg.ranking.subject_focus_weight = 0.0;
        cfg.ranking.reason_subject_focus_threshold = 0.0;
        cfg.ranking.reject_subject_focus_threshold = 0.0;
        cfg.ranking.reason_panning_streak = 0.0;
        cfg
    }
}

const BASE: CullingConfig = CullingConfig {
    grouping: GroupingConfig {
        time_window_default_seconds: 1,
        phash_hamming_auto: 10.0,
        burst_distance_auto: 0.12,
        duplicate_distance_auto: 0.05,
        duplicate_distance_min: 0.02,
        duplicate_distance_span: 0.06,
        phash_max: 64.0,
        duplicate_time_window_multiplier: 4,
        duplicate_time_window_min_seconds: 10,
        bracket_edges: true,
    },
    image_metrics: ImageMetricsConfig::defaults(),
    face_metrics: FaceMetricsConfig::defaults(),
    ranking: RankingConfig {
        face_group_weight_technical: 0.55,
        face_group_weight_face: 0.45,
        face_group_weight_aesthetic: 0.10,
        face_group_blink_penalty_weight: 0.10,
        face_group_occlusion_penalty_weight: 0.08,
        face_missing_technical_weight: 0.70,
        face_missing_penalty: 0.20,
        no_face_group_weight_aesthetic: 0.08,
        reason_blur_threshold: 0.20,
        reason_exposure_threshold: 0.35,
        reason_low_aesthetic_threshold: 0.35,
        reason_occlusion_threshold: 0.55,
        reason_sharpest_delta: 0.02,
        reason_best_face_delta: 0.03,
        reason_weak_face_delta: 0.10,
        reason_eyes_open_delta: 0.05,
        reason_possible_blink_threshold: 0.55,
        reject_score_delta: 0.18,
        reject_exposure_threshold: 0.28,
        reject_face_score_threshold: 0.30,
        reject_blink_penalty_threshold: 0.75,
        reject_occlusion_threshold: 0.75,
        sharpness_peak_weight: 0.35,
        recompute_technical_score: true,
        relative_normalization_weight: 0.5,
        relative_min_spread: 0.03,
        clip_penalty_weight: 0.2,
        clip_penalty_scale: 0.05,
        noise_twin_ratio: 1.6,
        noise_twin_sharpness_delta: 0.02,
        noise_twin_penalty: 0.15,
        aesthetic_model_weight: 0.9,
        // No axis by default: "default" spans every genre, and there is no
        // question that is right for all of them.
        semantic_prompt_set: None,
        semantic_weight: 0.0,
        emotion_prompt_set: None,
        eyes_closed_intent_threshold: 0.0,
        usable_gate: true,
        reject_blurred_without_alternative: true,
        reason_motion_anisotropy_threshold: 0.55,
        reason_moment_margin: 0.15,
        relative_defect_rules: true,
        // 0.14 since the public benchmark: the face state model's lids read
        // up to 0.1 apart between a frame and the same frame a stop brighter,
        // so at 0.10 originals were "blinks" against their own copies (reject
        // precision 82.3 % → 84.6 %, must-keep lost 25.4 % → 20.4 %, recall
        // 70.0 % → 69.7 %).
        relative_defect_margin: 0.14,
        relative_occlusion_margin: 0.20,
        relative_blur_rule: true,
        eye_focus_weight: 0.35,
        reason_eye_focus_threshold: 0.52,
        reject_eye_focus_threshold: 0.45,
        reason_face_focus_threshold: 0.36,
        reject_face_focus_threshold: 0.28,
        relative_focus_margin: 0.08,
        reason_eye_focus_delta: 0.02,
        reason_eye_closed_threshold: 0.5,
        reject_eye_closed_threshold: 0.55,
        reason_eyes_half_closed_threshold: 0.35,
        reason_eye_squint_threshold: 0.78,
        laugh_smile_threshold: 0.6,
        smile_weight: 0.0,
        reason_smile_delta: 0.05,
        reason_gaze_off_lens_degrees: 0.0,
        reason_mouth_open_threshold: 0.0,
        reason_mouth_open_delta: 0.15,
        reason_face_darker_stops: 0.75,
        reason_face_blown_threshold: 0.15,
        reason_face_hard_light_stops: 2.0,
        reason_face_hard_light_delta: 1.0,
        subject_focus_weight: 0.75,
        reason_subject_focus_threshold: 0.5,
        reject_subject_focus_threshold: 0.38,
        relative_subject_margin: 0.08,
        reason_background_sharper_margin: 0.05,
        reason_panning_streak: 0.2,
    },
    sets: SetDetectionConfig {
        enabled: true,
        min_frames: 3,
        bracket_min_ev_span: 1.0,
        bracket_max_frame_gap_seconds: 4.0,
        uniform_ev_tolerance: 0.34,
        focus_stack_min_region_travel: 0.12,
        focus_stack_min_peak_ratio: 0.35,
        static_framing_max_phash: 12,
        panorama_min_adjacent_distance: 0.06,
        panorama_max_adjacent_distance: 0.35,
    },
};

pub fn get_culling_config(preset: &str) -> CullingConfig {
    let mut cfg = BASE;
    match preset.trim().to_lowercase().as_str() {
        "portrait" => {
            cfg.ranking.face_group_weight_technical = 0.34;
            cfg.ranking.face_group_weight_face = 0.66;
            cfg.ranking.face_group_weight_aesthetic = 0.18;
            cfg.ranking.face_group_blink_penalty_weight = 0.20;
            cfg.ranking.face_group_occlusion_penalty_weight = 0.18;
            cfg.ranking.reason_possible_blink_threshold = 0.40;
            cfg.ranking.reason_occlusion_threshold = 0.45;
            cfg.ranking.reason_low_aesthetic_threshold = 0.42;
            cfg.ranking.reject_blink_penalty_threshold = 0.55;
            cfg.ranking.reject_face_score_threshold = 0.35;
            cfg.ranking.reject_occlusion_threshold = 0.55;
            // Portraits are shot wide open on purpose. The frame-wide number
            // measures the background falling out of focus, which is the
            // intent, not a fault.
            cfg.ranking.sharpness_peak_weight = 0.55;
            // A portrait is decided by micro-expression, and `cull_face_score`
            // is sharpness, size, position and a crude eye-openness proxy —
            // none of which can tell a warm smile from a rictus. Weighted
            // lower than sports because here the technical signals *are*
            // meaningful (the eye plane genuinely decides), so this breaks
            // ties rather than leading.
            cfg.ranking.semantic_prompt_set = Some("expression");
            cfg.ranking.semantic_weight = 0.20;
            // A couple's portrait has a kiss in it, and a good laugh shuts the
            // eyes; neither is a blink.
            cfg.ranking.eyes_closed_intent_threshold = 0.7;
            // The eye plane is what a portrait is judged on, at 100 %: eye
            // focus carries more of the face term and is held to a higher bar.
            cfg.ranking.eye_focus_weight = 0.55;
            cfg.ranking.reason_eye_focus_threshold = 0.55;
            cfg.ranking.reject_eye_focus_threshold = 0.47;
            // A portrait is the face: a half blink already spoils it, the
            // smile per face is worth scoring, and eye contact is a question
            // worth answering.
            cfg.ranking.reason_eye_closed_threshold = 0.45;
            cfg.ranking.reject_eye_closed_threshold = 0.5;
            cfg.ranking.smile_weight = 0.15;
            cfg.ranking.reason_gaze_off_lens_degrees = 25.0;
            cfg.ranking.reason_mouth_open_threshold = 0.2;
        }
        "street" => {
            cfg.ranking.face_group_weight_technical = 0.70;
            cfg.ranking.face_group_weight_face = 0.30;
            cfg.ranking.face_group_weight_aesthetic = 0.14;
            cfg.ranking.face_group_blink_penalty_weight = 0.06;
            cfg.ranking.face_group_occlusion_penalty_weight = 0.04;
            cfg.ranking.reason_possible_blink_threshold = 0.65;
            cfg.ranking.reject_blink_penalty_threshold = 0.85;
            cfg.ranking.reject_score_delta = 0.22;
            // Street work is often deep-focus and zone-focused, so the
            // frame-wide number is closer to the right question here than
            // anywhere else.
            cfg.ranking.sharpness_peak_weight = 0.25;
            // The highest weight of any preset, because street is the one genre
            // where the analysis is explicit that grain and motion blur are
            // legitimate and moment beats technique. Ranking the frames by how
            // tidy they are is close to the opposite of the job.
            cfg.ranking.semantic_prompt_set = Some("candid");
            cfg.ranking.semantic_weight = 0.32;
            // Moment over technique, and grain and motion blur are part of
            // the look: focus on the eyes is not what a street frame is
            // judged on, so it neither ranks nor rejects here.
            cfg.ranking.eye_focus_weight = 0.0;
            cfg.ranking.reason_eye_focus_threshold = 0.0;
            cfg.ranking.reject_eye_focus_threshold = 0.0;
            cfg.ranking.reason_face_focus_threshold = 0.0;
            cfg.ranking.reject_face_focus_threshold = 0.0;
            // Closed eyes are often the moment in street work.
            cfg.ranking.reason_eye_closed_threshold = 0.62;
            cfg.ranking.reject_eye_closed_threshold = 0.72;
            cfg.ranking.reason_face_darker_stops = 0.0;
            cfg.ranking.reason_face_blown_threshold = 0.0;
            cfg.ranking.reason_face_hard_light_stops = 0.0;
        }
        "event" => {
            cfg.grouping.time_window_default_seconds = 2;
            cfg.grouping.burst_distance_auto = 0.14;
            cfg.ranking.face_group_weight_technical = 0.48;
            cfg.ranking.face_group_weight_face = 0.52;
            cfg.ranking.face_group_weight_aesthetic = 0.14;
            cfg.ranking.face_group_blink_penalty_weight = 0.14;
            cfg.ranking.face_group_occlusion_penalty_weight = 0.10;
            cfg.ranking.reason_possible_blink_threshold = 0.50;
            cfg.ranking.reason_occlusion_threshold = 0.50;
            cfg.ranking.reason_low_aesthetic_threshold = 0.38;
            cfg.ranking.reject_blink_penalty_threshold = 0.62;
            cfg.ranking.reject_face_score_threshold = 0.33;
            cfg.ranking.reject_occlusion_threshold = 0.62;
            cfg.ranking.reject_score_delta = 0.20;
            cfg.ranking.sharpness_peak_weight = 0.40;
            // Receptions and ceremonies run dark by design; an absolute
            // mid-grey exposure target reads every frame as underexposed.
            cfg.ranking.relative_normalization_weight = 0.65;
            // The kiss, the ring, the reaction — the genre table gives "the
            // moment" as what decides an event pick, and at an event the
            // moment is mostly emotion: laughter, happy tears, a hug. The
            // moment leads among usable frames (`usable_gate` keeps a ruined
            // one out of the lead), below sports' weight because the face
            // signals carry real information here too.
            cfg.ranking.semantic_prompt_set = Some("candid");
            cfg.ranking.emotion_prompt_set = Some("event_emotion");
            cfg.ranking.semantic_weight = 0.45;
            // The kiss is the frame the blink proxy is surest about, and wrong.
            cfg.ranking.eyes_closed_intent_threshold = 0.7;
            // Everyone smiling is half of a group picture.
            cfg.ranking.smile_weight = 0.10;
            cfg.ranking.reason_mouth_open_threshold = 0.2;
        }
        "sports" => {
            cfg.grouping.time_window_default_seconds = 3;
            cfg.grouping.burst_distance_auto = 0.16;
            cfg.ranking.face_group_weight_technical = 0.75;
            cfg.ranking.face_group_weight_face = 0.25;
            cfg.ranking.face_group_weight_aesthetic = 0.10;
            cfg.ranking.face_group_blink_penalty_weight = 0.04;
            cfg.ranking.face_group_occlusion_penalty_weight = 0.04;
            cfg.ranking.reason_blur_threshold = 0.15;
            cfg.ranking.reject_score_delta = 0.24;
            cfg.ranking.reason_possible_blink_threshold = 0.75;
            cfg.ranking.reject_blink_penalty_threshold = 0.92;
            // Long lenses wide open: almost the entire frame is out of focus
            // by design, and the only question worth asking is whether the
            // subject is sharp.
            cfg.ranking.sharpness_peak_weight = 0.70;
            // High ISO across the board — judge noise against the other frames
            // from the same burst, not against a fixed ceiling.
            cfg.ranking.relative_normalization_weight = 0.65;
            // The athlete is what has to be sharp; with the subject measured,
            // the frame-wide reading adds nothing a pan does not spoil. On the
            // public benchmark 1.0 put 80 % of sport pairs in order, 0.75 79 %
            // and 0.5 77 %.
            cfg.ranking.subject_focus_weight = 1.0;
            // The whole point of the preset. Sharpness alone cannot tell the
            // frame with the ball from the frame just after it, and inside one
            // burst it is the only signal with any range left. Emotion is the
            // other half of a sports moment — the celebration, the despair —
            // and intensity counts, not whether it is happy. The moment leads
            // among usable frames; technique decides near-ties.
            cfg.ranking.semantic_prompt_set = Some("action");
            cfg.ranking.emotion_prompt_set = Some("sport_emotion");
            cfg.ranking.semantic_weight = 0.55;
            // Motion softens faces a little on frames that are still the
            // keeper, and what has to be sharp is the athlete as a whole: eye
            // focus counts for less and rejects only a clear miss.
            cfg.ranking.eye_focus_weight = 0.20;
            cfg.ranking.reason_eye_focus_threshold = 0.48;
            cfg.ranking.reject_eye_focus_threshold = 0.40;
            cfg.ranking.reason_face_focus_threshold = 0.32;
            cfg.ranking.reject_face_focus_threshold = 0.25;
            // Athletes squint and grimace; only shut eyes count.
            cfg.ranking.reason_eye_closed_threshold = 0.6;
            cfg.ranking.reject_eye_closed_threshold = 0.68;
            cfg.ranking.reason_eyes_half_closed_threshold = 0.0;
            cfg.ranking.reason_face_darker_stops = 0.0;
            cfg.ranking.reason_face_blown_threshold = 0.0;
            cfg.ranking.reason_face_hard_light_stops = 0.0;
        }
        "wildlife" => {
            // Long lenses wide open, like sports: the background is meant to
            // be soft, and the frame-wide number measures exactly that. The
            // subject is what has to be sharp, so it carries most of the
            // sharpness, and the pan of a bird in flight is a technique, not a
            // blur.
            cfg.grouping.time_window_default_seconds = 2;
            cfg.ranking.sharpness_peak_weight = 0.70;
            // All of it: on the public benchmark 1.0 nominated no keeper and
            // only rejects (12 of 12), where 0.75 lost 2 of 48 pans and soft
            // backgrounds.
            cfg.ranking.subject_focus_weight = 1.0;
            cfg.ranking.relative_normalization_weight = 0.65;
            // Behaviour is the moment: in flight, feeding, looking back. Below
            // sports' weight because the prompts are a first version and the
            // animal's eye, which often decides, is not judged yet.
            cfg.ranking.semantic_prompt_set = Some("wildlife");
            cfg.ranking.semantic_weight = 0.25;
            // People in a wildlife frame are rarely the subject.
            cfg.ranking.reason_face_darker_stops = 0.0;
            cfg.ranking.reason_face_blown_threshold = 0.0;
            cfg.ranking.reason_face_hard_light_stops = 0.0;
        }
        _ => {}
    }
    cfg
}

pub fn available_presets() -> Vec<&'static str> {
    vec![
        AUTO_PRESET,
        "default",
        "event",
        "portrait",
        "sports",
        "street",
        "wildlife",
    ]
}

/// Not a preset of its own: each group is ranked with the preset its photos'
/// genre calls for ([`preset_for_genres`]). On its own — for grouping, or where
/// no photo has a genre — it is `default`.
pub const AUTO_PRESET: &str = "auto";

/// The preset a photo genre (`lrg_ml::genre`'s ids) calls for. A concert is
/// people and a moment under stage light, like an event; everything without
/// people or a moment to judge — landscape, architecture, food, macro, the
/// sky — is the default.
///
/// A "street" genre is ranked as an event, not with the `street` preset: that
/// one leaves focus unjudged on purpose, which is the photographer's call to
/// make, not a guess's. The genre model calls many photos of people outdoors
/// "street" — on the public benchmark 13 of 24 portraits and 12 of 24 events —
/// and the `street` preset then found 63 % of the rejects where `event` found
/// 69 %, with as many keepers kept.
pub fn preset_for_genre(genre: &str) -> &'static str {
    match genre {
        "portrait" => "portrait",
        "event" | "concert" | "street" => "event",
        "sport" => "sports",
        "wildlife" => "wildlife",
        _ => "default",
    }
}

/// The preset most of these photos' genres call for, `None` when none has a
/// genre. A tie goes to the preset that asks more of the photos (event, then
/// portrait, sports, wildlife, default): a wedding burst split between
/// "event" and "portrait" still has its moment judged.
pub fn preset_for_genres<'a>(genres: impl IntoIterator<Item = &'a str>) -> Option<&'static str> {
    const ORDER: [&str; 5] = ["event", "portrait", "sports", "wildlife", "default"];
    let mut counts = [0usize; ORDER.len()];
    for genre in genres {
        let preset = preset_for_genre(genre);
        if let Some(i) = ORDER.iter().position(|p| *p == preset) {
            counts[i] += 1;
        }
    }
    let most = *counts.iter().max()?;
    (most > 0).then(|| ORDER[counts.iter().position(|&c| c == most).unwrap_or(0)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_genre_majority_chooses_and_a_tie_asks_more() {
        assert_eq!(preset_for_genres([]), None);
        assert_eq!(
            preset_for_genres(["sport", "sport", "landscape"]),
            Some("sports")
        );
        // A burst of a speech, half read as a portrait: the moment still counts.
        assert_eq!(
            preset_for_genres(["portrait", "event", "concert", "portrait"]),
            Some("event")
        );
        assert_eq!(preset_for_genres(["food", "other"]), Some("default"));
        // A guess of "street" does not switch focus off.
        assert_eq!(preset_for_genres(["street"]), Some("event"));
        // Every preset a genre can call for is one the API accepts.
        for genre in [
            "portrait",
            "event",
            "concert",
            "sport",
            "wildlife",
            "street",
            "landscape",
            "other",
        ] {
            assert!(available_presets().contains(&preset_for_genre(genre)));
        }
    }
}
