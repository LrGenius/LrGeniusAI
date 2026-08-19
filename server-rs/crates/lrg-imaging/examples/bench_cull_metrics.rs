//! Where per-photo time goes in the cull ingest path, once the SigLIP2
//! embedding is out of the picture.
//!
//! ```text
//! cargo run --release -p lrg-imaging --example bench_cull_metrics [iters]
//! ```
//!
//! Runs against a synthetic 2048px image, which is the working resolution the
//! plugin normalizes to (`GENIUSAI_CONVERT_MAX_EDGE`).

use std::time::Instant;

use lrg_imaging::cull_config::ImageMetricsConfig;
use lrg_imaging::metrics::{culling_metrics, perceptual_hash, RgbImage};

/// Deterministic noise with structure, so sharpness and colourfulness are not
/// degenerate.
fn synth(width: usize, height: usize) -> Vec<u8> {
    let mut px = vec![0u8; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 3;
            let fx = x as f32 / width as f32;
            let fy = y as f32 / height as f32;
            let edge = if (x / 37 + y / 53) % 2 == 0 {
                40.0
            } else {
                0.0
            };
            px[i] = (128.0 + 100.0 * (fx * 19.0).sin() + edge) as u8;
            px[i + 1] = (128.0 + 90.0 * (fy * 23.0).cos()) as u8;
            px[i + 2] = (128.0 + 80.0 * ((fx + fy) * 31.0).sin() - edge) as u8;
        }
    }
    px
}

fn time_it(label: &str, iters: u32, mut f: impl FnMut()) -> f64 {
    // One warm-up pass so allocator behaviour is not attributed to the metric.
    f();
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    let per = t0.elapsed().as_secs_f64() / iters as f64;
    println!("{label:<28} {:>9.2} ms/photo", per * 1000.0);
    per
}

fn main() {
    let iters: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    let (w, h) = (2048, 1365);
    let pixels = synth(w, h);
    let img = RgbImage {
        pixels: &pixels,
        width: w,
        height: h,
    };
    let cfg = ImageMetricsConfig::defaults();

    println!("{w}x{h} RGB8, {iters} iterations\n");
    let metrics = time_it("culling_metrics", iters, || {
        std::hint::black_box(culling_metrics(&img, &cfg));
    });
    let phash = time_it("perceptual_hash", iters, || {
        std::hint::black_box(perceptual_hash(&img));
    });
    println!(
        "{:<28} {:>9.2} ms/photo",
        "TOTAL",
        (metrics + phash) * 1000.0
    );
}
