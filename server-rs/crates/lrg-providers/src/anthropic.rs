//! Claude through Anthropic's own Messages API (`POST /v1/messages`,
//! `GET /v1/models`), talked to directly via `reqwest` — there is no official
//! Anthropic SDK for Rust.
//!
//! Not through the OpenAI compatibility layer Anthropic also serves at
//! `api.anthropic.com/v1/chat/completions`, which the Other AI server can be
//! pointed at. That layer covers the chat call only: its `/v1/models` is this
//! native route, which answers an OpenAI client with "anthropic-version:
//! header is required", and it silently ignores `response_format`, so a photo
//! request there gets no answer format at all. Here the answer is constrained
//! to the schema, and the run-constant half of the prompt is cached.
//!
//! What differs from the other cloud providers, and why:
//!
//! * **Schema.** Structured outputs (`output_config.format`) accept a subset
//!   of JSON Schema: numeric and length bounds are rejected with a 400,
//!   `minItems` only as 0 or 1, and every object must say
//!   `additionalProperties: false`. [`anthropic_schema`] moves the rejected
//!   bounds into the field's description, as Anthropic's own SDKs do.
//! * **Model capabilities.** Whether a model reads images, takes `effort`, or
//!   thinks differs per model, and `/v1/models` publishes it. It is read from
//!   there rather than from a list of model names that goes stale with the
//!   next release — sending `effort` to a model without it is a 400.
//! * **No temperature.** Current Claude models reject sampling parameters
//!   with a 400, so the plug-in's temperature setting does not apply here.
//! * **Thinking.** Newer models think by default, and some cannot be told not
//!   to. Thinking is billed as output and counts against `max_tokens`, so a
//!   request sends the plug-in's Analysis depth as `effort` where the model
//!   takes that level — `low` by default, since describing a photo rarely
//!   gains from long deliberation — and adds
//!   [`ReasoningEffort::thinking_headroom`] to the token limit, so thinking
//!   the user never sees does not cut the answer off.
//! * **Refusals.** A model's safety classifier can decline with a 200 and
//!   `stop_reason: "refusal"`. Where Anthropic offers it, the request opts
//!   into server-side fallback, and a photo another model answered says so.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use reqwest::Method;
use serde_json::{json, Map, Value};
use tokio::sync::Mutex;

use crate::edit_recipe::{normalize_edit_recipe, openai_edit_recipe_schema};
use crate::image_encode::image_to_base64;
use crate::keyword_taxonomy::KeywordLeafEncoding;
use crate::normalize::{
    alt_text_from, extract_json_value, join_warnings, missing_field_warning, normalize_keywords,
};
use crate::prompts::{
    prepare_edit_system_prompt, prepare_edit_user_prompt_split, prepare_system_prompt,
    prepare_user_prompt_split, SplitPrompt,
};
use crate::schema::prepare_response_structure;
use crate::types::{
    EditGenerationRequest, EditGenerationResponse, MetadataGenerationRequest,
    MetadataGenerationResponse, ReasoningEffort,
};

const API_BASE: &str = "https://api.anthropic.com/v1";
/// Every request carries it; without it the API answers 400 "anthropic-version:
/// header is required" — what the Other AI server ran into when pointed here.
const API_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 2048;
/// The default for [`AnthropicProvider::generate_text`] when the caller names
/// no model.
const DEFAULT_TEXT_MODEL: &str = "claude-opus-5-5";
const GENERATION_TIMEOUT: Duration = Duration::from_secs(300);
const TEXT_TIMEOUT: Duration = Duration::from_secs(120);
/// Listing runs inside the plug-in's model picker, alongside every other
/// provider: a hanging network must not hold them all up.
const LIST_TIMEOUT: Duration = Duration::from_secs(10);
const CACHE_TTL: Duration = Duration::from_secs(3600);
/// Retries for a rate limit, an overloaded API or a dropped connection. Only
/// for generation: listing has to answer within the plug-in's own timeout.
const MAX_RETRIES: u32 = 2;
const MAX_RETRY_WAIT: Duration = Duration::from_secs(30);

/// Server-side fallback: when a model's safety classifier declines, Anthropic
/// re-runs the request on the model it recommends for that kind of refusal
/// instead of returning the refusal. Offered for these models only; sending
/// it for another is not documented to work, so it is not sent.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const FALLBACK_MODELS: [&str; 5] = [
    "claude-fable-5-1",
    "claude-fable-5",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-sonnet-5-5",
];

/// Which `output_config.effort` levels a model accepts, one answer per level.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct EffortLevels {
    low: Option<bool>,
    medium: Option<bool>,
    high: Option<bool>,
}

impl EffortLevels {
    fn accepts(&self, effort: ReasoningEffort) -> Option<bool> {
        match effort {
            ReasoningEffort::Low => self.low,
            ReasoningEffort::Medium => self.medium,
            ReasoningEffort::High => self.high,
        }
    }
}

/// What `/v1/models` says about one model. `None` wherever it did not say —
/// every decision below then takes the choice that cannot cause a 400.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ModelInfo {
    image_input: Option<bool>,
    structured_outputs: Option<bool>,
    effort: EffortLevels,
    thinking: Option<bool>,
    /// The model's own output cap.
    max_output: Option<u32>,
}

impl ModelInfo {
    fn from_entry(entry: &Value) -> ModelInfo {
        let caps = entry.get("capabilities").unwrap_or(&Value::Null);
        let supported = |path: &str| {
            caps.pointer(path)
                .and_then(|c| c.get("supported"))
                .and_then(Value::as_bool)
        };
        // No effort at all means no level of it either, whatever the leaf says.
        let effort_level = |level: &str| match supported("/effort") {
            Some(false) => Some(false),
            _ => supported(&format!("/effort/{level}")),
        };
        ModelInfo {
            image_input: supported("/image_input"),
            structured_outputs: supported("/structured_outputs"),
            effort: EffortLevels {
                low: effort_level("low"),
                medium: effort_level("medium"),
                high: effort_level("high"),
            },
            thinking: supported("/thinking"),
            max_output: entry
                .get("max_tokens")
                .and_then(Value::as_u64)
                .map(|n| u32::try_from(n).unwrap_or(u32::MAX)),
        }
    }

    /// Worth offering in the model picker: it can read a photo and be held to
    /// the answer format. Only an explicit "no" drops a model.
    fn describes_photos(&self) -> bool {
        self.image_input != Some(false) && self.structured_outputs != Some(false)
    }

    /// `max_tokens` for a request whose answer may take `requested` tokens,
    /// thinking at `effort`.
    fn token_limit(&self, requested: u32, effort: ReasoningEffort) -> u32 {
        let limit = if self.thinking == Some(false) {
            requested
        } else {
            requested.saturating_add(effort.thinking_headroom())
        };
        self.max_output.map_or(limit, |cap| limit.min(cap))
    }
}

type ListCache = HashMap<String, (Instant, Vec<String>)>;

/// The model list, 1h per API key, like the other cloud providers.
fn list_cache() -> &'static Mutex<ListCache> {
    static CACHE: OnceLock<Mutex<ListCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(ListCache::new()))
}

/// What each model can do, filled by listing and by
/// [`AnthropicProvider::model_info`]. Capabilities belong to the model, not
/// the key, so one map serves every key.
fn info_cache() -> &'static std::sync::Mutex<HashMap<String, ModelInfo>> {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<String, ModelInfo>>> = OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Models whose request was refused over the `fallbacks` field itself. They
/// are asked without it from then on, for the life of the process.
fn fallback_refused() -> &'static std::sync::Mutex<HashSet<String>> {
    static REFUSED: OnceLock<std::sync::Mutex<HashSet<String>>> = OnceLock::new();
    REFUSED.get_or_init(|| std::sync::Mutex::new(HashSet::new()))
}

fn wants_fallback(model: &str) -> bool {
    FALLBACK_MODELS.contains(&model) && !fallback_refused().lock().unwrap().contains(model)
}

/// Constraints structured outputs reject with a 400. Stripping them is not
/// losing them: they are written into the field's description, so the model
/// still reads them, and every value is checked after parsing anyway
/// (`normalize.rs`, `normalize_edit_recipe`).
const MOVED_TO_DESCRIPTION: [&str; 8] = [
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "maxItems",
];

/// The schema as Anthropic's structured outputs accept it.
///
/// * Bounds it rejects move into the description (`{minimum: 0, maximum: 1}`).
///   `minItems` stays when it is 0 or 1, the only values it takes.
/// * Every object gets `additionalProperties: false`, which is mandatory.
/// * Every property becomes required. The API allows 24 optional properties
///   per request and each one roughly doubles part of the compiled grammar;
///   the edit recipe alone has more than that. OpenAI's strict mode forces
///   the same (`schema_strict.rs`).
pub fn anthropic_schema(schema: &Value) -> Value {
    let Value::Object(obj) = schema else {
        return schema.clone();
    };
    let mut out = Map::new();
    let mut moved: Vec<String> = Vec::new();
    for (key, value) in obj {
        match key.as_str() {
            k if MOVED_TO_DESCRIPTION.contains(&k) => moved.push(format!("{k}: {value}")),
            "minItems" if value.as_u64().is_some_and(|n| n > 1) => {
                moved.push(format!("minItems: {value}"));
            }
            "properties" | "$defs" | "definitions" => {
                let Value::Object(children) = value else {
                    out.insert(key.clone(), value.clone());
                    continue;
                };
                let children = children
                    .iter()
                    .map(|(name, child)| (name.clone(), anthropic_schema(child)))
                    .collect();
                out.insert(key.clone(), Value::Object(children));
            }
            "items" => {
                out.insert(key.clone(), anthropic_schema(value));
            }
            "anyOf" | "allOf" | "oneOf" => {
                let variants = match value {
                    Value::Array(list) => Value::Array(list.iter().map(anthropic_schema).collect()),
                    other => other.clone(),
                };
                out.insert(key.clone(), variants);
            }
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }

    let is_object =
        out.get("type").and_then(Value::as_str) == Some("object") || out.contains_key("properties");
    if is_object {
        out.insert("additionalProperties".into(), Value::Bool(false));
        let names: Vec<String> = out
            .get("properties")
            .and_then(Value::as_object)
            .map(|p| p.keys().cloned().collect())
            .unwrap_or_default();
        let mut required: Vec<Value> = match out.get("required") {
            Some(Value::Array(r)) => r.clone(),
            _ => Vec::new(),
        };
        for name in names {
            if !required.iter().any(|r| r.as_str() == Some(name.as_str())) {
                required.push(Value::String(name));
            }
        }
        out.insert("required".into(), Value::Array(required));
    }

    if !moved.is_empty() {
        let note = format!("{{{}}}", moved.join(", "));
        let description = match out.get("description").and_then(Value::as_str) {
            Some(existing) if !existing.trim().is_empty() => format!("{} {note}", existing.trim()),
            _ => note,
        };
        out.insert("description".into(), Value::String(description));
    }
    Value::Object(out)
}

/// Why a call produced nothing to read.
#[derive(Debug)]
enum CallError {
    /// No answer — connection, timeout, a body that is not JSON. Already a
    /// message for the user.
    Transport(String),
    /// An error status, with the API's own `error.type` and message.
    Status {
        status: u16,
        kind: String,
        message: String,
    },
}

/// `error.type` and `error.message` out of an error body, whatever it is.
fn error_parts(body: &str) -> (String, String) {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        let text = |pointer: &str| {
            value
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };
        let message = text("/error/message");
        if !message.is_empty() {
            return (text("/error/type"), message);
        }
    }
    // A proxy's HTML page or plain text: keep the start of it.
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    (String::new(), flat.chars().take(200).collect())
}

/// 408/409 and 429 are Anthropic's "try again"; 529 is "overloaded".
fn is_retryable(status: u16) -> bool {
    matches!(status, 408 | 409 | 429) || status >= 500
}

/// How long to wait before retry `attempt` (1-based): what `retry-after`
/// says, capped, or 2 s then 4 s.
fn retry_wait(attempt: u32, retry_after: Option<u64>) -> Duration {
    match retry_after {
        Some(secs) => Duration::from_secs(secs).min(MAX_RETRY_WAIT),
        None => Duration::from_secs(1u64 << attempt.min(4)),
    }
}

fn transport_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "Anthropic did not answer in time. The API may be busy; try again.".to_string();
    }
    if error.is_connect() {
        return "Could not reach Anthropic (api.anthropic.com). Check the internet connection."
            .to_string();
    }
    format!("The request to Anthropic failed: {error}")
}

/// Turn an error status into a message the user can act on.
fn http_error(status: u16, kind: &str, message: &str, model: &str) -> String {
    let detail = if message.is_empty() {
        String::new()
    } else {
        format!(" ({message})")
    };
    let lower = message.to_ascii_lowercase();
    match status {
        401 => format!(
            "Anthropic rejected the API key. Check the Anthropic key under Plug-in Manager → \
             Optional AI providers.{detail}"
        ),
        403 => format!(
            "The Anthropic API key is not allowed to do this. Check the key's workspace and \
             permissions in the Anthropic Console.{detail}"
        ),
        404 if !model.is_empty() => format!(
            "Anthropic does not offer the model \"{model}\" to this API key. Pick another Claude \
             model in the model list.{detail}"
        ),
        400 if lower.contains("credit balance") => format!(
            "The Anthropic account has no credit left. Add credit in the Anthropic Console under \
             Billing, then run the task again.{detail}"
        ),
        413 => format!(
            "The photo is too large for Anthropic. Lower the export size under Plug-in Manager → \
             Indexing and export, then try again.{detail}"
        ),
        429 => format!(
            "Anthropic is rate limiting requests. Wait a little and run the task again.{detail}"
        ),
        529 => format!(
            "Anthropic's servers are overloaded right now. Try again in a few minutes.{detail}"
        ),
        s if s >= 500 => {
            format!("Anthropic had an internal error (HTTP {s}). Try again later.{detail}")
        }
        s if kind.is_empty() => format!("Anthropic refused the request (HTTP {s}).{detail}"),
        s => format!("Anthropic refused the request (HTTP {s}, {kind}).{detail}"),
    }
}

/// One `/v1/messages` request, before it is turned into JSON.
struct MessageRequest<'a> {
    model: &'a str,
    system: &'a str,
    prompt: &'a SplitPrompt,
    /// Base64 JPEG, sent after the text.
    image_b64: Option<&'a str>,
    /// Already passed through [`anthropic_schema`].
    schema: Option<Value>,
    max_tokens: u32,
    /// Sent only where `info` says the model takes this level.
    effort: ReasoningEffort,
    info: ModelInfo,
}

impl MessageRequest<'_> {
    /// The request body. `fallback` opts into server-side fallback, which
    /// also needs [`FALLBACK_BETA`] in `anthropic-beta`.
    ///
    /// The run-constant half of the prompt carries the cache breakpoint: the
    /// system prompt and that half are byte-identical for every photo of a
    /// run (see `prompts.rs`), so from the second photo on they are read from
    /// the cache at a tenth of the input price. Below the model's minimum
    /// cacheable length the breakpoint is ignored, at no cost.
    fn body(&self, fallback: bool) -> Value {
        let mut content = Vec::new();
        if !self.prompt.stable.trim().is_empty() {
            content.push(json!({
                "type": "text",
                "text": self.prompt.stable,
                "cache_control": {"type": "ephemeral"},
            }));
        }
        if !self.prompt.per_photo.trim().is_empty() {
            content.push(json!({"type": "text", "text": self.prompt.per_photo}));
        }
        if let Some(data) = self.image_b64 {
            content.push(json!({
                "type": "image",
                "source": {"type": "base64", "media_type": "image/jpeg", "data": data},
            }));
        }

        let mut body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": [{"role": "user", "content": content}],
        });
        if !self.system.trim().is_empty() {
            body["system"] = json!([{"type": "text", "text": self.system}]);
        }
        let mut output_config = Map::new();
        if self.info.effort.accepts(self.effort) == Some(true) {
            output_config.insert("effort".into(), json!(self.effort.as_str()));
        }
        if let Some(schema) = &self.schema {
            output_config.insert(
                "format".into(),
                json!({"type": "json_schema", "schema": schema}),
            );
        }
        if !output_config.is_empty() {
            body["output_config"] = Value::Object(output_config);
        }
        if fallback {
            body["fallbacks"] = json!("default");
        }
        body
    }
}

/// A usable answer, before the photo-specific parsing.
#[derive(Debug, PartialEq)]
struct Answer {
    text: String,
    input_tokens: u32,
    output_tokens: u32,
    /// Set when a fallback model answered instead of the one asked for.
    fallback_warning: Option<String>,
}

/// A failed answer that was still billed.
#[derive(Debug, PartialEq)]
struct Billed {
    error: String,
    input_tokens: u32,
    output_tokens: u32,
}

/// `(input, output)` tokens. Input counts cache reads and writes too: they
/// are prompt tokens the request consumed, just billed at other rates.
fn usage_tokens(result: &Value) -> (u32, u32) {
    let count = |key: &str| {
        result
            .pointer(&format!("/usage/{key}"))
            .and_then(Value::as_u64)
            .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    (
        count("input_tokens")
            .saturating_add(count("cache_creation_input_tokens"))
            .saturating_add(count("cache_read_input_tokens")),
        count("output_tokens"),
    )
}

/// Read a `/v1/messages` answer: the text, or why there is none.
///
/// `requested` is the user's Max Tokens and `sent` the `max_tokens` actually
/// sent, which includes the thinking headroom.
fn read_answer(result: &Value, model: &str, requested: u32, sent: u32) -> Result<Answer, Billed> {
    let (input_tokens, output_tokens) = usage_tokens(result);
    let billed = |error: String| Billed {
        error,
        input_tokens,
        output_tokens,
    };
    let stop_reason = result
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    match stop_reason {
        "end_turn" | "stop_sequence" => {}
        "max_tokens" => return Err(billed(length_error(requested, sent, output_tokens))),
        "refusal" => return Err(billed(refusal_error(result, model))),
        other => {
            return Err(billed(format!(
                "Claude stopped without an answer (stop_reason \"{other}\"). Try the photo again, \
                 or pick another Claude model."
            )))
        }
    }

    let text = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    if text.trim().is_empty() {
        return Err(billed(
            "Claude returned no text for this photo.".to_string(),
        ));
    }

    let fallback_warning = fallback_model(result, model).map(|(declined, answered)| {
        format!(
            "{declined} declined this photo, so Anthropic had {answered} answer instead. Its \
             answer was kept."
        )
    });
    Ok(Answer {
        text,
        input_tokens,
        output_tokens,
        fallback_warning,
    })
}

/// `(declined, answered)` when a fallback model answered instead of `model`.
///
/// Read from the `fallback` block the API puts at each switch. Failing that,
/// from the answer's `model` — but not when one name merely extends the
/// other: an alias is answered under its dated id (`claude-haiku-4-5` →
/// `claude-haiku-4-5-20251001`), and calling that a fallback would warn on
/// every photo of the run.
fn fallback_model(result: &Value, model: &str) -> Option<(String, String)> {
    let block = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|block| block.get("type").and_then(Value::as_str) == Some("fallback"));
    if let Some(block) = block {
        let name = |side: &str| {
            block
                .pointer(&format!("/{side}/model"))
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
                .map(str::to_string)
        };
        let served = result.get("model").and_then(Value::as_str);
        let answered = name("to")
            .or_else(|| served.map(str::to_string))
            .unwrap_or_else(|| "another Claude model".to_string());
        return Some((name("from").unwrap_or_else(|| model.to_string()), answered));
    }
    let served = result.get("model").and_then(Value::as_str)?;
    let related = served.is_empty() || served.starts_with(model) || model.starts_with(served);
    (!related).then(|| (model.to_string(), served.to_string()))
}

/// The error for `stop_reason: "max_tokens"`. Raising the limit is the fix
/// only when the answer is genuinely long; a model that loops hits any limit
/// (see the Gemini provider, issue #368).
fn length_error(requested: u32, sent: u32, output_tokens: u32) -> String {
    let limit = if sent > requested {
        format!(
            "Max Tokens {requested}, plus {} for the model's thinking",
            sent - requested
        )
    } else {
        format!("max_tokens={sent}")
    };
    format!(
        "Claude stopped before finishing the answer because the token limit was reached ({limit}; \
         {output_tokens} output tokens used). Raise the Max Tokens setting in the plugin (General \
         tab → AI Model section) — try 4096 or higher. If the same photo still fails at a higher \
         limit, the model is repeating itself rather than running short of room: switch to a \
         different Claude model."
    )
}

fn refusal_error(result: &Value, model: &str) -> String {
    let details = result.get("stop_details");
    let category = details
        .and_then(|d| d.get("category"))
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty());
    let explanation = details
        .and_then(|d| d.get("explanation"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|e| !e.is_empty());
    let mut msg = format!("{model} declined to describe this photo");
    if let Some(category) = category {
        msg.push_str(&format!(
            " (Anthropic safety filter, category \"{category}\")"
        ));
    }
    msg.push_str(". Pick another Claude model for this photo.");
    if let Some(explanation) = explanation {
        msg.push_str(&format!(" Anthropic says: {explanation}"));
    }
    msg
}

/// The JSON object in an answer: strictly first, then leniently. With the
/// schema enforced the strict parse is what succeeds; the lenient one covers
/// the rare answer the API could not hold to it (a refusal text, a fence).
fn parse_answer(text: &str) -> Result<Value, String> {
    serde_json::from_str::<Value>(text)
        .ok()
        .filter(Value::is_object)
        .or_else(|| extract_json_value(text, '{'))
        .ok_or_else(|| {
            format!(
                "Claude returned an answer that is not the expected JSON (length={} chars). Try \
                 the photo again, or pick another Claude model.",
                text.len()
            )
        })
}

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        AnthropicProvider {
            client: reqwest::Client::new(),
            api_key,
        }
    }

    pub fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }

    /// This provider, or one for the key a request carries instead.
    fn for_key(&self, key: Option<&str>) -> AnthropicProvider {
        let key = key
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .unwrap_or(&self.api_key);
        AnthropicProvider {
            client: self.client.clone(),
            api_key: key.to_string(),
        }
    }

    fn request(&self, method: Method, url: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
    }

    /// Send a request and read the JSON answer, retrying up to `retries`
    /// times on what Anthropic says is worth retrying.
    async fn send_json(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
        timeout: Duration,
        retries: u32,
    ) -> Result<Value, CallError> {
        let mut attempt = 0;
        loop {
            let response = match build().timeout(timeout).send().await {
                Ok(r) => r,
                Err(e) if e.is_connect() && attempt < retries => {
                    attempt += 1;
                    tokio::time::sleep(retry_wait(attempt, None)).await;
                    continue;
                }
                Err(e) => return Err(CallError::Transport(transport_error(&e))),
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
                .map_err(|e| CallError::Transport(transport_error(&e)))?;
            if (200..300).contains(&status) {
                return serde_json::from_str(&text).map_err(|_| {
                    CallError::Transport(
                        "Anthropic answered with something that is not JSON.".to_string(),
                    )
                });
            }
            if is_retryable(status) && attempt < retries {
                attempt += 1;
                log::info!("Anthropic answered HTTP {status}; retry {attempt} of {retries}");
                tokio::time::sleep(retry_wait(attempt, retry_after)).await;
                continue;
            }
            let (kind, message) = error_parts(&text);
            return Err(CallError::Status {
                status,
                kind,
                message,
            });
        }
    }

    /// `POST /v1/messages`. A request refused over the `fallbacks` field is
    /// sent once more without it, and the model is remembered.
    async fn create_message(
        &self,
        request: &MessageRequest<'_>,
        timeout: Duration,
    ) -> Result<Value, String> {
        let url = format!("{API_BASE}/messages");
        let mut fallback = wants_fallback(request.model);
        loop {
            let body = request.body(fallback);
            let result = self
                .send_json(
                    || {
                        let builder = self.request(Method::POST, &url).json(&body);
                        if fallback {
                            builder.header("anthropic-beta", FALLBACK_BETA)
                        } else {
                            builder
                        }
                    },
                    timeout,
                    MAX_RETRIES,
                )
                .await;
            match result {
                Ok(value) => return Ok(value),
                Err(CallError::Status {
                    status: 400,
                    message,
                    ..
                }) if fallback && message.to_ascii_lowercase().contains("fallback") => {
                    log::warn!(
                        "Anthropic refused server-side fallback for {} ({message}); asking without it",
                        request.model
                    );
                    fallback_refused()
                        .lock()
                        .unwrap()
                        .insert(request.model.to_string());
                    fallback = false;
                }
                Err(CallError::Status {
                    status,
                    kind,
                    message,
                }) => return Err(http_error(status, &kind, &message, request.model)),
                Err(CallError::Transport(message)) => return Err(message),
            }
        }
    }

    /// What `model` can do: from the list when it was fetched, else from
    /// `GET /v1/models/{model}`. Unknown when neither answers — never an error,
    /// since every use of it has a safe default.
    async fn model_info(&self, model: &str) -> ModelInfo {
        if let Some(info) = info_cache().lock().unwrap().get(model) {
            return *info;
        }
        // The id goes into the URL path. Anthropic's ids are plain; anything
        // else is not looked up rather than escaped.
        let plain = !model.is_empty()
            && model
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if !plain {
            return ModelInfo::default();
        }
        let url = format!("{API_BASE}/models/{model}");
        match self
            .send_json(|| self.request(Method::GET, &url), LIST_TIMEOUT, 0)
            .await
        {
            Ok(entry) => {
                let info = ModelInfo::from_entry(&entry);
                info_cache().lock().unwrap().insert(model.to_string(), info);
                info
            }
            Err(e) => {
                log::warn!("Could not read the capabilities of {model}: {e:?}");
                ModelInfo::default()
            }
        }
    }

    /// Schema-free completion — see [`crate::provider::LlmProvider::generate_text`].
    pub async fn generate_text(
        &self,
        model: Option<&str>,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Option<String> {
        if !self.is_available() {
            return None;
        }
        let model = model
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .unwrap_or(DEFAULT_TEXT_MODEL);
        let info = self.model_info(model).await;
        let prompt = SplitPrompt {
            stable: user_prompt.to_string(),
            per_photo: String::new(),
        };
        let request = MessageRequest {
            model,
            system: system_prompt,
            prompt: &prompt,
            image_b64: None,
            schema: None,
            max_tokens: info.token_limit(4096, ReasoningEffort::Low),
            effort: ReasoningEffort::Low,
            info,
        };
        let result = match self.create_message(&request, TEXT_TIMEOUT).await {
            Ok(result) => result,
            Err(e) => {
                log::warn!("Anthropic text request failed: {e}");
                return None;
            }
        };
        read_answer(&result, model, 4096, request.max_tokens)
            .ok()
            .map(|answer| answer.text)
    }

    pub async fn generate_metadata(
        &self,
        request: &MetadataGenerationRequest,
    ) -> MetadataGenerationResponse {
        let provider = self.for_key(request.api_key.as_deref());
        if !provider.is_available() {
            return fail(&request.uuid, "Anthropic API not configured".to_string());
        }
        let image_b64 = match image_to_base64(&request.image_data) {
            Ok(b64) => b64,
            Err(e) => return fail(&request.uuid, e.to_string()),
        };
        let info = provider.model_info(&request.model).await;
        let system = prepare_system_prompt(request);
        let prompt = prepare_user_prompt_split(request);
        let requested = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
        let call = MessageRequest {
            model: &request.model,
            system: &system,
            prompt: &prompt,
            image_b64: Some(&image_b64),
            schema: Some(anthropic_schema(&prepare_response_structure(request))),
            max_tokens: info.token_limit(requested, request.reasoning_effort),
            effort: request.reasoning_effort,
            info,
        };
        let result = match provider.create_message(&call, GENERATION_TIMEOUT).await {
            Ok(result) => result,
            Err(e) => return fail(&request.uuid, e),
        };
        let answer = match read_answer(&result, &request.model, requested, call.max_tokens) {
            Ok(answer) => answer,
            Err(billed) => {
                return MetadataGenerationResponse {
                    input_tokens: billed.input_tokens,
                    output_tokens: billed.output_tokens,
                    ..fail(&request.uuid, billed.error)
                }
            }
        };
        let parsed = match parse_answer(&answer.text) {
            Ok(parsed) => parsed,
            Err(e) => {
                return MetadataGenerationResponse {
                    input_tokens: answer.input_tokens,
                    output_tokens: answer.output_tokens,
                    ..fail(&request.uuid, e)
                }
            }
        };

        let keywords = normalize_keywords(
            &parsed.get("keywords").cloned().unwrap_or(json!([])),
            request.keyword_categories.as_ref(),
            KeywordLeafEncoding::for_request(request.bilingual_keywords, request.generate_aliases),
        );
        let text_field = |enabled: bool, key: &str| {
            enabled
                .then(|| parsed.get(key).and_then(Value::as_str).map(str::to_string))
                .flatten()
        };
        let caption = text_field(request.generate_caption, "caption");
        let title = text_field(request.generate_title, "title");
        let alt_text = alt_text_from(&parsed, request.generate_alt_text, caption.as_ref());
        // A requested field the model did not return is a degraded success,
        // not a success: see `missing_field_warning`.
        let warning = join_warnings(
            missing_field_warning(
                request,
                Some(&keywords),
                caption.as_ref(),
                title.as_ref(),
                alt_text.as_ref(),
            ),
            answer.fallback_warning,
        );
        MetadataGenerationResponse {
            uuid: request.uuid.clone(),
            success: true,
            keywords: Some(keywords),
            caption,
            title,
            alt_text,
            input_tokens: answer.input_tokens,
            output_tokens: answer.output_tokens,
            error: None,
            warning,
        }
    }

    pub async fn generate_edit_recipe(
        &self,
        request: &EditGenerationRequest,
    ) -> EditGenerationResponse {
        let provider = self.for_key(request.api_key.as_deref());
        if !provider.is_available() {
            return fail_edit(&request.uuid, "Anthropic API not configured".to_string());
        }
        let image_b64 = match image_to_base64(&request.image_data) {
            Ok(b64) => b64,
            Err(e) => return fail_edit(&request.uuid, e.to_string()),
        };
        let info = provider.model_info(&request.model).await;
        let system = prepare_edit_system_prompt(request);
        let prompt = prepare_edit_user_prompt_split(request);
        let requested = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
        let call = MessageRequest {
            model: &request.model,
            system: &system,
            prompt: &prompt,
            image_b64: Some(&image_b64),
            schema: Some(anthropic_schema(openai_edit_recipe_schema(request.is_raw))),
            max_tokens: info.token_limit(requested, request.reasoning_effort),
            effort: request.reasoning_effort,
            info,
        };
        let result = match provider.create_message(&call, GENERATION_TIMEOUT).await {
            Ok(result) => result,
            Err(e) => return fail_edit(&request.uuid, e),
        };
        let billed_failure =
            |error: String, input_tokens: u32, output_tokens: u32| EditGenerationResponse {
                input_tokens,
                output_tokens,
                ..fail_edit(&request.uuid, error)
            };
        let answer = match read_answer(&result, &request.model, requested, call.max_tokens) {
            Ok(answer) => answer,
            Err(billed) => {
                return billed_failure(billed.error, billed.input_tokens, billed.output_tokens)
            }
        };
        let parsed = match parse_answer(&answer.text) {
            Ok(parsed) => parsed,
            Err(e) => return billed_failure(e, answer.input_tokens, answer.output_tokens),
        };
        EditGenerationResponse {
            uuid: request.uuid.clone(),
            success: true,
            recipe: Some(normalize_edit_recipe(&parsed, request.is_raw)),
            input_tokens: answer.input_tokens,
            output_tokens: answer.output_tokens,
            error: None,
            warning: answer.fallback_warning,
            guardrail_reasons: Vec::new(),
        }
    }

    pub async fn list_available_models(&self) -> Vec<String> {
        self.list_models_checked().await.unwrap_or_default()
    }

    /// The Claude models this key can use that read photos and can be held
    /// to the answer format, or why they could not be listed. A key the user
    /// entered that does not work has to say so, not show an empty list.
    pub async fn list_models_checked(&self) -> Result<Vec<String>, String> {
        if self.api_key.is_empty() {
            return Ok(Vec::new());
        }
        {
            let cache = list_cache().lock().await;
            if let Some((expires_at, models)) = cache.get(&self.api_key) {
                if Instant::now() < *expires_at {
                    return Ok(models.clone());
                }
            }
        }

        let mut entries: Vec<Value> = Vec::new();
        let mut after_id: Option<String> = None;
        for _ in 0..10 {
            let mut url = format!("{API_BASE}/models?limit=1000");
            if let Some(id) = &after_id {
                url.push_str("&after_id=");
                url.push_str(id);
            }
            let page = self
                .send_json(|| self.request(Method::GET, &url), LIST_TIMEOUT, 0)
                .await
                .map_err(|e| match e {
                    CallError::Transport(message) => message,
                    CallError::Status {
                        status,
                        kind,
                        message,
                    } => http_error(status, &kind, &message, ""),
                })?;
            if let Some(data) = page.get("data").and_then(Value::as_array) {
                entries.extend(data.iter().cloned());
            }
            after_id = page
                .get("last_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            if page.get("has_more").and_then(Value::as_bool) != Some(true) || after_id.is_none() {
                break;
            }
        }

        let models = offered_models(&entries);
        if !models.is_empty() {
            list_cache().lock().await.insert(
                self.api_key.clone(),
                (Instant::now() + CACHE_TTL, models.clone()),
            );
        }
        Ok(models)
    }
}

/// The ids worth offering out of a `/v1/models` listing, recording what each
/// model can do on the way.
fn offered_models(entries: &[Value]) -> Vec<String> {
    let mut ids = Vec::new();
    let mut infos = info_cache().lock().unwrap();
    for entry in entries {
        let Some(id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        let info = ModelInfo::from_entry(entry);
        infos.insert(id.to_string(), info);
        if info.describes_photos() && !ids.iter().any(|known| known == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

fn fail(uuid: &str, error: String) -> MetadataGenerationResponse {
    MetadataGenerationResponse {
        uuid: uuid.to_string(),
        success: false,
        error: Some(error),
        ..Default::default()
    }
}

fn fail_edit(uuid: &str, error: String) -> EditGenerationResponse {
    EditGenerationResponse {
        uuid: uuid.to_string(),
        success: false,
        error: Some(error),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key anywhere in a schema, to prove a rejected one survives nowhere.
    fn keys(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(obj) => {
                for (key, child) in obj {
                    out.push(key.clone());
                    keys(child, out);
                }
            }
            Value::Array(list) => list.iter().for_each(|child| keys(child, out)),
            _ => {}
        }
    }

    /// Every object in a schema, at any depth.
    fn objects(value: &Value, out: &mut Vec<Map<String, Value>>) {
        match value {
            Value::Object(obj) => {
                if obj.get("type").and_then(Value::as_str) == Some("object") {
                    out.push(obj.clone());
                }
                obj.values().for_each(|child| objects(child, out));
            }
            Value::Array(list) => list.iter().for_each(|child| objects(child, out)),
            _ => {}
        }
    }

    fn info(thinking: Option<bool>, low_effort: Option<bool>) -> ModelInfo {
        ModelInfo {
            thinking,
            effort: EffortLevels {
                low: low_effort,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn rejected_bounds_move_into_the_description() {
        let schema = json!({"type": "number", "minimum": -100.0, "maximum": 100.0});
        let out = anthropic_schema(&schema);
        assert!(out.get("minimum").is_none() && out.get("maximum").is_none());
        let description = out["description"].as_str().unwrap();
        assert!(description.contains("minimum: -100"), "{description}");
        assert!(description.contains("maximum: 100"), "{description}");

        let described = json!({"type": "string", "description": "A title.", "maxLength": 80});
        assert_eq!(
            anthropic_schema(&described)["description"],
            "A title. {maxLength: 80}"
        );
    }

    #[test]
    fn min_items_stays_only_where_the_api_takes_it() {
        let one =
            anthropic_schema(&json!({"type": "array", "minItems": 1, "items": {"type": "string"}}));
        assert_eq!(one["minItems"], json!(1));
        assert!(one.get("description").is_none());

        let pair = anthropic_schema(&json!({
            "type": "array", "minItems": 2, "maxItems": 2, "items": {"type": "string"}
        }));
        assert!(pair.get("minItems").is_none() && pair.get("maxItems").is_none());
        let description = pair["description"].as_str().unwrap();
        assert!(description.starts_with('{') && description.ends_with('}'));
        assert!(description.contains("minItems: 2"), "{description}");
        assert!(description.contains("maxItems: 2"), "{description}");
        assert_eq!(pair["items"]["type"], "string");
    }

    #[test]
    fn every_object_is_closed_and_fully_required() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": {"type": "string"},
                "b": {"type": "object", "properties": {"c": {"type": "integer"}}},
            },
            "required": ["a"],
        });
        let out = anthropic_schema(&schema);
        assert_eq!(out["additionalProperties"], json!(false));
        assert_eq!(out["required"], json!(["a", "b"]));
        assert_eq!(out["properties"]["b"]["additionalProperties"], json!(false));
        assert_eq!(out["properties"]["b"]["required"], json!(["c"]));
    }

    /// The real schemas, as sent: one rejected keyword anywhere fails every
    /// photo of the run with a 400.
    #[test]
    fn the_real_schemas_carry_nothing_the_api_rejects() {
        use crate::types::{KeywordCategories, MetadataGenerationRequest};

        let request = MetadataGenerationRequest {
            generate_keywords: true,
            generate_caption: true,
            generate_title: true,
            generate_alt_text: true,
            bilingual_keywords: true,
            generate_aliases: true,
            keyword_categories: Some(KeywordCategories::Flat(vec!["People".into()])),
            ..Default::default()
        };
        let schemas = [
            anthropic_schema(&prepare_response_structure(&request)),
            anthropic_schema(openai_edit_recipe_schema(Some(true))),
            anthropic_schema(openai_edit_recipe_schema(Some(false))),
        ];
        for schema in &schemas {
            let mut found = Vec::new();
            keys(schema, &mut found);
            for rejected in MOVED_TO_DESCRIPTION {
                assert!(!found.iter().any(|k| k == rejected), "{rejected} survived");
            }
            let mut found = Vec::new();
            objects(schema, &mut found);
            assert!(!found.is_empty());
            for object in found {
                assert_eq!(object.get("additionalProperties"), Some(&json!(false)));
                let properties = object["properties"].as_object().unwrap();
                let required = object["required"].as_array().unwrap();
                assert_eq!(required.len(), properties.len(), "{object:?}");
            }
        }
    }

    #[test]
    fn capabilities_are_read_from_the_model_listing() {
        let entry = json!({
            "id": "claude-opus-5-5",
            "max_tokens": 128000,
            "capabilities": {
                "image_input": {"supported": true},
                "structured_outputs": {"supported": true},
                "thinking": {"supported": true, "types": {"adaptive": {"supported": true}}},
                "effort": {
                    "supported": true,
                    "low": {"supported": true},
                    "medium": {"supported": true},
                    "high": {"supported": false},
                },
            }
        });
        assert_eq!(
            ModelInfo::from_entry(&entry),
            ModelInfo {
                image_input: Some(true),
                structured_outputs: Some(true),
                effort: EffortLevels {
                    low: Some(true),
                    medium: Some(true),
                    high: Some(false),
                },
                thinking: Some(true),
                max_output: Some(128000),
            }
        );
        // No effort at all means no level of it either, whatever the leaf says.
        let no_effort =
            json!({"capabilities": {"effort": {"supported": false, "low": {"supported": true}}}});
        let levels = ModelInfo::from_entry(&no_effort).effort;
        assert_eq!(levels.low, Some(false));
        assert_eq!(levels.high, Some(false));
        assert_eq!(ModelInfo::from_entry(&json!({})), ModelInfo::default());
    }

    #[test]
    fn only_models_that_cannot_describe_a_photo_are_dropped() {
        let entry = |id: &str, image: Option<bool>, structured: Option<bool>| {
            let mut caps = Map::new();
            if let Some(s) = image {
                caps.insert("image_input".into(), json!({"supported": s}));
            }
            if let Some(s) = structured {
                caps.insert("structured_outputs".into(), json!({"supported": s}));
            }
            json!({"id": id, "capabilities": caps})
        };
        let listed = offered_models(&[
            entry("test-vision", Some(true), Some(true)),
            entry("test-text-only", Some(false), Some(true)),
            entry("test-no-schema", Some(true), Some(false)),
            entry("test-unknown", None, None),
            entry("test-vision", Some(true), Some(true)),
        ]);
        assert_eq!(listed, vec!["test-vision", "test-unknown"]);
    }

    #[test]
    fn thinking_models_get_headroom_within_the_models_cap() {
        use ReasoningEffort::{High, Low};
        // Low keeps the 4096 every request had before Analysis depth existed.
        assert_eq!(info(Some(true), None).token_limit(2048, Low), 2048 + 4096);
        assert_eq!(
            info(Some(true), None).token_limit(2048, High),
            2048 + High.thinking_headroom()
        );
        // Unknown is treated as thinking: headroom costs nothing unless used.
        assert_eq!(info(None, None).token_limit(2048, Low), 2048 + 4096);
        assert_eq!(info(Some(false), None).token_limit(2048, High), 2048);
        let capped = ModelInfo {
            max_output: Some(3000),
            ..Default::default()
        };
        assert_eq!(capped.token_limit(2048, High), 3000);
    }

    fn prompt() -> SplitPrompt {
        SplitPrompt {
            stable: "Describe the photo.".into(),
            per_photo: "Taken in Berlin.".into(),
        }
    }

    #[test]
    fn body_caches_the_stable_prompt_and_sends_the_image_last() {
        let prompt = prompt();
        let request = MessageRequest {
            model: "claude-haiku-4-5",
            system: "You are a photo analyst.",
            prompt: &prompt,
            image_b64: Some("AAAA"),
            schema: Some(json!({"type": "object"})),
            max_tokens: 6144,
            effort: ReasoningEffort::Low,
            info: ModelInfo::default(),
        };
        let body = request.body(false);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["text"], "Describe the photo.");
        assert_eq!(content[0]["cache_control"], json!({"type": "ephemeral"}));
        assert!(content[1].get("cache_control").is_none());
        assert_eq!(content[2]["type"], "image");
        assert_eq!(content[2]["source"]["media_type"], "image/jpeg");
        assert_eq!(body["system"][0]["text"], "You are a photo analyst.");
        assert_eq!(body["max_tokens"], json!(6144));
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        // Current models reject sampling parameters with a 400.
        assert!(body.get("temperature").is_none());
        assert!(body.get("fallbacks").is_none());
    }

    #[test]
    fn effort_is_sent_only_to_models_that_take_it() {
        let prompt = prompt();
        let body_at = |info: ModelInfo, effort: ReasoningEffort| {
            MessageRequest {
                model: "m",
                system: "",
                prompt: &prompt,
                image_b64: None,
                schema: None,
                max_tokens: 100,
                effort,
                info,
            }
            .body(false)
        };
        let body = |info: ModelInfo| body_at(info, ReasoningEffort::Low);
        assert_eq!(
            body(info(None, Some(true)))["output_config"]["effort"],
            "low"
        );
        for unknown_or_no in [None, Some(false)] {
            assert!(body(info(None, unknown_or_no))
                .get("output_config")
                .is_none());
        }
        // The level asked for, where the model lists that level...
        let all_levels = ModelInfo {
            effort: EffortLevels {
                low: Some(true),
                medium: Some(true),
                high: Some(true),
            },
            ..Default::default()
        };
        for effort in [ReasoningEffort::Medium, ReasoningEffort::High] {
            assert_eq!(
                body_at(all_levels, effort)["output_config"]["effort"],
                effort.as_str()
            );
        }
        // ...and nothing where it does not: low support says nothing about high.
        assert!(body_at(info(None, Some(true)), ReasoningEffort::High)
            .get("output_config")
            .is_none());
        // An empty system prompt is left out, not sent as an empty block.
        assert!(body(ModelInfo::default()).get("system").is_none());
    }

    #[test]
    fn fallback_is_offered_only_where_anthropic_has_it() {
        assert!(wants_fallback("claude-opus-5-5"));
        assert!(wants_fallback("claude-sonnet-5-5"));
        assert!(!wants_fallback("claude-haiku-4-5"));
        assert!(!wants_fallback("claude-opus-4-8"));

        let prompt = prompt();
        let request = MessageRequest {
            model: "claude-opus-5-5",
            system: "",
            prompt: &prompt,
            image_b64: None,
            schema: None,
            max_tokens: 100,
            effort: ReasoningEffort::Low,
            info: ModelInfo::default(),
        };
        assert_eq!(request.body(true)["fallbacks"], "default");
    }

    fn answer(stop_reason: &str, model: &str, text: &str) -> Value {
        json!({
            "model": model,
            "stop_reason": stop_reason,
            "content": [
                {"type": "thinking", "thinking": ""},
                {"type": "text", "text": text},
            ],
            "usage": {
                "input_tokens": 100,
                "cache_creation_input_tokens": 20,
                "cache_read_input_tokens": 3000,
                "output_tokens": 50,
            },
        })
    }

    #[test]
    fn an_answer_counts_cached_input_and_skips_thinking_blocks() {
        let read = read_answer(
            &answer("end_turn", "claude-opus-5-5", "{\"title\":\"x\"}"),
            "claude-opus-5-5",
            2048,
            6144,
        )
        .unwrap();
        assert_eq!(
            read,
            Answer {
                text: "{\"title\":\"x\"}".into(),
                input_tokens: 3120,
                output_tokens: 50,
                fallback_warning: None,
            }
        );
    }

    #[test]
    fn a_fallback_answer_is_kept_and_reported() {
        let read = read_answer(
            &answer("end_turn", "claude-opus-4-8", "{}"),
            "claude-opus-5-5",
            2048,
            6144,
        )
        .unwrap();
        let warning = read.fallback_warning.unwrap();
        assert!(warning.contains("claude-opus-5-5 declined"), "{warning}");
        assert!(warning.contains("claude-opus-4-8"), "{warning}");

        // The switch block names both models, and wins over the answer's name.
        let mut switched = answer("end_turn", "claude-opus-5", "{}");
        switched["content"].as_array_mut().unwrap().insert(
            0,
            json!({"type": "fallback", "from": {"model": "claude-opus-5-5"}, "to": {"model": "claude-opus-5"}}),
        );
        let warning = read_answer(&switched, "claude-opus-5-5", 2048, 6144)
            .unwrap()
            .fallback_warning
            .unwrap();
        assert!(
            warning.starts_with(
                "claude-opus-5-5 declined this photo, so Anthropic had claude-opus-5 answer"
            ),
            "{warning}"
        );
    }

    /// An alias comes back under its dated id. That is the same model, and
    /// warning about it would flag every photo of the run.
    #[test]
    fn an_alias_answered_under_its_dated_id_is_not_a_fallback() {
        let read = read_answer(
            &answer("end_turn", "claude-haiku-4-5-20251001", "{}"),
            "claude-haiku-4-5",
            2048,
            6144,
        )
        .unwrap();
        assert_eq!(read.fallback_warning, None);
        let no_model =
            json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "{}"}]});
        assert_eq!(
            read_answer(&no_model, "m", 1, 1).unwrap().fallback_warning,
            None
        );
    }

    #[test]
    fn a_cut_off_answer_explains_the_thinking_headroom() {
        let err = read_answer(&answer("max_tokens", "m", "{"), "m", 2048, 6144).unwrap_err();
        assert!(
            err.error.contains("Max Tokens 2048, plus 4096"),
            "{}",
            err.error
        );
        assert!(err.error.contains("50 output tokens used"), "{}", err.error);
        // Billed, so the token summary must still count it.
        assert_eq!((err.input_tokens, err.output_tokens), (3120, 50));

        let plain = read_answer(&answer("max_tokens", "m", "{"), "m", 2048, 2048).unwrap_err();
        assert!(plain.error.contains("max_tokens=2048"), "{}", plain.error);
    }

    #[test]
    fn a_refusal_names_the_category_and_what_to_do() {
        let mut result = answer("refusal", "claude-sonnet-5-5", "");
        result["stop_details"] = json!({
            "type": "refusal", "category": "general_harms", "explanation": "Not allowed."
        });
        let err = read_answer(&result, "claude-sonnet-5-5", 2048, 6144).unwrap_err();
        assert!(err.error.contains("general_harms"), "{}", err.error);
        assert!(
            err.error.contains("Pick another Claude model"),
            "{}",
            err.error
        );
        assert!(err.error.contains("Not allowed."), "{}", err.error);

        result["stop_details"] = Value::Null;
        let bare = read_answer(&result, "claude-sonnet-5-5", 2048, 6144).unwrap_err();
        assert!(!bare.error.contains("category"), "{}", bare.error);
    }

    #[test]
    fn an_empty_answer_is_an_error_not_an_empty_success() {
        assert!(read_answer(&answer("end_turn", "m", "  "), "m", 1, 1).is_err());
        assert!(read_answer(&answer("pause_turn", "m", "{}"), "m", 1, 1).is_err());
    }

    #[test]
    fn answers_parse_strictly_then_leniently() {
        assert_eq!(parse_answer("{\"a\":1}").unwrap(), json!({"a": 1}));
        assert_eq!(
            parse_answer("```json\n{\"a\":1}\n```").unwrap(),
            json!({"a": 1})
        );
        assert!(parse_answer("I cannot help with that.").is_err());
    }

    #[test]
    fn error_bodies_are_read_in_anthropics_shape_and_otherwise() {
        let body = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        assert_eq!(
            error_parts(body),
            ("authentication_error".into(), "invalid x-api-key".into())
        );
        assert_eq!(
            error_parts("<html>  Bad   gateway </html>"),
            (String::new(), "<html> Bad gateway </html>".into())
        );
    }

    #[test]
    fn errors_say_what_to_do() {
        assert!(
            http_error(401, "authentication_error", "invalid x-api-key", "")
                .contains("Check the Anthropic key under Plug-in Manager")
        );
        assert!(http_error(404, "not_found_error", "model: x", "claude-x")
            .contains("Pick another Claude model"));
        let credit = http_error(
            400,
            "invalid_request_error",
            "Your credit balance is too low to access the Anthropic API.",
            "m",
        );
        assert!(credit.contains("no credit left"), "{credit}");
        assert!(http_error(529, "overloaded_error", "Overloaded", "m").contains("overloaded"));
        assert_eq!(
            http_error(400, "invalid_request_error", "bad", "m"),
            "Anthropic refused the request (HTTP 400, invalid_request_error). (bad)"
        );
        assert!(!http_error(500, "", "", "m").contains("  "));
    }

    #[test]
    fn retries_follow_retry_after_within_a_cap() {
        assert!(is_retryable(429) && is_retryable(529) && is_retryable(500));
        assert!(!is_retryable(400) && !is_retryable(401) && !is_retryable(404));
        assert_eq!(retry_wait(1, None), Duration::from_secs(2));
        assert_eq!(retry_wait(2, None), Duration::from_secs(4));
        assert_eq!(retry_wait(1, Some(3)), Duration::from_secs(3));
        assert_eq!(retry_wait(1, Some(600)), MAX_RETRY_WAIT);
    }

    #[test]
    fn a_request_key_takes_precedence_over_the_providers() {
        let provider = AnthropicProvider::new("sk-provider".into());
        assert_eq!(provider.for_key(Some(" sk-request ")).api_key, "sk-request");
        assert_eq!(provider.for_key(Some("  ")).api_key, "sk-provider");
        assert_eq!(provider.for_key(None).api_key, "sk-provider");
    }

    #[tokio::test]
    async fn no_key_lists_nothing_and_asks_nobody() {
        let provider = AnthropicProvider::new(String::new());
        assert_eq!(provider.list_models_checked().await, Ok(Vec::new()));
        assert_eq!(provider.generate_text(None, "s", "u").await, None);
    }
}
