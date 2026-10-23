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

/// Who said one turn of a conversation.
///
/// There is no tool role. The chat templates llama.cpp can apply without
/// Jinja know only these three, so a caller that runs tools renders their
/// results into a user turn itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalChatRole {
    System,
    User,
    Assistant,
}

impl LocalChatRole {
    /// The role name chat templates expect.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LocalChatRole::System => "system",
            LocalChatRole::User => "user",
            LocalChatRole::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocalChatMessage {
    pub role: LocalChatRole,
    pub text: String,
    /// Photos shown with this turn. llama.cpp places them after the text;
    /// with MLX the model's message format decides.
    pub images: Vec<LocalImage>,
}

/// A whole conversation, answered with one assistant turn.
///
/// Unlike [`LocalRequest`] nothing here is pinned: a conversation changes
/// with every turn, so the engine evaluates all of it each time.
#[derive(Debug, Clone, Default)]
pub struct LocalChatRequest {
    pub messages: Vec<LocalChatMessage>,
    /// JSON Schema to constrain the answer with.
    pub schema: Option<serde_json::Value>,
    pub max_tokens: u32,
    pub temperature: f32,
}

/// Where an engine sends the answer's text while it is being generated.
pub type TextDeltas = tokio::sync::mpsc::UnboundedSender<String>;

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

    /// Answer a conversation with one assistant turn, sending its text to
    /// `deltas` as it is generated. The returned text is the whole answer.
    ///
    /// Dropping the returned future asks the engine to stop. How soon it can
    /// differs: llama.cpp stops at the next token, the MLX sidecar only once
    /// its answer is complete.
    async fn chat(
        &self,
        request: LocalChatRequest,
        deltas: Option<TextDeltas>,
    ) -> Result<LocalResponse, String> {
        let _ = (request, deltas);
        Err(format!("{} cannot hold a conversation", self.model_name()))
    }

    /// The tokens one conversation may use, prompt and answer together, when
    /// the engine knows.
    fn context_window(&self) -> Option<u32> {
        None
    }
}

pub type SharedLocalEngine = Arc<dyn LocalEngine>;
