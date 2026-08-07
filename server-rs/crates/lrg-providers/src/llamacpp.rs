//! The provider that talks to an in-process engine ([`crate::local`]) instead
//! of an HTTP endpoint.
//!
//! Two things make it different from the REST providers, and both come from
//! owning the inference loop:
//!
//! * the prompt is handed over pre-split, so the engine can pin the
//!   run-constant half in its KV cache instead of re-prefilling it per photo;
//! * [`LlmProvider::generate_metadata_batch`] is a real batch — every photo in
//!   one call shares that pinned prefix and decodes in its own sequence,
//!   instead of the sequential loop the default impl provides.
//!
//! Note there is no `llama-cpp` dependency here, or anywhere in this crate:
//! the engine arrives as a `dyn LocalEngine` built by `lrg-api`.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::edit_recipe::{normalize_edit_recipe, openai_edit_recipe_schema};
use crate::image_encode::image_to_rgb;
use crate::local::{LocalImage, LocalRequest, SharedLocalEngine};
use crate::normalize::normalize_keywords_structure;
use crate::prompts::{
    prepare_edit_system_prompt, prepare_edit_user_prompt_split, prepare_system_prompt,
    prepare_user_prompt_split,
};
use crate::provider::LlmProvider;
use crate::schema::prepare_response_structure;
use crate::types::{
    EditGenerationRequest, EditGenerationResponse, MetadataGenerationRequest,
    MetadataGenerationResponse,
};

const DEFAULT_MAX_TOKENS: u32 = 2048;

pub struct LlamaCppProvider {
    engine: SharedLocalEngine,
}

impl LlamaCppProvider {
    #[must_use]
    pub fn new(engine: SharedLocalEngine) -> Self {
        LlamaCppProvider { engine }
    }

    /// Translate one metadata request into engine form. Kept separate from
    /// dispatch so the single and batch paths cannot drift apart.
    fn build_local_request(request: &MetadataGenerationRequest) -> Result<LocalRequest, String> {
        let image = if request.image_data.is_empty() {
            None
        } else {
            let (width, height, rgb) =
                image_to_rgb(&request.image_data).map_err(|e| e.to_string())?;
            Some(LocalImage { width, height, rgb })
        };
        let split = prepare_user_prompt_split(request);
        Ok(LocalRequest {
            system_prompt: prepare_system_prompt(request),
            stable_prompt: split.stable,
            per_photo_prompt: split.per_photo,
            image,
            schema: Some(prepare_response_structure(request)),
            max_tokens: request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            temperature: request.temperature as f32,
        })
    }

    fn metadata_from_text(
        request: &MetadataGenerationRequest,
        text: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) -> MetadataGenerationResponse {
        let parsed: Value = match serde_json::from_str(text.trim()) {
            Ok(v) => v,
            Err(e) => {
                return fail(
                    &request.uuid,
                    format!(
                        "the local model returned a response that could not be parsed as JSON \
                         (length={} chars). This usually means it hit the token limit — try \
                         raising Max Tokens in the plugin. ({e})",
                        text.len()
                    ),
                )
            }
        };

        let keywords =
            normalize_keywords_structure(&parsed.get("keywords").cloned().unwrap_or(json!([])));
        let field = |name: &str, wanted: bool| {
            wanted
                .then(|| parsed.get(name).and_then(Value::as_str).map(str::to_string))
                .flatten()
        };
        MetadataGenerationResponse {
            uuid: request.uuid.clone(),
            success: true,
            keywords: Some(keywords),
            caption: field("caption", request.generate_caption),
            title: field("title", request.generate_title),
            alt_text: field("alt_text", request.generate_alt_text),
            input_tokens: prompt_tokens,
            output_tokens: completion_tokens,
            error: None,
            warning: None,
        }
    }
}

#[async_trait]
impl LlmProvider for LlamaCppProvider {
    fn name(&self) -> &'static str {
        "llamacpp"
    }

    fn preferred_batch_size(&self) -> usize {
        self.engine.preferred_batch_size()
    }

    async fn generate_metadata(
        &self,
        request: &MetadataGenerationRequest,
    ) -> MetadataGenerationResponse {
        let mut responses = self
            .generate_metadata_batch(std::slice::from_ref(request))
            .await;
        responses.pop().unwrap_or_else(|| {
            fail(
                &request.uuid,
                "the local model returned no response".to_string(),
            )
        })
    }

    /// The batch path proper: one engine call for the whole group, so the
    /// pinned prefix is evaluated once and the photos decode in parallel
    /// sequences.
    async fn generate_metadata_batch(
        &self,
        requests: &[MetadataGenerationRequest],
    ) -> Vec<MetadataGenerationResponse> {
        // Requests that fail to convert (an undecodable image, say) never reach
        // the engine, but must still occupy their slot in the response so the
        // caller can zip results back to photos.
        let mut local_requests = Vec::with_capacity(requests.len());
        let mut conversion_errors: Vec<Option<String>> = Vec::with_capacity(requests.len());
        for request in requests {
            match Self::build_local_request(request) {
                Ok(local) => {
                    local_requests.push(local);
                    conversion_errors.push(None);
                }
                Err(e) => conversion_errors.push(Some(e)),
            }
        }

        let mut engine_results = self.engine.generate(local_requests).await.into_iter();

        requests
            .iter()
            .zip(conversion_errors)
            .map(|(request, conversion_error)| {
                if let Some(e) = conversion_error {
                    return fail(&request.uuid, e);
                }
                match engine_results.next() {
                    Some(Ok(output)) => Self::metadata_from_text(
                        request,
                        &output.text,
                        output.prompt_tokens,
                        output.completion_tokens,
                    ),
                    Some(Err(e)) => fail(&request.uuid, e),
                    None => fail(
                        &request.uuid,
                        "the local model returned no response".to_string(),
                    ),
                }
            })
            .collect()
    }

    async fn generate_edit_recipe(
        &self,
        request: &EditGenerationRequest,
    ) -> EditGenerationResponse {
        let image = if request.image_data.is_empty() {
            None
        } else {
            match image_to_rgb(&request.image_data) {
                Ok((width, height, rgb)) => Some(LocalImage { width, height, rgb }),
                Err(e) => return fail_edit(&request.uuid, e.to_string()),
            }
        };
        let split = prepare_edit_user_prompt_split(request);
        let local = LocalRequest {
            system_prompt: prepare_edit_system_prompt(request),
            stable_prompt: split.stable,
            per_photo_prompt: split.per_photo,
            image,
            schema: Some(openai_edit_recipe_schema().clone()),
            max_tokens: request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            temperature: request.temperature as f32,
        };

        let output = match self.engine.generate(vec![local]).await.pop() {
            Some(Ok(output)) => output,
            Some(Err(e)) => return fail_edit(&request.uuid, e),
            None => {
                return fail_edit(
                    &request.uuid,
                    "the local model returned no response".to_string(),
                )
            }
        };

        let parsed: Value = match serde_json::from_str(output.text.trim()) {
            Ok(v) => v,
            Err(e) => {
                return fail_edit(
                    &request.uuid,
                    format!(
                        "the local model returned a response that could not be parsed as JSON \
                         (length={} chars). This usually means it hit the token limit — try \
                         raising Max Tokens in the plugin. ({e})",
                        output.text.len()
                    ),
                )
            }
        };

        EditGenerationResponse {
            uuid: request.uuid.clone(),
            success: true,
            recipe: Some(normalize_edit_recipe(&parsed)),
            input_tokens: output.prompt_tokens,
            output_tokens: output.completion_tokens,
            error: None,
            warning: None,
        }
    }

    async fn generate_text(
        &self,
        _model: Option<&str>,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Option<String> {
        // The model is whichever GGUF is loaded; there is nothing to select.
        let local = LocalRequest {
            system_prompt: system_prompt.to_string(),
            // The caller's prompt is one blob with no stable/volatile split, so
            // treat all of it as run-constant: consecutive chunks of a keyword
            // clustering run share a long preamble and differ only at the end,
            // which the engine's own prefix matching still exploits.
            stable_prompt: user_prompt.to_string(),
            per_photo_prompt: String::new(),
            image: None,
            schema: None,
            max_tokens: 4096,
            temperature: 0.1,
        };
        match self.engine.generate(vec![local]).await.pop() {
            Some(Ok(output)) => Some(output.text),
            Some(Err(e)) => {
                log::warn!("local model text generation failed: {e}");
                None
            }
            None => None,
        }
    }

    async fn list_available_models(&self) -> Vec<String> {
        vec![self.engine.model_name()]
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
    use crate::local::{LocalEngine, LocalResponse};
    use std::sync::{Arc, Mutex};

    /// Records what the provider hands the engine and replays canned answers.
    struct FakeEngine {
        seen: Mutex<Vec<LocalRequest>>,
        replies: Mutex<Vec<Result<LocalResponse, String>>>,
    }

    impl FakeEngine {
        fn new(replies: Vec<Result<LocalResponse, String>>) -> Arc<Self> {
            Arc::new(FakeEngine {
                seen: Mutex::new(Vec::new()),
                replies: Mutex::new(replies),
            })
        }
        fn text(body: &str) -> Result<LocalResponse, String> {
            Ok(LocalResponse {
                text: body.to_string(),
                prompt_tokens: 11,
                completion_tokens: 7,
                prefix_reused: true,
            })
        }
    }

    #[async_trait]
    impl LocalEngine for FakeEngine {
        async fn generate(
            &self,
            requests: Vec<LocalRequest>,
        ) -> Vec<Result<LocalResponse, String>> {
            let n = requests.len();
            self.seen.lock().unwrap().extend(requests);
            let mut replies = self.replies.lock().unwrap();
            let take = n.min(replies.len());
            replies.drain(..take).collect()
        }
        fn supports_vision(&self) -> bool {
            true
        }
        fn model_name(&self) -> String {
            "fake.gguf".to_string()
        }
    }

    fn request(uuid: &str) -> MetadataGenerationRequest {
        MetadataGenerationRequest {
            uuid: uuid.to_string(),
            language: "English".to_string(),
            generate_keywords: true,
            generate_caption: true,
            catalog_keywords: Some(vec!["Landscape".to_string()]),
            date_time: Some("2026-08-07".to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn hands_the_engine_a_split_prompt() {
        let engine = FakeEngine::new(vec![FakeEngine::text(r#"{"caption":"a view"}"#)]);
        let provider = LlamaCppProvider::new(engine.clone());
        let response = provider.generate_metadata(&request("p1")).await;

        assert!(response.success);
        assert_eq!(response.caption.as_deref(), Some("a view"));

        let seen = engine.seen.lock().unwrap();
        let sent = &seen[0];
        assert!(
            sent.stable_prompt.contains("Landscape"),
            "run-constant vocabulary belongs in the pinned half"
        );
        assert!(
            sent.per_photo_prompt.contains("2026-08-07"),
            "per-photo capture time belongs in the volatile half"
        );
        assert!(
            !sent.stable_prompt.contains("2026-08-07"),
            "a per-photo fact in the pinned half would break the cache every photo"
        );
        assert!(sent.schema.is_some(), "decoding must be schema-constrained");
    }

    #[tokio::test]
    async fn batches_every_photo_into_a_single_engine_call() {
        let engine = FakeEngine::new(vec![
            FakeEngine::text(r#"{"caption":"one"}"#),
            FakeEngine::text(r#"{"caption":"two"}"#),
            FakeEngine::text(r#"{"caption":"three"}"#),
        ]);
        let provider = LlamaCppProvider::new(engine.clone());
        let requests = vec![request("p1"), request("p2"), request("p3")];

        let responses = provider.generate_metadata_batch(&requests).await;

        assert_eq!(responses.len(), 3);
        assert_eq!(responses[2].caption.as_deref(), Some("three"));
        assert_eq!(
            engine.seen.lock().unwrap().len(),
            3,
            "all three photos must reach the engine in one batch"
        );
    }

    #[tokio::test]
    async fn a_failed_photo_does_not_shift_the_others() {
        // The middle photo carries an undecodable image, so it never reaches
        // the engine — the remaining replies must still line up correctly.
        let engine = FakeEngine::new(vec![
            FakeEngine::text(r#"{"caption":"one"}"#),
            FakeEngine::text(r#"{"caption":"three"}"#),
        ]);
        let provider = LlamaCppProvider::new(engine.clone());
        let mut broken = request("p2");
        broken.image_data = b"definitely not an image".to_vec();
        let requests = vec![request("p1"), broken, request("p3")];

        let responses = provider.generate_metadata_batch(&requests).await;

        assert_eq!(responses.len(), 3);
        assert!(responses[0].success);
        assert!(!responses[1].success, "the broken photo must fail");
        assert_eq!(responses[1].uuid, "p2");
        assert!(responses[2].success, "the third photo must still succeed");
        assert_eq!(responses[2].caption.as_deref(), Some("three"));
        assert_eq!(engine.seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn unparseable_output_reports_a_token_limit_hint() {
        let engine = FakeEngine::new(vec![FakeEngine::text("{\"caption\": \"truncat")]);
        let provider = LlamaCppProvider::new(engine);
        let response = provider.generate_metadata(&request("p1")).await;

        assert!(!response.success);
        assert!(response.error.unwrap().contains("Max Tokens"));
    }

    #[tokio::test]
    async fn engine_errors_surface_against_the_right_photo() {
        let engine = FakeEngine::new(vec![Err("context overflow".to_string())]);
        let provider = LlamaCppProvider::new(engine);
        let response = provider.generate_metadata(&request("p9")).await;

        assert!(!response.success);
        assert_eq!(response.uuid, "p9");
        assert_eq!(response.error.as_deref(), Some("context overflow"));
    }
}
