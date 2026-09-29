//! Servers that speak the OpenAI chat API: LM Studio, found automatically on
//! this computer, and any server the user enters as "Other AI server" —
//! OpenRouter, llama.cpp's `llama-server`, LiteLLM, vLLM, or LM Studio and
//! Ollama running on another machine.
//!
//! One client serves both, because the wire format is the same:
//! `POST {api_base}/chat/completions` with the photo as a `data:` URI in an
//! `image_url` part, and `GET {api_base}/models` for the list. What differs is
//! handled here rather than by the user:
//!
//! * **The address.** People paste `localhost:8080`, `https://openrouter.ai/api/v1`
//!   or a full `…/v1/chat/completions` URL. [`ServerUrl::parse`] turns all of
//!   them into one API base, adding `/v1` when no path was given.
//! * **Structured output.** Support ranges from strict JSON-schema decoding
//!   (llama-server, vLLM, LM Studio) through "ignored" (some OpenRouter
//!   routes) to "rejected with a 400" (LiteLLM without `drop_params`). A
//!   server that rejects the schema is asked again with a looser mode — see
//!   [`SchemaMode`] — and the answer is parsed leniently either way.
//! * **Errors.** The status code is checked before the body is parsed, and
//!   each failure is turned into something the user can act on
//!   ([`OpenAiCompatibleProvider::http_error`]) instead of a JSON decode error.
//!
//! LM Studio keeps its own listing route, `/api/v0/models`: it lists every
//! downloaded model with its type, where `/v1/models` has no type and, with
//! just-in-time loading switched off, only lists what is loaded.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

use crate::edit_recipe::{normalize_edit_recipe, openai_edit_recipe_schema};
use crate::image_encode::image_to_base64;
use crate::keyword_taxonomy::KeywordLeafEncoding;
use crate::normalize::{
    alt_text_from, extract_json_value, missing_field_warning, normalize_keywords,
};
use crate::prompts::{
    prepare_edit_system_prompt, prepare_edit_user_prompt, prepare_system_prompt,
    prepare_user_prompt,
};
use crate::schema::prepare_response_structure;
use crate::types::{
    EditGenerationRequest, EditGenerationResponse, MetadataGenerationRequest,
    MetadataGenerationResponse,
};

/// Where LM Studio's server listens unless the user changed it.
pub const DEFAULT_LMSTUDIO_HOST: &str = "localhost:1234";
const DEFAULT_MAX_TOKENS: u32 = 2048;
/// A local model on a slow machine can take minutes per photo.
const GENERATION_TIMEOUT: Duration = Duration::from_secs(720);
const TEXT_TIMEOUT: Duration = Duration::from_secs(120);
/// OpenRouter's list runs to several hundred models.
const LIST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A server address, normalised from whatever the user typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerUrl {
    /// What `/chat/completions` and `/models` are appended to, e.g.
    /// `https://openrouter.ai/api/v1`.
    pub api_base: String,
    /// Scheme, host and port only, e.g. `http://localhost:1234` — for LM
    /// Studio's own `/api/v0` routes.
    pub root: String,
    /// A short name for messages and the model list: `OpenRouter`, or the
    /// host and port.
    pub label: String,
}

impl ServerUrl {
    /// Normalise a user-entered address.
    ///
    /// * No scheme: `http://` for this computer and the local network
    ///   (localhost, private and link-local addresses, `*.local`, or any
    ///   address with an explicit port — nobody runs TLS on `:8080`), and
    ///   `https://` for everything else.
    /// * A pasted endpoint (`…/chat/completions`, `…/models`) is cut back to
    ///   the API base.
    /// * No path at all means `/v1`, where every one of these servers mounts
    ///   its API. A path that was given is kept, so `…/api/v1` and
    ///   `…/openai/v1` work unchanged.
    ///
    /// `Err` is a message for the user.
    pub fn parse(input: &str) -> Result<ServerUrl, String> {
        let input = input.trim();
        if input.is_empty() {
            return Err("No AI server address is set.".to_string());
        }
        let with_scheme = if input.contains("://") {
            input.to_string()
        } else {
            let authority = input.split(['/', '?', '#']).next().unwrap_or(input);
            let scheme = if is_local_or_has_port(authority) {
                "http"
            } else {
                "https"
            };
            format!("{scheme}://{input}")
        };
        let url = reqwest::Url::parse(&with_scheme)
            .map_err(|e| format!("\"{input}\" is not a valid server address ({e})."))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!(
                "\"{input}\" is not a web address. It should start with http:// or https://."
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(
                "Put the API key in the API key field, not in the server address.".to_string(),
            );
        }
        let Some(host) = url.host_str() else {
            return Err(format!("\"{input}\" has no host name."));
        };
        let host_port = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        };
        let root = format!("{}://{host_port}", url.scheme());

        let mut path = url.path().trim_end_matches('/').to_string();
        loop {
            let trimmed = [
                "/chat/completions",
                "/completions",
                "/api/v0/models",
                "/models",
                "/api/tags",
            ]
            .iter()
            .find_map(|suffix| path.strip_suffix(suffix).map(str::to_string));
            match trimmed {
                Some(shorter) => path = shorter.trim_end_matches('/').to_string(),
                None => break,
            }
        }
        if path.is_empty() {
            path = "/v1".to_string();
        }

        let label = if host == "openrouter.ai" || host.ends_with(".openrouter.ai") {
            "OpenRouter".to_string()
        } else {
            host_port.clone()
        };
        Ok(ServerUrl {
            api_base: format!("{root}{path}"),
            root,
            label,
        })
    }

    fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.api_base)
    }

    fn models_url(&self) -> String {
        format!("{}/models", self.api_base)
    }
}

/// `true` for an address on this computer or the local network, or one with an
/// explicit port — the cases where a missing scheme almost certainly means
/// plain `http`.
fn is_local_or_has_port(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        // [::1]:8080
        let Some((host, after)) = rest.split_once(']') else {
            return false;
        };
        if after.starts_with(':') {
            return true;
        }
        host
    } else {
        match authority.rsplit_once(':') {
            Some((_, port)) if port.parse::<u16>().is_ok() => return true,
            _ => authority,
        }
    };
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".local") || host.ends_with(".localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(IpAddr::V6(ip)) => {
            ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00 // unique local
        }
        Err(_) => false,
    }
}

/// Which kind of server this client talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerKind {
    /// LM Studio on this computer, probed at its default address. Nobody
    /// configured it, so "not running" is not an error.
    LmStudio,
    /// The server the user entered under "Other AI server".
    Custom,
}

/// How much structure a server lets us ask for.
///
/// Remembered per server and model for the life of the process: every photo
/// builds a fresh provider, so without it each photo would pay for the same
/// rejected request again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchemaMode {
    /// `response_format: json_schema` — the server constrains the output.
    JsonSchema,
    /// `response_format: json_object`, with the schema in the prompt.
    JsonObject,
    /// No `response_format` at all, the schema in the prompt only.
    PromptOnly,
}

impl SchemaMode {
    fn next(self) -> Option<SchemaMode> {
        match self {
            SchemaMode::JsonSchema => Some(SchemaMode::JsonObject),
            SchemaMode::JsonObject => Some(SchemaMode::PromptOnly),
            SchemaMode::PromptOnly => None,
        }
    }
}

fn schema_modes() -> &'static Mutex<HashMap<(String, String), SchemaMode>> {
    static MODES: OnceLock<Mutex<HashMap<(String, String), SchemaMode>>> = OnceLock::new();
    MODES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn schema_mode_for(api_base: &str, model: &str) -> SchemaMode {
    schema_modes()
        .lock()
        .unwrap()
        .get(&(api_base.to_string(), model.to_string()))
        .copied()
        .unwrap_or(SchemaMode::JsonSchema)
}

/// Record a downgrade. `true` if this call is the one that made it, so the
/// warning about it is reported once rather than on every photo.
fn remember_schema_mode(api_base: &str, model: &str, mode: SchemaMode) -> bool {
    let mut modes = schema_modes().lock().unwrap();
    let key = (api_base.to_string(), model.to_string());
    let previous = modes.insert(key, mode);
    previous != Some(mode)
}

/// `true` when an error response says the server cannot do the structured
/// output we asked for, rather than failing for some other reason.
fn rejects_structured_output(status: u16, detail: &str) -> bool {
    if !matches!(status, 400 | 422 | 500 | 501) {
        return false;
    }
    let detail = detail.to_ascii_lowercase();
    [
        "response_format",
        "json_schema",
        "json_object",
        "structured",
        "grammar",
        "schema",
    ]
    .iter()
    .any(|needle| detail.contains(needle))
}

/// Why a chat request produced no usable answer.
#[derive(Debug)]
enum PostError {
    /// The server answered with an error status (or an `error` body).
    Status { status: u16, detail: String },
    /// No answer to read: connection, timeout, or a body that is not JSON.
    /// Already a message for the user.
    Transport(String),
}

/// The most useful line out of an error body, whatever shape it has.
fn error_detail(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        let candidates = [
            value.pointer("/error/message"),
            value.get("error"),
            value.get("detail"),
            value.pointer("/detail/0/msg"),
            value.get("message"),
        ];
        for candidate in candidates.into_iter().flatten() {
            if let Some(text) = candidate.as_str().filter(|s| !s.trim().is_empty()) {
                return text.trim().to_string();
            }
        }
    }
    // An HTML error page from a proxy: strip the tags and keep the start.
    let mut text = String::new();
    let mut in_tag = false;
    for c in body.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                text.push(' ');
            }
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    text.chars().take(200).collect()
}

/// `true` when an error says the model could not take the photo — a text-only
/// model, or llama-server started without its vision projector.
fn mentions_images(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    ["image", "vision", "multimodal", "mmproj", "image_url"]
        .iter()
        .any(|needle| detail.contains(needle))
}

pub struct OpenAiCompatibleProvider {
    client: reqwest::Client,
    kind: ServerKind,
    url: ServerUrl,
    api_key: Option<String>,
}

impl OpenAiCompatibleProvider {
    /// LM Studio on this computer, at `host` or its default address.
    pub fn lm_studio(host: Option<String>) -> Result<Self, String> {
        let host = host
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| DEFAULT_LMSTUDIO_HOST.to_string());
        Ok(Self::build(
            ServerKind::LmStudio,
            ServerUrl::parse(&host)?,
            None,
        ))
    }

    /// The server the user entered, with an optional API key.
    pub fn custom(url: &str, api_key: Option<String>) -> Result<Self, String> {
        let api_key = api_key
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty());
        Ok(Self::build(
            ServerKind::Custom,
            ServerUrl::parse(url)?,
            api_key,
        ))
    }

    fn build(kind: ServerKind, url: ServerUrl, api_key: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_default();
        OpenAiCompatibleProvider {
            client,
            kind,
            url,
            api_key,
        }
    }

    /// The wire provider name this client serves.
    pub fn provider_name(&self) -> &'static str {
        match self.kind {
            ServerKind::LmStudio => "lmstudio",
            ServerKind::Custom => "openai_compatible",
        }
    }

    pub fn kind(&self) -> ServerKind {
        self.kind
    }

    pub fn url(&self) -> &ServerUrl {
        &self.url
    }

    /// How the server is named in messages.
    fn display_name(&self) -> &str {
        match self.kind {
            ServerKind::LmStudio => "LM Studio",
            ServerKind::Custom => &self.url.label,
        }
    }

    fn get(&self, url: &str) -> reqwest::RequestBuilder {
        let request = self.client.get(url);
        match &self.api_key {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    }

    fn post(&self, url: &str) -> reqwest::RequestBuilder {
        let request = self.client.post(url);
        match &self.api_key {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    }

    /// A 2s TCP probe for LM Studio, so an instance that is not running costs
    /// nothing. The user's own server is assumed reachable: a failure there
    /// is reported with a proper message when it is used.
    pub async fn is_available(&self) -> bool {
        if self.kind == ServerKind::Custom {
            return true;
        }
        let Ok(url) = reqwest::Url::parse(&self.url.root) else {
            return false;
        };
        let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
            return false;
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect((host, port)),
        )
        .await
        .is_ok_and(|r| r.is_ok())
    }

    pub async fn list_available_models(&self) -> Vec<String> {
        self.list_models_checked().await.unwrap_or_default()
    }

    /// The models the server offers, or why they could not be listed.
    ///
    /// For LM Studio, "not running" is an empty list: it was never set up, so
    /// there is nothing for the user to fix. For the user's own server every
    /// failure is an `Err` with a message, because they asked for it.
    pub async fn list_models_checked(&self) -> Result<Vec<String>, String> {
        match self.kind {
            ServerKind::LmStudio => self.list_lm_studio_models().await,
            ServerKind::Custom => self.list_openai_models().await,
        }
    }

    async fn list_lm_studio_models(&self) -> Result<Vec<String>, String> {
        let url = format!("{}/api/v0/models", self.url.root);
        let response = match self.get(&url).timeout(Duration::from_secs(5)).send().await {
            Ok(r) => r,
            // Not running: LM Studio is only probed, never configured, so
            // there is nothing to tell the user.
            Err(e) if e.is_connect() || e.is_timeout() => return Ok(Vec::new()),
            Err(e) => return Err(format!("Could not list LM Studio's models: {e}")),
        };
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            // An LM Studio without the v0 REST API.
            return self.list_openai_models().await;
        }
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(self.http_error(status.as_u16(), &error_detail(&text), ""));
        }
        let body: Value = serde_json::from_str(&text)
            .map_err(|_| "LM Studio sent a model list that is not JSON.".to_string())?;
        Ok(parse_lm_studio_models(&body))
    }

    async fn list_openai_models(&self) -> Result<Vec<String>, String> {
        let response = self
            .get(&self.url.models_url())
            .timeout(LIST_TIMEOUT)
            .send()
            .await
            .map_err(|e| self.transport_error(&e))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(self.http_error(status.as_u16(), &error_detail(&text), ""));
        }
        let body: Value = serde_json::from_str(&text).map_err(|_| {
            format!(
                "{} answered, but not with a model list. Check the address — it usually ends in /v1.",
                self.display_name()
            )
        })?;
        Ok(parse_openai_models(&body))
    }

    /// Turn a failed request into a message the user can act on.
    fn transport_error(&self, error: &reqwest::Error) -> String {
        let name = self.display_name();
        if error.is_timeout() {
            return format!(
                "{name} did not answer in time. It may be busy or overloaded; try again."
            );
        }
        if error.is_connect() {
            return match self.kind {
                ServerKind::LmStudio => {
                    "Could not reach LM Studio. Start LM Studio and turn on its \
                     local server (Developer tab)."
                        .to_string()
                }
                ServerKind::Custom => format!(
                    "Could not reach {name}. Check the address under Plug-in Manager → Optional AI \
                     providers → Other AI server, and that the server is running."
                ),
            };
        }
        format!("The request to {name} failed: {error}")
    }

    /// Turn an error status into a message the user can act on.
    fn http_error(&self, status: u16, detail: &str, model: &str) -> String {
        let name = self.display_name();
        let detail_suffix = if detail.is_empty() {
            String::new()
        } else {
            format!(" ({detail})")
        };
        match status {
            401 | 403 if self.api_key.is_some() => format!(
                "{name} rejected the API key. Check the key under Plug-in Manager → Optional AI \
                 providers.{detail_suffix}"
            ),
            401 | 403 => format!(
                "{name} needs an API key. Enter it under Plug-in Manager → Optional AI \
                 providers.{detail_suffix}"
            ),
            402 => format!("{name} says there is no credit left on the account.{detail_suffix}"),
            404 if !model.is_empty() && detail.contains(model) => format!(
                "{name} does not know the model \"{model}\". Pick another one in the model \
                 list.{detail_suffix}"
            ),
            404 => format!(
                "{name} has no OpenAI-compatible API at this address. The address usually ends \
                 in /v1.{detail_suffix}"
            ),
            429 => format!(
                "{name} is rate limiting requests. Wait a little and run the task again.{detail_suffix}"
            ),
            400 | 422 | 500 if mentions_images(detail) => format!(
                "The model \"{model}\" on {name} cannot read photos. Pick a vision model — for \
                 llama-server, start it with --mmproj.{detail_suffix}"
            ),
            s if s >= 500 => format!("{name} had an internal error (HTTP {s}).{detail_suffix}"),
            s => format!("{name} refused the request (HTTP {s}).{detail_suffix}"),
        }
    }

    /// Send one chat request and read the answer.
    async fn post_chat(&self, body: &Value, timeout: Duration) -> Result<Value, PostError> {
        let response = self
            .post(&self.url.chat_url())
            .json(body)
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| PostError::Transport(self.transport_error(&e)))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| PostError::Transport(self.transport_error(&e)))?;
        if !status.is_success() {
            return Err(PostError::Status {
                status: status.as_u16(),
                detail: error_detail(&text),
            });
        }
        let value: Value = serde_json::from_str(&text).map_err(|_| {
            PostError::Transport(format!(
                "{} answered with something that is not JSON: {}",
                self.display_name(),
                error_detail(&text)
            ))
        })?;
        // Some servers report failures with a 200 and an `error` body.
        if value.get("choices").is_none() {
            if let Some(error) = value.get("error") {
                return Err(PostError::Status {
                    status: status.as_u16(),
                    detail: error_detail(&json!({ "error": error }).to_string()),
                });
            }
        }
        Ok(value)
    }

    /// Send a structured request, stepping down through [`SchemaMode`]s while
    /// the server rejects the one asked for.
    ///
    /// Returns the answer and, the first time this server and model are
    /// downgraded, a warning saying so.
    async fn chat_structured(
        &self,
        model: &str,
        build: impl Fn(SchemaMode) -> Value,
    ) -> Result<(Value, Option<String>), String> {
        let mut mode = schema_mode_for(&self.url.api_base, model);
        let mut warning = None;
        loop {
            match self.post_chat(&build(mode), GENERATION_TIMEOUT).await {
                Ok(result) => return Ok((result, warning)),
                Err(PostError::Status { status, detail })
                    if rejects_structured_output(status, &detail) =>
                {
                    let Some(next) = mode.next() else {
                        return Err(self.http_error(status, &detail, model));
                    };
                    log::info!(
                        "{} rejected {mode:?} output for {model} ({detail}); retrying with {next:?}",
                        self.display_name()
                    );
                    // The answer is still parsed and checked, and a field that
                    // really goes missing produces its own warning — but the
                    // user should know why that may now happen more often.
                    // Once per server and model, not on every photo.
                    if remember_schema_mode(&self.url.api_base, model, next) {
                        warning = Some(format!(
                            "{} cannot constrain \"{model}\" to the answer format, so the format is \
                             now only described in the prompt. Answers are still checked, but \
                             fields may be missing more often.",
                            self.display_name()
                        ));
                    }
                    mode = next;
                }
                Err(PostError::Status { status, detail }) => {
                    return Err(self.http_error(status, &detail, model))
                }
                Err(PostError::Transport(message)) => return Err(message),
            }
        }
    }

    /// The chat body for a photo request in the given mode.
    #[allow(clippy::too_many_arguments)]
    fn photo_body(
        model: &str,
        system_prompt: &str,
        user_prompt: &str,
        data_uri: &str,
        schema_name: &str,
        schema: &Value,
        mode: SchemaMode,
        temperature: f64,
        max_tokens: u32,
    ) -> Value {
        let system = match mode {
            SchemaMode::JsonSchema => system_prompt.to_string(),
            SchemaMode::JsonObject | SchemaMode::PromptOnly => format!(
                "{system_prompt}\n\nAnswer with one JSON object that matches this JSON Schema, \
                 and nothing else:\n{schema}"
            ),
        };
        let mut body = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": [
                    {"type": "text", "text": user_prompt},
                    {"type": "image_url", "image_url": {"url": data_uri}},
                ]},
            ],
            "temperature": temperature,
            "max_tokens": max_tokens,
            "stream": false,
        });
        let response_format = match mode {
            SchemaMode::JsonSchema => Some(json!({
                "type": "json_schema",
                "json_schema": {"name": schema_name, "schema": schema},
            })),
            SchemaMode::JsonObject => Some(json!({"type": "json_object"})),
            SchemaMode::PromptOnly => None,
        };
        if let Some(format) = response_format {
            body["response_format"] = format;
        }
        body
    }

    /// Read the JSON object out of a completion, strictly first and then
    /// leniently — a server that ignored the schema may wrap it in prose.
    fn parse_content(&self, content: &str) -> Result<Value, String> {
        serde_json::from_str::<Value>(content)
            .ok()
            .filter(Value::is_object)
            .or_else(|| extract_json_value(content, '{'))
            .ok_or_else(|| {
                format!(
                    "{} returned an answer that is not the expected JSON (length={} chars). This \
                     often means the answer was cut off — try raising the Max Tokens setting in \
                     the plugin — or that the model does not follow format instructions well.",
                    self.display_name(),
                    content.len()
                )
            })
    }

    fn length_error(&self, max_tokens: u32) -> String {
        format!(
            "{} stopped before finishing the response because the token limit was reached \
             (max_tokens={max_tokens}). Please raise the Max Tokens setting in the plugin (General \
             tab → AI Model section) — try 4096 or higher. If you use hierarchical keywords, a \
             large taxonomy increases token usage significantly.",
            self.display_name()
        )
    }

    /// Schema-free chat completion — see [`crate::provider::LlmProvider::generate_text`].
    pub async fn generate_text(
        &self,
        model: Option<&str>,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Option<String> {
        let body = json!({
            "model": model.unwrap_or("local-model"),
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt},
            ],
            "temperature": 0.1,
            "max_tokens": 4096,
            "stream": false,
        });
        match self.post_chat(&body, TEXT_TIMEOUT).await {
            Ok(result) => result["choices"][0]["message"]["content"]
                .as_str()
                .map(str::to_string),
            Err(e) => {
                log::warn!("{} text request failed: {e:?}", self.display_name());
                None
            }
        }
    }

    pub async fn generate_metadata(
        &self,
        request: &MetadataGenerationRequest,
    ) -> MetadataGenerationResponse {
        let image_b64 = match image_to_base64(&request.image_data) {
            Ok(b64) => b64,
            Err(e) => return fail(&request.uuid, e.to_string()),
        };
        let data_uri = format!("data:image/jpeg;base64,{image_b64}");
        let system_prompt = prepare_system_prompt(request);
        let user_prompt = prepare_user_prompt(request);
        let response_schema = prepare_response_structure(request);
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

        let (result, format_warning) = match self
            .chat_structured(&request.model, |mode| {
                Self::photo_body(
                    &request.model,
                    &system_prompt,
                    &user_prompt,
                    &data_uri,
                    "metadata_response",
                    &response_schema,
                    mode,
                    request.temperature,
                    max_tokens,
                )
            })
            .await
        {
            Ok(answer) => answer,
            Err(e) => return fail(&request.uuid, e),
        };

        let (input_tokens, output_tokens) = usage_tokens(&result);
        let choice = &result["choices"][0];
        if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
            return MetadataGenerationResponse {
                uuid: request.uuid.clone(),
                success: false,
                error: Some(self.length_error(max_tokens)),
                input_tokens,
                output_tokens,
                ..Default::default()
            };
        }

        let Some(content) = choice["message"]["content"].as_str() else {
            return fail(
                &request.uuid,
                format!("{} returned an empty answer.", self.display_name()),
            );
        };
        let parsed = match self.parse_content(content) {
            Ok(v) => v,
            Err(e) => return fail(&request.uuid, e),
        };

        let keywords = normalize_keywords(
            &parsed.get("keywords").cloned().unwrap_or(json!([])),
            request.keyword_categories.as_ref(),
            KeywordLeafEncoding::for_request(request.bilingual_keywords, request.generate_aliases),
        );
        let caption = if request.generate_caption {
            parsed
                .get("caption")
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        };
        let alt_text = alt_text_from(&parsed, request.generate_alt_text, caption.as_ref());
        let title = if request.generate_title {
            parsed
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        };
        // A requested field the model did not return is a degraded
        // success, not a success: see `missing_field_warning`. The response
        // has one warning slot, so both reports share it.
        let warning = join_warnings(
            format_warning,
            missing_field_warning(
                request,
                Some(&keywords),
                caption.as_ref(),
                title.as_ref(),
                alt_text.as_ref(),
            ),
        );
        MetadataGenerationResponse {
            uuid: request.uuid.clone(),
            success: true,
            keywords: Some(keywords),
            caption,
            title,
            alt_text,
            input_tokens,
            output_tokens,
            error: None,
            warning,
        }
    }

    pub async fn generate_edit_recipe(
        &self,
        request: &EditGenerationRequest,
    ) -> EditGenerationResponse {
        let image_b64 = match image_to_base64(&request.image_data) {
            Ok(b64) => b64,
            Err(e) => return fail_edit(&request.uuid, e.to_string()),
        };
        let data_uri = format!("data:image/jpeg;base64,{image_b64}");
        let system_prompt = prepare_edit_system_prompt(request);
        let user_prompt = prepare_edit_user_prompt(request);
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
        let schema = openai_edit_recipe_schema(request.is_raw);

        let (result, format_warning) = match self
            .chat_structured(&request.model, |mode| {
                Self::photo_body(
                    &request.model,
                    &system_prompt,
                    &user_prompt,
                    &data_uri,
                    "lightroom_edit_recipe",
                    schema,
                    mode,
                    request.temperature,
                    max_tokens,
                )
            })
            .await
        {
            Ok(answer) => answer,
            Err(e) => return fail_edit(&request.uuid, e),
        };

        let (input_tokens, output_tokens) = usage_tokens(&result);
        let choice = &result["choices"][0];
        if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
            return EditGenerationResponse {
                uuid: request.uuid.clone(),
                success: false,
                error: Some(self.length_error(max_tokens)),
                input_tokens,
                output_tokens,
                ..Default::default()
            };
        }

        let Some(content) = choice["message"]["content"].as_str() else {
            return fail_edit(
                &request.uuid,
                format!("{} returned an empty answer.", self.display_name()),
            );
        };
        let parsed = match self.parse_content(content) {
            Ok(v) => v,
            Err(e) => return fail_edit(&request.uuid, e),
        };

        EditGenerationResponse {
            uuid: request.uuid.clone(),
            success: true,
            recipe: Some(normalize_edit_recipe(&parsed, request.is_raw)),
            input_tokens,
            output_tokens,
            error: None,
            warning: format_warning,
            guardrail_reasons: Vec::new(),
        }
    }
}

/// LM Studio's `/api/v0/models`: every downloaded model, with a `type`.
/// Embedding models are dropped — they cannot describe a photo.
fn parse_lm_studio_models(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> = body
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| {
            !matches!(
                m.get("type").and_then(Value::as_str),
                Some("embeddings" | "embedding")
            )
        })
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// An OpenAI-style `/models` list (`{"data": [{"id": …}]}`).
///
/// Models a server marks as not taking images (OpenRouter's
/// `architecture.input_modalities`) are dropped, as are embedding models, so
/// the plugin's model list only offers things that can describe a photo.
fn parse_openai_models(body: &Value) -> Vec<String> {
    let entries = body
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| body.get("models").and_then(Value::as_array))
        .or_else(|| body.as_array());
    let mut ids: Vec<String> = entries
        .into_iter()
        .flatten()
        .filter(|m| {
            let modalities = m
                .pointer("/architecture/input_modalities")
                .and_then(Value::as_array);
            modalities.is_none_or(|list| list.iter().any(|v| v.as_str() == Some("image")))
        })
        .filter_map(|m| {
            m.get("id")
                .or_else(|| m.get("name"))
                .or_else(|| m.get("model"))
                .and_then(Value::as_str)
        })
        .filter(|id| !id.to_ascii_lowercase().contains("embed"))
        .map(str::to_string)
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn join_warnings(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(a), Some(b)) => Some(format!("{a} {b}")),
        (a, b) => a.or(b),
    }
}

fn usage_tokens(result: &Value) -> (u32, u32) {
    let usage = result.get("usage");
    let input_tokens = usage
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let output_tokens = usage
        .and_then(|u| u.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    (input_tokens, output_tokens)
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

    fn parsed(input: &str) -> ServerUrl {
        ServerUrl::parse(input).unwrap_or_else(|e| panic!("{input}: {e}"))
    }

    #[test]
    fn addresses_are_normalised_to_an_api_base() {
        for (input, api_base, root, label) in [
            // LM Studio's old host:port form.
            (
                "localhost:1234",
                "http://localhost:1234/v1",
                "http://localhost:1234",
                "localhost:1234",
            ),
            (
                "http://localhost:1234/",
                "http://localhost:1234/v1",
                "http://localhost:1234",
                "localhost:1234",
            ),
            // A llama-server on the local network, no scheme.
            (
                "192.168.1.20:8080",
                "http://192.168.1.20:8080/v1",
                "http://192.168.1.20:8080",
                "192.168.1.20:8080",
            ),
            // OpenRouter without a scheme gets https, and keeps its path.
            (
                "openrouter.ai/api/v1",
                "https://openrouter.ai/api/v1",
                "https://openrouter.ai",
                "OpenRouter",
            ),
            // A pasted endpoint is cut back to the API base.
            (
                "https://openrouter.ai/api/v1/chat/completions",
                "https://openrouter.ai/api/v1",
                "https://openrouter.ai",
                "OpenRouter",
            ),
            (
                "http://litellm.local:4000/v1/models",
                "http://litellm.local:4000/v1",
                "http://litellm.local:4000",
                "litellm.local:4000",
            ),
            (
                "gpu-box.local",
                "http://gpu-box.local/v1",
                "http://gpu-box.local",
                "gpu-box.local",
            ),
            (
                "[::1]:8000",
                "http://[::1]:8000/v1",
                "http://[::1]:8000",
                "[::1]:8000",
            ),
            (
                "  https://example.com/openai/v1/  ",
                "https://example.com/openai/v1",
                "https://example.com",
                "example.com",
            ),
        ] {
            let url = parsed(input);
            assert_eq!(url.api_base, api_base, "{input}");
            assert_eq!(url.root, root, "{input}");
            assert_eq!(url.label, label, "{input}");
        }
    }

    #[test]
    fn unusable_addresses_are_refused_with_a_reason() {
        for (input, expected) in [
            ("", "No AI server address"),
            ("   ", "No AI server address"),
            ("ftp://example.com", "http:// or https://"),
            ("https://user:secret@example.com/v1", "API key field"),
        ] {
            let err = ServerUrl::parse(input).err().unwrap_or_default();
            assert!(err.contains(expected), "{input:?} → {err}");
        }
    }

    #[test]
    fn a_schema_rejection_is_told_apart_from_other_errors() {
        assert!(rejects_structured_output(
            400,
            "'response_format.type' must be 'json_object' or 'text'"
        ));
        assert!(rejects_structured_output(
            422,
            "json_schema is not supported"
        ));
        assert!(rejects_structured_output(500, "failed to parse grammar"));
        assert!(!rejects_structured_output(401, "invalid schema key"));
        assert!(!rejects_structured_output(400, "model not found"));
        assert!(!rejects_structured_output(429, "slow down"));
    }

    #[test]
    fn error_details_are_read_from_every_common_shape() {
        assert_eq!(
            error_detail(r#"{"error":{"message":"Invalid API key"}}"#),
            "Invalid API key"
        );
        assert_eq!(
            error_detail(r#"{"error":"no such model"}"#),
            "no such model"
        );
        assert_eq!(error_detail(r#"{"detail":"Not Found"}"#), "Not Found");
        assert_eq!(
            error_detail("<html><body><h1>502 Bad Gateway</h1></body></html>"),
            "502 Bad Gateway"
        );
    }

    fn custom(key: Option<&str>) -> OpenAiCompatibleProvider {
        OpenAiCompatibleProvider::custom("https://openrouter.ai/api/v1", key.map(str::to_string))
            .unwrap()
    }

    #[test]
    fn errors_say_what_to_do() {
        let with_key = custom(Some("sk-or-test"));
        let without_key = custom(None);
        for (provider, status, detail, model, expected) in [
            (&with_key, 401, "bad key", "m", "rejected the API key"),
            (&without_key, 401, "", "m", "needs an API key"),
            (&with_key, 402, "", "m", "no credit left"),
            (
                &with_key,
                404,
                "model m not found",
                "m",
                "does not know the model",
            ),
            (&with_key, 404, "Not Found", "m", "usually ends in /v1"),
            (&with_key, 429, "", "m", "rate limiting"),
            (
                &with_key,
                400,
                "image input is not supported",
                "m",
                "cannot read photos",
            ),
            (&with_key, 502, "Bad Gateway", "m", "internal error"),
        ] {
            let message = provider.http_error(status, detail, model);
            assert!(message.contains(expected), "{status}: {message}");
            assert!(message.contains("OpenRouter"), "{message}");
        }
    }

    #[test]
    fn lm_studio_listing_drops_embedding_models() {
        let body = json!({"data": [
            {"id": "qwen2.5-vl-7b-instruct", "type": "vlm"},
            {"id": "llama-3.2-3b-instruct", "type": "llm"},
            {"id": "text-embedding-nomic-embed-text-v1.5", "type": "embeddings"},
        ]});
        assert_eq!(
            parse_lm_studio_models(&body),
            vec!["llama-3.2-3b-instruct", "qwen2.5-vl-7b-instruct"]
        );
    }

    #[test]
    fn openai_listing_keeps_only_models_that_take_images() {
        let body = json!({"data": [
            {"id": "google/gemini-2.5-flash",
             "architecture": {"input_modalities": ["text", "image"]}},
            {"id": "deepseek/deepseek-chat",
             "architecture": {"input_modalities": ["text"]}},
            // No modality information: kept, the server cannot tell us.
            {"id": "gemma-3-12b-it"},
            {"id": "nomic-embed-text"},
        ]});
        assert_eq!(
            parse_openai_models(&body),
            vec!["gemma-3-12b-it", "google/gemini-2.5-flash"]
        );
        // llama-server also answers with `models`.
        assert_eq!(
            parse_openai_models(&json!({"models": [{"name": "gemma"}]})),
            vec!["gemma"]
        );
    }

    #[test]
    fn a_downgrade_is_reported_once_per_server_and_model() {
        let base = "http://unit-test.invalid/v1";
        assert_eq!(schema_mode_for(base, "m"), SchemaMode::JsonSchema);
        assert!(remember_schema_mode(base, "m", SchemaMode::JsonObject));
        assert!(!remember_schema_mode(base, "m", SchemaMode::JsonObject));
        assert_eq!(schema_mode_for(base, "m"), SchemaMode::JsonObject);
        assert_eq!(schema_mode_for(base, "other"), SchemaMode::JsonSchema);
    }

    #[test]
    fn the_looser_modes_put_the_schema_in_the_prompt() {
        let schema = json!({"type": "object", "properties": {"title": {"type": "string"}}});
        let strict = OpenAiCompatibleProvider::photo_body(
            "m",
            "sys",
            "user",
            "data:",
            "n",
            &schema,
            SchemaMode::JsonSchema,
            0.2,
            100,
        );
        assert_eq!(strict["response_format"]["type"], "json_schema");
        assert_eq!(strict["messages"][0]["content"], "sys");

        let object = OpenAiCompatibleProvider::photo_body(
            "m",
            "sys",
            "user",
            "data:",
            "n",
            &schema,
            SchemaMode::JsonObject,
            0.2,
            100,
        );
        assert_eq!(object["response_format"]["type"], "json_object");
        assert!(object["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("\"title\""));

        let prompt_only = OpenAiCompatibleProvider::photo_body(
            "m",
            "sys",
            "user",
            "data:",
            "n",
            &schema,
            SchemaMode::PromptOnly,
            0.2,
            100,
        );
        assert!(prompt_only.get("response_format").is_none());
    }

    #[tokio::test]
    async fn lm_studio_not_running_is_not_an_error() {
        // Port 1 is never a running LM Studio in CI or on a dev machine.
        let provider =
            OpenAiCompatibleProvider::lm_studio(Some("127.0.0.1:1".to_string())).unwrap();
        assert!(!provider.is_available().await);
        assert_eq!(provider.list_models_checked().await, Ok(Vec::new()));
    }

    #[tokio::test]
    async fn the_users_server_not_running_is_an_error() {
        let provider = OpenAiCompatibleProvider::custom("127.0.0.1:1", None).unwrap();
        let err = provider.list_models_checked().await.unwrap_err();
        assert!(err.contains("Could not reach 127.0.0.1:1"), "{err}");
    }
}
