//! Ports of `services/index.py`: `_compute_perceptual_hash` (classic
//! 64-bit DCT pHash, bit-exact with the Python implementation thanks to
//! the Pillow-exact resampler) and `_compute_culling_metrics`.

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
        }
    }
}

// Defaults from BASE_CULLING_CONFIG["image_metrics"] in config.py.
const SHARPNESS_DENOMINATOR: f64 = 0.015;
const HIGHLIGHT_THRESHOLD: f32 = 0.98;
const SHADOW_THRESHOLD: f32 = 0.02;
const HIGHLIGHT_CLIP_WEIGHT: f64 = 2.5;
const SHADOW_CLIP_WEIGHT: f64 = 2.0;
const EXPOSURE_TARGET: f64 = 0.5;
const EXPOSURE_TOLERANCE: f64 = 0.35;
const EXPOSURE_BALANCE_WEIGHT: f64 = 0.75;
const EXPOSURE_CLIP_WEIGHT: f64 = 0.25;
const NOISE_DENOMINATOR: f64 = 0.08;
const TECHNICAL_WEIGHT_SHARPNESS: f64 = 0.5;
const TECHNICAL_WEIGHT_EXPOSURE: f64 = 0.35;
const TECHNICAL_WEIGHT_NOISE: f64 = 0.15;
const AESTHETIC_CONTRAST_WEIGHT: f64 = 0.45;
const AESTHETIC_COLORFULNESS_WEIGHT: f64 = 0.35;
const AESTHETIC_EXPOSURE_WEIGHT: f64 = 0.20;

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
pub fn culling_metrics(image: &RgbImage) -> CullingMetrics {
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

    // Laplacian variance over the interior.
    let iw = rw - 2;
    let ih = rh - 2;
    let mut lap_sum = 0.0f64;
    let mut lap_sq_sum = 0.0f64;
    for y in 1..rh - 1 {
        for x in 1..rw - 1 {
            let c = gray[y * rw + x];
            let lap = (-4.0 * c
                + gray[(y - 1) * rw + x]
                + gray[(y + 1) * rw + x]
                + gray[y * rw + x - 1]
                + gray[y * rw + x + 1]) as f64;
            lap_sum += lap;
            lap_sq_sum += lap * lap;
        }
    }
    let lap_n = (iw * ih) as f64;
    let lap_mean = lap_sum / lap_n;
    let sharpness_raw = lap_sq_sum / lap_n - lap_mean * lap_mean;
    let sharpness = unit(sharpness_raw / (sharpness_raw + SHARPNESS_DENOMINATOR));

    let mut lum_sum = 0.0f64;
    let mut highlight = 0usize;
    let mut shadow = 0usize;
    let mut gray_sq_sum = 0.0f64;
    for &v in &gray {
        lum_sum += v as f64;
        gray_sq_sum += (v as f64) * (v as f64);
        if v >= HIGHLIGHT_THRESHOLD {
            highlight += 1;
        }
        if v <= SHADOW_THRESHOLD {
            shadow += 1;
        }
    }
    let luminance_mean = lum_sum / n as f64;
    let highlight_clip = highlight as f64 / n as f64;
    let shadow_clip = shadow as f64 / n as f64;
    let clipping_penalty =
        unit(highlight_clip * HIGHLIGHT_CLIP_WEIGHT + shadow_clip * SHADOW_CLIP_WEIGHT);
    let exposure_balance =
        unit(1.0 - ((luminance_mean - EXPOSURE_TARGET).abs() / EXPOSURE_TOLERANCE));
    let exposure = unit(
        EXPOSURE_BALANCE_WEIGHT * exposure_balance
            + EXPOSURE_CLIP_WEIGHT * (1.0 - clipping_penalty),
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
    let noise_penalty = unit(noise_raw / NOISE_DENOMINATOR);

    let technical_score = unit(
        TECHNICAL_WEIGHT_SHARPNESS * sharpness
            + TECHNICAL_WEIGHT_EXPOSURE * exposure
            + TECHNICAL_WEIGHT_NOISE * (1.0 - noise_penalty),
    );

    let gray_var = gray_sq_sum / n as f64 - luminance_mean * luminance_mean;
    let contrast = unit(gray_var.max(0.0).sqrt() / 0.25);
    let colorfulness = unit(rg_yb_sum / n as f64 / 0.35);
    let aesthetic_score = unit(
        AESTHETIC_CONTRAST_WEIGHT * contrast
            + AESTHETIC_COLORFULNESS_WEIGHT * colorfulness
            + AESTHETIC_EXPOSURE_WEIGHT * exposure,
    );

    CullingMetrics {
        cull_sharpness: round4(sharpness),
        cull_exposure: round4(exposure),
        cull_noise: round4(noise_penalty),
        cull_highlight_clip: round4(highlight_clip),
        cull_shadow_clip: round4(shadow_clip),
        cull_technical_score: round4(technical_score),
        cull_aesthetic: round4(aesthetic_score),
    }
}
