//! The public handle. Everything here is llama.cpp-free: it owns a channel to
//! the worker thread (see [`crate::worker`]) and nothing else, so it is `Send`,
//! `Sync`, cheap to clone behind an `Arc`, and safe to hold in `AppState`.

use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::thread::JoinHandle;

use tokio::sync::oneshot;

use crate::worker::{self, Job};
use crate::{LlamaError, RgbImage};

#[derive(Debug, Clone)]
pub struct LlamaEngineConfig {
    /// Path to the text model GGUF.
    pub model_path: PathBuf,
    /// Path to the multimodal projector GGUF. Without it the engine is
    /// text-only and any request carrying an image is rejected.
    pub mmproj_path: Option<PathBuf>,
    /// Context window. Must accommodate `n_parallel * (prefix + per-photo +
    /// generation)`; [`LlamaEngine::new`] logs and reduces `n_parallel` rather
    /// than letting a decode run off the end.
    pub n_ctx: u32,
    /// Sequences decoded concurrently. Sharing one pinned prefix across
    /// several sequences is what makes batching worth more than the sum of its
    /// parts.
    pub n_parallel: u32,
    /// Layers offloaded to the GPU. `u32::MAX` means "all", which is what you
    /// want on Metal and on a CUDA/Vulkan card with enough VRAM.
    pub n_gpu_layers: u32,
    pub n_threads: Option<i32>,
}

impl Default for LlamaEngineConfig {
    fn default() -> Self {
        LlamaEngineConfig {
            model_path: PathBuf::new(),
            mmproj_path: None,
            n_ctx: 8192,
            n_parallel: 1,
            n_gpu_layers: u32::MAX,
            n_threads: None,
        }
    }
}

/// What the engine settled on after inspecting the model, for `/v1/llm/status`
/// and for the logs.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EngineInfo {
    pub model_path: String,
    pub mmproj_path: Option<String>,
    pub n_ctx: u32,
    pub n_parallel: u32,
    pub supports_vision: bool,
}

/// One photo's worth of work.
#[derive(Debug, Clone, Default)]
pub struct GenerationRequest {
    pub system_prompt: String,
    /// Run-constant. Identical values across requests let the engine skip
    /// re-evaluating this entirely.
    pub stable_prompt: String,
    /// Varies per photo; always re-evaluated.
    pub per_photo_prompt: String,
    pub image: Option<RgbImage>,
    /// JSON Schema to constrain decoding with. `None` decodes freely.
    pub schema: Option<serde_json::Value>,
    pub max_tokens: u32,
    pub temperature: f32,
}

#[derive(Debug, Clone, Default)]
pub struct GenerationOutput {
    pub text: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// True when the pinned prefix was reused instead of re-evaluated. Surfaced
    /// so the speedup is visible in `--debug` logs rather than merely believed.
    pub prefix_reused: bool,
}

pub struct LlamaEngine {
    tx: SyncSender<Job>,
    handle: Option<JoinHandle<()>>,
    info: EngineInfo,
}

impl LlamaEngine {
    /// Load a model and start its worker thread.
    ///
    /// Blocks until the model is resident, so call it from `spawn_blocking`.
    ///
    /// # Errors
    /// Propagates model/projector load failures.
    pub fn new(config: LlamaEngineConfig) -> Result<Self, LlamaError> {
        if !config.model_path.is_file() {
            return Err(LlamaError::ModelNotFound(
                config.model_path.display().to_string(),
            ));
        }
        if let Some(mmproj) = &config.mmproj_path {
            if !mmproj.is_file() {
                return Err(LlamaError::ModelNotFound(mmproj.display().to_string()));
            }
        }

        // Depth 0: a sender blocks until the worker picks the job up, so a
        // caller can never queue work behind an engine that is shutting down.
        let (tx, rx) = sync_channel::<Job>(0);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();

        let worker_config = config.clone();
        let handle = std::thread::Builder::new()
            .name("lrg-llama".to_string())
            .spawn(move || worker::run(worker_config, rx, ready_tx))
            .map_err(|e| LlamaError::Load(format!("could not start worker thread: {e}")))?;

        // The worker reports the outcome of loading before serving anything.
        let info = match ready_rx.recv() {
            Ok(Ok(info)) => info,
            Ok(Err(e)) => {
                let _ = handle.join();
                return Err(e);
            }
            Err(_) => {
                let _ = handle.join();
                return Err(LlamaError::Load(
                    "worker thread exited before reporting readiness".to_string(),
                ));
            }
        };

        log::info!(
            "llama engine ready: model={} mmproj={} n_ctx={} n_parallel={} vision={}",
            info.model_path,
            info.mmproj_path.as_deref().unwrap_or("none"),
            info.n_ctx,
            info.n_parallel,
            info.supports_vision,
        );

        Ok(LlamaEngine {
            tx,
            handle: Some(handle),
            info,
        })
    }

    #[must_use]
    pub fn info(&self) -> &EngineInfo {
        &self.info
    }

    /// Generate for a batch of photos that share a stable prefix.
    ///
    /// The batch is the unit of prefix reuse *and* of parallel decoding, so
    /// prefer one call with N requests over N calls with one. Each entry gets
    /// its own `Result`: one photo failing to tokenize must not sink the rest
    /// of the batch.
    ///
    /// # Errors
    /// Returns `Err` only if the engine itself is gone; per-photo failures are
    /// reported inside the returned `Vec`.
    pub async fn generate(
        &self,
        requests: Vec<GenerationRequest>,
    ) -> Result<Vec<Result<GenerationOutput, LlamaError>>, LlamaError> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(Job::Generate {
                requests,
                reply: reply_tx,
            })
            .map_err(|_| LlamaError::Shutdown)?;
        reply_rx.await.map_err(|_| LlamaError::Shutdown)
    }
}

impl Drop for LlamaEngine {
    fn drop(&mut self) {
        // Closing the channel is the shutdown signal; joining guarantees the
        // model and context are destroyed before we return, which matters
        // because the caller may be about to load a different model.
        let (tx, _) = sync_channel(0);
        let tx = std::mem::replace(&mut self.tx, tx);
        drop(tx);
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                log::error!("llama worker thread panicked during shutdown");
            }
        }
    }
}
