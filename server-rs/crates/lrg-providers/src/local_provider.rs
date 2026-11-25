//! The provider that talks to a local engine ([`crate::local`]) instead of an
//! HTTP endpoint.
//!
//! Two things make it different from the REST providers, and both come from
//! owning the inference loop:
//!
//! * the prompt is handed over pre-split, so an engine that can pin the
//!   run-constant half in its KV cache does not re-prefill it per photo;
//! * [`LlmProvider::generate_metadata_batch`] is a real batch — every photo in
//!   one call goes to the engine together, instead of the sequential loop the
//!   default impl provides.
//!
//! This is backend-agnostic and serves both local backends: llama.cpp
//! (`lrg-llama`) and MLX (`lrg-mlx`). Neither is a dependency here, or anywhere
//! in this crate — the engine arrives as a `dyn LocalEngine` built by
//! `lrg-api`, and the only thing the provider knows about which one it got is
//! the [`LocalProvider::backend`] label it reports in logs.
//!
//! The two do differ in what the split prompt buys: llama.cpp genuinely pins
//! the prefix, while the MLX sidecar re-prefills it per photo (see
//! `lrg_mlx`'s module docs). Sending the prompt pre-split regardless keeps the
//! ordering correct for both.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::edit_recipe::{normalize_edit_recipe, openai_edit_recipe_schema};
use crate::image_encode::image_to_rgb;
use crate::keyword_taxonomy::KeywordLeafEncoding;
use crate::local::{FinishReason, LocalImage, LocalRequest, LocalResponse, SharedLocalEngine};
use crate::normalize::{alt_text_from, missing_field_warning, normalize_keywords};
use crate::prompts::{
    prepare_edit_system_prompt, prepare_edit_user_prompt_split, prepare_system_prompt,
    prepare_user_prompt_split,
};
use crate::provider::LlmProvider;
use crate::schema::{bounded_for_constrained_decoding, prepare_response_structure};
use crate::types::{
    EditGenerationRequest, EditGenerationResponse, MetadataGenerationRequest,
    MetadataGenerationResponse,
};

const DEFAULT_MAX_TOKENS: u32 = 2048;

pub struct LocalProvider {
    engine: SharedLocalEngine,
    /// Which local backend produced `engine`, as reported by
    /// [`LlmProvider::name`]. A label: it appears in the indexing logs so a
    /// run can be attributed to the right backend.
    backend: &'static str,
    /// Whether the metadata schema gets upper bounds on its strings and lists
    /// ([`bounded_for_constrained_decoding`]). llama.cpp decodes against the
    /// schema through llguidance, which enforces them; the MLX sidecar's
    /// grammar library has not been checked against `maxLength`, so it keeps
    /// the schema it was tested with.
    bound_output: bool,
}

impl LocalProvider {
    #[must_use]
    pub fn llamacpp(engine: SharedLocalEngine) -> Self {
        LocalProvider {
            engine,
            backend: "llamacpp",
            bound_output: true,
        }
    }

    #[must_use]
    pub fn mlx(engine: SharedLocalEngine) -> Self {
        LocalProvider {
            engine,
            backend: "mlx",
            bound_output: false,
        }
    }

    /// Translate one metadata request into engine form. Kept separate from
    /// dispatch so the single and batch paths cannot drift apart.
    fn build_local_request(
        &self,
        request: &MetadataGenerationRequest,
    ) -> Result<LocalRequest, String> {
        let image = if request.image_data.is_empty() {
            None
        } else {
            let (width, height, rgb) =
                image_to_rgb(&request.image_data).map_err(|e| e.to_string())?;
            Some(LocalImage { width, height, rgb })
        };
        let split = prepare_user_prompt_split(request);
        let schema = prepare_response_structure(request);
        let schema = if self.bound_output {
            bounded_for_constrained_decoding(&schema)
        } else {
            schema
        };
        Ok(LocalRequest {
            system_prompt: prepare_system_prompt(request),
            stable_prompt: split.stable,
            per_photo_prompt: split.per_photo,
            image,
            schema: Some(schema),
            max_tokens: request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            temperature: request.temperature as f32,
        })
    }

    fn metadata_from_text(
        request: &MetadataGenerationRequest,
        output: &LocalResponse,
    ) -> MetadataGenerationResponse {
        let text = output.text.as_str();
        let prompt_tokens = output.prompt_tokens;
        let completion_tokens = output.completion_tokens;
        if output.finish == FinishReason::Repetition {
            // Checked before parsing: the engine stopped the answer partway,
            // so it is never complete JSON, and "raise Max Tokens" — the hint
            // a parse error gives — would only make the next loop longer.
            return fail(
                &request.uuid,
                format!(
                    "the local model started repeating itself and was stopped after \
                     {completion_tokens} tokens, so its answer is incomplete. Raising Max Tokens \
                     will not help. Try this photo with another model, or with a slightly higher \
                     Temperature."
                ),
            );
        }
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

        let keywords = normalize_keywords(
            &parsed.get("keywords").cloned().unwrap_or(json!([])),
            request.keyword_categories.as_ref(),
            KeywordLeafEncoding::for_request(request.bilingual_keywords, request.generate_aliases),
        );
        let field = |name: &str, wanted: bool| {
            wanted
                .then(|| parsed.get(name).and_then(Value::as_str).map(str::to_string))
                .flatten()
        };
        let caption = field("caption", request.generate_caption);
        let alt_text = alt_text_from(&parsed, request.generate_alt_text, caption.as_ref());
        let title = field("title", request.generate_title);
        // A requested field the model did not return is a degraded
        // success, not a success: see `missing_field_warning`. This matters
        // more here than for the cloud providers — a small local model is the
        // most likely of the five to silently drop a field.
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
            input_tokens: prompt_tokens,
            output_tokens: completion_tokens,
            error: None,
            warning,
        }
    }
}

#[async_trait]
impl LlmProvider for LocalProvider {
    fn name(&self) -> &'static str {
        self.backend
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
            match self.build_local_request(request) {
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
                    Some(Ok(output)) => {
                        // `prefix_reused` exists only here — it is not part of
                        // `MetadataGenerationResponse`. It is the one signal
                        // that says whether the run-constant half of the
                        // prompt (system prompt, keyword taxonomy) is being
                        // re-prefilled for every photo, which silently turns
                        // prefill into the dominant per-photo cost. The MLX
                        // sidecar always reports false; llama.cpp reporting
                        // false means its prefix cache did not engage.
                        log::debug!(
                            "local engine: prompt={} completion={} prefix_reused={}",
                            output.prompt_tokens,
                            output.completion_tokens,
                            output.prefix_reused
                        );
                        Self::metadata_from_text(request, &output)
                    }
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
            schema: Some(openai_edit_recipe_schema(request.is_raw).clone()),
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
            recipe: Some(normalize_edit_recipe(&parsed, request.is_raw)),
            input_tokens: output.prompt_tokens,
            output_tokens: output.completion_tokens,
            error: None,
            warning: None,
            guardrail_reasons: Vec::new(),
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
                finish: FinishReason::Stop,
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
        let provider = LocalProvider::llamacpp(engine.clone());
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
        let provider = LocalProvider::llamacpp(engine.clone());
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
        let provider = LocalProvider::llamacpp(engine.clone());
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
        let provider = LocalProvider::llamacpp(engine);
        let response = provider.generate_metadata(&request("p1")).await;

        assert!(!response.success);
        assert!(response.error.unwrap().contains("Max Tokens"));
    }

    /// Issue #490: a loop that ran into the token limit was reported as
    /// "raise Max Tokens", which only made the next loop longer.
    #[tokio::test]
    async fn a_stopped_loop_is_reported_as_a_loop() {
        let engine = FakeEngine::new(vec![Ok(LocalResponse {
            text: "{\"caption\": \"a dune a dune a dune".to_string(),
            prompt_tokens: 1200,
            completion_tokens: 64,
            prefix_reused: true,
            finish: FinishReason::Repetition,
        })]);
        let provider = LocalProvider::llamacpp(engine);
        let response = provider.generate_metadata(&request("p1")).await;

        assert!(!response.success);
        let error = response.error.unwrap();
        assert!(error.contains("repeating itself"), "{error}");
        assert!(error.contains("will not help"), "{error}");
    }

    #[tokio::test]
    async fn llamacpp_decodes_against_a_bounded_schema_and_mlx_does_not() {
        let reply = || FakeEngine::text(r#"{"caption":"a view"}"#);
        let llama = FakeEngine::new(vec![reply()]);
        LocalProvider::llamacpp(llama.clone())
            .generate_metadata(&request("p1"))
            .await;
        let schema = llama.seen.lock().unwrap()[0].schema.clone().unwrap();
        assert!(schema["properties"]["caption"].get("maxLength").is_some());

        let mlx = FakeEngine::new(vec![reply()]);
        LocalProvider::mlx(mlx.clone())
            .generate_metadata(&request("p1"))
            .await;
        let schema = mlx.seen.lock().unwrap()[0].schema.clone().unwrap();
        assert!(schema["properties"]["caption"].get("maxLength").is_none());
    }

    #[tokio::test]
    async fn engine_errors_surface_against_the_right_photo() {
        let engine = FakeEngine::new(vec![Err("context overflow".to_string())]);
        let provider = LocalProvider::llamacpp(engine);
        let response = provider.generate_metadata(&request("p9")).await;

        assert!(!response.success);
        assert_eq!(response.uuid, "p9");
        assert_eq!(response.error.as_deref(), Some("context overflow"));
    }
}
