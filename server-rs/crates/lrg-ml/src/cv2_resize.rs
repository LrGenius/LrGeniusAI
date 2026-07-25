//! cv2's `INTER_LINEAR` resize — plain 2-tap bilinear with no
//! antialiasing filter widening on downscale (unlike Pillow's BILINEAR,
//! which widens the kernel support proportionally when downsampling).
//! InsightFace's SCRFD/ArcFace preprocessing goes through
//! `cv2.resize`/`cv2.warpAffine`, so matching cv2's specific algorithm
//! (not Pillow's) is required for bit-compatible detections.

/// `cv2.resize(img, (out_w, out_h))`, INTER_LINEAR, for an interleaved
/// RGB u8 buffer. Source coordinate per cv2: `(dst + 0.5) * scale - 0.5`,
/// clamped, standard 4-tap (2x2) bilinear.
pub fn resize_rgb(pixels: &[u8], in_w: usize, in_h: usize, out_w: usize, out_h: usize) -> Vec<u8> {
    if in_w == out_w && in_h == out_h {
        return pixels.to_vec();
    }
    let scale_x = in_w as f64 / out_w as f64;
    let scale_y = in_h as f64 / out_h as f64;
    let mut out = vec![0u8; out_w * out_h * 3];

    for oy in 0..out_h {
        let sy = ((oy as f64 + 0.5) * scale_y - 0.5).max(0.0);
        let y0 = (sy as usize).min(in_h - 1);
        let y1 = (y0 + 1).min(in_h - 1);
        let fy = (sy - y0 as f64).clamp(0.0, 1.0);

        for ox in 0..out_w {
            let sx = ((ox as f64 + 0.5) * scale_x - 0.5).max(0.0);
            let x0 = (sx as usize).min(in_w - 1);
            let x1 = (x0 + 1).min(in_w - 1);
            let fx = (sx - x0 as f64).clamp(0.0, 1.0);

            for c in 0..3 {
                let p00 = pixels[(y0 * in_w + x0) * 3 + c] as f64;
                let p01 = pixels[(y0 * in_w + x1) * 3 + c] as f64;
                let p10 = pixels[(y1 * in_w + x0) * 3 + c] as f64;
                let p11 = pixels[(y1 * in_w + x1) * 3 + c] as f64;
                let top = p00 * (1.0 - fx) + p01 * fx;
                let bot = p10 * (1.0 - fx) + p11 * fx;
                let v = top * (1.0 - fy) + bot * fy;
                out[(oy * out_w + ox) * 3 + c] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_when_same_size() {
        let px = vec![1, 2, 3, 4, 5, 6];
        assert_eq!(resize_rgb(&px, 2, 1, 2, 1), px);
    }

    #[test]
    fn downscale_averages_neighbors() {
        // 2x1 -> 1x1 should land near the midpoint of the two source pixels.
        let px = vec![0, 0, 0, 200, 200, 200];
        let out = resize_rgb(&px, 2, 1, 1, 1);
        assert!((90..=110).contains(&(out[0] as i32)));
    }
}
