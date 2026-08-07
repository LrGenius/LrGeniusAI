//! The provider seam: one trait every LLM backend implements, plus the single
//! place that maps a wire provider name onto an implementation.
//!
//! Before this existed the same four-arm `match` was copy-pasted across
//! `index_upload.rs`, `edit.rs`, `keywords.rs`, `server.rs` and `text_llm.rs`,
//! and they had already drifted apart: `"openai"` was accepted as an alias for
//! `"chatgpt"` by two of them and rejected by the other three. Name resolution
//! now happens exactly once, in [`build_provider`].

use std::sync::Arc;

use async_trait::async_trait;

use crate::gemini::GeminiProvider;
use crate::llamacpp::LlamaCppProvider;
use crate::lmstudio::LmStudioProvider;
use crate::local::SharedLocalEngine;
use crate::ollama::OllamaProvider;
use crate::openai::OpenAiProvider;
use crate::types::{
    EditGenerationRequest, EditGenerationResponse, MetadataGenerationRequest,
    MetadataGenerationResponse,
};

#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Canonical wire name, i.e. the key this provider is listed under in
    /// `/models` and the string the plugin sends back as `provider`.
    fn name(&self) -> &'static str;

    async fn generate_metadata(
        &self,
        request: &MetadataGenerationRequest,
    ) -> MetadataGenerationResponse;

    async fn generate_edit_recipe(&self, request: &EditGenerationRequest)
        -> EditGenerationResponse;

    /// Schema-free chat completion, used by the keyword-cluster validation.
    /// Returns `None` on any failure (network, bad credentials, unexpected
    /// response shape) — callers fall back to the unvalidated CLIP clusters
    /// rather than surfacing an error.
    async fn generate_text(
        &self,
        model: Option<&str>,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Option<String>;

    async fn list_available_models(&self) -> Vec<String>;

    /// Cheap reachability probe. Locally hosted backends override this so an
    /// offline host fails in milliseconds instead of on the request timeout;
    /// remote providers are assumed reachable.
    async fn is_available(&self) -> bool {
        true
    }

    /// How many photos this provider wants per
    /// [`generate_metadata_batch`](Self::generate_metadata_batch) call.
    ///
    /// The default of 1 is what keeps the HTTP providers byte-identical to
    /// their pre-batching behaviour: one photo per call means the default
    /// [`generate_metadata_batch`] loops exactly once, so they still issue one
    /// request per photo. Only in-process backends report more.
    fn preferred_batch_size(&self) -> usize {
        1
    }

    /// Generate metadata for a group of photos.
    ///
    /// The default is a sequential loop, which is exactly what every caller
    /// did before this seam existed. In-process backends override it to share
    /// one pinned prompt prefix across parallel decode sequences — which is
    /// the entire point of batching photos in the first place.
    async fn generate_metadata_batch(
        &self,
        requests: &[MetadataGenerationRequest],
    ) -> Vec<MetadataGenerationResponse> {
        let mut responses = Vec::with_capacity(requests.len());
        for request in requests {
            responses.push(self.generate_metadata(request).await);
        }
        responses
    }
}

/// The four REST providers implement the trait by delegating to their own
/// inherent methods. Keeping the boilerplate here rather than in each provider
/// module leaves those files as pure REST clients, and makes it obvious at a
/// glance that no provider quietly diverges from the contract.
macro_rules! impl_llm_provider {
    ($ty:ty, $name:literal $(, $is_available:ident)?) => {
        #[async_trait]
        impl LlmProvider for $ty {
            fn name(&self) -> &'static str {
                $name
            }

            async fn generate_metadata(
                &self,
                request: &MetadataGenerationRequest,
            ) -> MetadataGenerationResponse {
                <$ty>::generate_metadata(self, request).await
            }

            async fn generate_edit_recipe(
                &self,
                request: &EditGenerationRequest,
            ) -> EditGenerationResponse {
                <$ty>::generate_edit_recipe(self, request).await
            }

            async fn generate_text(
                &self,
                model: Option<&str>,
                system_prompt: &str,
                user_prompt: &str,
            ) -> Option<String> {
                <$ty>::generate_text(self, model, system_prompt, user_prompt).await
            }

            async fn list_available_models(&self) -> Vec<String> {
                <$ty>::list_available_models(self).await
            }

            $(
                async fn $is_available(&self) -> bool {
                    <$ty>::is_available(self).await
                }
            )?
        }
    };
}

impl_llm_provider!(OpenAiProvider, "chatgpt");
impl_llm_provider!(GeminiProvider, "gemini");
impl_llm_provider!(OllamaProvider, "ollama", is_available);
impl_llm_provider!(LmStudioProvider, "lmstudio", is_available);

/// Everything needed to pick and construct a provider. Credentials and base
/// URLs live here rather than on the individual requests because they are
/// connection-level, not per-photo.
#[derive(Clone, Default)]
pub struct ProviderSelection {
    pub name: String,
    pub api_key: Option<String>,
    pub ollama_base_url: Option<String>,
    pub lmstudio_base_url: Option<String>,
    /// The in-process engine, when one is loaded. Supplied by `lrg-api`, which
    /// is the only crate that knows about `lrg-llama`; `None` here is why
    /// `"llamacpp"` reports "no local model loaded" rather than being unknown.
    pub local_engine: Option<SharedLocalEngine>,
}

impl std::fmt::Debug for ProviderSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderSelection")
            .field("name", &self.name)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("ollama_base_url", &self.ollama_base_url)
            .field("lmstudio_base_url", &self.lmstudio_base_url)
            .field("local_engine", &self.local_engine.is_some())
            .finish()
    }
}

impl ProviderSelection {
    pub fn new(name: impl Into<String>) -> Self {
        ProviderSelection {
            name: name.into(),
            ..Default::default()
        }
    }
}

/// Provider names accepted on the wire, canonical form first.
pub const KNOWN_PROVIDERS: &[&str] = &["chatgpt", "gemini", "ollama", "lmstudio", "llamacpp"];

/// Resolve a wire provider name to a live client.
///
/// `Err` carries a user-facing message: the plugin surfaces it verbatim, so
/// the wording matters.
pub fn build_provider(selection: &ProviderSelection) -> Result<Arc<dyn LlmProvider>, String> {
    let name = selection.name.trim().to_lowercase();
    let non_empty_key = || {
        selection
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_string)
    };

    match name.as_str() {
        "ollama" => Ok(Arc::new(OllamaProvider::new(
            selection.ollama_base_url.clone(),
        ))),
        "lmstudio" => Ok(Arc::new(LmStudioProvider::new(
            selection.lmstudio_base_url.clone(),
        ))),
        // `openai` has always been accepted as an alias by the index and edit
        // routes; honouring it everywhere is strictly less surprising.
        "chatgpt" | "openai" => match non_empty_key() {
            Some(key) => Ok(Arc::new(OpenAiProvider::new(key))),
            None => Err("OpenAI API not configured".to_string()),
        },
        "gemini" => match non_empty_key() {
            Some(key) => Ok(Arc::new(GeminiProvider::new(key))),
            None => Err("Gemini API not configured".to_string()),
        },
        "llamacpp" => match &selection.local_engine {
            Some(engine) => Ok(Arc::new(LlamaCppProvider::new(engine.clone()))),
            None => Err(
                "No local model is loaded. Download or select a GGUF model in the plugin \
                 settings first."
                    .to_string(),
            ),
        },
        other => Err(format!("Unknown provider '{other}'.")),
    }
}

/// Whether `name` is a provider we know how to build at all, ignoring whether
/// its credentials happen to be configured.
pub fn is_known_provider(name: &str) -> bool {
    matches!(
        name.trim().to_lowercase().as_str(),
        "chatgpt" | "openai" | "gemini" | "ollama" | "lmstudio" | "llamacpp"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The HTTP providers must keep issuing one request per photo. A width of
    /// 1 makes the default `generate_metadata_batch` loop exactly once, which
    /// is what makes server-side batching a no-op for them.
    #[test]
    fn remote_providers_do_not_batch() {
        for name in ["chatgpt", "gemini", "ollama", "lmstudio"] {
            let selection = ProviderSelection {
                name: name.to_string(),
                api_key: Some("sk-test".to_string()),
                ..Default::default()
            };
            let provider = build_provider(&selection).expect("should build");
            assert_eq!(
                provider.preferred_batch_size(),
                1,
                "{name} must not batch photos"
            );
        }
    }

    #[test]
    fn openai_alias_resolves_like_chatgpt() {
        for name in ["chatgpt", "openai", "OpenAI", " ChatGPT "] {
            let selection = ProviderSelection {
                name: name.to_string(),
                api_key: Some("sk-test".to_string()),
                ..Default::default()
            };
            let provider = build_provider(&selection).expect("should build");
            assert_eq!(provider.name(), "chatgpt");
        }
    }

    #[test]
    fn cloud_providers_require_a_non_empty_key() {
        for (name, expected) in [
            ("chatgpt", "OpenAI API not configured"),
            ("gemini", "Gemini API not configured"),
        ] {
            for key in [None, Some(String::new()), Some("   ".to_string())] {
                let selection = ProviderSelection {
                    name: name.to_string(),
                    api_key: key,
                    ..Default::default()
                };
                assert_eq!(build_provider(&selection).err().as_deref(), Some(expected));
            }
        }
    }

    #[test]
    fn local_providers_need_no_key() {
        for name in ["ollama", "lmstudio"] {
            let provider = build_provider(&ProviderSelection::new(name)).expect("should build");
            assert_eq!(provider.name(), name);
        }
    }

    #[test]
    fn unknown_provider_reports_the_offending_name() {
        let err = build_provider(&ProviderSelection::new("qwen")).err();
        assert_eq!(err.as_deref(), Some("Unknown provider 'qwen'."));
        assert!(!is_known_provider("qwen"));
        assert!(is_known_provider("OPENAI"));
    }

    #[test]
    fn known_providers_all_build_or_fail_only_on_credentials() {
        for name in KNOWN_PROVIDERS {
            assert!(is_known_provider(name), "{name} should be known");
        }
    }
}
