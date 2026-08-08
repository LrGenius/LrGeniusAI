//! Lifecycle for the in-process LLM, and the adapter that presents it to
//! `lrg-providers` as a `LocalEngine`.
//!
//! This is the only module that knows `lrg-llama` exists. Everything is behind
//! the `llamacpp` cargo feature; with the feature off, [`LlmEngineSlot`]
//! compiles to a stub that always reports "not available", so the routes,
//! `AppState` and the provider factory need no `#[cfg]` of their own.
//!
//! Loading is lazy and keyed by configuration: the first request that needs a
//! model loads it, and a later request naming a *different* model (or context
//! size) swaps it. That mirrors how `SiglipModel`/`FaceModel` behave, including
//! the idle unload driven by `AppState::run_maintenance`.

use std::path::PathBuf;
#[cfg(feature = "llamacpp")]
use std::time::Duration;

use serde::Serialize;

/// How long the model may sit unused before it is dropped. Matches the ONNX
/// models' policy — a multi-GB GGUF resident forever is the single biggest
/// memory regression this feature could introduce.
#[cfg(feature = "llamacpp")]
const IDLE_UNLOAD_AFTER: Duration = Duration::from_secs(30 * 60);

/// Everything that, when changed, requires a reload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmEngineSettings {
    pub model_path: PathBuf,
    pub mmproj_path: Option<PathBuf>,
    pub n_ctx: u32,
    pub n_parallel: u32,
    pub n_gpu_layers: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmEngineStatus {
    /// `"loaded"`, `"not_loaded"`, or `"unsupported"` when the binary was built
    /// without the `llamacpp` feature.
    pub status: &'static str,
    pub model_path: Option<String>,
    pub mmproj_path: Option<String>,
    pub supports_vision: bool,
    pub n_ctx: u32,
    pub n_parallel: u32,
}

#[cfg(feature = "llamacpp")]
mod imp {
    use super::{LlmEngineSettings, LlmEngineStatus, IDLE_UNLOAD_AFTER};

    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::Instant;

    use async_trait::async_trait;
    use lrg_llama::{GenerationRequest, LlamaEngine, LlamaEngineConfig, RgbImage};
    use lrg_providers::local::{LocalEngine, LocalRequest, LocalResponse, SharedLocalEngine};

    /// Wraps a loaded engine so `lrg-providers` can drive it without knowing
    /// anything about llama.cpp.
    struct LlamaLocalEngine {
        engine: Arc<LlamaEngine>,
    }

    #[async_trait]
    impl LocalEngine for LlamaLocalEngine {
        async fn generate(
            &self,
            requests: Vec<LocalRequest>,
        ) -> Vec<Result<LocalResponse, String>> {
            let count = requests.len();
            let mut converted = Vec::with_capacity(count);
            for request in requests {
                let image = match request.image {
                    Some(image) => match RgbImage::new(image.width, image.height, image.rgb) {
                        Ok(image) => Some(image),
                        Err(e) => return vec_of_errors(count, e.to_string()),
                    },
                    None => None,
                };
                converted.push(GenerationRequest {
                    system_prompt: request.system_prompt,
                    stable_prompt: request.stable_prompt,
                    per_photo_prompt: request.per_photo_prompt,
                    image,
                    schema: request.schema,
                    max_tokens: request.max_tokens,
                    temperature: request.temperature,
                });
            }

            match self.engine.generate(converted).await {
                Ok(outputs) => outputs
                    .into_iter()
                    .map(|result| {
                        result
                            .map(|output| LocalResponse {
                                text: output.text,
                                prompt_tokens: output.prompt_tokens,
                                completion_tokens: output.completion_tokens,
                                prefix_reused: output.prefix_reused,
                            })
                            .map_err(|e| e.to_string())
                    })
                    .collect(),
                // The engine only fails as a whole when its worker is gone.
                Err(e) => vec_of_errors(count, e.to_string()),
            }
        }

        fn supports_vision(&self) -> bool {
            self.engine.info().supports_vision
        }

        fn model_name(&self) -> String {
            std::path::Path::new(&self.engine.info().model_path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.engine.info().model_path.clone())
        }

        fn preferred_batch_size(&self) -> usize {
            // `LlamaEngine::new` may have reduced this below the configured
            // value to fit the context window, so read it back off the engine
            // rather than off the settings.
            self.engine.info().n_parallel.max(1) as usize
        }
    }

    fn vec_of_errors(count: usize, message: String) -> Vec<Result<LocalResponse, String>> {
        (0..count).map(|_| Err(message.clone())).collect()
    }

    struct Loaded {
        settings: LlmEngineSettings,
        engine: Arc<LlamaEngine>,
        last_used: Instant,
    }

    #[derive(Default)]
    pub struct LlmEngineSlot {
        loaded: StdMutex<Option<Loaded>>,
        /// Serializes loads so two concurrent requests cannot each spend a
        /// minute loading multi-GB weights.
        load_lock: tokio::sync::Mutex<()>,
    }

    impl LlmEngineSlot {
        /// Return an engine matching `settings`, loading or swapping as needed.
        ///
        /// # Errors
        /// Propagates the load failure message, which the plugin shows verbatim.
        pub async fn get_or_load(
            &self,
            settings: &LlmEngineSettings,
        ) -> Result<SharedLocalEngine, String> {
            if let Some(engine) = self.take_matching(settings) {
                return Ok(engine);
            }

            let _guard = self.load_lock.lock().await;
            // Another request may have loaded exactly this while we waited.
            if let Some(engine) = self.take_matching(settings) {
                return Ok(engine);
            }

            let config = LlamaEngineConfig {
                model_path: settings.model_path.clone(),
                mmproj_path: settings.mmproj_path.clone(),
                n_ctx: settings.n_ctx,
                n_parallel: settings.n_parallel,
                n_gpu_layers: settings.n_gpu_layers,
                n_threads: None,
            };
            log::info!("Loading local LLM: {}", settings.model_path.display());

            // Loading blocks for as long as it takes to read the weights, so it
            // must not run on a tokio worker.
            let engine = tokio::task::spawn_blocking(move || LlamaEngine::new(config))
                .await
                .map_err(|e| format!("model loading task failed: {e}"))?
                .map_err(|e| e.to_string())?;

            let engine = Arc::new(engine);
            // Replacing the slot drops the previous engine, which joins its
            // worker thread and frees the old weights before we return.
            *self.loaded.lock().unwrap() = Some(Loaded {
                settings: settings.clone(),
                engine: engine.clone(),
                last_used: Instant::now(),
            });
            Ok(Arc::new(LlamaLocalEngine { engine }))
        }

        fn take_matching(&self, settings: &LlmEngineSettings) -> Option<SharedLocalEngine> {
            let mut guard = self.loaded.lock().unwrap();
            let loaded = guard.as_mut()?;
            if loaded.settings != *settings {
                return None;
            }
            loaded.last_used = Instant::now();
            Some(Arc::new(LlamaLocalEngine {
                engine: loaded.engine.clone(),
            }))
        }

        /// The already-loaded engine, if any, without loading one. Used by
        /// `/models`, which must not trigger a multi-GB load just to answer a
        /// listing request.
        #[must_use]
        pub fn current(&self) -> Option<SharedLocalEngine> {
            let mut guard = self.loaded.lock().unwrap();
            let loaded = guard.as_mut()?;
            loaded.last_used = Instant::now();
            Some(Arc::new(LlamaLocalEngine {
                engine: loaded.engine.clone(),
            }))
        }

        pub fn unload(&self) {
            if self.loaded.lock().unwrap().take().is_some() {
                log::info!("Unloaded local LLM");
            }
        }

        pub fn unload_if_idle(&self) {
            let idle = {
                let guard = self.loaded.lock().unwrap();
                guard
                    .as_ref()
                    .is_some_and(|l| l.last_used.elapsed() >= IDLE_UNLOAD_AFTER)
            };
            if idle {
                log::info!("Unloading idle local LLM");
                self.loaded.lock().unwrap().take();
            }
        }

        #[must_use]
        pub fn status(&self) -> LlmEngineStatus {
            match self.loaded.lock().unwrap().as_ref() {
                Some(loaded) => {
                    let info = loaded.engine.info();
                    LlmEngineStatus {
                        status: "loaded",
                        model_path: Some(info.model_path.clone()),
                        mmproj_path: info.mmproj_path.clone(),
                        supports_vision: info.supports_vision,
                        n_ctx: info.n_ctx,
                        n_parallel: info.n_parallel,
                    }
                }
                None => LlmEngineStatus {
                    status: "not_loaded",
                    model_path: None,
                    mmproj_path: None,
                    supports_vision: false,
                    n_ctx: 0,
                    n_parallel: 0,
                },
            }
        }

        #[must_use]
        pub fn is_supported(&self) -> bool {
            true
        }
    }
}

#[cfg(not(feature = "llamacpp"))]
mod imp {
    use super::{LlmEngineSettings, LlmEngineStatus};
    use lrg_providers::local::SharedLocalEngine;

    /// Stub for builds without the `llamacpp` feature. Every entry point
    /// answers "unsupported" so callers need no `#[cfg]`.
    #[derive(Default)]
    pub struct LlmEngineSlot;

    impl LlmEngineSlot {
        pub async fn get_or_load(
            &self,
            _settings: &LlmEngineSettings,
        ) -> Result<SharedLocalEngine, String> {
            Err(UNSUPPORTED.to_string())
        }
        #[must_use]
        pub fn current(&self) -> Option<SharedLocalEngine> {
            None
        }
        pub fn unload(&self) {}
        pub fn unload_if_idle(&self) {}
        #[must_use]
        pub fn status(&self) -> LlmEngineStatus {
            LlmEngineStatus {
                status: "unsupported",
                model_path: None,
                mmproj_path: None,
                supports_vision: false,
                n_ctx: 0,
                n_parallel: 0,
            }
        }
        #[must_use]
        pub fn is_supported(&self) -> bool {
            false
        }
    }

    const UNSUPPORTED: &str =
        "This build of the LrGenius server was compiled without local model support.";
}

pub use imp::LlmEngineSlot;

/// Engine tuning a request may ask for, from the plugin's advanced fields.
///
/// All optional: `None` means "not specified", which falls back to the
/// environment and then to the defaults. Changing any of these makes
/// [`LlmEngineSlot::get_or_load`] reload the engine, so they are settings a
/// user adjusts occasionally, not per photo.
#[derive(Debug, Clone, Copy, Default)]
pub struct EngineOverrides {
    pub n_ctx: Option<u32>,
    pub n_parallel: Option<u32>,
    pub n_gpu_layers: Option<u32>,
}

/// Build engine settings for a request.
///
/// Precedence is request, then environment, then default. Defaults are
/// deliberately conservative — `n_parallel = 1` means no extra KV cache is
/// allocated, so enabling the local provider cannot surprise a small machine
/// with a multi-gigabyte memory jump.
#[must_use]
pub fn settings_for(
    model_path: PathBuf,
    mmproj_path: Option<PathBuf>,
    overrides: EngineOverrides,
) -> LlmEngineSettings {
    LlmEngineSettings {
        model_path,
        mmproj_path,
        n_ctx: overrides
            .n_ctx
            .unwrap_or_else(|| env_u32("LRG_LLAMA_N_CTX", 8192)),
        n_parallel: overrides
            .n_parallel
            .unwrap_or_else(|| env_u32("LRG_LLAMA_N_PARALLEL", 1))
            .max(1),
        // "All layers on the GPU" is right for Metal and for any card with
        // enough VRAM; llama.cpp silently keeps what does not fit on the CPU.
        n_gpu_layers: overrides
            .n_gpu_layers
            .unwrap_or_else(|| env_u32("LRG_LLAMA_GPU_LAYERS", u32::MAX)),
    }
}

fn env_u32(var: &str, default: u32) -> u32 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_to_a_single_sequence() {
        // Anything else would allocate extra KV cache the moment the provider
        // is selected, which is not a decision to make on the user's behalf.
        let settings = settings_for("/m/a.gguf".into(), None, EngineOverrides::default());
        assert_eq!(settings.n_parallel, 1);
        assert!(settings.n_ctx >= 4096);
    }

    #[test]
    fn settings_equality_drives_reloads() {
        let base = LlmEngineSettings {
            model_path: "/models/a.gguf".into(),
            mmproj_path: None,
            n_ctx: 8192,
            n_parallel: 2,
            n_gpu_layers: u32::MAX,
        };
        let mut other = base.clone();
        assert_eq!(base, other);
        // A different context size must count as a different engine, since it
        // cannot be changed on a live context.
        other.n_ctx = 4096;
        assert_ne!(base, other);
    }
}
