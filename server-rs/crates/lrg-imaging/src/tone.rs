//! A rendered photo's tones in numbers, for AI Edit's retoucher loop.
//!
//! A vision model judges a picture well and a histogram badly: it will call
//! a frame "slightly dark" whose shadows are a third clipped, and "neutral"
//! when every grey leans green. So each render the loop shows the model goes
//! with these measurements, and the model is told to trust them over its eye
//! for clipping, level and cast.
//!
//! Everything is read from the 8-bit sRGB render, the way the photographer
//! sees it, not from linear light: "median 38 %" means what the eye and
//! Lightroom's histogram mean by it.

use std::fmt::Write as _;

/// Pixels visited at most; a larger render is sampled on a regular grid.
const SAMPLE_TARGET: usize = 250_000;

/// A channel at or above this is clipped white in an 8-bit render.
const CLIP_HIGH: u8 = 254;
/// All channels at or below this are clipped black.
const CLIP_LOW: u8 = 2;

/// Luminance bounds (0..1) of the midtones the cast and saturation are read
/// from: deep shadows are noise and bright highlights are clipped, and both
/// read as colourless.
const MID_LOW: f64 = 0.15;
const MID_HIGH: f64 = 0.85;

/// Chroma below this share of luminance counts as a near-neutral pixel, the
/// kind whose colour is the light's rather than the object's.
const NEUTRAL_CHROMA: f64 = 0.12;

/// Fewer near-neutral pixels than this share and the cast is read from every
/// midtone pixel instead (a frame of foliage has no greys to go by).
const MIN_NEUTRAL_SHARE: f64 = 0.02;

/// What [`measure`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct ToneStats {
    /// Luminance percentiles 1, 5, 25, 50, 75, 95, 99, each 0..1.
    pub percentiles: [f64; 7],
    /// Share of pixels per band: blacks (<10 %), shadows (10–30 %),
    /// midtones (30–70 %), lights (70–90 %), whites (≥90 %).
    pub bands: [f64; 5],
    /// Share of pixels with any channel at the top of the range.
    pub clipped_highlights: f64,
    /// Share of pixels black in every channel.
    pub clipped_shadows: f64,
    /// Mean HSV saturation of midtone pixels, 0..1.
    pub midtone_saturation: f64,
    /// Lean of the neutral tones, in percent of full scale: positive is
    /// warm (yellow–orange), negative cool (blue).
    pub warmth: f64,
    /// Positive is magenta, negative green.
    pub tint: f64,
    /// Whether the cast was read from near-neutral pixels (true) or from
    /// all midtones (false, a weaker estimate).
    pub cast_from_neutrals: bool,
    /// Standard deviation of luminance, 0..0.5: global contrast.
    pub contrast: f64,
}

fn luminance(r: u8, g: u8, b: u8) -> f64 {
    (0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)) / 255.0
}

/// Measures a packed RGB8 image. `None` for an empty one.
pub fn measure(rgb: &[u8], width: usize, height: usize) -> Option<ToneStats> {
    let pixels = width.checked_mul(height)?;
    if pixels == 0 || rgb.len() < pixels * 3 {
        return None;
    }
    let step = ((pixels as f64 / SAMPLE_TARGET as f64).sqrt().floor() as usize).max(1);

    let mut histogram = [0u32; 256];
    let mut n = 0u64;
    let mut sum = 0.0;
    let mut sum_sq = 0.0;
    let mut clipped_high = 0u64;
    let mut clipped_low = 0u64;
    let mut mid = (0u64, 0.0f64, 0.0f64, 0.0f64); // count, saturation, warmth, tint
    let mut neutral = (0u64, 0.0f64, 0.0f64);

    for y in (0..height).step_by(step) {
        for x in (0..width).step_by(step) {
            let i = (y * width + x) * 3;
            let (r, g, b) = (rgb[i], rgb[i + 1], rgb[i + 2]);
            let l = luminance(r, g, b);
            histogram[(l * 255.0).round().clamp(0.0, 255.0) as usize] += 1;
            n += 1;
            sum += l;
            sum_sq += l * l;
            if r >= CLIP_HIGH || g >= CLIP_HIGH || b >= CLIP_HIGH {
                clipped_high += 1;
            }
            if r <= CLIP_LOW && g <= CLIP_LOW && b <= CLIP_LOW {
                clipped_low += 1;
            }
            if !(MID_LOW..=MID_HIGH).contains(&l) {
                continue;
            }
            let (rf, gf, bf) = (
                f64::from(r) / 255.0,
                f64::from(g) / 255.0,
                f64::from(b) / 255.0,
            );
            let max = rf.max(gf).max(bf);
            let min = rf.min(gf).min(bf);
            // Opponent axes: yellow–blue and magenta–green.
            let warm = (rf + gf) / 2.0 - bf;
            let tint = (rf + bf) / 2.0 - gf;
            mid.0 += 1;
            mid.1 += if max > 0.0 { (max - min) / max } else { 0.0 };
            mid.2 += warm;
            mid.3 += tint;
            if max - min < NEUTRAL_CHROMA * l.max(0.05) * 2.0 {
                neutral.0 += 1;
                neutral.1 += warm;
                neutral.2 += tint;
            }
        }
    }

    let n_f = n as f64;
    let percentile = |p: f64| {
        let target = (p * n_f).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for (value, count) in histogram.iter().enumerate() {
            seen += u64::from(*count);
            if seen >= target {
                return value as f64 / 255.0;
            }
        }
        1.0
    };
    let share = |from: f64, to: f64| {
        let lo = (from * 255.0).round() as usize;
        let hi = ((to * 255.0).round() as usize).min(256);
        histogram[lo..hi].iter().map(|c| u64::from(*c)).sum::<u64>() as f64 / n_f
    };
    let mean = sum / n_f;
    let use_neutrals = mid.0 > 0 && neutral.0 as f64 / n_f >= MIN_NEUTRAL_SHARE;
    let (warmth, tint) = if use_neutrals {
        (neutral.1 / neutral.0 as f64, neutral.2 / neutral.0 as f64)
    } else if mid.0 > 0 {
        (mid.2 / mid.0 as f64, mid.3 / mid.0 as f64)
    } else {
        (0.0, 0.0)
    };

    Some(ToneStats {
        percentiles: [0.01, 0.05, 0.25, 0.5, 0.75, 0.95, 0.99].map(percentile),
        bands: [
            share(0.0, 0.1),
            share(0.1, 0.3),
            share(0.3, 0.7),
            share(0.7, 0.9),
            share(0.9, 1.01),
        ],
        clipped_highlights: clipped_high as f64 / n_f,
        clipped_shadows: clipped_low as f64 / n_f,
        midtone_saturation: if mid.0 > 0 { mid.1 / mid.0 as f64 } else { 0.0 },
        warmth: warmth * 100.0,
        tint: tint * 100.0,
        cast_from_neutrals: use_neutrals,
        contrast: (sum_sq / n_f - mean * mean).max(0.0).sqrt(),
    })
}

fn pct(v: f64) -> String {
    format!("{:.0}", v * 100.0)
}

fn share_pct(v: f64) -> String {
    let p = v * 100.0;
    if p > 0.0 && p < 0.1 {
        "<0.1".to_string()
    } else if p < 10.0 {
        format!("{p:.1}")
    } else {
        format!("{p:.0}")
    }
}

impl ToneStats {
    /// The measurements as a few lines for a language model.
    pub fn describe(&self) -> String {
        let p = &self.percentiles;
        let b = &self.bands;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "Luminance (0-100): p1 {}, p5 {}, p25 {}, median {}, p75 {}, p95 {}, p99 {}; contrast (std) {}",
            pct(p[0]), pct(p[1]), pct(p[2]), pct(p[3]), pct(p[4]), pct(p[5]), pct(p[6]),
            pct(self.contrast)
        );
        let _ = writeln!(
            out,
            "Histogram: blacks {}% · shadows {}% · midtones {}% · lights {}% · whites {}%",
            share_pct(b[0]),
            share_pct(b[1]),
            share_pct(b[2]),
            share_pct(b[3]),
            share_pct(b[4])
        );
        let _ = writeln!(
            out,
            "Clipped: {}% of pixels have a channel at 255, {}% are pure black",
            share_pct(self.clipped_highlights),
            share_pct(self.clipped_shadows)
        );
        let lean = |v: f64, pos: &str, neg: &str| {
            if v.abs() < 0.5 {
                "neutral".to_string()
            } else {
                format!("{} {:.1}", if v > 0.0 { pos } else { neg }, v.abs())
            }
        };
        let _ = writeln!(
            out,
            "Colour of the {}: {}, {} (in % of full scale; above about 2 is a visible cast)",
            if self.cast_from_neutrals {
                "near-neutral tones"
            } else {
                "midtones (few neutral tones, so a weaker estimate)"
            },
            lean(self.warmth, "warm", "cool"),
            lean(self.tint, "magenta", "green"),
        );
        let _ = write!(out, "Midtone saturation: {}%", pct(self.midtone_saturation));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        let mut out = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                out.extend_from_slice(&f(x, y));
            }
        }
        out
    }

    #[test]
    fn an_empty_image_has_no_stats() {
        assert!(measure(&[], 0, 0).is_none());
        assert!(measure(&[1, 2], 1, 1).is_none());
    }

    #[test]
    fn a_ramp_spreads_over_every_band() {
        let img = image(256, 4, |x, _| [x as u8; 3]);
        let s = measure(&img, 256, 4).unwrap();
        assert!((s.percentiles[3] - 0.5).abs() < 0.02, "{:?}", s.percentiles);
        assert!((s.bands.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!((s.bands[2] - 0.4).abs() < 0.02, "{:?}", s.bands);
        assert!(s.clipped_highlights > 0.0 && s.clipped_highlights < 0.02);
        assert!(s.cast_from_neutrals);
        assert!(s.warmth.abs() < 0.01 && s.tint.abs() < 0.01);
    }

    #[test]
    fn blown_and_crushed_pixels_are_counted() {
        let img = image(10, 10, |x, _| match x {
            0..=2 => [255, 250, 240],
            3..=4 => [0, 0, 0],
            _ => [120, 120, 120],
        });
        let s = measure(&img, 10, 10).unwrap();
        assert!((s.clipped_highlights - 0.3).abs() < 1e-9);
        assert!((s.clipped_shadows - 0.2).abs() < 1e-9);
    }

    #[test]
    fn a_warm_grey_reads_as_warm_and_a_green_one_as_green() {
        let warm = image(20, 20, |_, _| [140, 128, 112]);
        let s = measure(&warm, 20, 20).unwrap();
        assert!(s.cast_from_neutrals);
        assert!(s.warmth > 3.0, "{}", s.warmth);

        let green = image(20, 20, |_, _| [120, 132, 120]);
        let s = measure(&green, 20, 20).unwrap();
        assert!(s.tint < -3.0, "{}", s.tint);
        assert!(s.describe().contains("green"));
    }

    #[test]
    fn foliage_without_greys_falls_back_to_the_midtones() {
        let img = image(20, 20, |_, _| [60, 160, 40]);
        let s = measure(&img, 20, 20).unwrap();
        assert!(!s.cast_from_neutrals);
        assert!(s.midtone_saturation > 0.5);
        assert!(s.describe().contains("weaker estimate"));
    }

    #[test]
    fn a_large_render_is_sampled_not_read_whole() {
        let img = image(2000, 1500, |x, _| [(x % 256) as u8; 3]);
        let s = measure(&img, 2000, 1500).unwrap();
        assert!((s.percentiles[3] - 0.5).abs() < 0.03);
    }
}
