//! A conversation with tools, independent of the model that answers it.
//!
//! The agent chat runs its loop against [`ChatModel`]. A local engine answers
//! through [`crate::local_chat::LocalChatModel`]; the cloud providers are
//! meant to answer through their own native tool calling, and the types here
//! are shaped so that mapping is direct: a tool call carries an id, and its
//! result comes back inside a user turn, the way Anthropic's and Gemini's
//! APIs expect it (OpenAI's `tool` role is a rename away).

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

/// An encoded image (JPEG or PNG), as a tool hands it back.
#[derive(Debug, Clone)]
pub struct ChatImage {
    pub data: Vec<u8>,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    pub text: String,
    pub images: Vec<ChatImage>,
    /// The tool ran and failed, or was not allowed to run. The model reads
    /// the text either way.
    pub is_error: bool,
}

#[derive(Debug, Clone)]
pub enum ChatPart {
    Text(String),
    Image(ChatImage),
    /// What the model said to itself before acting. Kept in the history
    /// because a local model copies the shape of its earlier turns, and turns
    /// without it teach it to stop reasoning.
    Reasoning(String),
    ToolCall(ToolCall),
    ToolResult(ToolResult),
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub parts: Vec<ChatPart>,
}

impl ChatMessage {
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        ChatMessage {
            role: ChatRole::User,
            parts: vec![ChatPart::Text(text.into())],
        }
    }
}

/// A tool as the model is told about it. `input_schema` is the tool's JSON
/// Schema as the MCP server publishes it, except that it may carry a
/// top-level `oneOf`/`anyOf` whose branches hold only `required` — "give one
/// of these arguments". An adapter whose API refuses that at the top of a
/// tool schema (Anthropic's and OpenAI's do) leaves it out.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
    pub temperature: f32,
}

/// One answer from the model: text for the user, tool calls, or both.
#[derive(Debug, Clone, Default)]
pub struct ChatTurn {
    pub reasoning: Option<String>,
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl ChatTurn {
    /// The turn as it goes into the history.
    #[must_use]
    pub fn to_message(&self) -> ChatMessage {
        let mut parts = Vec::new();
        if let Some(reasoning) = &self.reasoning {
            parts.push(ChatPart::Reasoning(reasoning.clone()));
        }
        if !self.text.is_empty() {
            parts.push(ChatPart::Text(self.text.clone()));
        }
        parts.extend(self.tool_calls.iter().cloned().map(ChatPart::ToolCall));
        ChatMessage {
            role: ChatRole::Assistant,
            parts,
        }
    }
}

/// A piece of a turn while it is being generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatDelta {
    Reasoning(String),
    Text(String),
}

pub type ChatDeltas = UnboundedSender<ChatDelta>;

#[async_trait]
pub trait ChatModel: Send + Sync {
    /// The model, as the chat page names it.
    fn name(&self) -> String;

    /// Tokens one request may use, prompt and answer together, when known.
    fn context_window(&self) -> Option<u32>;

    fn supports_images(&self) -> bool;

    /// Answer the conversation with one turn, sending its text to `deltas`
    /// while it is generated.
    async fn complete(
        &self,
        request: &ChatRequest,
        deltas: Option<ChatDeltas>,
    ) -> Result<ChatTurn, String>;
}
