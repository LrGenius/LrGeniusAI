//! Similarity transform estimation (Umeyama, no reflection allowed via
//! the `det(A) < 0` sign-flip) + `cv2.warpAffine`-equivalent sampling,
//! porting `insightface.utils.face_align` (`estimate_norm` + `norm_crop`,
//! backed by skimage's `SimilarityTransform.estimate`).
//!
//! Verified against real InsightFace output: see the unit test comparing
//! this implementation's affine matrix to skimage's for real detected
//! landmarks (bit-for-bit to 1e-9).

/// ArcFace reference landmarks for a 112x112 aligned crop
/// (`insightface.utils.face_align.arcface_dst`).
pub const ARCFACE_DST: [[f64; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

fn mean5(pts: &[[f64; 2]; 5]) -> [f64; 2] {
    let sx: f64 = pts.iter().map(|p| p[0]).sum();
    let sy: f64 = pts.iter().map(|p| p[1]).sum();
    [sx / 5.0, sy / 5.0]
}

/// Closed-form 2x2 similarity-transform estimation (Umeyama), matching
/// skimage's `SimilarityTransform.estimate(src, dst)` for the full-rank
/// (non-degenerate) case, which 5 real facial landmarks always are.
/// Returns the 2x3 affine matrix `[[a, b, tx], [c, d, ty]]` mapping
/// `src` points onto `dst` points.
pub fn estimate_similarity(src: &[[f64; 2]; 5], dst: &[[f64; 2]; 5]) -> [[f64; 3]; 2] {
    let src_mean = mean5(src);
    let dst_mean = mean5(dst);
    let src_d: Vec<[f64; 2]> = src
        .iter()
        .map(|p| [p[0] - src_mean[0], p[1] - src_mean[1]])
        .collect();
    let dst_d: Vec<[f64; 2]> = dst
        .iter()
        .map(|p| [p[0] - dst_mean[0], p[1] - dst_mean[1]])
        .collect();

    // A = dst_demean^T @ src_demean / n
    let n = 5.0f64;
    let mut a = [[0.0; 2]; 2];
    for i in 0..5 {
        a[0][0] += dst_d[i][0] * src_d[i][0];
        a[0][1] += dst_d[i][0] * src_d[i][1];
        a[1][0] += dst_d[i][1] * src_d[i][0];
        a[1][1] += dst_d[i][1] * src_d[i][1];
    }
    for row in a.iter_mut() {
        for v in row.iter_mut() {
            *v /= n;
        }
    }

    let det_a = a[0][0] * a[1][1] - a[0][1] * a[1][0];
    let d = if det_a < 0.0 { [1.0, -1.0] } else { [1.0, 1.0] };

    // SVD of A via eigendecomposition of the symmetric A^T A.
    let ata = [
        [
            a[0][0] * a[0][0] + a[1][0] * a[1][0],
            a[0][0] * a[0][1] + a[1][0] * a[1][1],
        ],
        [0.0, a[0][1] * a[0][1] + a[1][1] * a[1][1]],
    ];
    let m01 = ata[0][1];
    let trace = ata[0][0] + ata[1][1];
    let diff = ata[0][0] - ata[1][1];
    let disc = ((diff / 2.0).powi(2) + m01 * m01).sqrt();
    let eig1 = trace / 2.0 + disc;
    let eig2 = trace / 2.0 - disc;

    let v1 = if m01.abs() > 1e-12 {
        let raw = [eig1 - ata[1][1], m01];
        let norm = (raw[0] * raw[0] + raw[1] * raw[1]).sqrt();
        [raw[0] / norm, raw[1] / norm]
    } else if ata[0][0] >= ata[1][1] {
        [1.0, 0.0]
    } else {
        [0.0, 1.0]
    };
    let v2 = [-v1[1], v1[0]];

    let s1 = eig1.max(0.0).sqrt();
    let s2 = eig2.max(0.0).sqrt();

    let apply = |m: &[[f64; 2]; 2], v: [f64; 2]| {
        [
            m[0][0] * v[0] + m[0][1] * v[1],
            m[1][0] * v[0] + m[1][1] * v[1],
        ]
    };
    let norm2 = |v: [f64; 2]| (v[0] * v[0] + v[1] * v[1]).sqrt();

    let raw_u1 = apply(&a, v1);
    let n1 = norm2(raw_u1);
    let u1 = if n1 > 1e-9 {
        [raw_u1[0] / n1, raw_u1[1] / n1]
    } else {
        [1.0, 0.0]
    };
    let raw_u2 = apply(&a, v2);
    let n2 = norm2(raw_u2);
    let u2 = if n2 > 1e-9 {
        [raw_u2[0] / n2, raw_u2[1] / n2]
    } else {
        [-u1[1], u1[0]]
    };

    // R = U @ diag(d) @ V^T
    let ud = [[u1[0] * d[0], u2[0] * d[1]], [u1[1] * d[0], u2[1] * d[1]]];
    let vt = [[v1[0], v1[1]], [v2[0], v2[1]]];
    let r = [
        [
            ud[0][0] * vt[0][0] + ud[0][1] * vt[1][0],
            ud[0][0] * vt[0][1] + ud[0][1] * vt[1][1],
        ],
        [
            ud[1][0] * vt[0][0] + ud[1][1] * vt[1][0],
            ud[1][0] * vt[0][1] + ud[1][1] * vt[1][1],
        ],
    ];

    let var_sum: f64 = src_d.iter().map(|p| p[0] * p[0] + p[1] * p[1]).sum::<f64>() / n;
    let scale = if var_sum > 1e-12 {
        (s1 * d[0] + s2 * d[1]) / var_sum
    } else {
        1.0
    };

    let r_src_mean = apply(&r, src_mean);
    let t = [
        dst_mean[0] - scale * r_src_mean[0],
        dst_mean[1] - scale * r_src_mean[1],
    ];

    [
        [scale * r[0][0], scale * r[0][1], t[0]],
        [scale * r[1][0], scale * r[1][1], t[1]],
    ]
}

/// Invert a 2x3 affine matrix (as a 3x3 with an implicit `[0,0,1]` row).
fn invert_affine(m: [[f64; 3]; 2]) -> [[f64; 3]; 2] {
    let (a, b, tx) = (m[0][0], m[0][1], m[0][2]);
    let (c, d, ty) = (m[1][0], m[1][1], m[1][2]);
    let det = a * d - b * c;
    let ia = d / det;
    let ib = -b / det;
    let ic = -c / det;
    let id = a / det;
    let itx = -(ia * tx + ib * ty);
    let ity = -(ic * tx + id * ty);
    [[ia, ib, itx], [ic, id, ity]]
}

/// `cv2.warpAffine(img, m, (out_size, out_size), borderValue=0)`:
/// inverse-map each output pixel through `m`, bilinear sample with
/// per-tap `BORDER_CONSTANT` (0) for out-of-bounds neighbors.
pub fn warp_affine_rgb(
    pixels: &[u8],
    width: usize,
    height: usize,
    m: [[f64; 3]; 2],
    out_size: usize,
) -> Vec<u8> {
    let inv = invert_affine(m);
    let mut out = vec![0u8; out_size * out_size * 3];

    let sample = |x: i64, y: i64, c: usize| -> f64 {
        if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
            0.0
        } else {
            pixels[(y as usize * width + x as usize) * 3 + c] as f64
        }
    };

    for oy in 0..out_size {
        for ox in 0..out_size {
            let (dx, dy) = (ox as f64, oy as f64);
            let sx = inv[0][0] * dx + inv[0][1] * dy + inv[0][2];
            let sy = inv[1][0] * dx + inv[1][1] * dy + inv[1][2];

            let x0 = sx.floor() as i64;
            let y0 = sy.floor() as i64;
            let fx = sx - x0 as f64;
            let fy = sy - y0 as f64;

            for c in 0..3 {
                let p00 = sample(x0, y0, c);
                let p01 = sample(x0 + 1, y0, c);
                let p10 = sample(x0, y0 + 1, c);
                let p11 = sample(x0 + 1, y0 + 1, c);
                let top = p00 * (1.0 - fx) + p01 * fx;
                let bot = p10 * (1.0 - fx) + p11 * fx;
                let v = top * (1.0 - fy) + bot * fy;
                out[(oy * out_size + ox) * 3 + c] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Port of `face_align.norm_crop` for `image_size=112`: align 5 landmarks to
/// `ARCFACE_DST` and warp.
///
/// Still the canonical crop even though ArcFace itself is gone: the occlusion
/// measure in [`crate::face_quality`] is a mirror-symmetry test, which is only
/// meaningful on a face already rotated and scaled onto this template, and its
/// tuned thresholds are calibrated against exactly this framing. Keeping it
/// byte-for-byte is what makes the detector/recognizer swap leave every
/// culling score untouched.
pub fn norm_crop_112(pixels: &[u8], width: usize, height: usize, kps: &[[f64; 2]; 5]) -> Vec<u8> {
    let m = estimate_similarity(kps, &ARCFACE_DST);
    warp_affine_rgb(pixels, width, height, m, 112)
}

/// How much wider than the ArcFace framing FaceNet's crop is.
///
/// ArcFace was trained on tight `ARCFACE_DST` crops; FaceNet's VGGFace2
/// weights were trained on MTCNN boxes grown by a margin (44px on a 182px
/// crop in the reference `align_dataset_mtcnn.py` pipeline, ~1.25x), which
/// include forehead, chin and some hair. Feeding it the tight crop would be a
/// train/test framing mismatch, so the template is scaled down inside the
/// larger canvas by this factor, which shows the same extra context while
/// keeping the eye/nose/mouth geometry canonical.
///
/// A starting value derived from that reference pipeline, not a tuned one —
/// `testdata/make_face_goldens.py` reports intra- vs inter-person cosine
/// separation, which is the measurement to re-run if this is ever changed.
const FACENET_ZOOM_OUT: f64 = 1.25;

/// `ARCFACE_DST` mapped into a `size`x`size` canvas, scaled about its own
/// centre by `1 / FACENET_ZOOM_OUT`.
fn facenet_template(size: usize) -> [[f64; 2]; 5] {
    // The template's design canvas, whose centre the scaling is about.
    const SRC_SIZE: f64 = 112.0;
    let scale = (size as f64 / SRC_SIZE) / FACENET_ZOOM_OUT;
    let (src_c, dst_c) = (SRC_SIZE / 2.0, size as f64 / 2.0);
    ARCFACE_DST.map(|p| {
        [
            (p[0] - src_c) * scale + dst_c,
            (p[1] - src_c) * scale + dst_c,
        ]
    })
}

/// Align 5 landmarks onto a `size`x`size` canvas framed for FaceNet and warp.
pub fn norm_crop_facenet(
    pixels: &[u8],
    width: usize,
    height: usize,
    kps: &[[f64; 2]; 5],
    size: usize,
) -> Vec<u8> {
    let m = estimate_similarity(kps, &facenet_template(size));
    warp_affine_rgb(pixels, width, height, m, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_skimage_similarity_transform_for_real_landmarks() {
        // Real detected keypoints from InsightFace on a test photo, and
        // skimage's estimate_norm output for the same points.
        let kps: [[f64; 2]; 5] = [
            [204.37020874023438, 100.95319366455078],
            [247.24343872070312, 102.92520904541016],
            [224.07980346679688, 128.7403106689453],
            [202.8379364013672, 139.07981872558594],
            [244.8367919921875, 141.070068359375],
        ];
        let expected = [
            [
                0.8810885093238813,
                0.048312563138915285,
                -147.85207697119782,
            ],
            [-0.048312563138915285, 0.8810885093238813, -25.2253412787892],
        ];
        let got = estimate_similarity(&kps, &ARCFACE_DST);
        // Tolerance covers float64 rounding differences between this
        // closed-form 2x2 eigendecomposition and numpy/LAPACK's general
        // SVD; 1e-4 is far below any pixel-alignment-relevant scale.
        for i in 0..2 {
            for j in 0..3 {
                assert!(
                    (got[i][j] - expected[i][j]).abs() < 1e-4,
                    "M[{i}][{j}]: {} vs {}",
                    got[i][j],
                    expected[i][j]
                );
            }
        }
    }

    #[test]
    fn identity_transform_for_dst_equal_src() {
        let m = estimate_similarity(&ARCFACE_DST, &ARCFACE_DST);
        assert!((m[0][0] - 1.0).abs() < 1e-9);
        assert!(m[0][1].abs() < 1e-9);
        assert!(m[0][2].abs() < 1e-9);
        assert!(m[1][0].abs() < 1e-9);
        assert!((m[1][1] - 1.0).abs() < 1e-9);
        assert!(m[1][2].abs() < 1e-9);
    }

    /// The FaceNet template must stay centred and be strictly *smaller*
    /// relative to its canvas than the ArcFace one — that smaller footprint is
    /// the extra context FaceNet's training framing expects. Scaling about the
    /// origin instead of the centre would also shrink it, while sliding the
    /// face into the top-left corner, so the centring is asserted too.
    #[test]
    fn facenet_template_is_centred_and_zoomed_out() {
        let t = facenet_template(160);

        let cx: f64 = t.iter().map(|p| p[0]).sum::<f64>() / 5.0;
        let arc_cx: f64 = ARCFACE_DST.iter().map(|p| p[0]).sum::<f64>() / 5.0;
        // ARCFACE_DST's own centroid is slightly off-centre in its 112 canvas;
        // the mapping preserves that offset, scaled.
        let scale = (160.0 / 112.0) / FACENET_ZOOM_OUT;
        assert!((cx - ((arc_cx - 56.0) * scale + 80.0)).abs() < 1e-9);

        let arc_eye_span = ARCFACE_DST[1][0] - ARCFACE_DST[0][0];
        let eye_span = t[1][0] - t[0][0];
        assert!(
            eye_span / 160.0 < arc_eye_span / 112.0,
            "FaceNet crop must show more context than the ArcFace one"
        );
        assert!((eye_span - arc_eye_span * scale).abs() < 1e-9);
    }
}
