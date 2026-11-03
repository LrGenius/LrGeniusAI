//! Focus measured on one region of an image, at whatever resolution the caller
//! hands in.
//!
//! The frame metrics (`metrics::culling_metrics`) run on a 512 px reduction and
//! the face pass on the ≤2048 px working copy. Neither can answer the question
//! a portrait cull most often turns on — are the *eyes* sharp, or did focus land
//! on the nose or the ear? An eye that is a 13 px patch on the working copy
//! carries no defocus signal at all: on the reference portrait in
//! `testdata/astronaut.jpg` such a patch scores 0.86 sharp and 0.80 after a
//! Gaussian blur of sigma 2, where the same eye at 23 px scores 0.69 and 0.47.
//! So this module measures a rectangle of any RGB image and leaves the choice
//! of resolution to the caller (see `convert::decode_oriented`).
//!
//! The measure is the no-reference *blur effect* of Crété-Roffet et al. (2007),
//! as `skimage.measure.blur_effect` implements it: blur the patch along one axis
//! with a box filter and compare its gradient energy before and after. A sharp
//! patch loses most of it, because fine detail is exactly what the box removes;
//! a patch that was already soft loses little. Being a ratio of the patch's own
//! gradient energies, it depends far less on content and contrast than a
//! Laplacian variance does — which is what lets two frames of one burst be
//! compared on it, though not two different scenes on an absolute scale.
//!
//! `testdata/make_focus_goldens.py` pins this port against scikit-image.

/// skimage's default `h_size`: the length of the box filter the patch is
/// re-blurred with.
pub const BLUR_WINDOW: usize = 11;

/// skimage clamps both gradient magnitudes to this before comparing them
/// (`np.spacing(np.float64(1))`), so a perfectly flat patch divides by a tiny
/// number rather than by zero.
const EPSILON: f64 = f64::EPSILON;

/// Mean absolute gradient, in luma units per pixel, below which a patch has no
/// structure to judge: a blown highlight, a black frame, a plain wall. skimage
/// answers "fully blurred" (1.0) for such a patch, which is the wrong answer for
/// culling — an eye lost in a blown highlight is not out of focus, it is
/// unmeasurable.
const MIN_STRUCTURE: f64 = 1e-4;

/// A single-channel luma patch, row-major, values in `0..1`.
#[derive(Debug, Clone, PartialEq)]
pub struct LumaPatch {
    pub data: Vec<f64>,
    pub width: usize,
    pub height: usize,
}

impl LumaPatch {
    fn at(&self, x: usize, y: usize) -> f64 {
        self.data[y * self.width + x]
    }
}

/// Rec. 601 luma of one RGB8 pixel, in `0..1` — the weights the rest of the
/// culling metrics use.
fn luma(rgb: &[u8], i: usize) -> f64 {
    (0.299 * rgb[i * 3] as f64 + 0.587 * rgb[i * 3 + 1] as f64 + 0.114 * rgb[i * 3 + 2] as f64)
        / 255.0
}

/// The luma of the rectangle `[x0, x1) × [y0, y1)` of an RGB8 image, clamped
/// to the image, box-downsampled by a whole factor when its longer side exceeds
/// `max_side` (0 = never).
///
/// The cap bounds the cost of a close-up face at full resolution, which can be
/// millions of pixels; a whole-factor box keeps the same burst at the same
/// scale, so frames of it stay comparable. `None` when nothing of the
/// rectangle lies inside the image or `rgb` does not hold `width × height`
/// pixels.
pub fn luma_region(
    rgb: &[u8],
    width: usize,
    height: usize,
    rect: [f64; 4],
    max_side: usize,
) -> Option<LumaPatch> {
    if width == 0 || height == 0 || rgb.len() < width * height * 3 {
        return None;
    }
    let clamp_x = |v: f64| (v.max(0.0) as usize).min(width);
    let clamp_y = |v: f64| (v.max(0.0) as usize).min(height);
    let (x0, y0) = (clamp_x(rect[0].floor()), clamp_y(rect[1].floor()));
    let (x1, y1) = (clamp_x(rect[2].ceil()), clamp_y(rect[3].ceil()));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (rw, rh) = (x1 - x0, y1 - y0);
    let k = if max_side == 0 {
        1
    } else {
        rw.max(rh).div_ceil(max_side).max(1)
    };
    let (ow, oh) = (rw / k, rh / k);
    if ow == 0 || oh == 0 {
        return None;
    }
    let mut data = vec![0.0f64; ow * oh];
    let norm = 1.0 / (k * k) as f64;
    for oy in 0..oh {
        for ox in 0..ow {
            let mut sum = 0.0f64;
            for dy in 0..k {
                let row = (y0 + oy * k + dy) * width;
                for dx in 0..k {
                    sum += luma(rgb, row + x0 + ox * k + dx);
                }
            }
            data[oy * ow + ox] = sum * norm;
        }
    }
    Some(LumaPatch {
        data,
        width: ow,
        height: oh,
    })
}

/// scipy's `mode="reflect"` (half-sample symmetric: `d c b a | a b c d | d c b
/// a`), for an index any distance outside `0..n`.
fn reflect(i: isize, n: usize) -> usize {
    let n = n as isize;
    let period = 2 * n;
    let m = i.rem_euclid(period);
    (if m >= n { period - 1 - m } else { m }) as usize
}

/// `scipy.ndimage.uniform_filter1d(patch, window, axis)`, reflect mode.
fn box_filter(patch: &LumaPatch, window: usize, vertical: bool) -> Vec<f64> {
    let (w, h) = (patch.width, patch.height);
    let mut out = vec![0.0f64; w * h];
    let before = (window / 2) as isize;
    let after = window as isize - before;
    let norm = 1.0 / window as f64;
    if vertical {
        for x in 0..w {
            for y in 0..h {
                let mut sum = 0.0f64;
                for k in -before..after {
                    sum += patch.at(x, reflect(y as isize + k, h));
                }
                out[y * w + x] = sum * norm;
            }
        }
    } else {
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0.0f64;
                for k in -before..after {
                    sum += patch.at(reflect(x as isize + k, w), y);
                }
                out[y * w + x] = sum * norm;
            }
        }
    }
    out
}

/// `|skimage.filters.sobel(img, axis)|` at an interior pixel: the difference
/// across the edge axis, smoothed `[1, 2, 1] / 4` along the other. Only ever
/// evaluated where all eight neighbours exist, so no boundary rule is needed.
fn sobel(img: &[f64], w: usize, x: usize, y: usize, vertical: bool) -> f64 {
    let p = |xx: usize, yy: usize| img[yy * w + xx];
    let d = if vertical {
        0.25 * (p(x - 1, y + 1) - p(x - 1, y - 1))
            + 0.5 * (p(x, y + 1) - p(x, y - 1))
            + 0.25 * (p(x + 1, y + 1) - p(x + 1, y - 1))
    } else {
        0.25 * (p(x + 1, y - 1) - p(x - 1, y - 1))
            + 0.5 * (p(x + 1, y) - p(x - 1, y))
            + 0.25 * (p(x + 1, y + 1) - p(x - 1, y + 1))
    };
    d.abs()
}

/// `skimage.measure.blur_effect(patch, h_size=window)`: 0 for a perfectly sharp
/// patch, towards 1 for a blurred one, the larger of the two axes.
///
/// `None` when the patch has fewer than four pixels on a side (skimage's
/// interior slice would be empty), `window` is 0, or the patch has no structure
/// to judge — see [`MIN_STRUCTURE`].
pub fn blur_effect(patch: &LumaPatch, window: usize) -> Option<f64> {
    let (w, h) = (patch.width, patch.height);
    if w < 4 || h < 4 || window == 0 || patch.data.len() < w * h {
        return None;
    }
    let mut worst: Option<f64> = None;
    let mut structure = 0.0f64;
    for vertical in [true, false] {
        let blurred = box_filter(patch, window, vertical);
        let (mut m1, mut m2, mut raw) = (0.0f64, 0.0f64, 0.0f64);
        // skimage's `slice(2, s - 1)` on both axes.
        for y in 2..h - 1 {
            for x in 2..w - 1 {
                let sharp_raw = sobel(&patch.data, w, x, y, vertical);
                raw += sharp_raw;
                let sharp = sharp_raw.max(EPSILON);
                let soft = sobel(&blurred, w, x, y, vertical).max(EPSILON);
                m1 += sharp;
                m2 += (sharp - soft).max(0.0);
            }
        }
        let count = ((w - 3) * (h - 3)) as f64;
        structure = structure.max(raw / count);
        let b = (m1 - m2).abs() / m1;
        worst = Some(worst.map_or(b, |prev: f64| prev.max(b)));
    }
    if structure < MIN_STRUCTURE {
        return None;
    }
    worst
}

/// `1 - blur_effect`: the orientation the culling scores use, higher is sharper.
pub fn focus(patch: &LumaPatch, window: usize) -> Option<f64> {
    blur_effect(patch, window).map(|b| (1.0 - b).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic textured patch: a sum of sines at several frequencies,
    /// so a blur measurably removes the fine ones.
    fn texture(w: usize, h: usize) -> LumaPatch {
        let mut data = vec![0.0f64; w * h];
        for y in 0..h {
            for x in 0..w {
                let (xf, yf) = (x as f64, y as f64);
                let v = 0.5
                    + 0.18 * (xf * 1.3).sin() * (yf * 0.9).cos()
                    + 0.12 * (xf * 0.37 + yf * 0.21).sin()
                    + 0.08 * ((xf - yf) * 2.1).sin();
                data[y * w + x] = v.clamp(0.0, 1.0);
            }
        }
        LumaPatch {
            data,
            width: w,
            height: h,
        }
    }

    fn box_blur(p: &LumaPatch, radius: usize) -> LumaPatch {
        let window = 2 * radius + 1;
        let tmp = LumaPatch {
            data: box_filter(p, window, true),
            ..p.clone()
        };
        LumaPatch {
            data: box_filter(&tmp, window, false),
            ..p.clone()
        }
    }

    #[test]
    fn reflect_matches_scipy_half_sample_symmetric() {
        // scipy reflect on n=4: ... 1 0 | 0 1 2 3 | 3 2 ...
        let got: Vec<usize> = (-3..7).map(|i| reflect(i, 4)).collect();
        assert_eq!(got, vec![2, 1, 0, 0, 1, 2, 3, 3, 2, 1]);
        // Far outside wraps around again rather than panicking.
        assert_eq!(reflect(-9, 4), 0);
        assert_eq!(reflect(13, 4), 2);
    }

    #[test]
    fn blurring_a_patch_lowers_its_focus() {
        let sharp = texture(48, 48);
        let soft = box_blur(&sharp, 1);
        let softer = box_blur(&sharp, 2);
        let (a, b, c) = (
            focus(&sharp, BLUR_WINDOW).unwrap(),
            focus(&soft, BLUR_WINDOW).unwrap(),
            focus(&softer, BLUR_WINDOW).unwrap(),
        );
        assert!(a > b && b > c, "focus must fall with blur: {a} {b} {c}");
        assert!(
            a - c > 0.1,
            "a clear blur must be a clear difference: {a} vs {c}"
        );
    }

    #[test]
    fn contrast_alone_barely_moves_it() {
        // The same texture at half the contrast: a Laplacian variance would
        // drop by 4x, this ratio should hardly move.
        let sharp = texture(48, 48);
        let flat = LumaPatch {
            data: sharp.data.iter().map(|v| 0.5 + (v - 0.5) * 0.5).collect(),
            ..sharp.clone()
        };
        let (a, b) = (
            focus(&sharp, BLUR_WINDOW).unwrap(),
            focus(&flat, BLUR_WINDOW).unwrap(),
        );
        assert!((a - b).abs() < 0.01, "{a} vs {b}");
    }

    #[test]
    fn a_patch_without_structure_is_unmeasurable_not_blurred() {
        for v in [0.0, 0.5, 1.0] {
            let p = LumaPatch {
                data: vec![v; 32 * 32],
                width: 32,
                height: 32,
            };
            assert_eq!(blur_effect(&p, BLUR_WINDOW), None, "value {v}");
        }
    }

    #[test]
    fn too_small_or_degenerate_input_is_none() {
        let p = texture(3, 20);
        assert_eq!(blur_effect(&p, BLUR_WINDOW), None);
        assert_eq!(blur_effect(&texture(20, 20), 0), None);
    }

    #[test]
    fn luma_region_clamps_and_downsamples() {
        // 4x2 image: left half black, right half white.
        let (w, h) = (4usize, 2usize);
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 2..w {
                let i = (y * w + x) * 3;
                rgb[i..i + 3].fill(255);
            }
        }
        // Partly outside the image: clamped to the whole of it.
        let p = luma_region(&rgb, w, h, [-5.0, -5.0, 10.0, 10.0], 0).unwrap();
        assert_eq!((p.width, p.height), (4, 2));
        assert!((p.data[0] - 0.0).abs() < 1e-9 && (p.data[3] - 1.0).abs() < 1e-9);
        // A cap of 2 halves it by a 2x2 box: one black and one white pixel.
        let p = luma_region(&rgb, w, h, [0.0, 0.0, 4.0, 2.0], 2).unwrap();
        assert_eq!((p.width, p.height), (2, 1));
        assert!((p.data[0] - 0.0).abs() < 1e-9 && (p.data[1] - 1.0).abs() < 1e-9);
        // Nothing inside.
        assert_eq!(luma_region(&rgb, w, h, [5.0, 0.0, 9.0, 2.0], 0), None);
        // A buffer of the wrong size is refused, not indexed out of bounds.
        assert_eq!(luma_region(&rgb[..6], w, h, [0.0, 0.0, 4.0, 2.0], 0), None);
    }
}
