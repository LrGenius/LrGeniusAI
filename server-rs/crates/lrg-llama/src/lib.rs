//! In-process llama.cpp inference with a pinned prompt prefix.
//!
//! # Why this exists
//!
//! Indexing sends the same system prompt, catalog vocabulary and keyword
//! taxonomy with every single photo. Talking to LM Studio or Ollama over HTTP,
//! the best we can do is *arrange* for their prefix-KV heuristic to fire (see
//! `lrg_providers::prompts::SplitPrompt`). Running llama.cpp in-process lets us
//! own the KV cache outright: evaluate the run-constant prefix once, keep it at
//! a fixed position, and per photo evaluate only the volatile tail plus the
//! image — then drop just that tail again.
//!
//! # Threading
//!
//! `LlamaContext` borrows `LlamaModel`, and mtmd's `eval_chunks` is explicitly
//! not thread-safe. Both problems are solved the same way: a dedicated worker
//! thread owns the model and the context on its own stack and serves requests
//! off a channel. Callers hand over a job and await a oneshot, so no llama.cpp
//! work ever runs on a tokio worker. Dropping the [`LlamaEngine`] closes the
//! channel, the thread returns, and every native resource is freed in order.

mod engine;
mod worker;

pub use engine::{EngineInfo, GenerationOutput, GenerationRequest, LlamaEngine, LlamaEngineConfig};

/// A decoded RGB image, 3 bytes per pixel, ready for `MtmdBitmap`.
///
/// Deliberately *not* JPEG: the HTTP providers have to re-encode every photo to
/// base64 JPEG, and skipping that round-trip is part of what makes the
/// in-process path faster.
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
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self, LlamaError> {
        let expected = width as usize * height as usize * 3;
        if data.len() != expected {
            return Err(LlamaError::Image(format!(
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

#[derive(Debug, thiserror::Error)]
pub enum LlamaError {
    #[error("model file not found: {0}")]
    ModelNotFound(String),
    #[error("failed to load model: {0}")]
    Load(String),
    #[error("image error: {0}")]
    Image(String),
    #[error("prompt error: {0}")]
    Prompt(String),
    #[error("inference failed: {0}")]
    Inference(String),
    /// The prompt plus the requested generation budget cannot fit in the
    /// context window. Raised *before* any decode, because overrunning the
    /// context is the most common way to take llama.cpp down with it.
    #[error("{0}")]
    ContextOverflow(String),
    #[error("the llama engine is shutting down")]
    Shutdown,
}
