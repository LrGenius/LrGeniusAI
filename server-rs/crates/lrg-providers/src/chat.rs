//! A conversation with tools, independent of the model that answers it.
//!
//! The agent chat runs its loop against [`ChatModel`]. A local engine answers
//! through [`crate::local_chat::LocalChatModel`]; cloud models and AI servers
//! answer through their own tool calling ([`crate::cloud_chat`]), and the
//! types here are shaped so that mapping is direct: a tool call carries an id,
//! and its result comes back inside a user turn, the way Anthropic's and
//! Gemini's APIs expect it (OpenAI's `tool` role is a rename away).

use async_trait::async_trait;
use serde_json::{Map, Value};
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
    /// The turn as the API that produced it wrote it, for that API alone to
    /// send back unchanged: Anthropic's thinking blocks and Gemini's thought
    /// signatures must come back exactly as given, and the neutral parts
    /// beside it cannot carry them. Every other model ignores it.
    Native(NativeTurn),
}

/// See [`ChatPart::Native`].
#[derive(Debug, Clone)]
pub struct NativeTurn {
    /// Which adapter wrote it (`"anthropic"`, `"gemini"`).
    pub api: &'static str,
    /// Its own representation of the turn: Anthropic's content blocks,
    /// Gemini's parts.
    pub content: Vec<Value>,
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
    /// See [`ChatPart::Native`].
    pub native: Option<NativeTurn>,
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
        parts.extend(self.native.clone().map(ChatPart::Native));
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

/// A tool's argument schema as a cloud API takes it: references inlined (each
/// tool brings its own `$defs`), and the top-level choice of required
/// arguments that [`ToolSpec`] may carry left out, since Anthropic's and
/// OpenAI's APIs refuse a `oneOf` or `anyOf` there. The tool still checks its
/// arguments.
#[must_use]
pub fn portable_tool_schema(schema: &Value) -> Value {
    let defs = schema.get("$defs").or_else(|| schema.get("definitions"));
    let mut out = inline_refs(schema, defs, 0);
    if let Some(object) = out.as_object_mut() {
        for key in ["$schema", "$defs", "definitions", "oneOf", "anyOf"] {
            object.remove(key);
        }
        object
            .entry("type")
            .or_insert_with(|| Value::from("object"));
        object
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()));
    }
    out
}

fn inline_refs(schema: &Value, defs: Option<&Value>, depth: usize) -> Value {
    // Deep enough for any tool here; stops a recursive definition.
    if depth > 12 {
        return Value::Object(Map::new());
    }
    match schema {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                let target = reference
                    .rsplit('/')
                    .next()
                    .and_then(|name| defs?.get(name))
                    .map_or_else(
                        || Value::Object(Map::new()),
                        |t| inline_refs(t, defs, depth + 1),
                    );
                // A description written next to the reference says what the
                // argument is for, and wins over the target's own.
                let mut target = target;
                if let (Some(description), Some(t)) =
                    (object.get("description"), target.as_object_mut())
                {
                    t.insert("description".into(), description.clone());
                }
                return target;
            }
            Value::Object(
                object
                    .iter()
                    .filter(|(key, _)| *key != "$defs" && *key != "definitions")
                    .map(|(key, value)| (key.clone(), inline_refs(value, defs, depth + 1)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| inline_refs(item, defs, depth + 1))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_portable_schema_inlines_references_and_drops_the_choice() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$defs": {"Sort": {"type": "string", "enum": ["a", "b"], "description": "Order."}},
            "type": "object",
            "properties": {
                "sort": {"$ref": "#/$defs/Sort", "description": "How to sort."},
                "set": {"type": ["string", "null"]},
                "photo_ids": {"type": ["array", "null"], "items": {"type": "string"}}
            },
            "oneOf": [{"required": ["set"]}, {"required": ["photo_ids"]}]
        });
        assert_eq!(
            portable_tool_schema(&schema),
            json!({
                "type": "object",
                "properties": {
                    "sort": {"type": "string", "enum": ["a", "b"], "description": "How to sort."},
                    "set": {"type": ["string", "null"]},
                    "photo_ids": {"type": ["array", "null"], "items": {"type": "string"}}
                }
            })
        );
        assert_eq!(
            portable_tool_schema(&json!({"type": "object"})),
            json!({"type": "object", "properties": {}})
        );
    }
}
