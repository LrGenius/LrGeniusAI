//! End-to-end checks against a real GGUF.
//!
//! Ignored by default — they need model files, which CI does not carry. Point
//! `LRG_TEST_MODEL_GGUF` (and optionally `LRG_TEST_MMPROJ_GGUF`) at a small
//! vision model and run:
//!
//! ```text
//! cargo test -p lrg-llama --test engine_smoke -- --ignored --nocapture
//! ```

use lrg_llama::{GenerationRequest, LlamaEngine, LlamaEngineConfig, RgbImage};

fn config_from_env(n_parallel: u32) -> Option<LlamaEngineConfig> {
    let model_path = std::env::var("LRG_TEST_MODEL_GGUF").ok()?;
    Some(LlamaEngineConfig {
        model_path: model_path.into(),
        mmproj_path: std::env::var("LRG_TEST_MMPROJ_GGUF").ok().map(Into::into),
        // llama.cpp splits the KV cache between concurrent sequences and the
        // engine refuses to leave any of them under 4096 tokens, so a test that
        // wants `n_parallel` sequences has to pay for them.
        n_ctx: 4096 * n_parallel.max(1),
        n_parallel,
        ..Default::default()
    })
}

/// A trivially describable image: a solid red field. Good enough to prove the
/// projector ran without depending on a small model's descriptive accuracy.
fn red_image() -> RgbImage {
    let (w, h) = (224u32, 224u32);
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for _ in 0..w * h {
        data.extend_from_slice(&[220, 30, 30]);
    }
    RgbImage::new(w, h, data).expect("dimensions match the buffer")
}

fn base_request() -> GenerationRequest {
    GenerationRequest {
        system_prompt: "You are a concise photo analyst.".to_string(),
        stable_prompt: "Describe the photo and answer strictly as JSON.".to_string(),
        per_photo_prompt: String::new(),
        image: None,
        schema: None,
        max_tokens: 160,
        temperature: 0.0,
    }
}

#[tokio::test]
#[ignore = "requires LRG_TEST_MODEL_GGUF"]
async fn generates_text_and_reuses_the_prefix_across_calls() {
    let Some(config) = config_from_env(1) else {
        return;
    };
    let engine = tokio::task::spawn_blocking(move || LlamaEngine::new(config))
        .await
        .unwrap()
        .expect("engine should load");

    let mut first = base_request();
    first.per_photo_prompt = "Capture Time: 2026-01-01".to_string();
    let out = engine.generate(vec![first]).await.unwrap();
    let first_out = out[0].as_ref().expect("generation should succeed");
    assert!(
        !first_out.text.trim().is_empty(),
        "model produced no output"
    );
    assert!(
        !first_out.prefix_reused,
        "the very first call cannot be a cache hit"
    );

    // Same stable half, different volatile half: the prefix must be reused.
    let mut second = base_request();
    second.per_photo_prompt = "Capture Time: 2026-02-02".to_string();
    let out = engine.generate(vec![second]).await.unwrap();
    assert!(
        out[0]
            .as_ref()
            .expect("generation should succeed")
            .prefix_reused,
        "an unchanged stable prompt must hit the pinned prefix"
    );

    // Changing the stable half must invalidate it.
    let mut third = base_request();
    third.stable_prompt = "Completely different standing instructions.".to_string();
    let out = engine.generate(vec![third]).await.unwrap();
    assert!(
        !out[0]
            .as_ref()
            .expect("generation should succeed")
            .prefix_reused,
        "a changed stable prompt must invalidate the pinned prefix"
    );
}

#[tokio::test]
#[ignore = "requires LRG_TEST_MODEL_GGUF and LRG_TEST_MMPROJ_GGUF"]
async fn honours_a_json_schema_and_sees_the_image() {
    let Some(config) = config_from_env(1) else {
        return;
    };
    if config.mmproj_path.is_none() {
        return;
    }
    let engine = tokio::task::spawn_blocking(move || LlamaEngine::new(config))
        .await
        .unwrap()
        .expect("engine should load");
    assert!(engine.info().supports_vision);

    let mut request = base_request();
    request.image = Some(red_image());
    request.per_photo_prompt = "What is the dominant colour?".to_string();
    request.schema = Some(serde_json::json!({
        "type": "object",
        "properties": {
            "colour": {"type": "string"},
            "confidence": {"type": "number"}
        },
        "required": ["colour", "confidence"],
        "additionalProperties": false
    }));

    let out = engine.generate(vec![request]).await.unwrap();
    let response = out[0].as_ref().expect("generation should succeed");
    let parsed: serde_json::Value =
        serde_json::from_str(response.text.trim()).unwrap_or_else(|e| {
            panic!(
                "constrained output must be valid JSON: {e}\n{}",
                response.text
            )
        });
    assert!(parsed.get("colour").is_some(), "schema field missing");
    assert!(parsed.get("confidence").is_some(), "schema field missing");
}

#[tokio::test]
#[ignore = "requires LRG_TEST_MODEL_GGUF"]
async fn decodes_a_batch_in_parallel_sequences() {
    let Some(config) = config_from_env(3) else {
        return;
    };
    let engine = tokio::task::spawn_blocking(move || LlamaEngine::new(config))
        .await
        .unwrap()
        .expect("engine should load");

    let requests: Vec<GenerationRequest> = ["one", "two", "three"]
        .iter()
        .map(|tag| {
            let mut r = base_request();
            r.per_photo_prompt = format!("Photo tag: {tag}");
            r
        })
        .collect();

    let out = engine.generate(requests).await.unwrap();
    assert_eq!(out.len(), 3, "every request must get its own result");
    for (i, result) in out.iter().enumerate() {
        let response = result.as_ref().unwrap_or_else(|e| panic!("photo {i}: {e}"));
        assert!(
            !response.text.trim().is_empty(),
            "photo {i} produced no output"
        );
    }
}

/// Regression for #316: a generation long enough to fill a sequence used to die
/// mid-decode with `Decode Error 1: NoKvCacheSlot`, because the budget was
/// measured against the whole KV cache while llama.cpp hands each sequence only
/// `n_ctx / n_parallel` of it. One photo at a time must therefore be able to
/// spend the *whole* configured context.
#[tokio::test]
#[ignore = "requires LRG_TEST_MODEL_GGUF"]
async fn a_single_photo_may_use_the_whole_context() {
    let Some(mut config) = config_from_env(1) else {
        return;
    };
    config.n_ctx = 4096;
    let engine = tokio::task::spawn_blocking(move || LlamaEngine::new(config))
        .await
        .unwrap()
        .expect("engine should load");
    assert_eq!(
        engine.info().n_ctx_seq,
        engine.info().n_ctx,
        "a lone photo must not be charged for a sequence it does not have"
    );

    let mut request = base_request();
    // No schema, so nothing stops the model early: this runs until the budget
    // is spent, which is exactly the path that used to run off the end.
    request.schema = None;
    request.max_tokens = 2048;
    request.per_photo_prompt = "Describe this photo in exhaustive detail.".to_string();

    let out = engine.generate(vec![request]).await.unwrap();
    let response = out[0]
        .as_ref()
        .unwrap_or_else(|e| panic!("a photo inside the context window must not fail: {e}"));
    assert!(!response.text.trim().is_empty(), "no output");
}

#[tokio::test]
#[ignore = "requires LRG_TEST_MODEL_GGUF"]
async fn refuses_to_overrun_the_context_window() {
    let Some(mut config) = config_from_env(1) else {
        return;
    };
    config.n_ctx = 512;
    let engine = tokio::task::spawn_blocking(move || LlamaEngine::new(config))
        .await
        .unwrap()
        .expect("engine should load");

    let mut request = base_request();
    request.stable_prompt = "word ".repeat(2000);
    let out = engine.generate(vec![request]).await.unwrap();
    let err = out[0].as_ref().expect_err("should refuse, not truncate");
    assert!(
        matches!(err, lrg_llama::LlamaError::ContextOverflow(_)),
        "an overrun must be refused up front, not surface as a decode failure: {err}"
    );
    let message = err.to_string();
    assert!(
        message.contains("context"),
        "the error must explain the context window: {message}"
    );
}
