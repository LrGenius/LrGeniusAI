//! What the *source* image is, before anything is decided about it.
//!
//! The edit path used to predict slider values directly from a learned style,
//! which meant it applied the answer to a photograph that no longer existed:
//! `+25 contrast` was correct for the training frame's histogram, and on a
//! harshly lit midday frame the same number is destructive. Slider values are a
//! function of the starting point, so the starting point has to be measured.
//!
//! Everything here is measured on the working image, and two limits follow from
//! that and are load-bearing when reading these numbers:
//!
//! - **The working image is 8-bit.** [`SceneState::dynamic_range_stops`] is
//!   therefore bounded at ~8 stops and reflects the *rendered* range, not the
//!   sensor's. That is the right quantity for deciding how much tonal room an
//!   edit has, and the wrong one for describing the raw file.
//! - **Highlight clipping here is clipping in the rendering.** Whether it is
//!   recoverable depends on raw headroom, which only the caller knows; see
//!   [`SceneState::highlight_clip`].

/// A luminance histogram over `0..=255`, the shared basis for every percentile
/// and occupancy figure below.
struct Histogram {
    bins: [u64; 256],
    total: u64,
}

impl Histogram {
    fn from_luma(luma: &[f32]) -> Self {
        let mut bins = [0u64; 256];
        for &v in luma {
            let idx = ((v * 255.0).round().clamp(0.0, 255.0)) as usize;
            bins[idx] += 1;
        }
        Histogram {
            bins,
            total: luma.len() as u64,
        }
    }

    /// Linear-interpolated percentile in `0..1`. `p` is a fraction (0.5 = P50).
    fn percentile(&self, p: f64) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        let target = p.clamp(0.0, 1.0) * self.total as f64;
        let mut cumulative = 0u64;
        for (idx, &count) in self.bins.iter().enumerate() {
            let next = cumulative + count;
            if next as f64 >= target && count > 0 {
                return idx as f64 / 255.0;
            }
            cumulative = next;
        }
        1.0
    }

    /// Fraction of pixels at or above `threshold` (normalized luminance).
    fn fraction_above(&self, threshold: f64) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        let start = ((threshold * 255.0).ceil().clamp(0.0, 255.0)) as usize;
        let count: u64 = self.bins[start..].iter().sum();
        count as f64 / self.total as f64
    }

    /// Fraction of pixels at or below `threshold` (normalized luminance).
    fn fraction_below(&self, threshold: f64) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        let end = ((threshold * 255.0).floor().clamp(0.0, 255.0)) as usize;
        let count: u64 = self.bins[..=end].iter().sum();
        count as f64 / self.total as f64
    }
}

/// Tunables for [`scene_state`].
///
/// The normalizers are the honest weak point of this module: they set where a
/// measurement lands on `0..1`, and they were chosen to give sane spreads on
/// synthetic inputs rather than fitted to real photographs. The *ordering* the
/// tests pin (hard light above soft light above flat texture) is the part that
/// is meant to be trusted; the absolute values should be recalibrated against
/// real fixtures before anything downstream treats them as thresholds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneStateConfig {
    /// Long edge the frame is reduced to for the coarse-scale pass. Small
    /// enough that texture averages away and only lighting structure survives.
    pub coarse_long_edge: usize,
    /// Fraction of the strongest coarse-scale gradients averaged together to
    /// stand for the frame's largest surviving luminance step. Small enough to
    /// ignore the bulk of a gently lit frame, large enough that a lone pixel
    /// cannot carry the measurement.
    pub hardness_top_fraction: f64,
    /// Luminance step between adjacent coarse pixels that reads as half-hard.
    /// A step of this size across ~1/64th of the frame is a visible shadow edge.
    pub hardness_half: f64,
    /// Luminance at or above which a pixel counts as a highlight.
    pub highlight_threshold: f64,
    /// Luminance at or below which a pixel counts as a shadow.
    pub shadow_threshold: f64,
    /// A pixel is specular when it is at least this bright...
    pub specular_luma_threshold: f64,
    /// ...and no more saturated than this.
    pub specular_saturation_max: f64,
    /// Luminance floor used when taking logs, so a fully black pixel cannot
    /// send the dynamic range to infinity. One 8-bit code value.
    pub luminance_floor: f64,
}

impl SceneStateConfig {
    pub const fn defaults() -> Self {
        SceneStateConfig {
            coarse_long_edge: 64,
            hardness_top_fraction: 0.02,
            hardness_half: 0.15,
            highlight_threshold: 0.94,
            shadow_threshold: 0.06,
            specular_luma_threshold: 0.95,
            specular_saturation_max: 0.15,
            luminance_floor: 1.0 / 255.0,
        }
    }
}

impl Default for SceneStateConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

/// The measured state of an unedited frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneState {
    pub luminance_p005: f64,
    pub luminance_p01: f64,
    pub luminance_p10: f64,
    pub luminance_p50: f64,
    pub luminance_p90: f64,
    pub luminance_p99: f64,
    pub luminance_p995: f64,

    /// `log2(P99.5) - log2(P0.5)`, in stops, over the *rendered* image.
    ///
    /// Capped near 8 by the 8-bit working image. This is the quantity that
    /// decides how much tonal room an edit has: a frame already spanning the
    /// full range cannot absorb added contrast, whatever a style profile wants.
    pub dynamic_range_stops: f64,

    /// Fraction of pixels rendered at or above the highlight threshold.
    ///
    /// This says the *rendering* is clipped, not that the data is gone. A raw
    /// file usually holds a stop or more above what its default rendering
    /// shows, so a caller that knows the source is raw should treat this as
    /// recoverable; for a JPEG source it is lost. The distinction cannot be
    /// made from pixels alone, so it deliberately is not made here.
    pub highlight_clip: f64,
    /// Fraction of pixels at or below the shadow threshold.
    pub shadow_clip: f64,

    /// How much of the frame the tails hold relative to the midtones:
    /// `(shadow_occupancy + highlight_occupancy) / midtone_occupancy`.
    ///
    /// High for a scene split into lit and unlit halves, which is what harsh
    /// light does to a histogram; low for an evenly lit one.
    pub midtone_deficit: f64,

    /// Mean gradient magnitude at the coarse scale, where only lighting
    /// structure survives.
    pub coarse_gradient_energy: f64,
    /// Mean gradient magnitude at the working scale, carrying both lighting and
    /// texture.
    pub fine_gradient_energy: f64,

    /// How hard the light is, on `0..1`.
    ///
    /// The largest luminance step that survives heavy downscaling, rather than
    /// anything derived from contrast — contrast cannot tell a scene's lighting
    /// from its subject matter. Averaging the frame down to ~64px erases
    /// texture, because averaging is precisely what removes it, while a shadow
    /// edge stays a full-amplitude step between neighbouring coarse pixels. So
    /// foliage, fabric and crowd texture read *low* here and a plain wall in
    /// direct sun reads high, which is the distinction a naive contrast measure
    /// gets backwards and the one that decides whether adding contrast improves
    /// a photograph or ruins it.
    ///
    /// Two refinements that are not optional, each having broken an earlier
    /// version of this measure:
    ///
    /// - It is the largest step, not the mean coarse gradient. Downscaling by k
    ///   multiplies the per-pixel gradient of *any* large-scale structure by k,
    ///   so a hard shadow edge and a gentle ramp across the frame produce the
    ///   same mean; only the distribution separates them.
    /// - That largest step is a trimmed maximum over the top couple of percent,
    ///   not a quantile. An edge crossing the whole frame is still only ~1.6% of
    ///   the coarse gradients, so the 95th percentile reads zero on the hardest
    ///   possible input.
    pub light_hardness: f64,

    /// Fraction of pixels that are bright and nearly colourless.
    ///
    /// Does not distinguish a specular highlight from a blown sky, and does not
    /// need to: both mean the same thing for the only decision that reads it,
    /// which is whether there is room to push the white point further.
    pub specular_fraction: f64,
}

impl SceneState {
    /// A neutral, mid-exposed reading, used when the frame is too small to
    /// measure. Deliberately unremarkable: every guardrail keyed off these
    /// values stays inactive rather than firing on a fabricated extreme.
    pub fn unknown() -> Self {
        SceneState {
            luminance_p005: 0.05,
            luminance_p01: 0.08,
            luminance_p10: 0.2,
            luminance_p50: 0.45,
            luminance_p90: 0.75,
            luminance_p99: 0.9,
            luminance_p995: 0.93,
            dynamic_range_stops: 4.0,
            highlight_clip: 0.0,
            shadow_clip: 0.0,
            midtone_deficit: 0.5,
            coarse_gradient_energy: 0.0,
            fine_gradient_energy: 0.0,
            light_hardness: 0.4,
            specular_fraction: 0.0,
        }
    }
}

/// Forward-difference gradient magnitudes over a luminance plane.
fn gradient_magnitudes(plane: &[f32], width: usize, height: usize) -> Vec<f64> {
    if width < 2 || height < 2 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity((width - 1) * (height - 1));
    for y in 0..height - 1 {
        let row = y * width;
        let next = row + width;
        for x in 0..width - 1 {
            let here = plane[row + x];
            let dx = (plane[row + x + 1] - here) as f64;
            let dy = (plane[next + x] - here) as f64;
            out.push((dx * dx + dy * dy).sqrt());
        }
    }
    out
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

/// Mean of the largest `fraction` of the values — a trimmed maximum.
///
/// A plain quantile is the wrong tool here. A shadow edge is *thin*: one
/// crossing the whole frame occupies about 1.6% of the coarse plane's
/// gradients, so even the 95th percentile still reads zero and the hardest
/// possible frame scores as flat. A plain maximum has the opposite failure,
/// riding on a single pixel. Averaging the top slice keeps a real edge visible
/// while a lone outlier is diluted.
fn top_fraction_mean(values: &[f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let take =
        ((sorted.len() as f64 * fraction.clamp(0.0, 1.0)).round() as usize).clamp(1, sorted.len());
    sorted[..take].iter().sum::<f64>() / take as f64
}

/// Box-average `plane` down to at most `target_long_edge` on its long side.
///
/// A plain box filter is the right choice rather than a nicer kernel: the point
/// of the coarse pass is that texture *averages away*, and averaging is exactly
/// what a box filter does. A sharpening-adjacent kernel would preserve some of
/// the detail this pass exists to discard.
fn box_downscale(
    plane: &[f32],
    width: usize,
    height: usize,
    target_long_edge: usize,
) -> (Vec<f32>, usize, usize) {
    let long = width.max(height);
    if long <= target_long_edge || target_long_edge == 0 {
        return (plane.to_vec(), width, height);
    }
    let scale = target_long_edge as f64 / long as f64;
    let out_w = ((width as f64 * scale).round() as usize).max(2);
    let out_h = ((height as f64 * scale).round() as usize).max(2);

    let mut out = vec![0.0f32; out_w * out_h];
    for oy in 0..out_h {
        let y0 = oy * height / out_h;
        let y1 = (((oy + 1) * height) / out_h).max(y0 + 1).min(height);
        for ox in 0..out_w {
            let x0 = ox * width / out_w;
            let x1 = (((ox + 1) * width) / out_w).max(x0 + 1).min(width);
            let mut sum = 0.0f64;
            let mut count = 0u32;
            for y in y0..y1 {
                let row = y * width;
                for x in x0..x1 {
                    sum += plane[row + x] as f64;
                    count += 1;
                }
            }
            out[oy * out_w + ox] = if count == 0 {
                0.0
            } else {
                (sum / count as f64) as f32
            };
        }
    }
    (out, out_w, out_h)
}

/// Measure an unedited frame. `pixels` is 8-bit interleaved RGB.
pub fn scene_state(
    pixels: &[u8],
    width: usize,
    height: usize,
    cfg: &SceneStateConfig,
) -> SceneState {
    let n = width * height;
    if n == 0 || pixels.len() < n * 3 || width < 4 || height < 4 {
        return SceneState::unknown();
    }

    let mut luma = vec![0.0f32; n];
    let mut specular = 0u64;
    for i in 0..n {
        let r = pixels[i * 3] as f32 / 255.0;
        let g = pixels[i * 3 + 1] as f32 / 255.0;
        let b = pixels[i * 3 + 2] as f32 / 255.0;
        let y = 0.299 * r + 0.587 * g + 0.114 * b;
        luma[i] = y;

        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        // HSV saturation; 0 for black, which cannot be specular anyway.
        let saturation = if max <= 0.0 { 0.0 } else { (max - min) / max };
        if y as f64 >= cfg.specular_luma_threshold
            && saturation as f64 <= cfg.specular_saturation_max
        {
            specular += 1;
        }
    }

    let hist = Histogram::from_luma(&luma);
    let p005 = hist.percentile(0.005);
    let p01 = hist.percentile(0.01);
    let p10 = hist.percentile(0.10);
    let p50 = hist.percentile(0.50);
    let p90 = hist.percentile(0.90);
    let p99 = hist.percentile(0.99);
    let p995 = hist.percentile(0.995);

    let floor = cfg.luminance_floor.max(f64::EPSILON);
    let dynamic_range_stops = (p995.max(floor) / p005.max(floor)).log2().max(0.0);

    let highlight_clip = hist.fraction_above(cfg.highlight_threshold);
    let shadow_clip = hist.fraction_below(cfg.shadow_threshold);
    let shadow_occupancy = hist.fraction_below(1.0 / 3.0);
    let highlight_occupancy = hist.fraction_above(2.0 / 3.0);
    let midtone_occupancy = (1.0 - shadow_occupancy - highlight_occupancy).max(1e-6);
    let midtone_deficit = (shadow_occupancy + highlight_occupancy) / midtone_occupancy;

    let fine_gradient_energy = mean(&gradient_magnitudes(&luma, width, height));
    let (coarse, cw, ch) = box_downscale(&luma, width, height, cfg.coarse_long_edge);
    let coarse_gradients = gradient_magnitudes(&coarse, cw, ch);
    let coarse_gradient_energy = mean(&coarse_gradients);

    // Hardness is the *largest luminance step that survives the coarse pass*,
    // not the average gradient there.
    //
    // The mean cannot answer this. Downscaling by k multiplies the per-pixel
    // gradient of any structure that is already smooth at the coarse scale by k,
    // so a hard shadow edge and a gentle ramp across the frame both come back at
    // roughly k times their fine-scale energy — measured on a step and a ramp,
    // both land at a ratio of ~2.0 under a 2x reduction. What separates them is
    // how that energy is *distributed*: harsh light concentrates it into a few
    // large steps, soft light spreads a little of it everywhere. A high quantile
    // reads the former and ignores the latter, and texture is gone from the
    // coarse plane either way because averaging is what removes it.
    let coarse_gradient_peak = top_fraction_mean(&coarse_gradients, cfg.hardness_top_fraction);
    let light_hardness =
        (coarse_gradient_peak / (coarse_gradient_peak + cfg.hardness_half)).clamp(0.0, 1.0);

    SceneState {
        luminance_p005: p005,
        luminance_p01: p01,
        luminance_p10: p10,
        luminance_p50: p50,
        luminance_p90: p90,
        luminance_p99: p99,
        luminance_p995: p995,
        dynamic_range_stops,
        highlight_clip,
        shadow_clip,
        midtone_deficit,
        coarse_gradient_energy,
        fine_gradient_energy,
        light_hardness,
        specular_fraction: specular as f64 / n as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 128;
    const H: usize = 128;

    fn gray_image(f: impl Fn(usize, usize) -> f32) -> Vec<u8> {
        let mut px = vec![0u8; W * H * 3];
        for y in 0..H {
            for x in 0..W {
                let v = (f(x, y).clamp(0.0, 1.0) * 255.0).round() as u8;
                let i = (y * W + x) * 3;
                px[i] = v;
                px[i + 1] = v;
                px[i + 2] = v;
            }
        }
        px
    }

    fn measure(px: &[u8]) -> SceneState {
        scene_state(px, W, H, &SceneStateConfig::defaults())
    }

    /// Direct sun: the frame splits into a lit half and a shadowed half with a
    /// sharp boundary between them.
    fn hard_light() -> Vec<u8> {
        gray_image(|x, _| if x < W / 2 { 0.08 } else { 0.92 })
    }

    /// Overcast: everything sits in the midtones and varies gently.
    fn soft_light() -> Vec<u8> {
        gray_image(|x, _| 0.42 + 0.06 * (x as f32 / W as f32))
    }

    /// Foliage: strong detail everywhere, but evenly lit.
    fn busy_texture() -> Vec<u8> {
        gray_image(|x, y| if (x + y) % 2 == 0 { 0.35 } else { 0.62 })
    }

    #[test]
    fn hard_light_scores_above_soft_light() {
        assert!(
            measure(&hard_light()).light_hardness > measure(&soft_light()).light_hardness,
            "a split-lit frame must read harder than an evenly lit one"
        );
    }

    #[test]
    fn busy_texture_does_not_masquerade_as_hard_light() {
        // The whole reason hardness is measured across scales rather than from
        // contrast: a high-contrast checkerboard is *detail*, not harsh light,
        // and a contrast-based measure would rank it at the top.
        let texture = measure(&busy_texture());
        let hard = measure(&hard_light());

        assert!(
            texture.light_hardness < hard.light_hardness,
            "texture {} should read softer than hard light {}",
            texture.light_hardness,
            hard.light_hardness
        );
        assert!(
            texture.fine_gradient_energy > texture.coarse_gradient_energy,
            "texture must lose energy under downscaling"
        );
    }

    #[test]
    fn a_gentle_ramp_is_not_hard_light_despite_spanning_the_frame() {
        // The case that broke the first implementation. A ramp from one edge of
        // the frame to the other carries as much *total* coarse gradient energy
        // as a shadow edge does, because downscaling by k scales any large-scale
        // structure's per-pixel gradient by k. Only the distribution separates
        // them, so this is the test that keeps the measure honest.
        let ramp = measure(&soft_light());
        let step = measure(&hard_light());

        assert!(
            ramp.light_hardness < 0.15,
            "a gentle ramp must not read as hard light, got {}",
            ramp.light_hardness
        );
        assert!(
            step.light_hardness > 0.6,
            "a shadow edge must read as hard light, got {}",
            step.light_hardness
        );
    }

    #[test]
    fn dynamic_range_separates_a_split_scene_from_a_flat_one() {
        assert!(
            measure(&hard_light()).dynamic_range_stops > measure(&soft_light()).dynamic_range_stops
        );
    }

    #[test]
    fn midtone_deficit_is_high_when_the_scene_splits_into_tails() {
        let hard = measure(&hard_light());
        let soft = measure(&soft_light());
        assert!(
            hard.midtone_deficit > soft.midtone_deficit,
            "hard {} vs soft {}",
            hard.midtone_deficit,
            soft.midtone_deficit
        );
    }

    #[test]
    fn percentiles_are_ordered_and_track_exposure() {
        let dark = measure(&gray_image(|_, _| 0.12));
        let bright = measure(&gray_image(|_, _| 0.85));

        assert!(dark.luminance_p50 < bright.luminance_p50);
        let s = measure(&soft_light());
        assert!(s.luminance_p005 <= s.luminance_p10);
        assert!(s.luminance_p10 <= s.luminance_p50);
        assert!(s.luminance_p50 <= s.luminance_p90);
        assert!(s.luminance_p90 <= s.luminance_p995);
    }

    #[test]
    fn clipping_is_reported_at_both_ends() {
        let blown = measure(&gray_image(|x, _| if x < W / 4 { 0.99 } else { 0.5 }));
        assert!(blown.highlight_clip > 0.2 && blown.highlight_clip < 0.3);

        let crushed = measure(&gray_image(|x, _| if x < W / 4 { 0.01 } else { 0.5 }));
        assert!(crushed.shadow_clip > 0.2 && crushed.shadow_clip < 0.3);
    }

    #[test]
    fn specular_detection_ignores_bright_but_saturated_pixels() {
        // A saturated colour at full brightness is not a blown highlight.
        let mut px = vec![0u8; W * H * 3];
        for i in 0..W * H {
            px[i * 3] = 255; // pure red: bright, but fully saturated
        }
        assert_eq!(measure(&px).specular_fraction, 0.0);

        let white = gray_image(|_, _| 1.0);
        assert!(measure(&white).specular_fraction > 0.99);
    }

    #[test]
    fn a_degenerate_frame_reports_the_neutral_reading() {
        let unknown = SceneState::unknown();
        assert_eq!(
            scene_state(&[], 0, 0, &SceneStateConfig::defaults()),
            unknown
        );
        assert_eq!(
            scene_state(&[0, 0, 0], 1, 1, &SceneStateConfig::defaults()),
            unknown
        );
    }

    #[test]
    fn a_flat_frame_is_soft_not_hard() {
        // No detail at any scale: the ratio is undefined, and reporting "hard"
        // there would let a guardrail fire on an empty measurement.
        let flat = measure(&gray_image(|_, _| 0.5));
        assert_eq!(flat.light_hardness, 0.0);
    }
}
