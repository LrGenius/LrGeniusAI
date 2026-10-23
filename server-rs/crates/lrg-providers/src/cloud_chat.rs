//! The agent chat on cloud models and AI servers, through each API's own tool
//! calling — where [`crate::local_chat`] has to make do with a grammar.
//!
//! Three wire formats cover every provider the chat offers:
//!
//! * **OpenAI's Chat Completions** ([`OpenAiChat`]): OpenAI itself, LM Studio,
//!   Ollama (at its `/v1` endpoint) and the user's own AI server. Tool results
//!   go back as `tool` messages, which take text only, so a photo a tool
//!   returned follows in a user message.
//! * **Anthropic's Messages** ([`AnthropicChat`]): a turn with tool calls is
//!   sent back exactly as it came ([`NativeTurn`]), since its thinking blocks
//!   must return unchanged; the system prompt and the conversation so far are
//!   marked for prompt caching.
//! * **Gemini's `generateContent`** ([`GeminiChat`]): the model's turn goes
//!   back as it came too — thought signatures ride on its parts — and every
//!   function response names the call it answers by id.
//!
//! Answers are not streamed yet: the page shows each turn when it is
//! complete. None of these adapters sends a temperature to OpenAI, Anthropic
//! or Gemini, whose current models reject or discourage it.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::chat::{
    portable_tool_schema, ChatDeltas, ChatMessage, ChatModel, ChatPart, ChatRequest, ChatRole,
    ChatTurn, NativeTurn, ToolCall, ToolResult,
};
use crate::ollama::DEFAULT_BASE_URL as DEFAULT_OLLAMA;
use crate::openai_compatible::{ServerUrl, DEFAULT_LMSTUDIO_HOST as DEFAULT_LMSTUDIO};
use crate::provider::ProviderSelection;

const ANTHROPIC_API: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const GEMINI_API: &str = "https://generativelanguage.googleapis.com/v1beta";
const OPENAI_API: &str = "https://api.openai.com/v1";

const CHAT_TIMEOUT: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Retries for a rate limit, an overloaded API or a dropped connection.
const RETRIES: u32 = 2;

/// What the chat budgets for a cloud model's context. Their windows are far
/// larger, but every turn sends the whole conversation again — and is billed
/// for it — so a long chat lets its oldest tool results go well before that.
const CLOUD_BUDGET: u32 = 64_000;

/// Photos that stay attached, newest first; older ones become a line of text.
/// Each costs about as much as a page of text, every turn.
const CLOUD_IMAGES: usize = 4;

/// Tokens a model may think on top of the answer. Thinking counts against the
/// output limit on Anthropic, Gemini and OpenAI's reasoning models, and an
/// answer cut off mid-call is worth nothing.
const THINKING_HEADROOM: u32 = 8_192;

/// The same for a model on an AI server, which may think as well but often
/// runs with a small context window that a large limit would not fit.
const SERVER_HEADROOM: u32 = 2_048;

/// The chat model for a cloud provider or AI server, or why there is none.
///
/// `Err` carries a user-facing message, like [`crate::provider::build_provider`]'s.
pub fn build_chat_model(
    selection: &ProviderSelection,
    model: &str,
) -> Result<Arc<dyn ChatModel>, String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("No model is chosen for the chat.".to_string());
    }
    let key = selection
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string);
    let base = |url: &Option<String>, default: &str| {
        url.as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .unwrap_or(default)
            .to_string()
    };
    match selection.name.trim().to_lowercase().as_str() {
        "anthropic" => {
            let key = key.ok_or(
                "The Anthropic API key is not set. Enter it under Plug-in \
                                 Manager → Optional AI providers.",
            )?;
            Ok(Arc::new(AnthropicChat::new(ANTHROPIC_API, key, model)))
        }
        "gemini" => {
            let key = key.ok_or(
                "The Google Gemini API key is not set. Enter it under Plug-in \
                                 Manager → Optional AI providers.",
            )?;
            Ok(Arc::new(GeminiChat::new(GEMINI_API, key, model)))
        }
        "chatgpt" | "openai" => {
            let key = key.ok_or(
                "The OpenAI API key is not set. Enter it under Plug-in Manager \
                                 → Optional AI providers.",
            )?;
            Ok(Arc::new(OpenAiChat::new(
                &format!("{OPENAI_API}/chat/completions"),
                Some(key),
                model,
                "OpenAI",
                true,
            )))
        }
        "lmstudio" => {
            let url = ServerUrl::parse(&base(&selection.lmstudio_base_url, DEFAULT_LMSTUDIO))?;
            Ok(Arc::new(OpenAiChat::new(
                &url.chat_url(),
                None,
                model,
                "LM Studio",
                false,
            )))
        }
        "ollama" => {
            let root = base(&selection.ollama_base_url, DEFAULT_OLLAMA);
            let url = ServerUrl::parse(&format!("{}/v1", root.trim_end_matches('/')))?;
            Ok(Arc::new(OpenAiChat::new(
                &url.chat_url(),
                None,
                model,
                "Ollama",
                false,
            )))
        }
        "openai_compatible" => {
            let address = selection
                .server_url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .ok_or(
                    "No AI server address is set. Enter it under Plug-in Manager → Optional \
                        AI providers → Other AI server.",
                )?;
            let url = ServerUrl::parse(address)?;
            let label = url.label.clone();
            Ok(Arc::new(OpenAiChat::new(
                &url.chat_url(),
                key,
                model,
                &label,
                false,
            )))
        }
        other => Err(format!("The chat cannot use the provider '{other}'.")),
    }
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .unwrap_or_default()
}

/// POST `body` and read the JSON answer, retrying what is worth retrying.
/// `name` is how the service is named in messages; `key_set` tells a missing
/// key from a wrong one.
async fn post_json(
    build: impl Fn() -> reqwest::RequestBuilder,
    body: &Value,
    name: &str,
    key_set: bool,
) -> Result<Value, String> {
    let mut attempt = 0;
    loop {
        let response = match build().json(body).timeout(CHAT_TIMEOUT).send().await {
            Ok(response) => response,
            Err(e) if e.is_connect() && attempt < RETRIES => {
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(2 * u64::from(attempt))).await;
                continue;
            }
            Err(e) if e.is_timeout() => {
                return Err(format!(
                    "{name} did not answer within {} seconds. Try again.",
                    CHAT_TIMEOUT.as_secs()
                ))
            }
            Err(e) if e.is_connect() => {
                return Err(format!(
                    "Could not reach {name}. Check the internet connection, or that the server \
                     is running."
                ))
            }
            Err(e) => return Err(format!("The request to {name} failed: {e}")),
        };
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let text = response
            .text()
            .await
            .map_err(|e| format!("The answer from {name} could not be read: {e}"))?;
        if (200..300).contains(&status) {
            return serde_json::from_str(&text)
                .map_err(|_| format!("{name} answered with something that is not JSON."));
        }
        if matches!(status, 429 | 500 | 502 | 503 | 504 | 529) && attempt < RETRIES {
            attempt += 1;
            log::info!("Chat: {name} answered HTTP {status}; retry {attempt} of {RETRIES}");
            let wait = retry_after.unwrap_or(2 * u64::from(attempt)).min(30);
            tokio::time::sleep(Duration::from_secs(wait)).await;
            continue;
        }
        return Err(status_message(name, status, &error_detail(&text), key_set));
    }
}

/// The error message in an API's error body, whichever shape it has.
fn error_detail(body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let detail = parsed
        .as_ref()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("message"))
                .or_else(|| v.get("error"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.trim().to_string());
    if detail.chars().count() > 300 {
        format!("{}…", detail.chars().take(300).collect::<String>())
    } else {
        detail
    }
}

fn status_message(name: &str, status: u16, detail: &str, key_set: bool) -> String {
    let detail_suffix = if detail.is_empty() {
        String::new()
    } else {
        format!(" ({detail})")
    };
    let key_problem = detail.to_ascii_lowercase().contains("api key")
        || detail.to_ascii_lowercase().contains("api_key");
    match status {
        401 | 403 if key_set => format!(
            "{name} rejected the API key. Check it under Plug-in Manager → Optional AI \
             providers.{detail_suffix}"
        ),
        401 | 403 => format!(
            "{name} needs an API key. Enter it under Plug-in Manager → Optional AI \
             providers.{detail_suffix}"
        ),
        400 if key_problem => format!(
            "{name} rejected the API key. Check it under Plug-in Manager → Optional AI \
             providers.{detail_suffix}"
        ),
        404 => format!(
            "{name} does not know this model. Pick another one for the chat.{detail_suffix}"
        ),
        429 => format!(
            "{name} is rate limiting requests, or the account has run out of credit. Wait a \
             little, or check the account.{detail_suffix}"
        ),
        s if s >= 500 => {
            format!("{name} had an internal error (HTTP {s}). Try again.{detail_suffix}")
        }
        s => format!("{name} refused the request (HTTP {s}).{detail_suffix}"),
    }
}

fn base64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// The conversation with all but the newest [`CLOUD_IMAGES`] photos replaced
/// by a line of text.
fn with_recent_images(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    const EARLIER: &str = "[a photo shown earlier, no longer attached]";
    let mut keep = CLOUD_IMAGES;
    let mut out: Vec<ChatMessage> = messages.to_vec();
    for message in out.iter_mut().rev() {
        let mut parts = Vec::with_capacity(message.parts.len());
        for part in message.parts.drain(..).rev() {
            match part {
                ChatPart::Image(image) => {
                    if keep > 0 {
                        keep -= 1;
                        parts.push(ChatPart::Image(image));
                    } else {
                        parts.push(ChatPart::Text(EARLIER.to_string()));
                    }
                }
                ChatPart::ToolResult(mut result) => {
                    let mut kept = Vec::new();
                    for image in result.images.drain(..).rev() {
                        if keep > 0 {
                            keep -= 1;
                            kept.push(image);
                        } else {
                            result.text.push('\n');
                            result.text.push_str(EARLIER);
                        }
                    }
                    kept.reverse();
                    result.images = kept;
                    parts.push(ChatPart::ToolResult(result));
                }
                other => parts.push(other),
            }
        }
        parts.reverse();
        message.parts = parts;
    }
    out
}

/// The content of every native turn of `api` in a message, or `None` when it
/// has none and has to be rebuilt from its neutral parts.
fn native_content(message: &ChatMessage, api: &str) -> Option<Vec<Value>> {
    let natives: Vec<&NativeTurn> = message
        .parts
        .iter()
        .filter_map(|p| match p {
            ChatPart::Native(native) if native.api == api => Some(native),
            _ => None,
        })
        .collect();
    (!natives.is_empty()).then(|| natives.iter().flat_map(|n| n.content.clone()).collect())
}

fn tool_results(message: &ChatMessage) -> impl Iterator<Item = &ToolResult> {
    message.parts.iter().filter_map(|p| match p {
        ChatPart::ToolResult(result) => Some(result),
        _ => None,
    })
}

/// Ids for tool calls that came without one.
fn call_id(given: Option<&str>, index: usize) -> String {
    given
        .filter(|id| !id.is_empty())
        .map_or_else(|| format!("call_{index}"), str::to_string)
}

fn u32_at(value: &Value, pointer: &str) -> u32 {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// OpenAI Chat Completions
// ---------------------------------------------------------------------------

/// OpenAI, LM Studio, Ollama and the user's own AI server.
pub struct OpenAiChat {
    client: reqwest::Client,
    url: String,
    api_key: Option<String>,
    model: String,
    label: String,
    /// OpenAI itself: `max_completion_tokens`, and no temperature, which its
    /// reasoning models reject. Other servers get `max_tokens` and the
    /// chat's temperature.
    official: bool,
}

impl OpenAiChat {
    #[must_use]
    pub fn new(
        url: &str,
        api_key: Option<String>,
        model: &str,
        label: &str,
        official: bool,
    ) -> Self {
        OpenAiChat {
            client: http_client(),
            url: url.to_string(),
            api_key,
            model: model.to_string(),
            label: label.to_string(),
            official,
        }
    }

    fn body(&self, request: &ChatRequest) -> Value {
        let mut messages = vec![json!({"role": "system", "content": request.system})];
        for message in with_recent_images(&request.messages) {
            match message.role {
                ChatRole::Assistant => {
                    let text: String = message
                        .parts
                        .iter()
                        .filter_map(|p| match p {
                            ChatPart::Text(t) => Some(t.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let calls: Vec<Value> = message
                        .parts
                        .iter()
                        .filter_map(|p| match p {
                            ChatPart::ToolCall(call) => Some(json!({
                                "id": call.id,
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": call.arguments.to_string(),
                                },
                            })),
                            _ => None,
                        })
                        .collect();
                    let mut entry = json!({"role": "assistant"});
                    entry["content"] = if text.is_empty() {
                        Value::Null
                    } else {
                        json!(text)
                    };
                    if !calls.is_empty() {
                        entry["tool_calls"] = json!(calls);
                    }
                    messages.push(entry);
                }
                ChatRole::User => {
                    // Tool results first: they must follow the call right away.
                    let mut content: Vec<Value> = Vec::new();
                    for result in tool_results(&message) {
                        let text = if result.is_error {
                            format!("Error: {}", result.text)
                        } else {
                            result.text.clone()
                        };
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": result.call_id,
                            "content": text,
                        }));
                        if !result.images.is_empty() {
                            content.push(json!({
                                "type": "text",
                                "text": format!("Photos from {}:", result.name),
                            }));
                        }
                        for image in &result.images {
                            content.push(image_url(&image.mime_type, &image.data));
                        }
                    }
                    for part in &message.parts {
                        match part {
                            ChatPart::Text(text) => {
                                content.push(json!({"type": "text", "text": text}));
                            }
                            ChatPart::Image(image) => {
                                content.push(image_url(&image.mime_type, &image.data));
                            }
                            _ => {}
                        }
                    }
                    if !content.is_empty() {
                        messages.push(json!({"role": "user", "content": content}));
                    }
                }
            }
        }

        let mut body = json!({"model": self.model, "messages": messages});
        if !request.tools.is_empty() {
            body["tools"] = request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": portable_tool_schema(&tool.input_schema),
                        },
                    })
                })
                .collect();
        }
        if self.official {
            body["max_completion_tokens"] = json!(request.max_tokens + THINKING_HEADROOM);
        } else {
            body["max_tokens"] = json!(request.max_tokens + SERVER_HEADROOM);
            body["temperature"] = json!(request.temperature);
        }
        body
    }

    fn parse(&self, answer: &Value) -> Result<ChatTurn, String> {
        let choice = answer
            .pointer("/choices/0")
            .ok_or_else(|| format!("{} sent an answer without a choice.", self.label))?;
        let message = &choice["message"];
        let text = message["content"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        let reasoning = message
            .get("reasoning_content")
            .or_else(|| message.get("reasoning"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_string);
        let mut tool_calls = Vec::new();
        for (index, call) in message["tool_calls"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            // Arguments arrive as a JSON string; a broken one goes to the tool
            // as no arguments, and the tool says what is missing.
            let arguments = match call.pointer("/function/arguments") {
                Some(Value::String(raw)) => {
                    serde_json::from_str(raw).unwrap_or_else(|_| Value::Object(Map::new()))
                }
                Some(Value::Object(object)) => Value::Object(object.clone()),
                _ => Value::Object(Map::new()),
            };
            tool_calls.push(ToolCall {
                id: call_id(call["id"].as_str(), index + 1),
                name,
                arguments,
            });
        }
        if text.is_empty() && tool_calls.is_empty() {
            return Err(match choice["finish_reason"].as_str() {
                Some("length") => format!(
                    "{} ran out of room before it answered. Ask for less at once.",
                    self.label
                ),
                Some("content_filter") => format!("{} declined to answer this.", self.label),
                _ => format!("{} sent an empty answer. Try again.", self.label),
            });
        }
        Ok(ChatTurn {
            reasoning,
            text,
            tool_calls,
            input_tokens: u32_at(answer, "/usage/prompt_tokens"),
            output_tokens: u32_at(answer, "/usage/completion_tokens"),
            native: None,
        })
    }
}

fn image_url(mime_type: &str, data: &[u8]) -> Value {
    json!({
        "type": "image_url",
        "image_url": {"url": format!("data:{mime_type};base64,{}", base64(data))},
    })
}

#[async_trait]
impl ChatModel for OpenAiChat {
    fn name(&self) -> String {
        format!("{} ({})", self.model, self.label)
    }

    fn context_window(&self) -> Option<u32> {
        Some(CLOUD_BUDGET)
    }

    fn supports_images(&self) -> bool {
        true
    }

    async fn complete(
        &self,
        request: &ChatRequest,
        _deltas: Option<ChatDeltas>,
    ) -> Result<ChatTurn, String> {
        let body = self.body(request);
        let answer = post_json(
            || {
                let builder = self.client.post(&self.url);
                match &self.api_key {
                    Some(key) => builder.bearer_auth(key),
                    None => builder,
                }
            },
            &body,
            &self.label,
            self.api_key.is_some(),
        )
        .await?;
        // Some servers report a failure with a 200 and an `error` body.
        if answer.get("choices").is_none() {
            if let Some(error) = answer.get("error") {
                return Err(status_message(
                    &self.label,
                    400,
                    &error_detail(&json!({ "error": error }).to_string()),
                    self.api_key.is_some(),
                ));
            }
        }
        self.parse(&answer)
    }
}

// ---------------------------------------------------------------------------
// Anthropic Messages
// ---------------------------------------------------------------------------

/// Claude through Anthropic's Messages API.
pub struct AnthropicChat {
    client: reqwest::Client,
    api: String,
    api_key: String,
    model: String,
}

impl AnthropicChat {
    #[must_use]
    pub fn new(api: &str, api_key: String, model: &str) -> Self {
        AnthropicChat {
            client: http_client(),
            api: api.trim_end_matches('/').to_string(),
            api_key,
            model: model.to_string(),
        }
    }

    fn body(&self, request: &ChatRequest) -> Value {
        let mut messages: Vec<Value> = Vec::new();
        for message in with_recent_images(&request.messages) {
            let (role, content) = match message.role {
                ChatRole::Assistant => (
                    "assistant",
                    native_content(&message, "anthropic")
                        .unwrap_or_else(|| anthropic_assistant(&message)),
                ),
                ChatRole::User => ("user", anthropic_user(&message)),
            };
            if content.is_empty() {
                continue;
            }
            // Turns must alternate; two of the same in a row become one.
            match messages.last_mut() {
                Some(last) if last["role"] == role => {
                    if let Some(blocks) = last["content"].as_array_mut() {
                        blocks.extend(content);
                    }
                }
                _ => messages.push(json!({"role": role, "content": content})),
            }
        }
        // The conversation so far is cached up to its last block, so the next
        // turn reads it from the cache rather than paying for it in full.
        if let Some(block) = messages
            .last_mut()
            .and_then(|m| m["content"].as_array_mut())
            .and_then(|blocks| blocks.last_mut())
        {
            block["cache_control"] = json!({"type": "ephemeral"});
        }

        let mut body = json!({
            "model": self.model,
            "max_tokens": request.max_tokens + THINKING_HEADROOM,
            "system": [{
                "type": "text",
                "text": request.system,
                "cache_control": {"type": "ephemeral"},
            }],
            "messages": messages,
        });
        if !request.tools.is_empty() {
            body["tools"] = request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": portable_tool_schema(&tool.input_schema),
                    })
                })
                .collect();
        }
        body
    }

    fn parse(answer: &Value) -> Result<ChatTurn, String> {
        let blocks = answer["content"].as_array().cloned().unwrap_or_default();
        let mut text = Vec::new();
        let mut reasoning = Vec::new();
        let mut tool_calls = Vec::new();
        for (index, block) in blocks.iter().enumerate() {
            match block["type"].as_str() {
                Some("text") => text.push(block["text"].as_str().unwrap_or_default().to_string()),
                Some("thinking") => {
                    reasoning.push(block["thinking"].as_str().unwrap_or_default().to_string());
                }
                Some("tool_use") => tool_calls.push(ToolCall {
                    id: call_id(block["id"].as_str(), index + 1),
                    name: block["name"].as_str().unwrap_or_default().to_string(),
                    arguments: block
                        .get("input")
                        .cloned()
                        .unwrap_or_else(|| Value::Object(Map::new())),
                }),
                _ => {}
            }
        }
        let text = text.join("\n").trim().to_string();
        let stop = answer["stop_reason"].as_str().unwrap_or_default();
        if tool_calls.is_empty() && text.is_empty() {
            return Err(match stop {
                "refusal" => "Claude declined to answer this.".to_string(),
                "max_tokens" => {
                    "Claude ran out of room before it answered. Ask for less at once.".to_string()
                }
                _ => "Claude sent an empty answer. Try again.".to_string(),
            });
        }
        if stop == "max_tokens" && !tool_calls.is_empty() {
            return Err(
                "Claude ran out of room in the middle of a step. Ask for less at once.".to_string(),
            );
        }
        let reasoning = reasoning.join("\n").trim().to_string();
        Ok(ChatTurn {
            reasoning: (!reasoning.is_empty()).then_some(reasoning),
            text,
            tool_calls,
            // What the context holds, cached or not.
            input_tokens: u32_at(answer, "/usage/input_tokens")
                + u32_at(answer, "/usage/cache_read_input_tokens")
                + u32_at(answer, "/usage/cache_creation_input_tokens"),
            output_tokens: u32_at(answer, "/usage/output_tokens"),
            native: Some(NativeTurn {
                api: "anthropic",
                content: blocks,
            }),
        })
    }
}

fn anthropic_image(mime_type: &str, data: &[u8]) -> Value {
    json!({
        "type": "image",
        "source": {"type": "base64", "media_type": mime_type, "data": base64(data)},
    })
}

/// A turn of Claude's rebuilt from neutral parts — only when another model
/// wrote it. Its reasoning cannot go back without Anthropic's signature.
fn anthropic_assistant(message: &ChatMessage) -> Vec<Value> {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Text(text) if !text.is_empty() => Some(json!({"type": "text", "text": text})),
            ChatPart::ToolCall(call) => Some(json!({
                "type": "tool_use",
                "id": call.id,
                "name": call.name,
                "input": call.arguments,
            })),
            _ => None,
        })
        .collect()
}

fn anthropic_user(message: &ChatMessage) -> Vec<Value> {
    // Tool results must come first in the turn that answers the calls.
    let mut content: Vec<Value> = tool_results(message)
        .map(|result| {
            let mut blocks = vec![json!({"type": "text", "text": result.text})];
            blocks.extend(
                result
                    .images
                    .iter()
                    .map(|image| anthropic_image(&image.mime_type, &image.data)),
            );
            json!({
                "type": "tool_result",
                "tool_use_id": result.call_id,
                "content": blocks,
                "is_error": result.is_error,
            })
        })
        .collect();
    for part in &message.parts {
        match part {
            ChatPart::Text(text) if !text.is_empty() => {
                content.push(json!({"type": "text", "text": text}));
            }
            ChatPart::Image(image) => content.push(anthropic_image(&image.mime_type, &image.data)),
            _ => {}
        }
    }
    content
}

#[async_trait]
impl ChatModel for AnthropicChat {
    fn name(&self) -> String {
        format!("{} (Anthropic)", self.model)
    }

    fn context_window(&self) -> Option<u32> {
        Some(CLOUD_BUDGET)
    }

    fn supports_images(&self) -> bool {
        true
    }

    async fn complete(
        &self,
        request: &ChatRequest,
        _deltas: Option<ChatDeltas>,
    ) -> Result<ChatTurn, String> {
        let url = format!("{}/messages", self.api);
        let answer = post_json(
            || {
                self.client
                    .post(&url)
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", ANTHROPIC_VERSION)
            },
            &self.body(request),
            "Anthropic",
            true,
        )
        .await?;
        Self::parse(&answer)
    }
}

// ---------------------------------------------------------------------------
// Gemini generateContent
// ---------------------------------------------------------------------------

/// Gemini through Google's `generateContent`.
pub struct GeminiChat {
    client: reqwest::Client,
    api: String,
    api_key: String,
    model: String,
}

impl GeminiChat {
    #[must_use]
    pub fn new(api: &str, api_key: String, model: &str) -> Self {
        GeminiChat {
            client: http_client(),
            api: api.trim_end_matches('/').to_string(),
            api_key,
            // The model list names them `models/gemini-…`; the URL adds that.
            model: model.trim_start_matches("models/").to_string(),
        }
    }

    fn body(&self, request: &ChatRequest) -> Value {
        let mut contents: Vec<Value> = Vec::new();
        for message in with_recent_images(&request.messages) {
            let (role, parts) = match message.role {
                ChatRole::Assistant => (
                    "model",
                    native_content(&message, "gemini").unwrap_or_else(|| gemini_model(&message)),
                ),
                ChatRole::User => ("user", gemini_user(&message)),
            };
            if parts.is_empty() {
                continue;
            }
            match contents.last_mut() {
                Some(last) if last["role"] == role => {
                    if let Some(existing) = last["parts"].as_array_mut() {
                        existing.extend(parts);
                    }
                }
                _ => contents.push(json!({"role": role, "parts": parts})),
            }
        }
        let mut body = json!({
            "systemInstruction": {"parts": [{"text": request.system}]},
            "contents": contents,
            "generationConfig": {"maxOutputTokens": request.max_tokens + THINKING_HEADROOM},
        });
        if !request.tools.is_empty() {
            let declarations: Vec<Value> = request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "parametersJsonSchema": portable_tool_schema(&tool.input_schema),
                    })
                })
                .collect();
            body["tools"] = json!([{"functionDeclarations": declarations}]);
        }
        body
    }

    fn parse(answer: &Value) -> Result<ChatTurn, String> {
        let Some(candidate) = answer.pointer("/candidates/0") else {
            let reason = answer
                .pointer("/promptFeedback/blockReason")
                .and_then(Value::as_str);
            return Err(match reason {
                Some(reason) => format!("Gemini blocked the request ({reason})."),
                None => "Gemini sent an answer without a candidate. Try again.".to_string(),
            });
        };
        let parts = candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut text = Vec::new();
        let mut reasoning = Vec::new();
        let mut tool_calls = Vec::new();
        for part in &parts {
            if let Some(call) = part.get("functionCall") {
                tool_calls.push(ToolCall {
                    id: call_id(call["id"].as_str(), tool_calls.len() + 1),
                    name: call["name"].as_str().unwrap_or_default().to_string(),
                    arguments: call
                        .get("args")
                        .cloned()
                        .unwrap_or_else(|| Value::Object(Map::new())),
                });
            } else if let Some(t) = part["text"].as_str() {
                if part["thought"].as_bool() == Some(true) {
                    reasoning.push(t.to_string());
                } else {
                    text.push(t.to_string());
                }
            }
        }
        let text = text.join("").trim().to_string();
        let finish = candidate["finishReason"].as_str().unwrap_or_default();
        if tool_calls.is_empty() && text.is_empty() {
            return Err(match finish {
                "MAX_TOKENS" => {
                    "Gemini ran out of room before it answered. Ask for less at once.".to_string()
                }
                "SAFETY" | "PROHIBITED_CONTENT" | "BLOCKLIST" | "RECITATION" => {
                    format!("Gemini declined to answer this ({finish}).")
                }
                _ => "Gemini sent an empty answer. Try again.".to_string(),
            });
        }
        let reasoning = reasoning.join("").trim().to_string();
        Ok(ChatTurn {
            reasoning: (!reasoning.is_empty()).then_some(reasoning),
            text,
            tool_calls,
            input_tokens: u32_at(answer, "/usageMetadata/promptTokenCount"),
            output_tokens: u32_at(answer, "/usageMetadata/candidatesTokenCount")
                + u32_at(answer, "/usageMetadata/thoughtsTokenCount"),
            native: Some(NativeTurn {
                api: "gemini",
                content: parts,
            }),
        })
    }
}

fn inline_data(mime_type: &str, data: &[u8]) -> Value {
    json!({"inlineData": {"mimeType": mime_type, "data": base64(data)}})
}

/// A model turn rebuilt from neutral parts — only when another model wrote it.
fn gemini_model(message: &ChatMessage) -> Vec<Value> {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Text(text) if !text.is_empty() => Some(json!({"text": text})),
            ChatPart::ToolCall(call) => Some(json!({
                "functionCall": {"id": call.id, "name": call.name, "args": call.arguments},
            })),
            _ => None,
        })
        .collect()
}

fn gemini_user(message: &ChatMessage) -> Vec<Value> {
    let mut parts: Vec<Value> = Vec::new();
    let mut photos: Vec<Value> = Vec::new();
    for result in tool_results(message) {
        let response = if result.is_error {
            json!({"error": result.text})
        } else {
            json!({"result": result.text})
        };
        parts.push(json!({
            "functionResponse": {"id": result.call_id, "name": result.name, "response": response},
        }));
        photos.extend(
            result
                .images
                .iter()
                .map(|image| inline_data(&image.mime_type, &image.data)),
        );
    }
    parts.extend(photos);
    for part in &message.parts {
        match part {
            ChatPart::Text(text) if !text.is_empty() => parts.push(json!({"text": text})),
            ChatPart::Image(image) => parts.push(inline_data(&image.mime_type, &image.data)),
            _ => {}
        }
    }
    parts
}

#[async_trait]
impl ChatModel for GeminiChat {
    fn name(&self) -> String {
        format!("{} (Gemini)", self.model)
    }

    fn context_window(&self) -> Option<u32> {
        Some(CLOUD_BUDGET)
    }

    fn supports_images(&self) -> bool {
        true
    }

    async fn complete(
        &self,
        request: &ChatRequest,
        _deltas: Option<ChatDeltas>,
    ) -> Result<ChatTurn, String> {
        let url = format!("{}/models/{}:generateContent", self.api, self.model);
        let answer = post_json(
            || {
                self.client
                    .post(&url)
                    .header("x-goog-api-key", &self.api_key)
            },
            &self.body(request),
            "Gemini",
            true,
        )
        .await?;
        Self::parse(&answer)
    }
}
