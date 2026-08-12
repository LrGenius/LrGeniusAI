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

/// How far the detected landmarks are from a plausible face geometry, as a
/// fraction of the aligned crop's width.
///
/// The five SCRFD keypoints are fitted to ArcFace's canonical template with the
/// same similarity transform the recognition pipeline already uses. A similarity
/// transform has four degrees of freedom and can absorb any translation,
/// rotation, uniform scale — so on an unobstructed frontal face the residual is
/// near zero regardless of where the head is or how it is tilted. What it cannot
/// absorb is the *shape* changing: a hand, a microphone or another head across
/// part of the face pushes the detector's estimate of the covered points off
/// their true positions, and that shows up here and nowhere else.
///
/// Strong out-of-plane rotation raises it too, because a profile face really is
/// a different 2-D shape. That is not a false positive for culling's purpose —
/// the plan calls this signal "occlusion / poor facial visibility" — but it is
/// the reason the field is not a pure occlusion measure and should not be read
/// as one.
fn landmark_fit_residual(kps: &[[f64; 2]; 5]) -> f64 {
    let m = crate::umeyama::estimate_similarity(kps, &crate::umeyama::ARCFACE_DST);
    let mut sq_sum = 0.0f64;
    for (src, dst) in kps.iter().zip(crate::umeyama::ARCFACE_DST.iter()) {
        let x = m[0][0] * src[0] + m[0][1] * src[1] + m[0][2];
        let y = m[1][0] * src[0] + m[1][1] * src[1] + m[1][2];
        sq_sum += (x - dst[0]).powi(2) + (y - dst[1]).powi(2);
    }
    // RMS residual in aligned-crop pixels, expressed as a fraction of the
    // 112px crop so the number is scale-free.
    (sq_sum / 5.0).sqrt() / 112.0
}

/// Mean absolute luminance difference between the aligned face crop and its
/// mirror image, normalized by the crop's own luminance spread.
///
/// A face is close to bilaterally symmetric once aligned to the canonical
/// template, so anything covering one side and not the other shows up directly.
/// Normalizing by the crop's standard deviation is what keeps this from simply
/// re-measuring contrast: a flat, evenly lit face and a high-contrast one with
/// the same *relative* asymmetry score the same.
///
/// Side lighting also breaks symmetry, which is the main thing this over-reports
/// on. It is therefore combined with the landmark residual rather than trusted
/// alone — the two fail on different inputs.
fn mirror_asymmetry(aligned: &[u8], size: usize) -> f64 {
    if size < 4 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    let mut sq_sum = 0.0f64;
    let mut diff_sum = 0.0f64;
    let mut n = 0usize;
    for y in 0..size {
        for x in 0..size / 2 {
            let left = luma(aligned, size, x, y);
            let right = luma(aligned, size, size - 1 - x, y);
            diff_sum += (left - right).abs();
            sum += left + right;
            sq_sum += left * left + right * right;
            n += 2;
        }
    }
    if n == 0 {
        return 0.0;
    }
    let mean = sum / n as f64;
    let std_dev = (sq_sum / n as f64 - mean * mean).max(0.0).sqrt();
    // Below this the crop carries no structure to compare — a blown-out or
    // pitch-black face — and the ratio would explode on rounding noise.
    if std_dev < 0.01 {
        return 0.0;
    }
    let mean_diff = diff_sum / (n as f64 / 2.0);
    unit(mean_diff / (2.0 * std_dev))
}

/// How obstructed or turned-away the face is, in `0..1`.
///
/// This replaces a function that never looked at the image: the old
/// `occlusion_proxy` was `1 - (0.55·det_score + 0.20·center_proximity +
/// 0.25·eye_openness)`, a recombination of three numbers the caller already had.
/// It was therefore ~collinear with detector confidence, `center_proximity` is a
/// *composition* measure (a face near the frame edge is not occluded), and
/// `reject_occlusion_threshold` was in practice a second confidence gate. Both
/// terms here are measured from pixels or from landmark geometry.
///
/// The two are summed rather than maxed because they catch different halves of
/// the problem — geometry catches a covered feature the detector mislocated,
/// symmetry catches an obstruction the detector placed points around correctly —
/// and either one alone produces too many false alarms to gate a reject on.
pub fn occlusion_from_crop(
    aligned_crop: &[u8],
    aligned_size: usize,
    kps: &[[f64; 2]; 5],
    cfg: &FaceMetricsConfig,
) -> f64 {
    let geometry = unit(landmark_fit_residual(kps) / cfg.occlusion_geometry_normalizer.max(1e-6));
    let symmetry = mirror_asymmetry(aligned_crop, aligned_size);
    unit(cfg.occlusion_geometry_weight * geometry + cfg.occlusion_symmetry_weight * symmetry)
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

    /// A symmetric, textured 112x112 stand-in for an aligned face crop.
    fn symmetric_crop() -> Vec<u8> {
        let mut px = vec![0u8; 112 * 112 * 3];
        for y in 0..112 {
            for x in 0..112 {
                // Mirror-symmetric about the vertical axis, with enough
                // variation that the crop has a real luminance spread.
                let d = (x as i32 - 56).abs();
                let v = (90 + d * 2).min(255) as u8;
                let i = (y * 112 + x) * 3;
                px[i..i + 3].fill(v);
            }
        }
        px
    }

    /// Blacks out the left third — a hand or another head across one side.
    fn half_covered_crop() -> Vec<u8> {
        let mut px = symmetric_crop();
        for y in 0..112 {
            for x in 0..37 {
                let i = (y * 112 + x) * 3;
                px[i..i + 3].fill(10);
            }
        }
        px
    }

    /// Keypoints already at the canonical positions: a perfectly plausible
    /// frontal face, so the geometry term must contribute nothing.
    fn canonical_kps() -> [[f64; 2]; 5] {
        crate::umeyama::ARCFACE_DST
    }

    /// The same face translated, rotated and scaled. A similarity transform
    /// absorbs all three, so the residual must stay ~0 — otherwise this signal
    /// would just be measuring head position and size.
    fn transformed_kps() -> [[f64; 2]; 5] {
        let (angle, scale, tx, ty) = (0.35f64, 1.7f64, 210.0f64, -40.0f64);
        let (c, s) = (angle.cos() * scale, angle.sin() * scale);
        crate::umeyama::ARCFACE_DST.map(|p| [c * p[0] - s * p[1] + tx, s * p[0] + c * p[1] + ty])
    }

    #[test]
    fn a_clean_symmetric_frontal_face_reads_as_unoccluded() {
        let occ = occlusion_from_crop(
            &symmetric_crop(),
            112,
            &canonical_kps(),
            &FaceMetricsConfig::defaults(),
        );
        assert!(occ < 0.15, "occlusion {occ} should be low");
    }

    #[test]
    fn covering_one_side_of_the_face_raises_occlusion() {
        let cfg = FaceMetricsConfig::defaults();
        let clean = occlusion_from_crop(&symmetric_crop(), 112, &canonical_kps(), &cfg);
        let covered = occlusion_from_crop(&half_covered_crop(), 112, &canonical_kps(), &cfg);
        assert!(
            covered > clean + 0.2,
            "covered {covered} should clearly exceed clean {clean}"
        );
    }

    /// The old signal was ~collinear with detector confidence and with the
    /// face's distance from the frame centre. Neither is an input any more, and
    /// pose/position must not move the score on its own.
    #[test]
    fn moving_rotating_and_scaling_the_face_does_not_change_occlusion() {
        let cfg = FaceMetricsConfig::defaults();
        let a = occlusion_from_crop(&symmetric_crop(), 112, &canonical_kps(), &cfg);
        let b = occlusion_from_crop(&symmetric_crop(), 112, &transformed_kps(), &cfg);
        assert!(
            (a - b).abs() < 1e-6,
            "a similarity transform must be absorbed: {a} vs {b}"
        );
    }

    /// Landmarks that no face could produce — one eye dragged across the
    /// mouth — are the case the geometry term exists for.
    #[test]
    fn implausible_landmark_geometry_raises_occlusion() {
        let cfg = FaceMetricsConfig::defaults();
        let mut kps = canonical_kps();
        kps[0] = [60.0, 95.0];
        let clean = occlusion_from_crop(&symmetric_crop(), 112, &canonical_kps(), &cfg);
        let broken = occlusion_from_crop(&symmetric_crop(), 112, &kps, &cfg);
        assert!(
            broken > clean + 0.3,
            "broken geometry {broken} should clearly exceed clean {clean}"
        );
    }

    /// A blown-out or pitch-black crop has no structure to compare halves of,
    /// and the asymmetry ratio would otherwise divide by ~0.
    #[test]
    fn a_flat_crop_does_not_explode_the_symmetry_term() {
        let cfg = FaceMetricsConfig::defaults();
        for value in [0u8, 128, 255] {
            let occ = occlusion_from_crop(&vec![value; 112 * 112 * 3], 112, &canonical_kps(), &cfg);
            assert!(occ.is_finite() && occ < 0.1, "value {value}: {occ}");
        }
    }
}
