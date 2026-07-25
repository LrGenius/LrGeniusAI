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
}

impl Filter {
    fn support(self) -> f64 {
        match self {
            Filter::Bilinear => 1.0,
            Filter::Lanczos3 => 3.0,
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
    // Horizontal pass first, then vertical (Pillow order).
    let mut data = input.to_vec();
    let mut w = in_w;
    if out_w != in_w {
        let (ksize, bounds, kk) = precompute_coeffs(in_w, out_w, filter);
        let kk = normalize_coeffs_8bpc(&kk);
        data = resample_horizontal(&data, in_w, in_h, out_w, ksize, &bounds, &kk);
        w = out_w;
    }
    if out_h != in_h {
        let (ksize, bounds, kk) = precompute_coeffs(in_h, out_h, filter);
        let kk = normalize_coeffs_8bpc(&kk);
        data = resample_vertical(&data, w, out_h, ksize, &bounds, &kk);
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
