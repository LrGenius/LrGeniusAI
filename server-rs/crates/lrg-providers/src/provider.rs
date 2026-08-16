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
use crate::lmstudio::LmStudioProvider;
use crate::local::SharedLocalEngine;
use crate::local_provider::LocalProvider;
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
    /// The default issues the requests with bounded concurrency. It used to be
    /// a strictly sequential loop, which — together with a preferred batch size
    /// of 1 and the plugin's single worker — meant a cloud run of 10,000 photos
    /// was 10,000 round trips end to end, each waiting out the last one's
    /// latency with the machine idle.
    ///
    /// Concurrency is capped at [`MAX_CONCURRENT_REQUESTS`] and never exceeds
    /// the batch the caller asked for. In-process backends override this
    /// wholesale to share one pinned prompt prefix across parallel decode
    /// sequences, which is a different mechanism entirely.
    ///
    /// Responses come back in request order regardless of completion order —
    /// callers pair them positionally with their photos.
    async fn generate_metadata_batch(
        &self,
        requests: &[MetadataGenerationRequest],
    ) -> Vec<MetadataGenerationResponse> {
        use futures::stream::{self, StreamExt};

        let concurrency = requests.len().clamp(1, MAX_CONCURRENT_REQUESTS);
        // Collected before streaming: building the futures inside the stream's
        // closure makes it higher-ranked over the request lifetime, which the
        // boxed future `async_trait` produces cannot satisfy.
        let calls: Vec<_> = requests
            .iter()
            .map(|request| self.generate_metadata(request))
            .collect();
        stream::iter(calls).buffered(concurrency).collect().await
    }
}

/// Ceiling on in-flight requests for the default batch implementation.
///
/// Deliberately small. The providers behind it are rate limited per account
/// and per minute, and a tripped limit costs far more than the latency saved:
/// the run fails photos the user then has to find and redo. Four is enough to
/// hide most of the round-trip latency without looking like a burst.
pub const MAX_CONCURRENT_REQUESTS: usize = 4;

/// The four REST providers implement the trait by delegating to their own
/// inherent methods. Keeping the boilerplate here rather than in each provider
/// module leaves those files as pure REST clients, and makes it obvious at a
/// glance that no provider quietly diverges from the contract.
macro_rules! impl_llm_provider {
    ($ty:ty, $name:literal, batch = $batch:expr $(, $is_available:ident)?) => {
        #[async_trait]
        impl LlmProvider for $ty {
            fn name(&self) -> &'static str {
                $name
            }

            fn preferred_batch_size(&self) -> usize {
                $batch
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

// The two cloud providers overlap requests: their latency is network round
// trips, and four in flight hides most of it.
impl_llm_provider!(OpenAiProvider, "chatgpt", batch = MAX_CONCURRENT_REQUESTS);
impl_llm_provider!(GeminiProvider, "gemini", batch = MAX_CONCURRENT_REQUESTS);
// Ollama and LM Studio stay at one. They are REST clients, but the server on
// the other end is a single local model: it serialises the work anyway (Ollama
// needs OLLAMA_NUM_PARALLEL raised to do otherwise), so overlapping requests
// buys nothing and risks thrashing a machine that is also running Lightroom.
impl_llm_provider!(OllamaProvider, "ollama", batch = 1, is_available);
impl_llm_provider!(LmStudioProvider, "lmstudio", batch = 1, is_available);

/// Everything needed to pick and construct a provider. Credentials and base
/// URLs live here rather than on the individual requests because they are
/// connection-level, not per-photo.
#[derive(Clone, Default)]
pub struct ProviderSelection {
    pub name: String,
    pub api_key: Option<String>,
    pub ollama_base_url: Option<String>,
    pub lmstudio_base_url: Option<String>,
    /// The local engine, when one is loaded. Supplied by `lrg-api`, the only
    /// crate that knows `lrg-llama` and `lrg-mlx` exist; `None` here is why
    /// `"llamacpp"` and `"mlx"` report "no local model loaded" rather than
    /// being unknown providers.
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
pub const KNOWN_PROVIDERS: &[&str] =
    &["chatgpt", "gemini", "ollama", "lmstudio", "llamacpp", "mlx"];

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
            Some(engine) => Ok(Arc::new(LocalProvider::llamacpp(engine.clone()))),
            None => Err(
                "No local model is loaded. Download or select a GGUF model in the plugin \
                 settings first."
                    .to_string(),
            ),
        },
        // Both local backends share `LocalProvider`; which engine was built is
        // decided upstream in `lrg-api`, and only the log label differs here.
        "mlx" => match &selection.local_engine {
            Some(engine) => Ok(Arc::new(LocalProvider::mlx(engine.clone()))),
            None => Err(
                "No MLX model is loaded. Download or select an MLX model in the plugin \
                 settings first. MLX needs an Apple silicon Mac."
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
        "chatgpt" | "openai" | "gemini" | "ollama" | "lmstudio" | "llamacpp" | "mlx"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_named(name: &str) -> std::sync::Arc<dyn LlmProvider> {
        let selection = ProviderSelection {
            name: name.to_string(),
            api_key: Some("sk-test".to_string()),
            ..Default::default()
        };
        build_provider(&selection).expect("should build")
    }

    /// The cloud providers overlap requests; their cost is network latency,
    /// and waiting out each round trip in turn is what made a 10,000-photo run
    /// 10,000 sequential round trips.
    #[test]
    fn cloud_providers_overlap_requests() {
        for name in ["chatgpt", "gemini"] {
            assert_eq!(
                provider_named(name).preferred_batch_size(),
                MAX_CONCURRENT_REQUESTS,
                "{name} should overlap requests"
            );
        }
    }

    /// The local REST providers must keep issuing one request at a time. The
    /// server on the other end is a single local model that serialises the
    /// work anyway, so concurrency buys nothing and competes with Lightroom
    /// for the same machine.
    #[test]
    fn local_rest_providers_do_not_batch() {
        for name in ["ollama", "lmstudio"] {
            assert_eq!(
                provider_named(name).preferred_batch_size(),
                1,
                "{name} must not batch photos"
            );
        }
    }

    /// Bounded, and never wider than what was asked for: a batch of two must
    /// not put four requests in flight.
    #[test]
    fn concurrency_never_exceeds_the_batch_or_the_cap() {
        for (batch, expected) in [(0usize, 1usize), (1, 1), (2, 2), (4, 4), (50, 4)] {
            assert_eq!(
                batch.clamp(1, MAX_CONCURRENT_REQUESTS),
                expected,
                "batch of {batch}"
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
