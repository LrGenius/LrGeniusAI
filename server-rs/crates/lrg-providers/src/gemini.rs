//! Port of `providers/gemini.py`. The Python code goes through the
//! `google-genai` SDK; this talks to the same public Generative Language
//! REST API (`v1beta/models/{model}:generateContent`, `v1beta/models`)
//! directly via `reqwest`. Not live-tested against a real API key in this
//! environment (none was reachable) — request/response shapes follow the
//! documented REST contract and the SDK's own field mapping.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::edit_recipe::{gemini_edit_recipe_schema, normalize_edit_recipe};
use crate::gemini_schema::prepare_gemini_response_schema;
use crate::image_encode::image_to_base64;
use crate::keyword_taxonomy::KeywordLeafEncoding;
use crate::normalize::{alt_text_from, missing_field_warning, normalize_keywords};
use crate::prompts::{
    prepare_edit_system_prompt, prepare_edit_user_prompt, prepare_system_prompt,
    prepare_user_prompt,
};
use crate::types::{
    EditGenerationRequest, EditGenerationResponse, MetadataGenerationRequest,
    MetadataGenerationResponse, ProviderError,
};

const DEFAULT_MAX_TOKENS: u32 = 2048;
const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Gemini accepts the API key either as a `?key=` query parameter or in this
/// header. The header is the one to use: a URL travels into places a header
/// does not — `reqwest`'s error messages (which this module hands to the
/// plugin, and the plugin shows the user), proxy and server access logs, and
/// anything that later prints the request. Same wire, one fewer place for the
/// key to end up in clear text.
const API_KEY_HEADER: &str = "x-goog-api-key";

const ALLOWED_PREFIX: &str = "gemini-";
const BLOCKED_PREFIXES: [&str; 1] = ["gemini-1.0"];
const BLOCKED_SUBSTRINGS: [&str; 11] = [
    "tuning",
    "embedding",
    "aqa",
    "imagen",
    "bison",
    "tts",
    "image",
    "computer",
    "customtools",
    "robotics",
    "latest",
];
const CACHE_TTL: Duration = Duration::from_secs(3600);

fn snapshot_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"-\d{3}$").unwrap())
}

fn family_rank(model_id: &str) -> i32 {
    for (i, prefix) in ["gemini-3.", "gemini-2.5", "gemini-2."].iter().enumerate() {
        if model_id.starts_with(prefix) {
            return (i as i32) * 2 + i32::from(model_id.contains("preview"));
        }
    }
    99
}

/// Thinking config for models that support/require it, REST-shaped
/// (`thinkingBudget` or `thinkingLevel`), or `None` for everything else.
fn thinking_config(model_name: &str) -> Option<Value> {
    match model_name {
        "gemini-2.5-pro" => Some(json!({"thinkingBudget": 128})),
        "gemini-2.5-flash" | "gemini-2.5-flash-lite" => Some(json!({"thinkingBudget": 0})),
        "gemini-3-pro-preview" => Some(json!({"thinkingLevel": "low"})),
        _ => None,
    }
}

/// Port of `_clean_gemini_response`: strips markdown code-fence artifacts
/// Gemini sometimes wraps structured output in.
fn clean_gemini_response(text: &str) -> String {
    let mut t = text;
    if let Some(rest) = t.strip_prefix("```json") {
        t = rest;
    } else if let Some(rest) = t.strip_prefix("```") {
        t = rest;
    }
    if let Some(rest) = t.strip_suffix("```") {
        t = rest;
    }
    t.trim().to_string()
}

type ModelCache = std::collections::HashMap<String, (Instant, Vec<String>)>;
static MODEL_CACHE: OnceLock<Mutex<ModelCache>> = OnceLock::new();
fn model_cache() -> &'static Mutex<ModelCache> {
    MODEL_CACHE.get_or_init(|| Mutex::new(ModelCache::new()))
}

pub struct GeminiProvider {
    client: reqwest::Client,
    api_key: String,
}

impl GeminiProvider {
    pub fn new(api_key: String) -> Self {
        GeminiProvider {
            client: reqwest::Client::new(),
            api_key,
        }
    }

    pub fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }

    fn generation_config(
        &self,
        response_schema: Value,
        temperature: f64,
        max_output_tokens: u32,
        model_name: &str,
    ) -> Value {
        let mut config = json!({
            "responseMimeType": "application/json",
            "responseSchema": response_schema,
            "temperature": temperature,
            "maxOutputTokens": max_output_tokens,
        });
        if let Some(tc) = thinking_config(model_name) {
            config["thinkingConfig"] = tc;
        }
        config
    }

    async fn call_generate_content(
        &self,
        model_name: &str,
        system_instruction: &str,
        user_prompt: &str,
        image_data: &[u8],
        generation_config: Value,
    ) -> Result<Value, String> {
        let image_b64 = image_to_base64(image_data).map_err(|e| e.to_string())?;
        let url = format!("{API_BASE}/models/{model_name}:generateContent");
        let body = json!({
            "systemInstruction": {"parts": [{"text": system_instruction}]},
            "contents": [{
                "role": "user",
                "parts": [
                    {"text": user_prompt},
                    {"inlineData": {"mimeType": "image/jpeg", "data": image_b64}},
                ],
            }],
            "generationConfig": generation_config,
        });

        let resp = self
            .client
            .post(&url)
            .header(API_KEY_HEADER, &self.api_key)
            .json(&body)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| ProviderError::Http(e).to_string())?;
        let result: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Http(e).to_string())?;

        if let Some(err) = result.get("error") {
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown Gemini error");
            return Err(msg.to_string());
        }
        Ok(result)
    }

    /// Schema-free chat completion — see [`crate::provider::LlmProvider::generate_text`].
    pub async fn generate_text(
        &self,
        model: Option<&str>,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Option<String> {
        let model = model.unwrap_or("gemini-2.0-flash");
        let url = format!("{API_BASE}/models/{model}:generateContent");
        let body = json!({
            "systemInstruction": {"parts": [{"text": system_prompt}]},
            "contents": [{"role": "user", "parts": [{"text": user_prompt}]}],
            "generationConfig": {"temperature": 0.1, "maxOutputTokens": 4096},
        });
        let resp = self
            .client
            .post(&url)
            .header(API_KEY_HEADER, &self.api_key)
            .json(&body)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .ok()?;
        let result: Value = resp.json().await.ok()?;
        let parts = result["candidates"][0]["content"]["parts"].as_array()?;
        let text: String = parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        (!text.is_empty()).then_some(text)
    }

    pub async fn generate_metadata(
        &self,
        request: &MetadataGenerationRequest,
    ) -> MetadataGenerationResponse {
        let api_key = if let Some(k) = &request.api_key {
            k.clone()
        } else {
            self.api_key.clone()
        };
        if api_key.is_empty() {
            return fail(&request.uuid, "Gemini API not configured".to_string());
        }
        let effective = GeminiProvider::new(api_key);

        let system_instruction = prepare_system_prompt(request);
        let user_prompt = prepare_user_prompt(request);
        let response_schema = prepare_gemini_response_schema(request);
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
        let generation_config = effective.generation_config(
            response_schema,
            request.temperature,
            max_tokens,
            &request.model,
        );

        let result = match effective
            .call_generate_content(
                &request.model,
                &system_instruction,
                &user_prompt,
                &request.image_data,
                generation_config,
            )
            .await
        {
            Ok(r) => r,
            Err(e) => return fail(&request.uuid, e),
        };

        let (input_tokens, output_tokens) = usage_tokens(&result);

        if let Some(block_reason) = result
            .get("promptFeedback")
            .and_then(|pf| pf.get("blockReason"))
            .and_then(Value::as_str)
        {
            return MetadataGenerationResponse {
                uuid: request.uuid.clone(),
                success: false,
                error: Some(format!("Gemini blocked request: {block_reason}")),
                input_tokens,
                output_tokens,
                ..Default::default()
            };
        }

        let candidate = &result["candidates"][0];
        let finish_reason = candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .unwrap_or("");
        if finish_reason.contains("MAX_TOKENS") {
            return fail(&request.uuid, format!(
                "Gemini stopped before finishing the response because the token limit was reached (max_output_tokens={max_tokens}). Please raise the Max Tokens setting in the plugin (General tab → AI Model section) — try 4096 or higher. If you use hierarchical keywords, a large taxonomy increases token usage significantly."
            ));
        }

        let text = extract_text(candidate);
        let Some(text) = text else {
            return fail(
                &request.uuid,
                "Gemini returned no usable text in response".to_string(),
            );
        };
        let cleaned = clean_gemini_response(&text);
        let parsed: Value = match serde_json::from_str(&cleaned) {
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

    pub async fn generate_edit_recipe(
        &self,
        request: &EditGenerationRequest,
    ) -> EditGenerationResponse {
        let api_key = if let Some(k) = &request.api_key {
            k.clone()
        } else {
            self.api_key.clone()
        };
        if api_key.is_empty() {
            return fail_edit(&request.uuid, "Gemini API not configured".to_string());
        }
        let effective = GeminiProvider::new(api_key);

        let system_instruction = prepare_edit_system_prompt(request);
        let user_prompt = prepare_edit_user_prompt(request);
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
        let generation_config = effective.generation_config(
            gemini_edit_recipe_schema(request.is_raw).clone(),
            request.temperature,
            max_tokens,
            &request.model,
        );

        let result = match effective
            .call_generate_content(
                &request.model,
                &system_instruction,
                &user_prompt,
                &request.image_data,
                generation_config,
            )
            .await
        {
            Ok(r) => r,
            Err(e) => return fail_edit(&request.uuid, e),
        };

        let (input_tokens, output_tokens) = usage_tokens(&result);

        let candidate = &result["candidates"][0];
        let finish_reason = candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .unwrap_or("");
        if finish_reason.contains("MAX_TOKENS") {
            return fail_edit(&request.uuid, format!(
                "Gemini stopped before finishing the response because the token limit was reached (max_output_tokens={max_tokens}). Please raise the Max Tokens setting in the plugin (General tab → AI Model section) — try 4096 or higher."
            ));
        }

        let text = extract_text(candidate);
        let Some(text) = text else {
            return fail_edit(
                &request.uuid,
                "Gemini returned no usable text in response".to_string(),
            );
        };
        let cleaned = clean_gemini_response(&text);
        let parsed: Value = match serde_json::from_str(&cleaned) {
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

    /// Port of `list_available_models`: paginate `v1beta/models`, keep
    /// `generateContent`-capable `gemini-*` ids, apply block lists,
    /// collapse `<stem>`/`<stem>-latest` to the shorter id, cache 1h.
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

        let mut raw_models: Vec<Value> = Vec::new();
        let mut page_token: Option<String> = None;
        for _ in 0..20 {
            let mut url = format!("{API_BASE}/models?pageSize=200");
            if let Some(token) = &page_token {
                url.push_str(&format!("&pageToken={token}"));
            }
            let Ok(resp) = self
                .client
                .get(&url)
                .header(API_KEY_HEADER, &self.api_key)
                .send()
                .await
            else {
                return Vec::new();
            };
            let Ok(body) = resp.json::<Value>().await else {
                return Vec::new();
            };
            if let Some(models) = body.get("models").and_then(Value::as_array) {
                raw_models.extend(models.iter().cloned());
            }
            page_token = body
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if page_token.is_none() {
                break;
            }
        }

        let mut candidates: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for model in &raw_models {
            let methods = model
                .get("supportedGenerationMethods")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>())
                .unwrap_or_default();
            if !methods.contains(&"generateContent") {
                continue;
            }
            let full_name = model.get("name").and_then(Value::as_str).unwrap_or("");
            let model_id = full_name.strip_prefix("models/").unwrap_or(full_name);

            if !model_id.starts_with(ALLOWED_PREFIX) {
                continue;
            }
            if BLOCKED_PREFIXES.iter().any(|p| model_id.starts_with(p)) {
                continue;
            }
            if BLOCKED_SUBSTRINGS.iter().any(|s| model_id.contains(s)) {
                continue;
            }
            if snapshot_re().is_match(model_id) {
                continue;
            }

            let stem = model_id.strip_suffix("-latest").unwrap_or(model_id);
            match candidates.get(stem) {
                Some(existing) if existing.len() <= model_id.len() => {}
                _ => {
                    candidates.insert(stem.to_string(), model_id.to_string());
                }
            }
        }

        let mut filtered: Vec<String> = candidates.into_values().collect();
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

fn extract_text(candidate: &Value) -> Option<String> {
    let parts = candidate.get("content")?.get("parts")?.as_array()?;
    let collected: Vec<&str> = parts
        .iter()
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect();
    if collected.is_empty() {
        None
    } else {
        Some(collected.join("\n"))
    }
}

fn usage_tokens(result: &Value) -> (u32, u32) {
    let usage = result.get("usageMetadata");
    let input_tokens = usage
        .and_then(|u| u.get("promptTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let output_tokens = usage
        .and_then(|u| u.get("candidatesTokenCount"))
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
    fn clean_response_strips_json_code_fence() {
        assert_eq!(
            clean_gemini_response("```json\n{\"a\":1}\n```"),
            "{\"a\":1}"
        );
        assert_eq!(clean_gemini_response("```\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(clean_gemini_response("  {\"a\":1}  "), "{\"a\":1}");
    }

    #[test]
    fn family_rank_prefers_newest_and_penalizes_preview() {
        assert!(family_rank("gemini-3.0-pro") < family_rank("gemini-2.5-pro"));
        assert!(family_rank("gemini-2.5-pro") < family_rank("gemini-2.5-pro-preview"));
        assert!(family_rank("gemini-2.5-pro") < family_rank("gemini-2.0-flash"));
        assert_eq!(family_rank("gemini-1.0-pro"), 99);
    }

    #[test]
    fn thinking_config_matches_model_specific_shape() {
        assert_eq!(
            thinking_config("gemini-2.5-pro"),
            Some(json!({"thinkingBudget": 128}))
        );
        assert_eq!(
            thinking_config("gemini-2.5-flash"),
            Some(json!({"thinkingBudget": 0}))
        );
        assert_eq!(
            thinking_config("gemini-3-pro-preview"),
            Some(json!({"thinkingLevel": "low"}))
        );
        assert_eq!(thinking_config("gemini-2.0-flash"), None);
    }

    #[test]
    fn extract_text_joins_multiple_parts() {
        let candidate = json!({"content": {"parts": [{"text": "a"}, {"text": "b"}]}});
        assert_eq!(extract_text(&candidate), Some("a\nb".to_string()));
        assert_eq!(extract_text(&json!({})), None);
    }

    #[test]
    fn usage_tokens_reads_prompt_and_candidate_counts() {
        let result = json!({"usageMetadata": {"promptTokenCount": 12, "candidatesTokenCount": 34}});
        assert_eq!(usage_tokens(&result), (12, 34));
        assert_eq!(usage_tokens(&json!({})), (0, 0));
    }
}
