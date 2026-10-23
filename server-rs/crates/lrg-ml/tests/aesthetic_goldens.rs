//! End-to-end golden verification of AestheticSigLIP: Rust preprocessing plus
//! ort inference against the scores the authors' own fp32 code produces
//! (`testdata/aesthetic_goldens.json`, written by
//! `scripts/export_aesthetic_siglip_fp16.py` in the run that exports the ONNX
//! file, so the goldens always describe the model that was published).
//!
//! Needs the exported ONNX file, looked up where the server looks
//! (`LRG_AESTHETIC_ONNX`, else `~/.cache/lrgenius/models/`). Skips when it is
//! absent; set `LRG_REQUIRE_GOLDENS=aesthetic` and the skip becomes a failure.

use lrg_ml::aesthetic::{AestheticModel, MODEL_ID};

mod common;

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

#[test]
fn scores_match_the_authors_reference() {
    let paths = lrg_ml::model_paths::resolve_aesthetic();
    if !common::assets_ready(
        "aesthetic",
        paths.all_exist(),
        &format!("AestheticSigLIP not found at {}", paths.onnx.display()),
    ) {
        return;
    }
    let model = AestheticModel::new(paths);

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/aesthetic_goldens.json"
    );
    let goldens: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        goldens["model_id"].as_str(),
        Some(MODEL_ID),
        "testdata/aesthetic_goldens.json is for another export; use the one \
         `scripts/export_aesthetic_siglip_fp16.py` published next to the ONNX file"
    );

    for (name, expected) in goldens["images"].as_object().unwrap() {
        let (pixels, width, height) = build_image(name);
        let got = model.score_image(&pixels, width, height).unwrap();
        let want = expected["score"].as_f64().unwrap() as f32;
        // Scores run 1..9; fp16 against fp32 stays within a few hundredths.
        assert!(
            (got - want).abs() < 0.03,
            "{name}: score {got} vs {want} (fp32 reference)"
        );
        println!("{name}: score {got:.3} vs {want:.3}");
    }
}
