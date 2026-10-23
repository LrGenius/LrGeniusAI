//! `naflex_pre` against transformers' own `Siglip2ImageProcessorPil` and
//! torch's `F.interpolate` (`testdata/naflex_pre_goldens.json`, from
//! `testdata/make_naflex_pre_goldens.py`).
//!
//! Needs no model files, so unlike `goldens.rs` it always runs: the
//! preprocessing is where a silent mismatch would hide, because the model
//! still returns a plausible embedding for slightly wrong patches.

use lrg_ml::naflex_pre::{self, PATCH_DIM, POSITION_GRID};

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

fn build_image(name: &str) -> (Vec<u8>, usize, usize) {
    match name {
        "gradient_640x480" => (gradient(640, 480), 640, 480),
        "lcg_noise_800x600" => (lcg_bytes(800 * 600 * 3, 42), 800, 600),
        "small_320x240" => (lcg_bytes(320 * 240 * 3, 99), 320, 240),
        "portrait_300x900" => (lcg_bytes(300 * 900 * 3, 7), 300, 900),
        "panorama_1200x200" => (gradient(1200, 200), 1200, 200),
        other => panic!("unknown golden image {other}"),
    }
}

fn goldens() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/naflex_pre_goldens.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn floats(v: &serde_json::Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

#[test]
fn patches_match_transformers() {
    let goldens = goldens();
    let max = goldens["max_num_patches"].as_u64().unwrap() as usize;
    assert_eq!(max, naflex_pre::MAX_NUM_PATCHES);

    for (name, want) in goldens["images"].as_object().unwrap() {
        let (pixels, width, height) = build_image(name);
        let got = naflex_pre::preprocess(&pixels, width, height, max);

        assert_eq!(
            got.grid_h as u64,
            want["grid_h"].as_u64().unwrap(),
            "{name}: grid_h"
        );
        assert_eq!(
            got.grid_w as u64,
            want["grid_w"].as_u64().unwrap(),
            "{name}: grid_w"
        );

        let n = got.num_patches();
        let real = &got.pixel_values[..n * PATCH_DIM];
        let sum: f64 = real.iter().map(|v| *v as f64).sum();
        let sum_sq: f64 = real.iter().map(|v| (*v as f64) * (*v as f64)).sum();
        // Bit-exact resize, so only float summation order separates the two.
        let tol = 1e-6 * real.len() as f64;
        assert!(
            (sum - want["sum"].as_f64().unwrap()).abs() < tol,
            "{name}: sum {sum}"
        );
        assert!(
            (sum_sq - want["sum_sq"].as_f64().unwrap()).abs() < tol,
            "{name}: sum_sq {sum_sq}"
        );

        for (row, values) in want["rows"].as_object().unwrap() {
            let row: usize = row.parse().unwrap();
            for (i, w) in floats(values).iter().enumerate() {
                let g = got.pixel_values[row * PATCH_DIM + i] as f64;
                assert!(
                    (g - w).abs() < 1e-6,
                    "{name}: patch {row} value {i}: {g} vs {w}"
                );
            }
        }
    }
}

/// `rows @ grid @ cols^T` per channel must be torch's antialiased bilinear
/// resize — this is what the exported graph computes in place of it.
#[test]
fn position_interpolation_matches_torch() {
    let goldens = goldens();
    let grid = floats(&goldens["position_grid"]);
    let channels = grid.len() / (POSITION_GRID * POSITION_GRID);

    for case in goldens["position_interpolations"].as_array().unwrap() {
        let gh = case["grid_h"].as_u64().unwrap() as usize;
        let gw = case["grid_w"].as_u64().unwrap() as usize;
        let want = floats(&case["resized"]);
        let input = naflex_pre::NaflexInput {
            pixel_values: Vec::new(),
            attention_mask: Vec::new(),
            grid_h: gh,
            grid_w: gw,
            max_num_patches: 0,
        };
        let (rows, cols) = input.position_interpolation();

        for c in 0..channels {
            let plane = &grid[c * POSITION_GRID * POSITION_GRID..];
            for y in 0..gh {
                for x in 0..gw {
                    let mut v = 0.0f64;
                    for i in 0..POSITION_GRID {
                        for j in 0..POSITION_GRID {
                            v += rows[y * POSITION_GRID + i] as f64
                                * plane[i * POSITION_GRID + j]
                                * cols[x * POSITION_GRID + j] as f64;
                        }
                    }
                    let w = want[c * gh * gw + y * gw + x];
                    assert!((v - w).abs() < 1e-4, "{gh}x{gw} c{c} ({y},{x}): {v} vs {w}");
                }
            }
        }
    }
}
