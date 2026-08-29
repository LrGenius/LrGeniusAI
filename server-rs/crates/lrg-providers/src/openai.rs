//! Port of `providers/chatgpt.py`, using `reqwest` directly against the
//! OpenAI REST API (`/v1/chat/completions`, `/v1/models`) rather than
//! the `openai` Python SDK.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::edit_recipe::{normalize_edit_recipe, openai_edit_recipe_schema};
use crate::image_encode::image_to_base64;
use crate::keyword_taxonomy::KeywordLeafEncoding;
use crate::normalize::{alt_text_from, missing_field_warning, normalize_keywords};
use crate::prompts::{
    prepare_edit_system_prompt, prepare_edit_user_prompt, prepare_system_prompt,
    prepare_user_prompt,
};
use crate::schema::prepare_response_structure;
use crate::schema_strict::make_schema_strict;
use crate::types::{
    EditGenerationRequest, EditGenerationResponse, MetadataGenerationRequest,
    MetadataGenerationResponse, ProviderError,
};

const DEFAULT_MAX_TOKENS: u32 = 2048;
const CACHE_TTL: Duration = Duration::from_secs(3600);

/// OpenAI deprecated `max_tokens` in the Chat Completions API, and the
/// reasoning families reject it outright: "Unsupported parameter:
/// 'max_tokens' is not supported with this model. Use
/// 'max_completion_tokens' instead." (issue #324). Every model this
/// provider offers accepts the new name, so we always send that one.
const TOKEN_LIMIT_FIELD: &str = "max_completion_tokens";

/// GPT-5 and the o-series are reasoning models: they accept only the
/// default temperature, they take a `reasoning_effort`, and they spend
/// part of the completion budget on hidden reasoning tokens.
fn is_reasoning_model(model: &str) -> bool {
    ["gpt-5", "o1", "o3", "o4"]
        .iter()
        .any(|prefix| model.starts_with(prefix))
}

/// Fill in the tuning knobs every chat call shares: the completion-token
/// cap (under the name the model accepts — see [`TOKEN_LIMIT_FIELD`]), a
/// temperature the model will not reject, and reasoning effort.
fn apply_model_tuning(body: &mut Value, model: &str, temperature: f64, max_tokens: u32) {
    body[TOKEN_LIMIT_FIELD] = json!(max_tokens);
    if is_reasoning_model(model) {
        body["temperature"] = json!(1.0);
        body["reasoning_effort"] = json!("low");
    } else {
        body["temperature"] = json!(temperature);
    }
}

/// The user-facing explanation for `finish_reason == "length"`.
fn length_error(model: &str, max_tokens: u32) -> String {
    let mut msg = format!(
        "OpenAI stopped before finishing the response because the token limit was reached ({TOKEN_LIMIT_FIELD}={max_tokens}). Please raise the Max Tokens setting in the plugin (General tab → AI Model section) — try 4096 or higher."
    );
    if is_reasoning_model(model) {
        msg.push_str(
            " GPT-5 and o-series models also spend part of that budget on internal reasoning, so they need a higher limit than other models do.",
        );
    }
    msg
}

const ALLOWED_PREFIXES: [&str; 2] = ["gpt-4.1", "gpt-5"];
const BLOCKED_SUBSTRINGS: [&str; 9] = [
    "-instruct",
    "-audio",
    "-realtime",
    "-transcribe",
    "-tts",
    "-search",
    "-moderation",
    "-codex",
    "-chat",
];
const EXCLUDED_IDS: [&str; 1] = ["chatgpt-4o-latest"];
const EXCLUDED_PREFIXES: [&str; 1] = ["o1-mini"];

fn snapshot_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"-(\d{4}-\d{2}-\d{2}|\d{4})$").unwrap())
}

fn family_rank(model_id: &str) -> i32 {
    for (i, prefix) in ["gpt-5", "gpt-4.1", "gpt-4o", "o4", "o3"]
        .iter()
        .enumerate()
    {
        if model_id.starts_with(prefix) {
            return i as i32;
        }
    }
    99
}

type ModelCache = std::collections::HashMap<String, (Instant, Vec<String>)>;

/// In-memory 1h cache keyed by api_key, matching the Python module-level cache.
static MODEL_CACHE: OnceLock<Mutex<ModelCache>> = OnceLock::new();

fn model_cache() -> &'static Mutex<ModelCache> {
    MODEL_CACHE.get_or_init(|| Mutex::new(ModelCache::new()))
}

pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: String,
}

impl OpenAiProvider {
    pub fn new(api_key: String) -> Self {
        OpenAiProvider {
            client: reqwest::Client::new(),
            api_key,
        }
    }

    pub fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }

    fn response_format(&self, request: &MetadataGenerationRequest) -> Value {
        let schema = make_schema_strict(&prepare_response_structure(request));
        json!({"type": "json_schema", "json_schema": {"name": "metadata_response", "schema": schema, "strict": true}})
    }

    /// Schema-free chat completion — see [`crate::provider::LlmProvider::generate_text`].
    pub async fn generate_text(
        &self,
        model: Option<&str>,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Option<String> {
        let model = model.unwrap_or("gpt-4.1");
        let mut body = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt},
            ],
        });
        apply_model_tuning(&mut body, model, 0.1, 4096);
        let resp = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&body)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .ok()?;
        let result: Value = resp.json().await.ok()?;
        result["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
    }

    pub async fn generate_metadata(
        &self,
        request: &MetadataGenerationRequest,
    ) -> MetadataGenerationResponse {
        if !self.is_available() {
            return fail(&request.uuid, "OpenAI API not configured".to_string());
        }

        let image_b64 = match image_to_base64(&request.image_data) {
            Ok(b64) => b64,
            Err(e) => return fail(&request.uuid, e.to_string()),
        };
        let data_uri = format!("data:image/jpeg;base64,{image_b64}");
        let system_prompt = prepare_system_prompt(request);
        let user_prompt = prepare_user_prompt(request);
        let response_format = self.response_format(request);
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

        let mut body = json!({
            "model": request.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt},
                {"role": "user", "content": [{"type": "image_url", "image_url": {"url": data_uri}}]},
            ],
            "response_format": response_format,
        });
        apply_model_tuning(&mut body, &request.model, request.temperature, max_tokens);

        let resp = match self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&body)
            .timeout(Duration::from_secs(120))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return fail(&request.uuid, ProviderError::Http(e).to_string()),
        };
        let result: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => return fail(&request.uuid, ProviderError::Http(e).to_string()),
        };

        if let Some(err) = result.get("error") {
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown OpenAI error");
            return fail(&request.uuid, msg.to_string());
        }

        let choice = &result["choices"][0];
        let finish_reason = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or("");
        let usage = result.get("usage");
        let input_tokens = usage
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let output_tokens = usage
            .and_then(|u| u.get("completion_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;

        if finish_reason != "stop" {
            let msg = if finish_reason == "length" {
                length_error(&request.model, max_tokens)
            } else {
                format!("OpenAI generation failed: {finish_reason}")
            };
            return MetadataGenerationResponse {
                uuid: request.uuid.clone(),
                success: false,
                error: Some(msg),
                input_tokens,
                output_tokens,
                ..Default::default()
            };
        }

        let Some(content) = choice["message"]["content"].as_str() else {
            return fail(
                &request.uuid,
                "Empty response content from OpenAI".to_string(),
            );
        };
        let parsed: Value = match serde_json::from_str(content) {
            Ok(v) => v,
            Err(e) => return fail(&request.uuid, format!("JSON parsing error: {e}")),
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
        // success, not a success: see `missing_field_warning`.
        let warning = missing_field_warning(
            request,
            Some(&keywords),
            caption.as_ref(),
            title.as_ref(),
            alt_text.as_ref(),
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

    fn edit_response_format(&self, is_raw: Option<bool>) -> Value {
        let schema = make_schema_strict(openai_edit_recipe_schema(is_raw));
        json!({"type": "json_schema", "json_schema": {"name": "lightroom_edit_recipe", "schema": schema, "strict": true}})
    }

    pub async fn generate_edit_recipe(
        &self,
        request: &EditGenerationRequest,
    ) -> EditGenerationResponse {
        if !self.is_available() {
            return fail_edit(&request.uuid, "OpenAI API not configured".to_string());
        }

        let image_b64 = match image_to_base64(&request.image_data) {
            Ok(b64) => b64,
            Err(e) => return fail_edit(&request.uuid, e.to_string()),
        };
        let data_uri = format!("data:image/jpeg;base64,{image_b64}");
        let system_prompt = prepare_edit_system_prompt(request);
        let user_prompt = prepare_edit_user_prompt(request);
        let response_format = self.edit_response_format(request.is_raw);
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

        let mut body = json!({
            "model": request.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt},
                {"role": "user", "content": [{"type": "image_url", "image_url": {"url": data_uri}}]},
            ],
            "response_format": response_format,
        });
        apply_model_tuning(&mut body, &request.model, request.temperature, max_tokens);

        let resp = match self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&body)
            .timeout(Duration::from_secs(120))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return fail_edit(&request.uuid, ProviderError::Http(e).to_string()),
        };
        let result: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => return fail_edit(&request.uuid, ProviderError::Http(e).to_string()),
        };

        if let Some(err) = result.get("error") {
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown OpenAI error");
            return fail_edit(&request.uuid, msg.to_string());
        }

        let choice = &result["choices"][0];
        let finish_reason = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or("");
        let usage = result.get("usage");
        let input_tokens = usage
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let output_tokens = usage
            .and_then(|u| u.get("completion_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;

        if finish_reason != "stop" {
            let msg = if finish_reason == "length" {
                length_error(&request.model, max_tokens)
            } else {
                format!("OpenAI generation failed: {finish_reason}")
            };
            return EditGenerationResponse {
                uuid: request.uuid.clone(),
                success: false,
                error: Some(msg),
                input_tokens,
                output_tokens,
                ..Default::default()
            };
        }

        let Some(content) = choice["message"]["content"].as_str() else {
            return fail_edit(
                &request.uuid,
                "Empty response content from OpenAI".to_string(),
            );
        };
        let parsed: Value = match serde_json::from_str(content) {
            Ok(v) => v,
            Err(e) => return fail_edit(&request.uuid, format!("JSON parsing error: {e}")),
        };

        EditGenerationResponse {
            uuid: request.uuid.clone(),
            success: true,
            recipe: Some(normalize_edit_recipe(&parsed, request.is_raw)),
            input_tokens,
            output_tokens,
            error: None,
            warning: None,
            guardrail_reasons: Vec::new(),
        }
    }

    /// Port of `list_available_models`: fetch `/v1/models`, apply the
    /// vision-family allowlist + blocklist, cache 1h per api_key.
    pub async fn list_available_models(&self) -> Vec<String> {
        if self.api_key.is_empty() {
            return Vec::new();
        }
        {
            let cache = model_cache().lock().await;
            if let Some((expires_at, models)) = cache.get(&self.api_key) {
                if Instant::now() < *expires_at {
                    return models.clone();
                }
            }
        }

        let resp = match self
            .client
            .get("https://api.openai.com/v1/models")
            .bearer_auth(&self.api_key)
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let Ok(body) = resp.json::<Value>().await else {
            return Vec::new();
        };
        let Some(data) = body.get("data").and_then(Value::as_array) else {
            return Vec::new();
        };

        let mut filtered: Vec<String> = data
            .iter()
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            .filter(|id| ALLOWED_PREFIXES.iter().any(|p| id.starts_with(p)))
            .filter(|id| !EXCLUDED_IDS.contains(id))
            .filter(|id| !EXCLUDED_PREFIXES.iter().any(|p| id.starts_with(p)))
            .filter(|id| !BLOCKED_SUBSTRINGS.iter().any(|s| id.contains(s)))
            .filter(|id| !snapshot_re().is_match(id))
            .map(str::to_string)
            .collect();
        filtered.sort_by(|a, b| family_rank(a).cmp(&family_rank(b)).then(a.cmp(b)));

        if !filtered.is_empty() {
            let mut cache = model_cache().lock().await;
            cache.insert(
                self.api_key.clone(),
                (Instant::now() + CACHE_TTL, filtered.clone()),
            );
        }
        filtered
    }
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

    #[test]
    fn model_filter_keeps_vision_families_and_drops_snapshots_and_blocked() {
        let ids = vec![
            "gpt-4.1",
            "gpt-4.1-mini",
            "gpt-4.1-2025-04-14",
            "gpt-4o",
            "gpt-5",
            "gpt-5-chat",
            "chatgpt-4o-latest",
            "o1-mini",
            "gpt-4.1-audio",
        ];
        let filtered: Vec<&str> = ids
            .into_iter()
            .filter(|id| ALLOWED_PREFIXES.iter().any(|p| id.starts_with(p)))
            .filter(|id| !EXCLUDED_IDS.contains(id))
            .filter(|id| !EXCLUDED_PREFIXES.iter().any(|p| id.starts_with(p)))
            .filter(|id| !BLOCKED_SUBSTRINGS.iter().any(|s| id.contains(s)))
            .filter(|id| !snapshot_re().is_match(id))
            .collect();
        // gpt-5 itself is a legitimate standalone model (only "gpt-5-chat"
        // is blocked via the "-chat" substring).
        assert_eq!(filtered, vec!["gpt-4.1", "gpt-4.1-mini", "gpt-5"]);
    }

    #[test]
    fn body_sends_max_completion_tokens_not_the_rejected_max_tokens() {
        // Issue #324: GPT-5 answers `max_tokens` with "Unsupported
        // parameter ... Use 'max_completion_tokens' instead".
        for model in ["gpt-5-mini", "gpt-4.1", "o3"] {
            let mut body = json!({"model": model});
            apply_model_tuning(&mut body, model, 0.2, 1234);
            assert_eq!(body["max_completion_tokens"], json!(1234), "{model}");
            assert!(body.get("max_tokens").is_none(), "{model}");
        }
    }

    #[test]
    fn reasoning_models_get_default_temperature_and_effort() {
        let mut body = json!({});
        apply_model_tuning(&mut body, "gpt-5.6-luna", 0.2, 2048);
        assert_eq!(body["temperature"], json!(1.0));
        assert_eq!(body["reasoning_effort"], json!("low"));

        let mut body = json!({});
        apply_model_tuning(&mut body, "gpt-4.1-mini", 0.2, 2048);
        assert_eq!(body["temperature"], json!(0.2));
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn length_error_names_the_parameter_actually_sent() {
        let msg = length_error("gpt-4.1", 2048);
        assert!(msg.contains("max_completion_tokens=2048"), "{msg}");
        assert!(!msg.contains("reasoning"), "{msg}");
        // Reasoning models burn part of the same budget invisibly, so the
        // advice to raise the limit needs that context.
        assert!(length_error("gpt-5-mini", 2048).contains("internal reasoning"));
    }

    #[test]
    fn family_rank_orders_newest_first() {
        assert!(family_rank("gpt-5") < family_rank("gpt-4.1"));
        assert!(family_rank("gpt-4.1") < family_rank("gpt-4o"));
        assert_eq!(family_rank("unknown-model"), 99);
    }
}
