//! Ports of `services/index.py`: `_compute_perceptual_hash` (classic
//! 64-bit DCT pHash, bit-exact with the Python implementation thanks to
//! the Pillow-exact resampler) and `_compute_culling_metrics`.

use crate::cull_config::ImageMetricsConfig;
use crate::pil_resample::{resize_plane, rgb_to_luma, Filter};

/// An 8-bit RGB image in row-major interleaved layout.
pub struct RgbImage<'a> {
    pub pixels: &'a [u8],
    pub width: usize,
    pub height: usize,
}

fn build_dct_matrix(size: usize) -> Vec<f32> {
    let mut matrix = vec![0.0f32; size * size];
    let scale = std::f32::consts::PI / (2.0 * size as f32);
    for u in 0..size {
        let alpha = if u == 0 {
            (1.0f32 / size as f32).sqrt()
        } else {
            (2.0f32 / size as f32).sqrt()
        };
        for i in 0..size {
            matrix[u * size + i] = alpha * ((2.0 * i as f32 + 1.0) * u as f32 * scale).cos();
        }
    }
    matrix
}

/// f32 matrix product (row-major), matching numpy's float32 `@`.
fn matmul32(a: &[f32], b: &[f32], n: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; n * n];
    for i in 0..n {
        for k in 0..n {
            let aik = a[i * n + k];
            if aik == 0.0 {
                continue;
            }
            for j in 0..n {
                out[i * n + j] += aik * b[k * n + j];
            }
        }
    }
    out
}

/// Classic 64-bit pHash, 16-char hex; empty string on failure.
/// Port of `_compute_perceptual_hash`.
pub fn perceptual_hash(image: &RgbImage) -> String {
    if image.width == 0 || image.height == 0 {
        return String::new();
    }
    let gray = rgb_to_luma(image.pixels, image.width * image.height);
    let small = resize_plane(&gray, image.width, image.height, 32, 32, Filter::Lanczos3);
    let pixels: Vec<f32> = small.iter().map(|&v| v as f32).collect();

    let dct = build_dct_matrix(32);
    // dct @ pixels @ dct.T
    let tmp = matmul32(&dct, &pixels, 32);
    let mut dct_t = vec![0.0f32; 32 * 32];
    for u in 0..32 {
        for i in 0..32 {
            dct_t[i * 32 + u] = dct[u * 32 + i];
        }
    }
    let transformed = matmul32(&tmp, &dct_t, 32);

    let mut low_freq = [0.0f32; 64];
    for r in 0..8 {
        for c in 0..8 {
            low_freq[r * 8 + c] = transformed[r * 32 + c];
        }
    }
    // median of low_freq[1:, :] (56 values, numpy median = mean of middles)
    let mut tail: Vec<f32> = low_freq[8..].to_vec();
    tail.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = (tail[27] as f64 + tail[28] as f64) / 2.0;

    let mut hash_value: u64 = 0;
    for &v in &low_freq {
        hash_value = (hash_value << 1) | u64::from((v as f64) > median);
    }
    format!("{hash_value:016x}")
}

#[derive(Debug, Clone, PartialEq)]
pub struct CullingMetrics {
    pub cull_sharpness: f64,
    pub cull_exposure: f64,
    pub cull_noise: f64,
    pub cull_highlight_clip: f64,
    pub cull_shadow_clip: f64,
    pub cull_technical_score: f64,
    pub cull_aesthetic: f64,
    /// Sharpness of the *sharpest tile*, normalized like [`Self::cull_sharpness`].
    ///
    /// The frame-wide Laplacian variance answers "is this frame busy?", not "is
    /// anything in focus?". An f/1.4 portrait or a 400mm wildlife frame has a
    /// razor-sharp subject against smooth bokeh and scores *lower* globally
    /// than a mediocre f/8 snapshot of foliage. Taking the max over tiles asks
    /// the question culling actually cares about, and the pair
    /// (global, peak) separates "soft everywhere" from "correctly shallow".
    pub cull_sharpness_peak: f64,
    /// How localized the focus is: `1 - in_focus_tiles / total_tiles`, where a
    /// tile is in focus at `sharpness_tile_focus_fraction` of the peak tile.
    ///
    /// Near 0 for a deep-focus landscape (everything equally sharp), near 1 for
    /// a subject isolated against bokeh. This is what tells a *shallow* frame
    /// apart from a *soft* one when both have low global sharpness.
    pub cull_focus_concentration: f64,
    /// Variance-weighted centroid of the in-focus tiles, normalized to `0..1`
    /// across the frame. Used to tell a focus stack (sharp region walks between
    /// frames) from a burst (sharp region stays put), and available as a
    /// composition signal.
    pub cull_sharp_region_x: f64,
    pub cull_sharp_region_y: f64,
    /// Directional coherence of the image gradient, from the structure tensor:
    /// `sqrt((Jxx-Jyy)^2 + 4*Jxy^2) / (Jxx+Jyy)`.
    ///
    /// 0 is isotropic, 1 is a single dominant edge direction. Directional
    /// camera shake pushes this up because it destroys detail along one axis
    /// only — but so does genuinely directional *content* (architecture,
    /// horizons, rain). It is therefore never scored on its own: it only
    /// distinguishes the *reason* a frame that is already soft is soft, which
    /// is the difference between the `motion_blur` and `blurred` reason codes.
    pub cull_motion_anisotropy: f64,
}

impl CullingMetrics {
    pub fn failed() -> Self {
        CullingMetrics {
            cull_sharpness: 0.0,
            cull_exposure: 0.0,
            cull_noise: 1.0,
            cull_highlight_clip: 0.0,
            cull_shadow_clip: 0.0,
            cull_technical_score: 0.0,
            cull_aesthetic: 0.0,
            cull_sharpness_peak: 0.0,
            cull_focus_concentration: 0.0,
            cull_sharp_region_x: 0.5,
            cull_sharp_region_y: 0.5,
            cull_motion_anisotropy: 0.0,
        }
    }
}

fn unit(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

/// Python 3 round(x, 4): round-half-to-even at 4 decimals.
fn round4(v: f64) -> f64 {
    (v * 10000.0).round_ties_even() / 10000.0
}

/// Python round-half-even to int (used for the resize dimensions).
fn py_round(v: f64) -> i64 {
    v.round_ties_even() as i64
}

/// Port of `_compute_culling_metrics`. Mean/variance accumulate in f64,
/// matching numpy's pairwise-summation accuracy closely enough for the
/// 1e-4 rounding in the output.
///
/// `cfg` carries what used to be file-private constants; passing
/// [`ImageMetricsConfig::default`] reproduces the historical numbers exactly.
pub fn culling_metrics(image: &RgbImage, cfg: &ImageMetricsConfig) -> CullingMetrics {
    let (w, h) = (image.width, image.height);
    if w == 0 || h == 0 {
        return CullingMetrics::failed();
    }

    // Resize to max 512 (BILINEAR), min dimension 32 — per plane.
    let (rw, rh, rgb): (usize, usize, Vec<u8>) = if w.max(h) > 512 {
        let scale = 512.0 / w.max(h) as f64;
        let rw = (py_round(w as f64 * scale).max(32)) as usize;
        let rh = (py_round(h as f64 * scale).max(32)) as usize;
        let n = w * h;
        let mut planes = Vec::with_capacity(3);
        for c in 0..3 {
            let plane: Vec<u8> = (0..n).map(|i| image.pixels[i * 3 + c]).collect();
            planes.push(resize_plane(&plane, w, h, rw, rh, Filter::Bilinear));
        }
        let mut inter = vec![0u8; rw * rh * 3];
        for i in 0..rw * rh {
            inter[i * 3] = planes[0][i];
            inter[i * 3 + 1] = planes[1][i];
            inter[i * 3 + 2] = planes[2][i];
        }
        (rw, rh, inter)
    } else {
        (w, h, image.pixels.to_vec())
    };

    if rh < 8 || rw < 8 {
        return CullingMetrics::failed();
    }

    let n = rw * rh;
    let mut gray = vec![0.0f32; n];
    let mut rg_yb_sum = 0.0f64; // mean of sqrt(rg^2 + yb^2)
    for i in 0..n {
        let r = rgb[i * 3] as f32 / 255.0;
        let g = rgb[i * 3 + 1] as f32 / 255.0;
        let b = rgb[i * 3 + 2] as f32 / 255.0;
        gray[i] = 0.299 * r + 0.587 * g + 0.114 * b;
        let rg = (r - g).abs();
        let yb = (0.5 * (r + g) - b).abs();
        rg_yb_sum += ((rg * rg + yb * yb) as f64).sqrt();
    }

    // Laplacian variance over the interior, accumulated globally *and* per
    // tile in the same pass. The per-tile sums cost two extra adds per pixel
    // and are what make subject-region sharpness essentially free; the global
    // accumulators are untouched so `cull_sharpness` stays bit-identical to
    // the Python original. The structure-tensor sums for `cull_motion_anisotropy`
    // ride along on the same neighbour reads.
    let iw = rw - 2;
    let ih = rh - 2;
    let grid = cfg.sharpness_tile_grid.max(1);
    let tile_count = grid * grid;
    let mut tile_sum = vec![0.0f64; tile_count];
    let mut tile_sq_sum = vec![0.0f64; tile_count];
    let mut tile_n = vec![0u32; tile_count];
    let mut lap_sum = 0.0f64;
    let mut lap_sq_sum = 0.0f64;
    let (mut jxx, mut jyy, mut jxy) = (0.0f64, 0.0f64, 0.0f64);
    for y in 1..rh - 1 {
        // Tile row/column from the *interior* extent, so every tile gets a
        // share even when the frame is barely larger than the grid.
        let ty = ((y - 1) * grid / ih).min(grid - 1);
        for x in 1..rw - 1 {
            let c = gray[y * rw + x];
            let (up, down) = (gray[(y - 1) * rw + x], gray[(y + 1) * rw + x]);
            let (left, right) = (gray[y * rw + x - 1], gray[y * rw + x + 1]);
            let lap = (-4.0 * c + up + down + left + right) as f64;
            lap_sum += lap;
            lap_sq_sum += lap * lap;

            let tx = ((x - 1) * grid / iw).min(grid - 1);
            let t = ty * grid + tx;
            tile_sum[t] += lap;
            tile_sq_sum[t] += lap * lap;
            tile_n[t] += 1;

            // Central differences; the 0.5 factors cancel in the coherence
            // ratio, so they are left out.
            let gx = (right - left) as f64;
            let gy = (down - up) as f64;
            jxx += gx * gx;
            jyy += gy * gy;
            jxy += gx * gy;
        }
    }
    let lap_n = (iw * ih) as f64;
    let lap_mean = lap_sum / lap_n;
    let sharpness_raw = lap_sq_sum / lap_n - lap_mean * lap_mean;
    let sharpness = unit(sharpness_raw / (sharpness_raw + cfg.sharpness_denominator));

    let tile_variance: Vec<f64> = (0..tile_count)
        .map(|t| {
            if tile_n[t] < 2 {
                return 0.0;
            }
            let n = tile_n[t] as f64;
            let mean = tile_sum[t] / n;
            (tile_sq_sum[t] / n - mean * mean).max(0.0)
        })
        .collect();
    let peak_variance = tile_variance.iter().cloned().fold(0.0f64, f64::max);
    let sharpness_peak = unit(peak_variance / (peak_variance + cfg.sharpness_denominator));

    // In-focus tiles, their share of the frame, and their weighted centroid.
    let focus_cutoff = peak_variance * cfg.sharpness_tile_focus_fraction.clamp(0.0, 1.0);
    let (mut focus_tiles, mut wsum, mut wx, mut wy) = (0usize, 0.0f64, 0.0f64, 0.0f64);
    for (t, &v) in tile_variance.iter().enumerate() {
        if peak_variance <= 0.0 || v < focus_cutoff {
            continue;
        }
        focus_tiles += 1;
        // Tile centre in normalized frame coordinates.
        let (tx, ty) = ((t % grid) as f64, (t / grid) as f64);
        let cx = (tx + 0.5) / grid as f64;
        let cy = (ty + 0.5) / grid as f64;
        wsum += v;
        wx += v * cx;
        wy += v * cy;
    }
    let focus_concentration = if focus_tiles == 0 {
        0.0
    } else {
        unit(1.0 - focus_tiles as f64 / tile_count as f64)
    };
    let (sharp_region_x, sharp_region_y) = if wsum > 0.0 {
        (unit(wx / wsum), unit(wy / wsum))
    } else {
        (0.5, 0.5)
    };

    let trace = jxx + jyy;
    let motion_anisotropy = if trace > 0.0 {
        unit((((jxx - jyy).powi(2) + 4.0 * jxy * jxy).sqrt()) / trace)
    } else {
        0.0
    };

    // Compared against f32 luma, so narrow once rather than widening per pixel.
    let highlight_threshold = cfg.highlight_threshold as f32;
    let shadow_threshold = cfg.shadow_threshold as f32;
    let mut lum_sum = 0.0f64;
    let mut highlight = 0usize;
    let mut shadow = 0usize;
    let mut gray_sq_sum = 0.0f64;
    for &v in &gray {
        lum_sum += v as f64;
        gray_sq_sum += (v as f64) * (v as f64);
        if v >= highlight_threshold {
            highlight += 1;
        }
        if v <= shadow_threshold {
            shadow += 1;
        }
    }
    let luminance_mean = lum_sum / n as f64;
    let highlight_clip = highlight as f64 / n as f64;
    let shadow_clip = shadow as f64 / n as f64;
    let clipping_penalty =
        unit(highlight_clip * cfg.highlight_clip_weight + shadow_clip * cfg.shadow_clip_weight);
    let exposure_balance =
        unit(1.0 - ((luminance_mean - cfg.exposure_target).abs() / cfg.exposure_tolerance));
    let exposure = unit(
        cfg.exposure_balance_weight * exposure_balance
            + cfg.exposure_clip_weight * (1.0 - clipping_penalty),
    );

    // 3x3 box-blur residual noise estimate over the interior.
    let mut resid_all_sum = 0.0f64;
    let mut resid_mid_sum = 0.0f64;
    let mut mid_count = 0usize;
    for y in 1..rh - 1 {
        for x in 1..rw - 1 {
            let mut acc = 0.0f32;
            for dy in 0..3 {
                for dx in 0..3 {
                    acc += gray[(y - 1 + dy) * rw + (x - 1 + dx)];
                }
            }
            let blurred = acc / 9.0;
            let reference = gray[y * rw + x];
            let residual = (reference - blurred).abs() as f64;
            resid_all_sum += residual;
            if reference > 0.15 && reference < 0.85 {
                resid_mid_sum += residual;
                mid_count += 1;
            }
        }
    }
    let noise_raw = if mid_count > 0 {
        resid_mid_sum / mid_count as f64
    } else {
        resid_all_sum / lap_n
    };
    let noise_penalty = unit(noise_raw / cfg.noise_denominator);

    let technical_score = unit(
        cfg.technical_weight_sharpness * sharpness
            + cfg.technical_weight_exposure * exposure
            + cfg.technical_weight_noise * (1.0 - noise_penalty),
    );

    let gray_var = gray_sq_sum / n as f64 - luminance_mean * luminance_mean;
    let contrast = unit(gray_var.max(0.0).sqrt() / 0.25);
    let colorfulness = unit(rg_yb_sum / n as f64 / 0.35);
    let aesthetic_score = unit(
        cfg.aesthetic_contrast_weight * contrast
            + cfg.aesthetic_colorfulness_weight * colorfulness
            + cfg.aesthetic_exposure_weight * exposure,
    );

    CullingMetrics {
        cull_sharpness: round4(sharpness),
        cull_exposure: round4(exposure),
        cull_noise: round4(noise_penalty),
        cull_highlight_clip: round4(highlight_clip),
        cull_shadow_clip: round4(shadow_clip),
        cull_technical_score: round4(technical_score),
        cull_aesthetic: round4(aesthetic_score),
        cull_sharpness_peak: round4(sharpness_peak),
        cull_focus_concentration: round4(focus_concentration),
        cull_sharp_region_x: round4(sharp_region_x),
        cull_sharp_region_y: round4(sharp_region_y),
        cull_motion_anisotropy: round4(motion_anisotropy),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame that is sharp in one corner and smooth everywhere else — an
    /// f/1.4 subject against bokeh. Global variance is diluted by the smooth
    /// majority; the peak tile is not, and the centroid points at the subject.
    fn subject_in_corner(size: usize, subject: usize) -> Vec<u8> {
        let mut px = vec![110u8; size * size * 3];
        for y in 0..subject {
            for x in 0..subject {
                // High-frequency checker: maximal Laplacian energy.
                let v = if (x + y) % 2 == 0 { 20 } else { 235 };
                let i = (y * size + x) * 3;
                px[i] = v;
                px[i + 1] = v;
                px[i + 2] = v;
            }
        }
        px
    }

    fn metrics_of(pixels: &[u8], size: usize) -> CullingMetrics {
        culling_metrics(
            &RgbImage {
                pixels,
                width: size,
                height: size,
            },
            &ImageMetricsConfig::default(),
        )
    }

    #[test]
    fn peak_sharpness_survives_a_smooth_background() {
        let m = metrics_of(&subject_in_corner(256, 64), 256);
        assert!(
            m.cull_sharpness_peak > m.cull_sharpness,
            "peak {} should beat global {}",
            m.cull_sharpness_peak,
            m.cull_sharpness
        );
        assert!(
            m.cull_sharpness_peak > 0.9,
            "an in-focus region is present: {}",
            m.cull_sharpness_peak
        );
    }

    #[test]
    fn focus_centroid_points_at_the_sharp_region() {
        let m = metrics_of(&subject_in_corner(256, 64), 256);
        assert!(
            m.cull_sharp_region_x < 0.3 && m.cull_sharp_region_y < 0.3,
            "subject is top-left, got ({}, {})",
            m.cull_sharp_region_x,
            m.cull_sharp_region_y
        );
        assert!(
            m.cull_focus_concentration > 0.7,
            "focus is localized: {}",
            m.cull_focus_concentration
        );
    }

    /// A uniformly detailed frame is sharp everywhere, so concentration must
    /// drop — that is the signal that separates deep focus from shallow.
    #[test]
    fn uniform_detail_has_low_focus_concentration() {
        let size = 256;
        let mut px = vec![0u8; size * size * 3];
        for y in 0..size {
            for x in 0..size {
                let v = if (x + y) % 2 == 0 { 20 } else { 235 };
                let i = (y * size + x) * 3;
                px[i] = v;
                px[i + 1] = v;
                px[i + 2] = v;
            }
        }
        let m = metrics_of(&px, size);
        assert!(
            m.cull_focus_concentration < 0.1,
            "every tile is in focus: {}",
            m.cull_focus_concentration
        );
    }

    /// Horizontal stripes are a single dominant gradient direction; a checker
    /// spreads energy over both axes. The coherence measure must separate them.
    #[test]
    fn anisotropy_separates_directional_from_isotropic_detail() {
        let size = 128;
        let mut stripes = vec![0u8; size * size * 3];
        let mut checker = vec![0u8; size * size * 3];
        // Period 8, not 2: a 1px period sits exactly at the Nyquist limit of
        // the central difference, where both gradients read zero.
        for y in 0..size {
            for x in 0..size {
                let i = (y * size + x) * 3;
                let s = if (y / 4) % 2 == 0 { 20 } else { 235 };
                let c = if ((x / 4) + (y / 4)) % 2 == 0 {
                    20
                } else {
                    235
                };
                stripes[i..i + 3].fill(s);
                checker[i..i + 3].fill(c);
            }
        }
        let ms = metrics_of(&stripes, size);
        let mc = metrics_of(&checker, size);
        assert!(
            ms.cull_motion_anisotropy > 0.9,
            "stripes are strongly directional: {}",
            ms.cull_motion_anisotropy
        );
        assert!(
            mc.cull_motion_anisotropy < 0.2,
            "a checker is isotropic: {}",
            mc.cull_motion_anisotropy
        );
    }

    /// A flat frame has no focus anywhere; the centroid must not divide by zero
    /// or claim a subject that is not there.
    #[test]
    fn flat_frame_reports_no_focus() {
        let m = metrics_of(&vec![128u8; 64 * 64 * 3], 64);
        assert_eq!(m.cull_sharpness_peak, 0.0);
        assert_eq!(m.cull_focus_concentration, 0.0);
        assert_eq!(m.cull_sharp_region_x, 0.5);
        assert_eq!(m.cull_sharp_region_y, 0.5);
    }
}
