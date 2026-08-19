//! Detection of deliberately-captured multi-frame sets: exposure brackets,
//! focus stacks and panoramas.
//!
//! Culling's whole premise is that a group of similar frames contains one
//! keeper and some near-misses. For these three kinds of group that premise is
//! false — the frames *are* the picture, and every one of them is needed. The
//! grouper happily collects a five-frame AEB sequence (identical framing,
//! seconds apart, identical pHash) into one group, nominates whichever frame
//! landed nearest mid-grey as the winner, and marks the other four as reject
//! candidates. For landscape, architecture and real-estate work that is the
//! single most damaging thing the feature does.
//!
//! So this module does not try to rank these sets better. It identifies them so
//! ranking can be suppressed: no reject candidates, and a group type the plugin
//! can route to a "keep all" collection.
//!
//! Detection is deliberately conservative in both directions. Calling a burst a
//! bracket costs the user rejects they wanted; calling a bracket a burst
//! destroys a shot. The second error is worse, but a detector that fires on
//! ordinary bursts would make the feature useless, so every rule below needs
//! corroborating evidence from more than one signal.

use crate::culling_config::{IntentionalSet, SetDetectionConfig};

/// One frame's worth of the evidence set detection reads.
///
/// Everything here is optional because it comes from three different places
/// with three different availability stories: `exposure_bias` needs a plugin
/// new enough to send it, `sharp_region` needs a re-index for
/// `cull_sharp_region_*`, and `embedding` needs a full Analyze & Index rather
/// than the fast cull pass. A detector that demanded all three would never
/// fire; one that guessed from none would fire constantly. Each rule below
/// names exactly which fields it requires and declines when they are missing.
#[derive(Debug, Clone, Default)]
pub struct SetFrame<'a> {
    pub capture_time: Option<f64>,
    /// Exposure compensation in EV, from the plugin's `exposureBias`.
    pub exposure_bias: Option<f64>,
    pub phash: Option<u64>,
    /// L2-normalizable embedding; `None` for cull-only-indexed photos.
    pub embedding: Option<&'a [f32]>,
    /// Centroid of the in-focus region, `cull_sharp_region_{x,y}`.
    pub sharp_region: Option<(f64, f64)>,
}

fn cosine_distance(a: &[f32], b: &[f32]) -> Option<f64> {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
    let na: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let nb: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return None;
    }
    Some(1.0 - (dot / (na * nb)).clamp(-1.0, 1.0))
}

/// True when every pair of frames hashes close enough to be the same framing.
///
/// Requires *every* frame to carry a pHash: a single missing hash means the
/// tripod claim is unverified, and both bracket and focus-stack detection lean
/// on it as their only evidence that the camera did not move.
fn framing_is_static(frames: &[SetFrame], cfg: &SetDetectionConfig) -> bool {
    let hashes: Vec<u64> = frames.iter().filter_map(|f| f.phash).collect();
    if hashes.len() != frames.len() {
        return false;
    }
    hashes.iter().enumerate().all(|(i, a)| {
        hashes[i + 1..]
            .iter()
            .all(|b| (a ^ b).count_ones() <= cfg.static_framing_max_phash)
    })
}

/// Exposure compensation for every frame, or `None` if any frame lacks it.
fn all_exposure_bias(frames: &[SetFrame]) -> Option<Vec<f64>> {
    let values: Vec<f64> = frames.iter().filter_map(|f| f.exposure_bias).collect();
    (values.len() == frames.len()).then_some(values)
}

/// An AEB sequence: three or more evenly-spaced exposure stops, spanning at
/// least a full EV, shot back to back.
///
/// **This deliberately does not check framing, and that is the interesting
/// part.** The obvious tripod test — do all the frames hash alike — is
/// unusable here, because changing the exposure is precisely what breaks a
/// pHash. Measured on a real +2 EV frame of an otherwise identical scene, the
/// Hamming distance to the base frame was **61 of 64 bits**: pushing the
/// histogram into the highlights flips which DCT coefficients sit above the
/// median, so the hash comes back near-complemented. A hash gate at any
/// sensible threshold rejects every bracket it is supposed to confirm, and the
/// first version of this function did exactly that.
///
/// What is left is strong enough on its own. Three or more *evenly spaced*
/// compensation values spanning a full stop, all within a couple of seconds, is
/// a pattern only auto exposure bracketing produces. The photographer riding
/// the compensation dial through a burst — the false positive this needs to
/// reject — lands on uneven steps and repeats values, which the even-spacing
/// test catches without needing to know where the camera was pointing.
fn is_bracket(frames: &[SetFrame], cfg: &SetDetectionConfig) -> bool {
    let Some(bias) = all_exposure_bias(frames) else {
        return false;
    };
    let min = bias.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = bias.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if max - min < cfg.bracket_min_ev_span {
        return false;
    }

    // Distinct stops at 1/3-EV resolution, the finest step any camera brackets
    // at. Working in thirds keeps this integer arithmetic, so "evenly spaced"
    // is an exact test rather than a float comparison.
    let mut steps: Vec<i64> = bias
        .iter()
        .map(|v| (v * 3.0).round_ties_even() as i64)
        .collect();
    steps.sort_unstable();
    steps.dedup();
    if steps.len() < 3 {
        return false;
    }
    let stride = steps[1] - steps[0];
    if !steps.windows(2).all(|w| w[1] - w[0] == stride) {
        return false;
    }

    // Shot as one action. A bracket fires in a burst; three frames that happen
    // to sit a stop apart across a minute of shooting are three photographs.
    // Unknown capture times are not disqualifying — the exposure pattern has
    // already done the work — but a known, wide gap is.
    let times: Vec<f64> = frames.iter().filter_map(|f| f.capture_time).collect();
    if times.len() == frames.len() {
        let mut sorted = times;
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if sorted
            .windows(2)
            .any(|w| w[1] - w[0] > cfg.bracket_max_frame_gap_seconds)
        {
            return false;
        }
    }
    true
}

/// A focus stack: identical framing, one exposure, and the in-focus region
/// visibly travelling across the frame.
///
/// The travel requirement is the whole detector. Without it this is
/// indistinguishable from a tripod burst, which is a group culling *should*
/// rank. It needs `cull_sharp_region_*`, so a catalog indexed before those
/// existed simply never matches — the frames stay a burst, which is the old
/// behaviour rather than a new failure.
fn is_focus_stack(frames: &[SetFrame], cfg: &SetDetectionConfig) -> bool {
    let regions: Vec<(f64, f64)> = frames.iter().filter_map(|f| f.sharp_region).collect();
    if regions.len() != frames.len() {
        return false;
    }
    // Exposure must be uniform where it is known at all; a bracket that also
    // happens to shift focus is a bracket.
    if let Some(bias) = all_exposure_bias(frames) {
        let min = bias.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = bias.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if max - min > cfg.uniform_ev_tolerance {
            return false;
        }
    }
    if !framing_is_static(frames, cfg) {
        return false;
    }
    let span = |axis: fn(&(f64, f64)) -> f64| -> f64 {
        let min = regions.iter().map(axis).fold(f64::INFINITY, f64::min);
        let max = regions.iter().map(axis).fold(f64::NEG_INFINITY, f64::max);
        max - min
    };
    let travel = span(|r| r.0).hypot(span(|r| r.1));
    travel >= cfg.focus_stack_min_region_travel
}

/// A panorama sweep: uniform exposure, and consecutive frames that overlap
/// without repeating.
///
/// The shape being matched is a *sweep*: each frame is a bit further from the
/// first than the one before it, while each adjacent pair still overlaps. A
/// burst has adjacent distances near zero and no drift; a set of unrelated
/// frames never reaches the grouper at all. Requiring monotone drift is what
/// keeps a tripod burst with a slowly changing subject from matching.
fn is_panorama(frames: &[SetFrame], cfg: &SetDetectionConfig) -> bool {
    let embeddings: Vec<&[f32]> = frames.iter().filter_map(|f| f.embedding).collect();
    if embeddings.len() != frames.len() {
        return false;
    }
    if let Some(bias) = all_exposure_bias(frames) {
        let min = bias.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = bias.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if max - min > cfg.uniform_ev_tolerance {
            return false;
        }
    }
    // Identical pHashes mean the camera did not move, which rules a sweep out.
    if framing_is_static(frames, cfg) {
        return false;
    }
    for pair in embeddings.windows(2) {
        let Some(d) = cosine_distance(pair[0], pair[1]) else {
            return false;
        };
        if d < cfg.panorama_min_adjacent_distance || d > cfg.panorama_max_adjacent_distance {
            return false;
        }
    }
    // Monotone drift away from the first frame.
    let mut previous = 0.0f64;
    for e in embeddings.iter().skip(1) {
        let Some(d) = cosine_distance(embeddings[0], e) else {
            return false;
        };
        if d <= previous {
            return false;
        }
        previous = d;
    }
    true
}

/// Classifies a group, or returns `None` for an ordinary burst / near-duplicate
/// set that culling should rank normally.
///
/// `frames` must be in capture order — the panorama rule reads adjacency, and
/// the grouper already hands over records sorted by capture time.
pub fn detect_intentional_set(
    frames: &[SetFrame],
    cfg: &SetDetectionConfig,
) -> Option<IntentionalSet> {
    if !cfg.enabled || frames.len() < cfg.min_frames.max(2) {
        return None;
    }
    // Order matters: a bracket that also drifts in focus is still a bracket,
    // and both are checked before the panorama rule, which is the loosest.
    if is_bracket(frames, cfg) {
        return Some(IntentionalSet::Bracket);
    }
    if is_focus_stack(frames, cfg) {
        return Some(IntentionalSet::FocusStack);
    }
    if is_panorama(frames, cfg) {
        return Some(IntentionalSet::Panorama);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SetDetectionConfig {
        crate::culling_config::get_culling_config("default").sets
    }

    fn frame(bias: Option<f64>, phash: u64, region: (f64, f64)) -> SetFrame<'static> {
        SetFrame {
            capture_time: Some(0.0),
            exposure_bias: bias,
            phash: Some(phash),
            embedding: None,
            sharp_region: Some(region),
        }
    }

    /// Three frames a second apart at the given compensation values.
    fn bracket_frames(values: &[f64]) -> Vec<SetFrame<'static>> {
        values
            .iter()
            .enumerate()
            .map(|(i, &ev)| SetFrame {
                capture_time: Some(1000.0 + i as f64),
                exposure_bias: Some(ev),
                // Deliberately wildly different hashes: a real bracket's frames
                // hash nothing alike, which is the whole reason framing is not
                // checked here.
                phash: Some(0xffff_ffff_0000_0000u64.rotate_left(i as u32 * 17)),
                embedding: None,
                sharp_region: None,
            })
            .collect()
    }

    #[test]
    fn a_three_stop_aeb_sequence_is_a_bracket() {
        assert_eq!(
            detect_intentional_set(&bracket_frames(&[-2.0, 0.0, 2.0]), &cfg()),
            Some(IntentionalSet::Bracket)
        );
    }

    /// The finding that shaped this detector: a bracket's frames do *not* hash
    /// alike, because changing the exposure is what breaks a pHash. Measured at
    /// 61 of 64 bits between a base frame and the same scene at +2 EV. Any
    /// framing gate rejects every bracket it exists to confirm.
    #[test]
    fn wildly_different_hashes_do_not_prevent_bracket_detection() {
        let mut frames = bracket_frames(&[-2.0, 0.0, 2.0]);
        frames[0].phash = Some(0x0000_0000_0000_0000);
        frames[1].phash = Some(0xffff_ffff_ffff_ffff);
        frames[2].phash = Some(0x0f0f_0f0f_0f0f_0f0f);
        assert_eq!(
            detect_intentional_set(&frames, &cfg()),
            Some(IntentionalSet::Bracket)
        );
    }

    /// Even spacing is what replaced the framing check as the guard against
    /// false positives. Values a stop apart but *unevenly* spaced are a
    /// photographer working the dial, not an AEB sequence.
    #[test]
    fn unevenly_spaced_exposure_values_are_not_a_bracket() {
        assert_eq!(
            detect_intentional_set(&bracket_frames(&[-2.0, -1.7, 1.0]), &cfg()),
            None
        );
    }

    /// Five- and seven-frame brackets are ordinary on any modern body.
    #[test]
    fn a_five_frame_bracket_is_detected() {
        assert_eq!(
            detect_intentional_set(&bracket_frames(&[-2.0, -1.0, 0.0, 1.0, 2.0]), &cfg()),
            Some(IntentionalSet::Bracket)
        );
    }

    /// Frames a stop apart shot minutes apart are separate photographs. AEB
    /// fires as a burst, so a wide gap between frames rules it out.
    #[test]
    fn evenly_spaced_exposures_spread_over_minutes_are_not_a_bracket() {
        let mut frames = bracket_frames(&[-2.0, 0.0, 2.0]);
        frames[1].capture_time = Some(1120.0);
        frames[2].capture_time = Some(1240.0);
        assert_eq!(detect_intentional_set(&frames, &cfg()), None);
    }

    /// The failure that matters: an ordinary burst must keep being culled.
    #[test]
    fn a_plain_burst_is_not_a_set() {
        let frames = [
            frame(Some(0.0), 0xabcd_0000_0000_0000, (0.5, 0.5)),
            frame(Some(0.0), 0xabcd_0000_0000_0001, (0.5, 0.5)),
            frame(Some(0.0), 0xabcd_0000_0000_0003, (0.51, 0.5)),
            frame(Some(0.0), 0xabcd_0000_0000_0007, (0.5, 0.49)),
        ];
        assert_eq!(detect_intentional_set(&frames, &cfg()), None);
    }

    /// Riding the compensation dial across a burst lands on two values, not on
    /// three discrete stops, so it must not read as a bracket.
    #[test]
    fn a_two_value_exposure_drift_is_not_a_bracket() {
        let frames = [
            frame(Some(0.0), 0xabcd_0000_0000_0000, (0.5, 0.5)),
            frame(Some(0.0), 0xabcd_0000_0000_0000, (0.5, 0.5)),
            frame(Some(1.5), 0xabcd_0000_0000_0000, (0.5, 0.5)),
        ];
        assert_eq!(detect_intentional_set(&frames, &cfg()), None);
    }

    /// Same exposure, same framing, focus walking front-to-back.
    #[test]
    fn focus_walking_across_the_frame_is_a_stack() {
        let frames = [
            frame(Some(0.0), 0xabcd_0000_0000_0000, (0.2, 0.8)),
            frame(Some(0.0), 0xabcd_0000_0000_0000, (0.5, 0.5)),
            frame(Some(0.0), 0xabcd_0000_0000_0001, (0.8, 0.2)),
        ];
        assert_eq!(
            detect_intentional_set(&frames, &cfg()),
            Some(IntentionalSet::FocusStack)
        );
    }

    /// Without `cull_sharp_region_*` there is no evidence of travel, so an
    /// un-re-indexed catalog falls back to ordinary burst behaviour rather
    /// than guessing.
    #[test]
    fn a_stack_without_region_data_is_not_detected() {
        let mut frames = [
            frame(Some(0.0), 0xabcd_0000_0000_0000, (0.2, 0.8)),
            frame(Some(0.0), 0xabcd_0000_0000_0000, (0.5, 0.5)),
            frame(Some(0.0), 0xabcd_0000_0000_0001, (0.8, 0.2)),
        ];
        frames[1].sharp_region = None;
        assert_eq!(detect_intentional_set(&frames, &cfg()), None);
    }

    /// A focus stack still checks framing, and must: there the exposure is
    /// constant, so the pHash is meaningful, and without it the detector cannot
    /// tell a stack from a tripod burst.
    #[test]
    fn a_focus_stack_with_moving_framing_is_declined() {
        let frames = [
            frame(Some(0.0), 0x0000_0000_0000_0000, (0.2, 0.8)),
            frame(Some(0.0), 0xffff_ffff_0000_0000, (0.5, 0.5)),
            frame(Some(0.0), 0xffff_ffff_ffff_ffff, (0.8, 0.2)),
        ];
        assert_eq!(detect_intentional_set(&frames, &cfg()), None);
    }

    #[test]
    fn a_sweep_of_overlapping_frames_is_a_panorama() {
        // Vectors drifting steadily around a circle: adjacent frames overlap,
        // and each is further from the first than the last.
        let vectors: Vec<Vec<f32>> = (0..4)
            .map(|i| {
                let t = i as f32 * 0.45;
                vec![t.cos(), t.sin(), 0.0]
            })
            .collect();
        let frames: Vec<SetFrame> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| SetFrame {
                capture_time: Some(i as f64),
                exposure_bias: Some(0.0),
                phash: Some(0x0f0f_0f0f_0f0f_0f0f << i),
                embedding: Some(v.as_slice()),
                sharp_region: None,
            })
            .collect();
        assert_eq!(
            detect_intentional_set(&frames, &cfg()),
            Some(IntentionalSet::Panorama)
        );
    }

    /// Near-identical embeddings are a burst, not a sweep: the adjacent
    /// distances sit below the overlap floor.
    #[test]
    fn a_static_burst_is_not_a_panorama() {
        let vectors: Vec<Vec<f32>> = (0..4).map(|i| vec![1.0, i as f32 * 0.001, 0.0]).collect();
        let frames: Vec<SetFrame> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| SetFrame {
                capture_time: Some(i as f64),
                exposure_bias: Some(0.0),
                phash: Some(0xdead_beef_0000_0000 + i as u64),
                embedding: Some(v.as_slice()),
                sharp_region: None,
            })
            .collect();
        assert_eq!(detect_intentional_set(&frames, &cfg()), None);
    }

    #[test]
    fn two_frames_are_never_a_set() {
        assert_eq!(
            detect_intentional_set(&bracket_frames(&[-2.0, 2.0]), &cfg()),
            None
        );
    }

    #[test]
    fn detection_can_be_switched_off() {
        let mut c = cfg();
        c.enabled = false;
        assert_eq!(
            detect_intentional_set(&bracket_frames(&[-2.0, 0.0, 2.0]), &c),
            None
        );
    }
}
