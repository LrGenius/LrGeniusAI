//! End-to-end golden verification: Rust preprocessing (image
//! shortest-edge resize + center crop + OpenAI-CLIP normalize, text
//! clean + Gemma tokenize) plus ort inference
//! against real torch-computed embeddings from `make_siglip_goldens.py`
//! (which runs through open_clip's actual `preprocess`/`tokenizer`
//! pipeline, not just the exported ONNX graph — this is the full-stack
//! check that spike S1 didn't cover).
//!
//! Requires local model files (from the M0 S1 spike export) — set:
//!   LRG_SIGLIP_IMAGE_ONNX, LRG_SIGLIP_TEXT_ONNX, LRG_SIGLIP_TOKENIZER
//! Skips (not fails) when unset, so CI without the multi-GB models stays
//! green until M9 wires up model distribution.

use std::path::PathBuf;

use lrg_ml::siglip::{l2_normalize, ModelPaths, SiglipModel};

fn env_paths() -> Option<ModelPaths> {
    Some(ModelPaths {
        image_onnx: PathBuf::from(std::env::var("LRG_SIGLIP_IMAGE_ONNX").ok()?),
        text_onnx: PathBuf::from(std::env::var("LRG_SIGLIP_TEXT_ONNX").ok()?),
        tokenizer_json: PathBuf::from(std::env::var("LRG_SIGLIP_TOKENIZER").ok()?),
    })
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb)
}

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
        other => panic!("unknown golden image {other}"),
    }
}

#[test]
fn image_and_text_embeddings_match_torch() {
    let Some(paths) = env_paths() else {
        eprintln!("LRG_SIGLIP_* env vars not set; skipping ONNX golden test");
        return;
    };
    assert!(paths.all_exist(), "configured model paths do not exist");
    let model = SiglipModel::new(paths);

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/siglip_goldens.json"
    );
    let goldens: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();

    for (name, expected) in goldens["images"].as_object().unwrap() {
        let (pixels, width, height) = build_image(name);
        assert_eq!(width as u64, expected["width"].as_u64().unwrap());
        assert_eq!(height as u64, expected["height"].as_u64().unwrap());

        let mut got = model.embed_image(&pixels, width, height).unwrap();
        let mut want: Vec<f32> = expected["embedding"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        // encode_image output is pre-normalization in both torch and ort
        // (search.py normalizes after the call); normalize both sides so
        // the comparison is scale-invariant, matching how it's actually used.
        l2_normalize(&mut got);
        l2_normalize(&mut want);
        let cos = cosine(&got, &want);
        assert!(cos > 0.999, "{name}: image cosine {cos} too low");
        println!("{name}: image cosine {cos}");
    }

    let texts: Vec<String> = goldens["texts"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let mut got_batch = model.embed_text(&texts).unwrap();
    for (text, got) in texts.iter().zip(got_batch.iter_mut()) {
        let mut want: Vec<f32> = goldens["texts"][text]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        l2_normalize(got);
        l2_normalize(&mut want);
        let cos = cosine(got, &want);
        assert!(cos > 0.999, "{text:?}: text cosine {cos} too low");
        println!("{text:?}: text cosine {cos}");
    }
}
