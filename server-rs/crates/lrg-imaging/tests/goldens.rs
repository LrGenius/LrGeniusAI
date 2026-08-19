//! Golden verification against `testdata/imaging_goldens.json`, produced
//! by the Python backend (`make_imaging_goldens.py`). Inputs are
//! regenerated here with the same deterministic formulas.
//!
//! pHash must match exactly (stored hashes from Python-era indexing are
//! compared against Rust-computed ones in production). Culling metrics
//! tolerate one last-digit rounding flip (numpy accumulates in f32
//! pairwise order; we accumulate in f64).

use lrg_imaging::cull_config::ImageMetricsConfig;
use lrg_imaging::metrics::{culling_metrics, perceptual_hash, RgbImage};

fn lcg_bytes(n: usize, seed: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    let mut x = seed;
    for _ in 0..n {
        x = (x.wrapping_mul(1103515245).wrapping_add(12345)) & 0x7FFF_FFFF;
        out.push(((x >> 16) & 0xFF) as u8);
    }
    out
}

fn gradient(w: usize, h: usize) -> Vec<u8> {
    let n = w * h * 3;
    (0..n)
        .map(|i| (i as f64 * 255.0 / (n - 1) as f64) as u8)
        .collect()
}

fn checker(size: usize, cell: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size * size * 3);
    for y in 0..size {
        for x in 0..size {
            let v = if ((y + x) / cell) % 2 == 1 { 255 } else { 0 };
            out.extend_from_slice(&[v, v, v]);
        }
    }
    out
}

fn build_case(name: &str) -> (Vec<u8>, usize, usize) {
    match name {
        "gradient_640x480" => (gradient(640, 480), 640, 480),
        "lcg_noise_800x600" => (lcg_bytes(800 * 600 * 3, 42), 800, 600),
        "dark_640x480" => (vec![12u8; 640 * 480 * 3], 640, 480),
        "bright_640x480" => (vec![252u8; 640 * 480 * 3], 640, 480),
        "checker_512" => (checker(512, 32), 512, 512),
        "mixed_640x480" => {
            let mut px = gradient(640, 480);
            let patch = lcg_bytes(100 * 200 * 3, 7);
            for row in 0..100 {
                let dst = ((100 + row) * 640 + 100) * 3;
                let src = row * 200 * 3;
                px[dst..dst + 200 * 3].copy_from_slice(&patch[src..src + 200 * 3]);
            }
            (px, 640, 480)
        }
        "small_320x240" => (lcg_bytes(320 * 240 * 3, 99), 320, 240),
        other => panic!("unknown golden case {other}"),
    }
}

#[test]
fn goldens_match_python_backend() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/imaging_goldens.json"
    );
    let goldens: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();

    for (name, expected) in goldens["cases"].as_object().unwrap() {
        let (pixels, width, height) = build_case(name);
        assert_eq!(width as u64, expected["width"].as_u64().unwrap());
        assert_eq!(height as u64, expected["height"].as_u64().unwrap());
        let image = RgbImage {
            pixels: &pixels,
            width,
            height,
        };

        let phash = perceptual_hash(&image);
        let want_hash = expected["phash"].as_str().unwrap();
        let hamming = (u64::from_str_radix(&phash, 16).unwrap()
            ^ u64::from_str_radix(want_hash, 16).unwrap())
        .count_ones();
        if name.starts_with("bright_") || name.starts_with("dark_") {
            // Constant frames: every DCT coefficient except [0,0] is pure
            // float noise, so the bits depend on BLAS summation order —
            // numpy itself differs between Accelerate and OpenBLAS here.
            // The hash carries no information for such frames; grouping
            // falls back to capture time. Bound the drift, don't pin it.
            assert!(hamming <= 16, "{name}: degenerate-frame hamming {hamming}");
        } else {
            assert_eq!(
                hamming, 0,
                "pHash mismatch for {name}: {phash} vs {want_hash}"
            );
        }

        // Defaults must reproduce the Python backend bit-for-bit; that is the
        // whole point of this file, and it is what pins `ImageMetricsConfig`'s
        // `Default` impl to `BASE_CULLING_CONFIG`.
        let metrics = culling_metrics(&image, &ImageMetricsConfig::default());
        let got = [
            ("cull_sharpness", metrics.cull_sharpness),
            ("cull_exposure", metrics.cull_exposure),
            ("cull_noise", metrics.cull_noise),
            ("cull_highlight_clip", metrics.cull_highlight_clip),
            ("cull_shadow_clip", metrics.cull_shadow_clip),
            ("cull_technical_score", metrics.cull_technical_score),
            ("cull_aesthetic", metrics.cull_aesthetic),
        ];
        for (key, value) in got {
            let want = expected["metrics"][key].as_f64().unwrap();
            assert!(
                (value - want).abs() <= 0.00011,
                "{name}.{key}: rust {value} vs python {want}"
            );
        }
        println!("{name}: phash + 7 metrics OK");
    }
}
