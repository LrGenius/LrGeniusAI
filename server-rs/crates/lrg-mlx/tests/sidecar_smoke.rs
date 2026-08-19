//! End-to-end checks against a real sidecar and a real MLX model.
//!
//! `#[ignore]`d, exactly like `lrg-llama`'s `engine_smoke`: these need Apple
//! silicon, a built `lrgenius-mlx`, and multi-gigabyte weights on disk, none of
//! which belong in `cargo test`.
//!
//! ```bash
//! # once: build the sidecar (needs the Metal toolchain — see
//! # native/mlx-sidecar/README.md)
//! cd native/mlx-sidecar && xcodebuild build -scheme lrgenius-mlx \
//!   -destination 'platform=macOS,arch=arm64' -configuration Release \
//!   -derivedDataPath .build/xcode -skipPackagePluginValidation
//!
//! export LRG_MLX_SIDECAR=$PWD/.build/xcode/Build/Products/Release/lrgenius-mlx
//! export LRG_TEST_MLX_MODEL_DIR=/path/to/mlx-community/gemma-4-e2b-it-4bit
//! cargo test -p lrg-mlx --test sidecar_smoke -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use lrg_mlx::{GenerationRequest, MlxEngine, MlxEngineConfig, RgbImage};

fn model_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("LRG_TEST_MLX_MODEL_DIR")
            .expect("set LRG_TEST_MLX_MODEL_DIR to an MLX model directory"),
    )
}

async fn engine() -> MlxEngine {
    MlxEngine::new(MlxEngineConfig {
        model_dir: model_dir(),
        sidecar_path: std::env::var_os("LRG_MLX_SIDECAR").map(PathBuf::from),
    })
    .await
    .expect("the sidecar should start and load the model")
}

/// A synthetic image with obvious, describable structure: a red/blue vertical
/// split behind a green horizontal band. Enough for a model to say something
/// specific, without shipping a binary fixture.
fn test_image() -> RgbImage {
    let (width, height) = (512u32, 512u32);
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            let pixel = if (height * 2 / 5..height * 3 / 5).contains(&y) {
                [30, 180, 60]
            } else if x < width / 2 {
                [200, 40, 40]
            } else {
                [40, 70, 200]
            };
            data.extend_from_slice(&pixel);
        }
    }
    RgbImage::new(width, height, data).expect("the buffer is sized correctly")
}

fn metadata_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "caption": {"type": "string"},
            "keywords": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["caption", "keywords"],
    })
}

#[tokio::test]
#[ignore = "needs Apple silicon, a built sidecar and a real MLX model"]
async fn describes_a_photo_as_schema_valid_json() {
    let engine = engine().await;
    assert!(
        engine.info().supports_vision,
        "the test model must be a vision model"
    );

    let outputs = engine
        .generate(vec![GenerationRequest {
            system_prompt: "You are a photo cataloguing assistant.".to_string(),
            stable_prompt: "Describe the photo and list keywords.".to_string(),
            per_photo_prompt: "Captured 2026-08-10.".to_string(),
            image: Some(test_image()),
            schema: Some(metadata_schema()),
            max_tokens: 200,
            temperature: 0.0,
        }])
        .await
        .expect("the engine should answer");

    let output = outputs
        .into_iter()
        .next()
        .expect("one request, one result")
        .expect("generation should succeed");

    // The point of constrained decoding: this must parse, always.
    let parsed: serde_json::Value =
        serde_json::from_str(output.text.trim()).expect("output must be valid JSON");
    assert!(parsed.get("caption").and_then(|v| v.as_str()).is_some());
    assert!(parsed.get("keywords").and_then(|v| v.as_array()).is_some());
    assert!(output.prompt_tokens > 0 && output.completion_tokens > 0);
}

/// Every photo in a batch must get its own slot, in order — this is what the
/// provider relies on to zip results back to photos.
#[tokio::test]
#[ignore = "needs Apple silicon, a built sidecar and a real MLX model"]
async fn a_batch_returns_one_result_per_request() {
    let engine = engine().await;
    let request = || GenerationRequest {
        system_prompt: "You are a photo cataloguing assistant.".to_string(),
        stable_prompt: "Describe the photo and list keywords.".to_string(),
        per_photo_prompt: "Captured 2026-08-10.".to_string(),
        image: Some(test_image()),
        schema: Some(metadata_schema()),
        max_tokens: 120,
        temperature: 0.0,
    };

    let outputs = engine
        .generate(vec![request(), request(), request()])
        .await
        .expect("the engine should answer");

    assert_eq!(outputs.len(), 3);
    for output in outputs {
        let output = output.expect("each photo should succeed");
        serde_json::from_str::<serde_json::Value>(output.text.trim())
            .expect("each result must be valid JSON");
    }
}

/// The text-only path, used by keyword clustering.
#[tokio::test]
#[ignore = "needs Apple silicon, a built sidecar and a real MLX model"]
async fn generates_unconstrained_text_without_an_image() {
    let engine = engine().await;
    let outputs = engine
        .generate(vec![GenerationRequest {
            system_prompt: String::new(),
            stable_prompt: "Name three primary colours.".to_string(),
            per_photo_prompt: String::new(),
            image: None,
            schema: None,
            max_tokens: 40,
            temperature: 0.0,
        }])
        .await
        .expect("the engine should answer");

    let output = outputs
        .into_iter()
        .next()
        .expect("one request, one result")
        .expect("generation should succeed");
    assert!(!output.text.trim().is_empty());
}

/// A missing model directory must fail on `new`, before anything is spawned or
/// any weights are touched.
#[tokio::test]
#[ignore = "needs Apple silicon and a built sidecar"]
async fn a_bad_model_directory_fails_to_load() {
    let result = MlxEngine::new(MlxEngineConfig {
        model_dir: PathBuf::from("/no/such/mlx/model"),
        sidecar_path: std::env::var_os("LRG_MLX_SIDECAR").map(PathBuf::from),
    })
    .await;
    assert!(result.is_err(), "a missing model directory must not load");
}
