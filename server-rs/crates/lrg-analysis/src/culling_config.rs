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
    /// How much of the aesthetic signal comes from CLIP-IQA over the stored
    /// SigLIP2 embedding rather than the contrast/colourfulness heuristic.
    /// Only applies to photos that actually have an embedding *and* a
    /// reachable text tower; otherwise the heuristic carries the full weight.
    pub aesthetic_iqa_weight: f64,
    /// Which zero-shot semantic axis this genre is judged on, if any:
    /// `"action"`, `"expression"` or `"candid"`. `None` disables the pass.
    ///
    /// A string rather than an enum because the prompt sets live in `lrg-ml`,
    /// which this crate cannot depend on. The API layer resolves the name and
    /// logs an unknown one rather than failing the request.
    ///
    /// **One axis per preset, deliberately.** Blending two doubles the
    /// text-tower work, dilutes both, and — with no validated fixture to check
    /// against — is exactly the sort of plausible-sounding over-reach that put
    /// a contrast heuristic in charge of "aesthetics" for a year. If a genre
    /// genuinely needs two, that should be demonstrated on a fixture first.
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
    /// Applies only where a SigLIP2 embedding exists, so the fast `tasks=cull`
    /// ingest never sees it. Weighted rather than decisive: these models read
    /// coarse content and affect well and fine detail poorly, so the axis
    /// should be able to break a near-tie, not overrule a clear technical
    /// verdict. Values are conservative and **unvalidated** — score them on a
    /// real fixture before trusting them.
    pub semantic_weight: f64,
    /// Above this gradient-direction coherence, a soft frame is reported as
    /// `motion_blur` rather than `blurred`. Purely a labelling threshold — the
    /// score does not read it, because directional *content* raises the same
    /// measure.
    pub reason_motion_anisotropy_threshold: f64,
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
    /// - `aesthetic_iqa_weight` — the contrast/colourfulness heuristic floors
    ///   muted and low-key work (item 4)
    /// - `sets.enabled` and `grouping.bracket_edges` — brackets and stacks were
    ///   being culled, and could not even be grouped (item 2)
    pub fn python_parity() -> Self {
        let mut cfg = BASE;
        cfg.ranking.sharpness_peak_weight = 0.0;
        cfg.ranking.recompute_technical_score = false;
        cfg.ranking.relative_normalization_weight = 0.0;
        cfg.ranking.aesthetic_iqa_weight = 0.0;
        cfg.sets.enabled = false;
        cfg.grouping.bracket_edges = false;
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
        aesthetic_iqa_weight: 0.8,
        // No axis by default: "default" spans every genre, and there is no
        // question that is right for all of them.
        semantic_prompt_set: None,
        semantic_weight: 0.0,
        reason_motion_anisotropy_threshold: 0.55,
    },
    sets: SetDetectionConfig {
        enabled: true,
        min_frames: 3,
        bracket_min_ev_span: 1.0,
        bracket_max_frame_gap_seconds: 4.0,
        uniform_ev_tolerance: 0.34,
        focus_stack_min_region_travel: 0.12,
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
            // moment" as what decides an event pick. Below street's weight
            // because event work still has to be technically deliverable, and
            // the face signals are already carrying real information here.
            cfg.ranking.semantic_prompt_set = Some("candid");
            cfg.ranking.semantic_weight = 0.22;
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
            // The whole point of the preset. Sharpness alone cannot tell the
            // frame with the ball from the frame just after it, and inside one
            // burst it is the only signal with any range left.
            cfg.ranking.semantic_prompt_set = Some("action");
            cfg.ranking.semantic_weight = 0.30;
        }
        _ => {}
    }
    cfg
}

pub fn available_presets() -> Vec<&'static str> {
    vec!["default", "event", "portrait", "sports", "street"]
}
