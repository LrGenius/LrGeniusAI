//! Per-photo quality flags: the technical defects a photo can be found by in
//! Lightroom (the plug-in's *Quality flags* field), derived from the culling
//! metrics indexing already stores.
//!
//! Culling judges a frame against its burst. A flag has no burst to lean on: it
//! is written on a single photo and searched across the whole catalog, so every
//! flag here is an **absolute, per-photo** reading, and only signals that mean
//! the same thing on any photo qualify. That rules out four of the signals
//! culling reads:
//!
//! - `cull_exposure` is scored against a mid-grey target, which marks a
//!   correctly dark concert frame or a high-key portrait as badly exposed (see
//!   `RankingConfig::reject_blurred_without_alternative` for the same reasoning
//!   on singles). The clipping fractions are kept instead: those are genuinely
//!   absolute.
//! - The face proxies (`cull_eye_openness`, `cull_blink_penalty`,
//!   `cull_occlusion`) only discriminate *within* a group. Measured on a real
//!   shoot, the blink penalty sat above the reject threshold on 767 of 769
//!   frames with faces — see `RankingConfig::relative_defect_rules`.
//! - `cull_noise_flat` is measured on the 512 px working copy of whatever
//!   preview was sent, so the same grain reads about half as strong each time
//!   the preview's resolution doubles (Gaussian grain of σ = 8/255 read 0.59,
//!   0.17 and 0.09 from 512, 1024 and 2048 px previews). The preview size is a
//!   user setting, so a fixed threshold would flag the same photo or not
//!   depending on it.
//!
//! What is left, and how each is judged:
//!
//! | flag | when |
//! | --- | --- |
//! | `blurred` | effective sharpness below the blur reason threshold, gradients not dominated by one direction |
//! | `motion_blur` | the same softness, with the gradient coherence above the motion-blur reason threshold |
//! | `highlights_clipped` | at least [`QualityThresholds::highlight_clip`] of the frame at the clip point |
//! | `shadows_clipped` | at least [`QualityThresholds::shadow_clip`] of the frame at black |
//!
//! The blur rules are culling's own reason codes (`blurred` / `motion_blur` in
//! [`crate::grouping`]), at the default preset's thresholds — the same
//! preset indexing measures under. Only their absolute half is used: the
//! relative blur rule (`relative_blur_rule`) is a group judgement.
//!
//! Derived when read, not stored. Thresholds can move without re-indexing a
//! catalog, and a photo indexed before this module existed gets flags as soon
//! as it has the signals they read.

use serde_json::{json, Map, Value};

use crate::culling_config::get_culling_config;

/// One technical defect a photo can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QualityFlag {
    Blurred,
    MotionBlur,
    HighlightsClipped,
    ShadowsClipped,
}

impl QualityFlag {
    /// Every flag, in the order they are reported. The order is part of the
    /// contract: the plug-in compares the joined text with what the field
    /// already holds and only writes when it differs, so the same flags must
    /// always come out the same way round.
    pub const ALL: [QualityFlag; 4] = [
        QualityFlag::Blurred,
        QualityFlag::MotionBlur,
        QualityFlag::HighlightsClipped,
        QualityFlag::ShadowsClipped,
    ];

    /// Stable id, for clients that branch on a flag.
    pub fn id(self) -> &'static str {
        match self {
            QualityFlag::Blurred => "blurred",
            QualityFlag::MotionBlur => "motion_blur",
            QualityFlag::HighlightsClipped => "highlights_clipped",
            QualityFlag::ShadowsClipped => "shadows_clipped",
        }
    }

    /// What the Lightroom field shows, and what a user types into the Library
    /// filter to find it.
    pub fn label(self) -> &'static str {
        match self {
            QualityFlag::Blurred => "blurred",
            // "Camera shake" would claim more than the measurement knows: a
            // soft frame whose gradients all point one way is shake *or*
            // subject motion, which is also how the culling explanation puts
            // it.
            QualityFlag::MotionBlur => "motion blur",
            QualityFlag::HighlightsClipped => "highlights clipped",
            QualityFlag::ShadowsClipped => "shadows clipped",
        }
    }
}

/// The stored metrics every flag reads. A photo lacking any of them was
/// indexed before the signal existed, and gets no flags rather than flags
/// computed from part of the evidence: without `cull_sharpness_peak`, a sharp
/// subject on smooth bokeh reads as blurred.
pub const REQUIRED_SIGNALS: [&str; 5] = [
    "cull_sharpness",
    "cull_sharpness_peak",
    "cull_motion_anisotropy",
    "cull_highlight_clip",
    "cull_shadow_clip",
];

/// The thresholds behind each flag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityThresholds {
    /// Effective sharpness below which a frame is soft. Culling's
    /// `reason_blur_threshold`.
    pub blur: f64,
    /// Share of the effective sharpness taken from the sharpest tile rather
    /// than the whole frame. Culling's `sharpness_peak_weight`.
    pub sharpness_peak_weight: f64,
    /// Gradient coherence above which a soft frame is motion blur rather than
    /// missed focus. Culling's `reason_motion_anisotropy_threshold`.
    pub motion_anisotropy: f64,
    /// Share of the frame at luma ≥ `highlight_threshold` (0.98 by default) at
    /// which highlights count as clipped.
    ///
    /// **Chosen, not tuned**: no culling rule reads the clip fractions on their
    /// own, so there was no threshold to reuse. One twentieth of the frame is a
    /// blown area anyone would see — a white sky, a burnt-out window — while a
    /// sun disc or a few specular glints stay well below it.
    pub highlight_clip: f64,
    /// Share of the frame at luma ≤ `shadow_threshold` (0.02 by default) at
    /// which shadows count as clipped.
    ///
    /// **Chosen, not tuned**, and twice the highlight share on purpose: night,
    /// stage and silhouette work put large areas at black by intent, and a
    /// black background reads as one too. It is still a fact about the photo,
    /// so it is reported — just not for a sliver.
    pub shadow_clip: f64,
}

impl QualityThresholds {
    /// The default culling preset's blur thresholds, which are what indexing
    /// measures under, and the two clip shares.
    pub fn defaults() -> Self {
        let ranking = get_culling_config("default").ranking;
        QualityThresholds {
            blur: ranking.reason_blur_threshold,
            sharpness_peak_weight: ranking.sharpness_peak_weight.clamp(0.0, 1.0),
            motion_anisotropy: ranking.reason_motion_anisotropy_threshold,
            highlight_clip: 0.05,
            shadow_clip: 0.10,
        }
    }
}

impl Default for QualityThresholds {
    fn default() -> Self {
        Self::defaults()
    }
}

fn number(m: &Map<String, Value>, key: &str) -> Option<f64> {
    m.get(key).and_then(Value::as_f64).filter(|v| v.is_finite())
}

/// The photo's flags under the default thresholds, `None` when it lacks a
/// signal they need ([`REQUIRED_SIGNALS`]). An empty list is a photo that was
/// measured and has nothing to report.
pub fn quality_flags(m: &Map<String, Value>) -> Option<Vec<QualityFlag>> {
    quality_flags_with(m, &QualityThresholds::defaults())
}

/// [`quality_flags`] with explicit thresholds.
pub fn quality_flags_with(
    m: &Map<String, Value>,
    t: &QualityThresholds,
) -> Option<Vec<QualityFlag>> {
    let sharpness = number(m, "cull_sharpness")?;
    let peak = number(m, "cull_sharpness_peak")?;
    let anisotropy = number(m, "cull_motion_anisotropy")?;
    let highlight_clip = number(m, "cull_highlight_clip")?;
    let shadow_clip = number(m, "cull_shadow_clip")?;

    // The same blend ranking uses (`sharpness_effective`), so a photo the cull
    // calls "noticeably blurred" is the photo this flags.
    let w = t.sharpness_peak_weight;
    let effective = (1.0 - w) * sharpness.clamp(0.0, 1.0) + w * peak.clamp(0.0, 1.0);

    let mut flags = Vec::new();
    if effective < t.blur {
        // One diagnosis or the other, never both: the reason codes split a
        // soft frame the same way.
        flags.push(if anisotropy > t.motion_anisotropy {
            QualityFlag::MotionBlur
        } else {
            QualityFlag::Blurred
        });
    }
    if highlight_clip >= t.highlight_clip {
        flags.push(QualityFlag::HighlightsClipped);
    }
    if shadow_clip >= t.shadow_clip {
        flags.push(QualityFlag::ShadowsClipped);
    }
    flags.sort_by_key(|f| QualityFlag::ALL.iter().position(|a| a == f));
    Some(flags)
}

/// What the plug-in writes into the field: the labels, comma-separated, in
/// [`QualityFlag::ALL`] order. Empty for a photo with no flags.
pub fn flags_text(flags: &[QualityFlag]) -> String {
    flags
        .iter()
        .map(|f| f.label())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The `quality` block of a photo in API responses.
///
/// `{"measured": true, "flags": [ids], "text": "..."}` for a photo that has
/// the signals, `{"measured": false}` for one indexed before they existed. A
/// flag rather than `null` because the plug-in's JSON decoder turns `null`
/// into an absent key, and "this backend sends no block" (older than the
/// field) and "this photo was not measured" need different answers there.
pub fn quality_json(m: &Map<String, Value>) -> Value {
    match quality_flags(m) {
        Some(flags) => json!({
            "measured": true,
            "flags": flags.iter().map(|f| f.id()).collect::<Vec<_>>(),
            "text": flags_text(&flags),
        }),
        None => json!({"measured": false}),
    }
}

/// Whether the photo carries every signal the flags read.
pub fn has_required_signals(m: &Map<String, Value>) -> bool {
    REQUIRED_SIGNALS.iter().all(|k| number(m, k).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(sharpness: f64, peak: f64, anisotropy: f64, hi: f64, lo: f64) -> Map<String, Value> {
        let Value::Object(m) = json!({
            "cull_sharpness": sharpness,
            "cull_sharpness_peak": peak,
            "cull_motion_anisotropy": anisotropy,
            "cull_highlight_clip": hi,
            "cull_shadow_clip": lo,
        }) else {
            unreachable!()
        };
        m
    }

    #[test]
    fn a_clean_photo_has_no_flags_and_empty_text() {
        let flags = quality_flags(&photo(0.7, 0.8, 0.2, 0.0, 0.0)).unwrap();
        assert!(flags.is_empty());
        assert_eq!(flags_text(&flags), "");
        assert_eq!(
            quality_json(&photo(0.7, 0.8, 0.2, 0.0, 0.0)),
            json!({"measured": true, "flags": [], "text": ""})
        );
    }

    /// The defaults are culling's own reason thresholds, not a second copy of
    /// them that could drift.
    #[test]
    fn blur_thresholds_come_from_the_default_culling_preset() {
        let t = QualityThresholds::defaults();
        let ranking = get_culling_config("default").ranking;
        assert_eq!(t.blur, ranking.reason_blur_threshold);
        assert_eq!(t.sharpness_peak_weight, ranking.sharpness_peak_weight);
        assert_eq!(
            t.motion_anisotropy,
            ranking.reason_motion_anisotropy_threshold
        );
    }

    #[test]
    fn soft_everywhere_is_blurred_and_directional_softness_is_motion_blur() {
        let blurred = quality_flags(&photo(0.05, 0.08, 0.2, 0.0, 0.0)).unwrap();
        assert_eq!(blurred, vec![QualityFlag::Blurred]);
        let shaken = quality_flags(&photo(0.05, 0.08, 0.88, 0.0, 0.0)).unwrap();
        assert_eq!(shaken, vec![QualityFlag::MotionBlur]);
        assert_eq!(flags_text(&shaken), "motion blur");
    }

    /// Directional *content* — a facade, a horizon — raises the coherence on a
    /// sharp frame too. Only a soft frame is ever diagnosed with it.
    #[test]
    fn coherence_alone_never_flags_a_sharp_frame() {
        let flags = quality_flags(&photo(0.7, 0.9, 0.95, 0.0, 0.0)).unwrap();
        assert!(flags.is_empty());
    }

    /// The reason the peak tile is required: an f/1.4 portrait is soft almost
    /// everywhere and razor sharp on the eyes. Its frame-wide sharpness alone
    /// is under the blur threshold; the blend is not.
    #[test]
    fn a_sharp_subject_on_bokeh_is_not_blurred() {
        let flags = quality_flags(&photo(0.10, 0.92, 0.3, 0.0, 0.0)).unwrap();
        assert!(flags.is_empty(), "{flags:?}");
    }

    #[test]
    fn clipping_flags_follow_their_own_shares() {
        let t = QualityThresholds::defaults();
        // Just under each share: nothing.
        let under = photo(
            0.7,
            0.8,
            0.2,
            t.highlight_clip - 0.001,
            t.shadow_clip - 0.001,
        );
        assert!(quality_flags(&under).unwrap().is_empty());
        // At the share: flagged.
        let at = photo(0.7, 0.8, 0.2, t.highlight_clip, t.shadow_clip);
        assert_eq!(
            quality_flags(&at).unwrap(),
            vec![QualityFlag::HighlightsClipped, QualityFlag::ShadowsClipped]
        );
    }

    /// The −2 and +2 EV frames of the landscape fixture's bracket are clipped
    /// at one end each; the base frame is not.
    #[test]
    fn bracket_frames_from_the_culling_fixture_are_flagged_at_their_clipped_end() {
        let dark = quality_flags(&photo(0.72, 0.8, 0.2, 0.0, 0.3)).unwrap();
        assert_eq!(dark, vec![QualityFlag::ShadowsClipped]);
        let bright = quality_flags(&photo(0.72, 0.8, 0.2, 0.27, 0.0)).unwrap();
        assert_eq!(bright, vec![QualityFlag::HighlightsClipped]);
        let base = quality_flags(&photo(0.72, 0.8, 0.2, 0.0, 0.0)).unwrap();
        assert!(base.is_empty());
    }

    /// Several flags always come out in one order, so the joined text is
    /// stable and the plug-in can tell "unchanged" from "changed".
    #[test]
    fn several_flags_are_reported_in_a_fixed_order() {
        let flags = quality_flags(&photo(0.02, 0.03, 0.9, 0.4, 0.4)).unwrap();
        assert_eq!(
            flags_text(&flags),
            "motion blur, highlights clipped, shadows clipped"
        );
        assert_eq!(
            quality_json(&photo(0.02, 0.03, 0.9, 0.4, 0.4))["flags"],
            json!(["motion_blur", "highlights_clipped", "shadows_clipped"])
        );
    }

    /// A photo indexed before a signal existed gets no flags at all — `None`,
    /// not an empty list — and the API says it was not measured.
    #[test]
    fn a_photo_missing_any_signal_has_no_flags() {
        for missing in REQUIRED_SIGNALS {
            let mut m = photo(0.05, 0.05, 0.2, 0.5, 0.5);
            m.remove(missing);
            assert_eq!(quality_flags(&m), None, "without {missing}");
            assert!(!has_required_signals(&m));
            assert_eq!(quality_json(&m), json!({"measured": false}));
        }
        assert!(has_required_signals(&photo(0.5, 0.5, 0.5, 0.0, 0.0)));
    }

    #[test]
    fn a_non_numeric_signal_counts_as_missing() {
        let mut m = photo(0.05, 0.05, 0.2, 0.0, 0.0);
        m.insert("cull_sharpness".into(), json!("0.05"));
        assert_eq!(quality_flags(&m), None);
    }

    #[test]
    fn ids_and_labels_are_distinct_and_cover_every_flag() {
        let ids: std::collections::HashSet<_> = QualityFlag::ALL.iter().map(|f| f.id()).collect();
        let labels: std::collections::HashSet<_> =
            QualityFlag::ALL.iter().map(|f| f.label()).collect();
        assert_eq!(ids.len(), QualityFlag::ALL.len());
        assert_eq!(labels.len(), QualityFlag::ALL.len());
        // A label must not contain the separator, or the field could not be
        // split back into flags.
        assert!(labels.iter().all(|l| !l.contains(',')));
    }
}
