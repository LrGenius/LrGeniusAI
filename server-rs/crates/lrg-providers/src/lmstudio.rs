//! Port of `providers/lmstudio.py`. The Python code goes through the
//! `lmstudio-python` WebSocket SDK; per the rewrite plan this instead
//! talks to LM Studio's plain REST surface: the OpenAI-compatible
//! `/v1/chat/completions` for generation (same request shape as
//! `openai.rs`/`ollama.rs`), and LM Studio's own `/api/v0/models` for
//! listing — the OpenAI-compat `/v1/models` only returns already-loaded
//! models, whereas `/api/v0/models` matches the SDK's `list_downloaded()`
//! (all models on disk, loaded or not). Not live-tested against a running
//! LM Studio instance in this environment (none was reachable); the 2s
//! TCP probe from `is_available` is preserved so an offline host never
//! blocks on the request's own long timeout.

use std::time::Duration;

use serde_json::{json, Value};

use crate::edit_recipe::{normalize_edit_recipe, openai_edit_recipe_schema};
use crate::image_encode::image_to_base64;
use crate::keyword_taxonomy::KeywordLeafEncoding;
use crate::normalize::{alt_text_from, missing_field_warning, normalize_keywords};
use crate::prompts::{
    prepare_edit_system_prompt, prepare_edit_user_prompt, prepare_system_prompt,
    prepare_user_prompt,
};
use crate::schema::prepare_response_structure;
use crate::types::{
    EditGenerationRequest, EditGenerationResponse, MetadataGenerationRequest,
    MetadataGenerationResponse, ProviderError,
};

pub const DEFAULT_HOST: &str = "localhost:1234";
const DEFAULT_MAX_TOKENS: u32 = 2048;

fn normalize_base_url(host: &str) -> String {
    let trimmed = host.trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

pub struct LmStudioProvider {
    client: reqwest::Client,
    host: String,
}

impl LmStudioProvider {
    pub fn new(host: Option<String>) -> Self {
        LmStudioProvider {
            client: reqwest::Client::new(),
            host: host.unwrap_or_else(|| DEFAULT_HOST.to_string()),
        }
    }

    /// Port of `is_available`: a 2s TCP probe against `host:port` so an
    /// offline LM Studio instance never blocks on a long HTTP timeout.
    pub async fn is_available(&self) -> bool {
        let Some((host_part, port_str)) = self.host.rsplit_once(':') else {
            return false;
        };
        let host_part = host_part
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        let Ok(port) = port_str.trim_end_matches('/').parse::<u16>() else {
            return false;
        };
        let addr = format!("{host_part}:{port}");
        tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect(&addr),
        )
        .await
        .is_ok_and(|r| r.is_ok())
    }

    /// Port of `list_available_models`, via LM Studio's own `/api/v0/models`
    /// (lists every downloaded model, matching `list_downloaded()`) rather
    /// than the OpenAI-compat `/v1/models` (loaded models only).
    pub async fn list_available_models(&self) -> Vec<String> {
        let url = format!("{}/api/v0/models", normalize_base_url(&self.host));
        let Ok(resp) = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        else {
            return Vec::new();
        };
        let Ok(body) = resp.json::<Value>().await else {
            return Vec::new();
        };
        body.get("data")
            .and_then(Value::as_array)
            .map(|models| {
                models
                    .iter()
                    .filter_map(|m| m.get("id").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Schema-free chat completion — see [`crate::provider::LlmProvider::generate_text`].
    pub async fn generate_text(
        &self,
        model: Option<&str>,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Option<String> {
        let model = model.unwrap_or("local-model");
        let body = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt},
            ],
            "temperature": 0.1,
            "max_tokens": 4096,
        });
        let resp = self
            .client
            .post(format!(
                "{}/v1/chat/completions",
                normalize_base_url(&self.host)
            ))
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
        let host = request.lmstudio_base_url.as_deref().unwrap_or(&self.host);
        let url = format!("{}/v1/chat/completions", normalize_base_url(host));

        let image_b64 = match image_to_base64(&request.image_data) {
            Ok(b64) => b64,
            Err(e) => return fail(&request.uuid, e.to_string()),
        };
        let data_uri = format!("data:image/jpeg;base64,{image_b64}");
        let system_prompt = prepare_system_prompt(request);
        let user_prompt = prepare_user_prompt(request);
        let response_schema = prepare_response_structure(request);
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

        let body = json!({
            "model": request.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": [
                    {"type": "text", "text": user_prompt},
                    {"type": "image_url", "image_url": {"url": data_uri}},
                ]},
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": "metadata_response", "schema": response_schema},
            },
            "temperature": request.temperature,
            "max_tokens": max_tokens,
            "stream": false,
        });

        let resp = match self
            .client
            .post(&url)
            .json(&body)
            .timeout(Duration::from_secs(720))
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
                .unwrap_or("unknown LM Studio error");
            return fail(&request.uuid, msg.to_string());
        }

        let (input_tokens, output_tokens) = usage_tokens(&result);
        let choice = &result["choices"][0];
        let finish_reason = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or("");
        if finish_reason == "length" {
            return MetadataGenerationResponse {
                uuid: request.uuid.clone(),
                success: false,
                error: Some(format!(
                    "LM Studio stopped before finishing the response because the token limit was reached (max_tokens={max_tokens}). Please raise the Max Tokens setting in the plugin (General tab → AI Model section) — try 4096 or higher. If you use hierarchical keywords, a large taxonomy increases token usage significantly."
                )),
                input_tokens,
                output_tokens,
                ..Default::default()
            };
        }

        let Some(content) = choice["message"]["content"].as_str() else {
            return fail(
                &request.uuid,
                "Empty response content from LM Studio".to_string(),
            );
        };
        let parsed: Value = match serde_json::from_str(content) {
            Ok(v) => v,
            Err(e) => {
                return fail(
                    &request.uuid,
                    format!(
                        "LM Studio returned a response that could not be parsed as JSON (length={} chars). This often means the response was truncated — try raising the Max Tokens setting in the plugin. ({e})",
                        content.len()
                    ),
                )
            }
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

    pub async fn generate_edit_recipe(
        &self,
        request: &EditGenerationRequest,
    ) -> EditGenerationResponse {
        let host = request.lmstudio_base_url.as_deref().unwrap_or(&self.host);
        let url = format!("{}/v1/chat/completions", normalize_base_url(host));

        let image_b64 = match image_to_base64(&request.image_data) {
            Ok(b64) => b64,
            Err(e) => return fail_edit(&request.uuid, e.to_string()),
        };
        let data_uri = format!("data:image/jpeg;base64,{image_b64}");
        let system_prompt = prepare_edit_system_prompt(request);
        let user_prompt = prepare_edit_user_prompt(request);
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

        let body = json!({
            "model": request.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": [
                    {"type": "text", "text": user_prompt},
                    {"type": "image_url", "image_url": {"url": data_uri}},
                ]},
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": "lightroom_edit_recipe", "schema": openai_edit_recipe_schema(request.is_raw)},
            },
            "temperature": request.temperature,
            "max_tokens": max_tokens,
            "stream": false,
        });

        let resp = match self
            .client
            .post(&url)
            .json(&body)
            .timeout(Duration::from_secs(720))
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
                .unwrap_or("unknown LM Studio error");
            return fail_edit(&request.uuid, msg.to_string());
        }

        let (input_tokens, output_tokens) = usage_tokens(&result);
        let choice = &result["choices"][0];
        let finish_reason = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or("");
        if finish_reason == "length" {
            return EditGenerationResponse {
                uuid: request.uuid.clone(),
                success: false,
                error: Some(format!(
                    "LM Studio stopped before finishing the response because the token limit was reached (max_tokens={max_tokens}). Please raise the Max Tokens setting in the plugin (General tab → AI Model section) — try 4096 or higher."
                )),
                input_tokens,
                output_tokens,
                ..Default::default()
            };
        }

        let Some(content) = choice["message"]["content"].as_str() else {
            return fail_edit(
                &request.uuid,
                "Empty response content from LM Studio".to_string(),
            );
        };
        let parsed: Value = match serde_json::from_str(content) {
            Ok(v) => v,
            Err(e) => {
                return fail_edit(
                    &request.uuid,
                    format!(
                        "LM Studio returned a response that could not be parsed as JSON (length={} chars). This often means the response was truncated — try raising the Max Tokens setting in the plugin. ({e})",
                        content.len()
                    ),
                )
            }
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

    #[test]
    fn normalize_base_url_adds_scheme_when_missing() {
        assert_eq!(
            normalize_base_url("localhost:1234"),
            "http://localhost:1234"
        );
        assert_eq!(
            normalize_base_url("http://localhost:1234/"),
            "http://localhost:1234"
        );
        assert_eq!(
            normalize_base_url("https://lmstudio.local:1234"),
            "https://lmstudio.local:1234"
        );
    }

    #[tokio::test]
    async fn is_available_false_when_nothing_listening() {
        // Port 1 is a reserved low port that's never a running LM Studio
        // instance in CI/dev; the 2s probe should fail fast without hanging.
        let provider = LmStudioProvider::new(Some("127.0.0.1:1".to_string()));
        assert!(!provider.is_available().await);
    }
}
