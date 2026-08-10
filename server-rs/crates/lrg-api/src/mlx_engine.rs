//! Lifecycle for the MLX engine, and the adapter that presents it to
//! `lrg-providers` as a `LocalEngine`.
//!
//! The MLX counterpart to [`crate::llm_engine`], with the same lazy,
//! settings-keyed loading and the same idle unload. Two things differ:
//!
//! * **No cargo feature.** `llamacpp` is gated because it compiles llama.cpp
//!   from source and needs cmake + libclang; `lrg-mlx` only spawns a process,
//!   so it costs nothing to compile everywhere. Availability is therefore a
//!   *runtime* question — Apple silicon, and is the sidecar installed? — which
//!   [`MlxEngineSlot::availability`] answers.
//! * **No tuning knobs.** There is no `n_ctx`/`n_gpu_layers` equivalent to
//!   expose: MLX sizes its cache from the prompt and uses unified memory, so
//!   the model directory is the whole of the configuration.

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Serialize;

use lrg_mlx::{GenerationRequest, MlxEngine, MlxEngineConfig, RgbImage};
use lrg_providers::local::{LocalEngine, LocalRequest, LocalResponse, SharedLocalEngine};

/// How long the model may sit unused before it is dropped. Matches the ONNX
/// models' and llama.cpp's policy — and here it also reaps the sidecar process,
/// which is the thing actually holding the weights.
const IDLE_UNLOAD_AFTER: Duration = Duration::from_secs(30 * 60);

/// Everything that, when changed, requires a reload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlxEngineSettings {
    pub model_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct MlxEngineStatus {
    /// `"loaded"`, `"not_loaded"`, or `"unsupported"` when this host cannot run
    /// MLX at all.
    pub status: &'static str,
    pub model_path: Option<String>,
    pub model_name: Option<String>,
    pub supports_vision: bool,
}

/// Why MLX is or is not usable here. Returned to the plugin so the settings
/// panel can explain itself rather than showing an inert control.
#[derive(Debug, Clone, Serialize)]
pub struct MlxAvailability {
    pub supported: bool,
    /// User-facing explanation when `supported` is false.
    pub reason: Option<String>,
}

/// Wraps a loaded engine so `lrg-providers` can drive it without knowing
/// anything about MLX.
struct MlxLocalEngine {
    engine: Arc<MlxEngine>,
}

#[async_trait]
impl LocalEngine for MlxLocalEngine {
    async fn generate(&self, requests: Vec<LocalRequest>) -> Vec<Result<LocalResponse, String>> {
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
            // The engine only fails as a whole when the sidecar is gone.
            Err(e) => vec_of_errors(count, e.to_string()),
        }
    }

    fn supports_vision(&self) -> bool {
        self.engine.info().supports_vision
    }

    fn model_name(&self) -> String {
        self.engine.info().model_name.clone()
    }

    fn preferred_batch_size(&self) -> usize {
        // One. Unlike llama.cpp's `n_parallel` there are no concurrent decode
        // sequences to fill and no pinned prefix to amortise, so a larger batch
        // would only delay the first result without producing any of them
        // sooner. See `lrg_mlx`'s module docs.
        1
    }
}

fn vec_of_errors(count: usize, message: String) -> Vec<Result<LocalResponse, String>> {
    (0..count).map(|_| Err(message.clone())).collect()
}

struct Loaded {
    settings: MlxEngineSettings,
    engine: Arc<MlxEngine>,
    last_used: Instant,
}

#[derive(Default)]
pub struct MlxEngineSlot {
    loaded: StdMutex<Option<Loaded>>,
    /// Serializes loads so two concurrent requests cannot each spend a minute
    /// reading multi-GB weights.
    load_lock: tokio::sync::Mutex<()>,
}

impl MlxEngineSlot {
    /// Return an engine matching `settings`, loading or swapping as needed.
    ///
    /// # Errors
    /// Propagates the load failure message, which the plugin shows verbatim.
    pub async fn get_or_load(
        &self,
        settings: &MlxEngineSettings,
    ) -> Result<SharedLocalEngine, String> {
        if let Some(engine) = self.take_matching(settings) {
            return Ok(engine);
        }

        let _guard = self.load_lock.lock().await;
        // Another request may have loaded exactly this while we waited.
        if let Some(engine) = self.take_matching(settings) {
            return Ok(engine);
        }

        log::info!("Loading MLX model: {}", settings.model_dir.display());
        // Dropping any previous engine *before* loading the next one keeps peak
        // memory at one model rather than two, which on a 16 GB machine is the
        // difference between a swap and a swap storm.
        self.unload();

        let engine = MlxEngine::new(MlxEngineConfig {
            model_dir: settings.model_dir.clone(),
            sidecar_path: std::env::var_os("LRG_MLX_SIDECAR").map(PathBuf::from),
        })
        .await
        .map_err(|e| e.to_string())?;

        let engine = Arc::new(engine);
        *self.loaded.lock().unwrap() = Some(Loaded {
            settings: settings.clone(),
            engine: engine.clone(),
            last_used: Instant::now(),
        });
        Ok(Arc::new(MlxLocalEngine { engine }))
    }

    fn take_matching(&self, settings: &MlxEngineSettings) -> Option<SharedLocalEngine> {
        let mut guard = self.loaded.lock().unwrap();
        let loaded = guard.as_mut()?;
        if loaded.settings != *settings {
            return None;
        }
        loaded.last_used = Instant::now();
        Some(Arc::new(MlxLocalEngine {
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
        Some(Arc::new(MlxLocalEngine {
            engine: loaded.engine.clone(),
        }))
    }

    pub fn unload(&self) {
        if self.loaded.lock().unwrap().take().is_some() {
            log::info!("Unloaded MLX model");
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
            log::info!("Unloading idle MLX model");
            self.loaded.lock().unwrap().take();
        }
    }

    #[must_use]
    pub fn status(&self) -> MlxEngineStatus {
        match self.loaded.lock().unwrap().as_ref() {
            Some(loaded) => {
                let info = loaded.engine.info();
                MlxEngineStatus {
                    status: "loaded",
                    model_path: Some(info.model_path.clone()),
                    model_name: Some(info.model_name.clone()),
                    supports_vision: info.supports_vision,
                }
            }
            None => MlxEngineStatus {
                status: if availability().supported {
                    "not_loaded"
                } else {
                    "unsupported"
                },
                model_path: None,
                model_name: None,
                supports_vision: false,
            },
        }
    }

    #[must_use]
    pub fn availability(&self) -> MlxAvailability {
        availability()
    }

    #[must_use]
    pub fn is_supported(&self) -> bool {
        availability().supported
    }
}

/// Whether this host can run MLX, and why not if it cannot.
///
/// Both checks are runtime rather than compile-time: the same binary is not
/// shipped to Intel Macs, but a developer may well run one, and the sidecar can
/// be missing from an otherwise fine install if only the Rust half was rebuilt.
fn availability() -> MlxAvailability {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return MlxAvailability {
            supported: false,
            reason: Some(
                "MLX runs only on Apple silicon Macs. Use the llama.cpp local model instead."
                    .to_string(),
            ),
        };
    }
    match lrg_mlx::resolve_sidecar() {
        Ok(_) => MlxAvailability {
            supported: true,
            reason: None,
        },
        Err(e) => MlxAvailability {
            supported: false,
            reason: Some(e.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_equality_drives_reloads() {
        let base = MlxEngineSettings {
            model_dir: "/models/mlx/gemma-4-e4b-it-4bit".into(),
        };
        assert_eq!(base, base.clone());
        assert_ne!(
            base,
            MlxEngineSettings {
                model_dir: "/models/mlx/gemma-4-e2b-it-4bit".into(),
            }
        );
    }

    /// An unsupported host must always explain itself: the plugin shows this
    /// string directly, and "unsupported" with no reason is the least useful
    /// thing the settings panel could say.
    #[test]
    fn unavailability_always_carries_a_reason() {
        let availability = availability();
        assert_eq!(availability.supported, availability.reason.is_none());
    }

    #[test]
    fn an_empty_slot_reports_no_model() {
        let slot = MlxEngineSlot::default();
        let status = slot.status();
        assert!(status.model_path.is_none());
        assert!(matches!(status.status, "not_loaded" | "unsupported"));
    }
}
