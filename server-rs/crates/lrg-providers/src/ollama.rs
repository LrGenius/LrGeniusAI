//! Port of `providers/ollama.py`. The Python code goes through the
//! `ollama` Python SDK, which is a thin wrapper over Ollama's REST API —
//! this talks to that REST API directly via `reqwest` instead of
//! depending on an SDK.

use std::time::Duration;

use serde_json::{json, Value};

use crate::edit_recipe::{normalize_edit_recipe, openai_edit_recipe_schema};
use crate::image_encode::image_to_base64;
use crate::normalize::normalize_keywords_structure;
use crate::prompts::{
    prepare_edit_system_prompt, prepare_edit_user_prompt, prepare_system_prompt,
    prepare_user_prompt,
};
use crate::schema::prepare_response_structure;
use crate::types::{
    EditGenerationRequest, EditGenerationResponse, MetadataGenerationRequest,
    MetadataGenerationResponse, ProviderError,
};

pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";
const DEFAULT_MAX_TOKENS: u32 = 2048;

pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
}

impl OllamaProvider {
    pub fn new(base_url: Option<String>) -> Self {
        OllamaProvider {
            client: reqwest::Client::new(),
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
        }
    }

    /// Port of `is_available`: a short-timeout probe against `/api/tags`.
    pub async fn is_available(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        self.client
            .get(&url)
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
    }

    /// Port of `list_available_models`.
    pub async fn list_available_models(&self) -> Vec<String> {
        let url = format!("{}/api/tags", self.base_url);
        let Ok(resp) = self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        else {
            return Vec::new();
        };
        let Ok(body) = resp.json::<Value>().await else {
            return Vec::new();
        };
        body.get("models")
            .and_then(Value::as_array)
            .map(|models| {
                models
                    .iter()
                    .filter_map(|m| {
                        m.get("name")
                            .or_else(|| m.get("model"))
                            .or_else(|| m.get("tag"))
                            .and_then(Value::as_str)
                    })
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
        let model = model.unwrap_or("llama3");
        let body = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt},
            ],
            "stream": false,
        });
        let resp = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .ok()?;
        let result: Value = resp.json().await.ok()?;
        result["message"]["content"].as_str().map(str::to_string)
    }

    pub async fn generate_metadata(
        &self,
        request: &MetadataGenerationRequest,
    ) -> MetadataGenerationResponse {
        let base = request.ollama_base_url.as_deref().unwrap_or(&self.base_url);
        let url = format!("{base}/api/chat");

        let image_b64 = match image_to_base64(&request.image_data) {
            Ok(b64) => b64,
            Err(e) => return fail(&request.uuid, e.to_string()),
        };
        let system_prompt = prepare_system_prompt(request);
        let user_prompt = prepare_user_prompt(request);
        let response_schema = prepare_response_structure(request);
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

        let body = json!({
            "model": request.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt, "images": [image_b64]},
            ],
            "format": response_schema,
            "options": {
                "temperature": request.temperature,
                "top_p": 0.9,
                "num_keep": -1,
                "num_predict": max_tokens,
            },
            "stream": false,
        });

        let resp = match self
            .client
            .post(&url)
            .json(&body)
            .timeout(std::time::Duration::from_secs(120))
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

        let done_reason = result.get("done_reason").and_then(Value::as_str);
        if done_reason == Some("length") {
            return fail(
                &request.uuid,
                format!(
                    "Ollama stopped before finishing the response because the token limit was reached (num_predict={max_tokens}). Please raise the Max Tokens setting in the plugin (General tab → AI Model section) — try 4096 or higher. If you use hierarchical keywords, a large taxonomy increases token usage significantly."
                ),
            );
        }

        let Some(content) = result
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
        else {
            return fail(
                &request.uuid,
                "Empty response content from Ollama".to_string(),
            );
        };
        if content.is_empty() {
            return fail(
                &request.uuid,
                "Empty response content from Ollama".to_string(),
            );
        }

        let parsed: Value = match serde_json::from_str(content) {
            Ok(v) => v,
            Err(e) => return fail(&request.uuid, format!("JSON parsing error: {e}")),
        };
        build_success(request, &parsed)
    }

    pub async fn generate_edit_recipe(
        &self,
        request: &EditGenerationRequest,
    ) -> EditGenerationResponse {
        let base = request.ollama_base_url.as_deref().unwrap_or(&self.base_url);
        let url = format!("{base}/api/chat");

        let image_b64 = match image_to_base64(&request.image_data) {
            Ok(b64) => b64,
            Err(e) => return fail_edit(&request.uuid, e.to_string()),
        };
        let system_prompt = prepare_edit_system_prompt(request);
        let user_prompt = prepare_edit_user_prompt(request);
        let response_schema = openai_edit_recipe_schema();
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

        let body = json!({
            "model": request.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt, "images": [image_b64]},
            ],
            "format": response_schema,
            "options": {
                "temperature": request.temperature,
                "top_p": 0.9,
                "num_keep": -1,
                "num_predict": max_tokens,
            },
            "stream": false,
        });

        let resp = match self
            .client
            .post(&url)
            .json(&body)
            .timeout(std::time::Duration::from_secs(120))
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

        let done_reason = result.get("done_reason").and_then(Value::as_str);
        if done_reason == Some("length") {
            return fail_edit(
                &request.uuid,
                format!(
                    "Ollama stopped before finishing the response because the token limit was reached (num_predict={max_tokens}). Please raise the Max Tokens setting in the plugin (General tab → AI Model section) — try 4096 or higher."
                ),
            );
        }

        let content = result
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if content.is_empty() {
            return fail_edit(
                &request.uuid,
                "Empty response content from Ollama".to_string(),
            );
        }

        let parsed: Value = match serde_json::from_str(content) {
            Ok(v) => v,
            Err(e) => return fail_edit(&request.uuid, format!("JSON parsing error: {e}")),
        };

        EditGenerationResponse {
            uuid: request.uuid.clone(),
            success: true,
            recipe: Some(normalize_edit_recipe(&parsed)),
            input_tokens: 0,
            output_tokens: 0,
            error: None,
            warning: None,
        }
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

fn build_success(
    request: &MetadataGenerationRequest,
    parsed: &Value,
) -> MetadataGenerationResponse {
    let keywords_raw = parsed.get("keywords").cloned().unwrap_or(json!([]));
    let keywords = normalize_keywords_structure(&keywords_raw);
    MetadataGenerationResponse {
        uuid: request.uuid.clone(),
        success: true,
        keywords: Some(keywords),
        caption: if request.generate_caption {
            parsed
                .get("caption")
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        },
        title: if request.generate_title {
            parsed
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        },
        alt_text: if request.generate_alt_text {
            parsed
                .get("alt_text")
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        },
        input_tokens: 0,
        output_tokens: 0,
        error: None,
        warning: None,
    }
}
