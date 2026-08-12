//! Face-crop quality proxies — port of the numpy math in
//! `services/face.py`: Laplacian sharpness, a cheap vertical-gradient
//! eye-openness proxy, and an occlusion proxy blending det/center/eye
//! signals.
//!
//! Tunables come from [`FaceMetricsConfig`]; they used to be private `const`s
//! here that shadowed the identically-named preset fields, so no culling preset
//! could move them. `FaceMetricsConfig::defaults()` reproduces the old values
//! exactly.

use lrg_imaging::cull_config::FaceMetricsConfig;

fn unit(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

fn luma(rgb: &[u8], w: usize, x: usize, y: usize) -> f64 {
    let i = (y * w + x) * 3;
    (0.299 * rgb[i] as f64 + 0.587 * rgb[i + 1] as f64 + 0.114 * rgb[i + 2] as f64) / 255.0
}

/// Port of `_compute_face_sharpness`.
pub fn face_sharpness(crop: &[u8], w: usize, h: usize, cfg: &FaceMetricsConfig) -> f64 {
    if w == 0 || h == 0 || w < 3 || h < 3 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    let mut sq_sum = 0.0f64;
    let mut n = 0usize;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let lap = -4.0 * luma(crop, w, x, y)
                + luma(crop, w, x, y - 1)
                + luma(crop, w, x, y + 1)
                + luma(crop, w, x - 1, y)
                + luma(crop, w, x + 1, y);
            sum += lap;
            sq_sum += lap * lap;
            n += 1;
        }
    }
    if n == 0 {
        return 0.0;
    }
    let mean = sum / n as f64;
    let variance = sq_sum / n as f64 - mean * mean;
    unit(variance / (variance + cfg.face_sharpness_denominator))
}

/// Port of `_compute_eye_openness_proxy`. `kps` are crop-local-origin
/// keypoints already offset by `(x1, y1)` — pass the raw detector kps
/// and bbox; this function subtracts the bbox origin internally, same
/// as the Python `ex = round(kps[i,0] - x1)`.
pub fn eye_openness_proxy(
    crop: &[u8],
    w: usize,
    h: usize,
    bbox: [i64; 4],
    kps: &[[f64; 2]; 5],
    cfg: &FaceMetricsConfig,
) -> f64 {
    if w == 0 || h == 0 || w < 6 || h < 6 {
        return 0.0;
    }
    let (x1, y1, x2, y2) = (bbox[0], bbox[1], bbox[2], bbox[3]);
    let face_span = ((x2 - x1).min(y2 - y1) as f64).max(4.0);
    let patch_radius = ((face_span * cfg.eye_patch_ratio).round() as i64)
        .clamp(cfg.eye_patch_radius_min, cfg.eye_patch_radius_max);

    let mut scores = Vec::new();
    for eye in kps.iter().take(2) {
        let ex = (eye[0] - x1 as f64).round() as i64;
        let ey = (eye[1] - y1 as f64).round() as i64;
        let px1 = (ex - patch_radius).max(0);
        let py1 = (ey - patch_radius).max(0);
        let px2 = (ex + patch_radius + 1).min(w as i64);
        let py2 = (ey + patch_radius + 1).min(h as i64);
        if px2 - px1 < 3 || py2 - py1 < 3 {
            continue;
        }
        // Mean absolute vertical gradient within the eye patch.
        let mut grad_sum = 0.0f64;
        let mut grad_n = 0usize;
        for y in py1..py2 - 1 {
            for x in px1..px2 {
                let a = luma(crop, w, x as usize, y as usize);
                let b = luma(crop, w, x as usize, (y + 1) as usize);
                grad_sum += (b - a).abs();
                grad_n += 1;
            }
        }
        if grad_n == 0 {
            continue;
        }
        let score_raw = grad_sum / grad_n as f64;
        scores.push(unit(score_raw / (score_raw + cfg.eye_openness_denominator)));
    }
    if scores.is_empty() {
        0.0
    } else {
        scores.iter().sum::<f64>() / scores.len() as f64
    }
}

/// Port of `_compute_occlusion_proxy`.
pub fn occlusion_proxy(
    det_score: f64,
    center_proximity: f64,
    eye_openness: f64,
    cfg: &FaceMetricsConfig,
) -> f64 {
    unit(
        1.0 - (cfg.occlusion_det_weight * unit(det_score)
            + cfg.occlusion_center_weight * unit(center_proximity)
            + cfg.occlusion_eye_weight * unit(eye_openness)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_crop_has_zero_sharpness() {
        let crop = vec![128u8; 20 * 20 * 3];
        assert_eq!(
            face_sharpness(&crop, 20, 20, &FaceMetricsConfig::defaults()),
            0.0
        );
    }

    #[test]
    fn occlusion_is_low_when_all_signals_strong() {
        // High det score, centered, eyes open -> low occlusion.
        let occ = occlusion_proxy(0.95, 0.9, 0.8, &FaceMetricsConfig::defaults());
        assert!(occ < 0.15, "occlusion {occ} should be low");
    }

    #[test]
    fn occlusion_is_high_when_all_signals_weak() {
        let occ = occlusion_proxy(0.1, 0.1, 0.1, &FaceMetricsConfig::defaults());
        assert!(occ > 0.85, "occlusion {occ} should be high");
    }
}
