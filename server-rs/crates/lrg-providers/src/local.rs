//! The port for an in-process inference engine.
//!
//! `lrg-providers` deliberately knows nothing about llama.cpp: it defines the
//! shape of a local engine here, `lrg-llama` implements it, and `lrg-api` wires
//! the two together. That keeps the heavy native toolchain behind a single
//! feature flag in one crate instead of leaking a `#[cfg]` into every provider.
//!
//! The distinguishing feature versus the REST providers is [`LocalRequest`]'s
//! split prompt: a local engine can pin the run-constant half in its KV cache
//! and evaluate only the volatile half per photo, which is the whole reason
//! for running inference in-process.

use std::sync::Arc;

use async_trait::async_trait;

/// A decoded RGB image, 3 bytes per pixel. Passing raw pixels avoids the
/// JPEG-encode/base64 round-trip every HTTP provider has to pay.
#[derive(Debug, Clone)]
pub struct LocalImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct LocalRequest {
    pub system_prompt: String,
    /// Byte-identical across a whole indexing run; safe to pin in a KV cache.
    pub stable_prompt: String,
    /// Varies per photo; always re-evaluated.
    pub per_photo_prompt: String,
    pub image: Option<LocalImage>,
    /// JSON Schema to constrain decoding with.
    pub schema: Option<serde_json::Value>,
    pub max_tokens: u32,
    pub temperature: f32,
}

#[derive(Debug, Clone, Default)]
pub struct LocalResponse {
    pub text: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub prefix_reused: bool,
}

#[async_trait]
pub trait LocalEngine: Send + Sync {
    /// Generate for a group of photos sharing one stable prefix.
    ///
    /// The batch is the unit of both prefix reuse and parallel decoding, so
    /// callers should prefer one call with N requests over N calls with one.
    /// Each entry gets its own result: one photo failing must not sink the rest.
    async fn generate(&self, requests: Vec<LocalRequest>) -> Vec<Result<LocalResponse, String>>;

    fn supports_vision(&self) -> bool;

    /// Human-readable identifier for the loaded model, as reported to `/models`.
    fn model_name(&self) -> String;

    /// How many photos this engine wants per [`generate`](Self::generate) call.
    ///
    /// For llama.cpp this is `n_parallel`: the number of sequences that can
    /// decode concurrently against the shared prefix. Handing it more than
    /// that is not wrong, just pointless — the surplus waits for a free slot.
    fn preferred_batch_size(&self) -> usize {
        1
    }
}

pub type SharedLocalEngine = Arc<dyn LocalEngine>;
