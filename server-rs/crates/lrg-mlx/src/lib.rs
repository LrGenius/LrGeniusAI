//! MLX inference on Apple silicon, driven through the `lrgenius-mlx` sidecar.
//!
//! # Why a sidecar
//!
//! MLX has no usable Rust binding — the one crate that claims VLM support is a
//! single 0.1.0 release with no adoption — but Apple ships `mlx-swift-lm`,
//! which covers Gemma 4 vision, JSON-Schema-constrained decoding and the whole
//! quantized-model zoo on Hugging Face. So the inference lives in a small Swift
//! executable (`native/mlx-sidecar/`) and this crate is its supervisor: it
//! spawns the process, speaks its JSON-lines protocol, and presents the result
//! to `lrg-api` in the same shape [`crate::MlxEngine`]'s llama.cpp counterpart
//! uses, so both can be adapted onto `lrg_providers::local::LocalEngine`.
//!
//! # Threading
//!
//! The sidecar serves one request at a time, so the connection is guarded by a
//! single async mutex. A caller holds it for the length of its request, writes
//! a line, and reads until the reply with its id arrives. That makes response
//! correlation trivial and matches how the batch API is actually used: one
//! indexing run at a time, saturating the GPU.
//!
//! # Differences from the llama.cpp backend
//!
//! There is no pinned prompt prefix here. `GuidedGenerationLoop.run` allocates
//! a fresh KV cache per call, so the run-constant half of the prompt is
//! re-prefilled for every photo and [`GenerationOutput::prefix_reused`] is
//! always false. The split prompt is still sent, and still ordered
//! stable-then-volatile, so the ordering is right the day that changes.

mod engine;
mod protocol;

pub use engine::{resolve_sidecar, EngineInfo, MlxEngine, MlxEngineConfig};

/// A decoded RGB image, 3 bytes per pixel.
///
/// Same representation as `lrg_llama::RgbImage` for the same reason: the HTTP
/// providers have to re-encode every photo to base64 JPEG and skipping that is
/// part of the point of running locally.
#[derive(Debug, Clone)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    /// `width * height * 3` bytes, row-major RGB.
    pub data: Vec<u8>,
}

impl RgbImage {
    /// # Errors
    /// Returns `Err` if `data` is not exactly `width * height * 3` bytes.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self, MlxError> {
        let expected = width as usize * height as usize * 3;
        if data.len() != expected {
            return Err(MlxError::Image(format!(
                "expected {expected} bytes for {width}x{height} RGB, got {}",
                data.len()
            )));
        }
        Ok(RgbImage {
            width,
            height,
            data,
        })
    }
}

/// One photo's worth of work. Mirrors `lrg_llama::GenerationRequest`.
#[derive(Debug, Clone, Default)]
pub struct GenerationRequest {
    pub system_prompt: String,
    /// Run-constant across an indexing run.
    pub stable_prompt: String,
    /// Varies per photo.
    pub per_photo_prompt: String,
    pub image: Option<RgbImage>,
    /// JSON Schema to constrain decoding with. `None` decodes freely.
    pub schema: Option<serde_json::Value>,
    pub max_tokens: u32,
    pub temperature: f32,
}

#[derive(Debug, Clone, Default)]
pub struct GenerationOutput {
    pub text: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// Always false on this backend — see the module docs.
    pub prefix_reused: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum MlxError {
    /// The sidecar binary could not be found. Its message names the places
    /// searched, because the usual cause is a dev build run from a tree where
    /// `swift build` has not been run yet.
    #[error("{0}")]
    SidecarMissing(String),
    #[error("MLX requires an Apple silicon Mac")]
    UnsupportedPlatform,
    #[error("model directory not found: {0}")]
    ModelNotFound(String),
    #[error("failed to load model: {0}")]
    Load(String),
    #[error("image error: {0}")]
    Image(String),
    #[error("inference failed: {0}")]
    Inference(String),
    /// The sidecar died, or its stdout stopped making sense. Either way the
    /// engine is unusable and must be rebuilt.
    #[error("the MLX sidecar is not responding: {0}")]
    Sidecar(String),
    #[error("the MLX engine is shutting down")]
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_image_rejects_a_mismatched_buffer() {
        // Silently accepting a short buffer would surface much later as a
        // baffling image-decode failure inside Swift.
        assert!(RgbImage::new(2, 2, vec![0; 11]).is_err());
        assert!(RgbImage::new(2, 2, vec![0; 12]).is_ok());
    }
}
