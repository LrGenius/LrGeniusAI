//! `aesthetic` preprocessing against AestheticSigLIP's own Python
//! (`naflex.compute_patch_grid` and `naflex.preprocess_image`;
//! `testdata/aesthetic_pre_goldens.json`, written by
//! `scripts/export_aesthetic_siglip_fp16.py`).
//!
//! Needs no model files. The grid rule is a small search with float tie-breaks,
//! and the model only ever saw the grids it picks, so a port that differs on
//! some shape would score those photos from a layout it was not trained on
//! without an error of its own.

use lrg_ml::aesthetic;
use lrg_ml::naflex_pre::PATCH_DIM;

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
        "/../../testdata/aesthetic_pre_goldens.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn patch_grid_matches_the_authors_rule() {
    let goldens = goldens();
    assert_eq!(
        goldens["max_num_patches"].as_u64().unwrap() as usize,
        aesthetic::MAX_NUM_PATCHES
    );
    let grids = goldens["grids"].as_object().unwrap();
    assert!(grids.len() >= 40, "the golden should cover many shapes");
    for (size, want) in grids {
        let (w, h) = size.split_once('x').unwrap();
        let (w, h): (usize, usize) = (w.parse().unwrap(), h.parse().unwrap());
        let got = aesthetic::patch_grid(h, w, aesthetic::MAX_NUM_PATCHES);
        let want = (
            want[0].as_u64().unwrap() as usize,
            want[1].as_u64().unwrap() as usize,
        );
        assert_eq!(got, want, "{size}: grid (rows, cols)");
        assert!(got.0 * got.1 <= aesthetic::MAX_NUM_PATCHES, "{size}");
    }
}

#[test]
fn patches_match_the_authors_preprocessing() {
    let goldens = goldens();
    for (name, want) in goldens["images"].as_object().unwrap() {
        let (pixels, width, height) = build_image(name);
        let got = aesthetic::preprocess(&pixels, width, height);
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
        assert_eq!(got.max_num_patches, aesthetic::MAX_NUM_PATCHES);

        let real = &got.pixel_values[..got.num_patches() * PATCH_DIM];
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
            for (i, w) in values.as_array().unwrap().iter().enumerate() {
                let g = got.pixel_values[row * PATCH_DIM + i] as f64;
                let w = w.as_f64().unwrap();
                assert!(
                    (g - w).abs() < 1e-6,
                    "{name}: patch {row} value {i}: {g} vs {w}"
                );
            }
        }
    }
}
