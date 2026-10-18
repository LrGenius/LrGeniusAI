//! Bit-exact port of Pillow's `Resample.c` fixed-point resampling for
//! 8-bit planes (horizontal pass then vertical pass, INT32 coefficients
//! with PRECISION_BITS = 22), plus Pillow's RGB->L conversion.
//!
//! Bit-exactness matters: pHash values stored by the Python backend must
//! match hashes computed by this code on the same pixels, otherwise
//! burst/duplicate grouping drifts after the rewrite.

const PRECISION_BITS: i32 = 32 - 8 - 2;

#[derive(Clone, Copy, PartialEq)]
pub enum Filter {
    /// Triangle filter, support 1.0 (PIL BILINEAR).
    Bilinear,
    /// Lanczos windowed sinc, support 3.0 (PIL LANCZOS / ANTIALIAS).
    Lanczos3,
    /// Cubic convolution (a = -0.5), support 2.0 (PIL BICUBIC). Used for
    /// open_clip's "squash" preprocessing resize.
    Bicubic,
}

impl Filter {
    fn support(self) -> f64 {
        match self {
            Filter::Bilinear => 1.0,
            Filter::Lanczos3 => 3.0,
            Filter::Bicubic => 2.0,
        }
    }

    fn eval(self, x: f64) -> f64 {
        match self {
            Filter::Bilinear => {
                let x = x.abs();
                if x < 1.0 {
                    1.0 - x
                } else {
                    0.0
                }
            }
            Filter::Lanczos3 => {
                fn sinc(x: f64) -> f64 {
                    if x == 0.0 {
                        1.0
                    } else {
                        let px = std::f64::consts::PI * x;
                        px.sin() / px
                    }
                }
                if x.abs() < 3.0 {
                    sinc(x) * sinc(x / 3.0)
                } else {
                    0.0
                }
            }
            Filter::Bicubic => {
                const A: f64 = -0.5;
                let x = x.abs();
                if x < 1.0 {
                    ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0
                } else if x < 2.0 {
                    (((x - 5.0) * x + 8.0) * x - 4.0) * A
                } else {
                    0.0
                }
            }
        }
    }
}

/// Pillow `precompute_coeffs`: per output pixel, the input window
/// [xmin, xmin+ksize) and normalized double coefficients.
fn precompute_coeffs(
    in_size: usize,
    out_size: usize,
    filter: Filter,
) -> (usize, Vec<i32>, Vec<f64>) {
    let scale = in_size as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = filter.support() * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;

    let mut bounds = Vec::with_capacity(out_size * 2);
    let mut kk = vec![0.0f64; out_size * ksize];

    for xx in 0..out_size {
        let center = (xx as f64 + 0.5) * scale;
        let mut ww = 0.0f64;
        let ss = 1.0 / filterscale;
        // Round the value.
        let mut xmin = (center - support + 0.5) as i64;
        if xmin < 0 {
            xmin = 0;
        }
        // Round the value.
        let mut xmax = (center + support + 0.5) as i64;
        if xmax > in_size as i64 {
            xmax = in_size as i64;
        }
        let xmax = (xmax - xmin) as usize;
        let k = &mut kk[xx * ksize..(xx + 1) * ksize];
        for (x, kv) in k.iter_mut().enumerate().take(xmax) {
            let w = filter.eval((xmin as f64 + x as f64 - center + 0.5) * ss);
            *kv = w;
            ww += w;
        }
        for kv in k.iter_mut().take(xmax) {
            if ww != 0.0 {
                *kv /= ww;
            }
        }
        // Remaining values should stay empty if they are used despite xmax.
        for kv in k.iter_mut().take(ksize).skip(xmax) {
            *kv = 0.0;
        }
        bounds.push(xmin as i32);
        bounds.push(xmax as i32);
    }
    (ksize, bounds, kk)
}

/// The resampling as a dense `out_size x in_size` weight matrix, row-major.
///
/// Row `i` holds the weights output sample `i` takes from each input sample —
/// the same coefficients [`resize_plane_f32`] applies, laid out so a caller can
/// resize along one axis with a matrix product instead. SigLIP2 NaFlex needs
/// exactly that: its position-embedding grid is resized per image with
/// torch's `F.interpolate(bilinear, antialias=True)`, which is separable and
/// uses these kernels, and the exported graph does the resize as two matrix
/// products against matrices built here (see `lrg_ml::naflex_pre`).
pub fn resample_matrix(in_size: usize, out_size: usize, filter: Filter) -> Vec<f32> {
    let (ksize, bounds, kk) = precompute_coeffs(in_size, out_size, filter);
    let mut out = vec![0.0f32; out_size * in_size];
    for row in 0..out_size {
        let xmin = bounds[row * 2] as usize;
        let xlen = bounds[row * 2 + 1] as usize;
        for x in 0..xlen {
            out[row * in_size + xmin + x] = kk[row * ksize + x] as f32;
        }
    }
    out
}

/// Pillow `normalize_coeffs_8bpc`.
fn normalize_coeffs_8bpc(kk: &[f64]) -> Vec<i32> {
    kk.iter()
        .map(|&v| {
            if v < 0.0 {
                (-0.5 + v * (1i64 << PRECISION_BITS) as f64) as i32
            } else {
                (0.5 + v * (1i64 << PRECISION_BITS) as f64) as i32
            }
        })
        .collect()
}

#[inline]
fn clip8(v: i32) -> u8 {
    let v = v >> PRECISION_BITS;
    v.clamp(0, 255) as u8
}

fn resample_horizontal(
    input: &[u8],
    in_w: usize,
    height: usize,
    out_w: usize,
    ksize: usize,
    bounds: &[i32],
    kk: &[i32],
) -> Vec<u8> {
    let mut out = vec![0u8; out_w * height];
    for yy in 0..height {
        let row = &input[yy * in_w..(yy + 1) * in_w];
        for xx in 0..out_w {
            let xmin = bounds[xx * 2] as usize;
            let xmax = bounds[xx * 2 + 1] as usize;
            let k = &kk[xx * ksize..];
            let mut ss0: i32 = 1 << (PRECISION_BITS - 1);
            for x in 0..xmax {
                ss0 += (row[xmin + x] as i32) * k[x];
            }
            out[yy * out_w + xx] = clip8(ss0);
        }
    }
    out
}

fn resample_vertical(
    input: &[u8],
    width: usize,
    out_h: usize,
    ksize: usize,
    bounds: &[i32],
    kk: &[i32],
) -> Vec<u8> {
    let mut out = vec![0u8; width * out_h];
    for yy in 0..out_h {
        let ymin = bounds[yy * 2] as usize;
        let ymax = bounds[yy * 2 + 1] as usize;
        let k = &kk[yy * ksize..];
        for xx in 0..width {
            let mut ss0: i32 = 1 << (PRECISION_BITS - 1);
            for y in 0..ymax {
                ss0 += (input[(ymin + y) * width + xx] as i32) * k[y];
            }
            out[yy * width + xx] = clip8(ss0);
        }
    }
    out
}

/// Resize a single 8-bit plane exactly like Pillow's `im.resize(...)`.
pub fn resize_plane(
    input: &[u8],
    in_w: usize,
    in_h: usize,
    out_w: usize,
    out_h: usize,
    filter: Filter,
) -> Vec<u8> {
    assert_eq!(input.len(), in_w * in_h);
    // Pillow's pass order (`ImagingResampleInner`, Resample.c): horizontal
    // first, unless the height shrinks by more than twice what the width does,
    // in which case vertical goes first to make the second pass cheaper. The
    // 8-bit path rounds after every pass, so the order changes the output.
    let height_drop = in_h as i64 - out_h as i64;
    let width_drop = in_w as i64 - out_w as i64;
    let horizontal_first = !(height_drop > 0 && height_drop > width_drop * 2);

    let horizontal = |data: Vec<u8>, h: usize| {
        if out_w == in_w {
            return data;
        }
        let (ksize, bounds, kk) = precompute_coeffs(in_w, out_w, filter);
        let kk = normalize_coeffs_8bpc(&kk);
        resample_horizontal(&data, in_w, h, out_w, ksize, &bounds, &kk)
    };
    let vertical = |data: Vec<u8>, w: usize| {
        if out_h == in_h {
            return data;
        }
        let (ksize, bounds, kk) = precompute_coeffs(in_h, out_h, filter);
        let kk = normalize_coeffs_8bpc(&kk);
        resample_vertical(&data, w, out_h, ksize, &bounds, &kk)
    };

    let data = input.to_vec();
    if horizontal_first {
        let data = horizontal(data, in_h);
        vertical(data, out_w)
    } else {
        let data = vertical(data, in_w);
        horizontal(data, out_h)
    }
}

fn resample_horizontal_f32(
    input: &[f32],
    in_w: usize,
    height: usize,
    out_w: usize,
    ksize: usize,
    bounds: &[i32],
    kk: &[f64],
) -> Vec<f32> {
    let mut out = vec![0.0f32; out_w * height];
    for yy in 0..height {
        let row = &input[yy * in_w..(yy + 1) * in_w];
        for xx in 0..out_w {
            let xmin = bounds[xx * 2] as usize;
            let xmax = bounds[xx * 2 + 1] as usize;
            let k = &kk[xx * ksize..];
            let mut ss = 0.0f64;
            for x in 0..xmax {
                ss += (row[xmin + x] as f64) * k[x];
            }
            out[yy * out_w + xx] = ss as f32;
        }
    }
    out
}

fn resample_vertical_f32(
    input: &[f32],
    width: usize,
    out_h: usize,
    ksize: usize,
    bounds: &[i32],
    kk: &[f64],
) -> Vec<f32> {
    let mut out = vec![0.0f32; width * out_h];
    for yy in 0..out_h {
        let ymin = bounds[yy * 2] as usize;
        let ymax = bounds[yy * 2 + 1] as usize;
        let k = &kk[yy * ksize..];
        for xx in 0..width {
            let mut ss = 0.0f64;
            for y in 0..ymax {
                ss += (input[(ymin + y) * width + xx] as f64) * k[y];
            }
            out[yy * width + xx] = ss as f32;
        }
    }
    out
}

/// Same resampling as [`resize_plane`], but in the float domain.
///
/// [`resize_plane`] is a bit-exact port of Pillow's *8-bit* path, which
/// rounds every intermediate to `u8` (`normalize_coeffs_8bpc` + `clip8`).
/// That is correct for anything comparing against Pillow — pHash, the SigLIP2
/// preprocessing — but wrong for a transform that resizes *after*
/// `ToTensor()`, i.e. on floats already scaled to `[0, 1]`. BioCLIP's
/// transform is exactly that case (`bioclip_pre`), so it uses this instead
/// and skips the round-trip through `u8`.
///
/// The filter kernels and coefficient windows are shared with the 8-bit path,
/// which is what makes this match torchvision's `antialias=True` resize:
/// torchvision widens the kernel support by the downscale factor the same way
/// Pillow does. It is close, not bit-exact — `lrg-ml`'s BioCLIP goldens are
/// what actually pin the residual down.
pub fn resize_plane_f32(
    input: &[f32],
    in_w: usize,
    in_h: usize,
    out_w: usize,
    out_h: usize,
    filter: Filter,
) -> Vec<f32> {
    assert_eq!(input.len(), in_w * in_h);
    let mut data = input.to_vec();
    let mut w = in_w;
    if out_w != in_w {
        let (ksize, bounds, kk) = precompute_coeffs(in_w, out_w, filter);
        data = resample_horizontal_f32(&data, in_w, in_h, out_w, ksize, &bounds, &kk);
        w = out_w;
    }
    if out_h != in_h {
        let (ksize, bounds, kk) = precompute_coeffs(in_h, out_h, filter);
        data = resample_vertical_f32(&data, w, out_h, ksize, &bounds, &kk);
    }
    data
}

/// Pillow RGB -> L: `L = (R*19595 + G*38470 + B*7471 + 0x8000) >> 16`.
pub fn rgb_to_luma(rgb: &[u8], pixels: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels);
    for i in 0..pixels {
        let r = rgb[i * 3] as u32;
        let g = rgb[i * 3 + 1] as u32;
        let b = rgb[i * 3 + 2] as u32;
        out.push(((r * 19595 + g * 38470 + b * 7471 + 0x8000) >> 16) as u8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The matrix must be the same resize as [`resize_plane_f32`], both up and
    /// down: a 1-pixel-wide column resized either way has to agree.
    #[test]
    fn resample_matrix_matches_resize_plane_f32() {
        let column: Vec<f32> = (0..16).map(|i| (i as f32 * 0.37).sin()).collect();
        for out_len in [5, 16, 23, 40] {
            let expected = resize_plane_f32(&column, 1, 16, 1, out_len, Filter::Bilinear);
            let m = resample_matrix(16, out_len, Filter::Bilinear);
            for (row, want) in expected.iter().enumerate() {
                let got: f32 = (0..16).map(|j| m[row * 16 + j] * column[j]).sum();
                assert!((got - want).abs() < 1e-5, "out_len {out_len} row {row}");
            }
            for row in 0..out_len {
                let sum: f32 = m[row * 16..(row + 1) * 16].iter().sum();
                assert!((sum - 1.0).abs() < 1e-5);
            }
        }
    }

    /// Pillow runs the vertical pass first when the height drops by more than
    /// twice what the width does. Each pass rounds to `u8`, so the order is
    /// visible in the output; this pins both orders to the 1-D passes.
    #[test]
    fn pass_order_follows_pillow() {
        let (w, h) = (30usize, 90usize);
        let mut x = 7u64;
        let input: Vec<u8> = (0..w * h)
            .map(|_| {
                x = x.wrapping_mul(1103515245).wrapping_add(12345) & 0x7FFF_FFFF;
                ((x >> 16) & 0xFF) as u8
            })
            .collect();
        let f = Filter::Bilinear;

        // Height drops 90 -> 60 (30) against width 30 -> 22 (8 * 2 = 16): vertical first.
        let vh = resize_plane(&resize_plane(&input, w, h, w, 60, f), w, 60, 22, 60, f);
        assert_eq!(resize_plane(&input, w, h, 22, 60, f), vh);

        // Height drops 90 -> 80 (10) against width 30 -> 10 (20 * 2 = 40): horizontal first.
        let hv = resize_plane(&resize_plane(&input, w, h, 10, h, f), 10, h, 10, 80, f);
        assert_eq!(resize_plane(&input, w, h, 10, 80, f), hv);
    }
}
