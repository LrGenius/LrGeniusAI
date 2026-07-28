//! ort-backed SigLIP2 (ViT-SO400M-16-SigLIP2-384) image + text towers.
//! Port of the model-management half of `server_lifecycle.py`: lazy load,
//! 30-minute idle unload, `/health`-style status.
//!
//! Device selection mirrors `config.TORCH_DEVICE`: CoreML on macOS,
//! CPU elsewhere (Windows is deliberately CPU-only in the Python backend
//! to avoid VRAM contention with local CUDA LLMs; Linux/Docker CUDA is a
//! later EP addition here).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use ort::session::Session;
use ort::value::Tensor;

use crate::image_pre;
use crate::text_pre::{SiglipTokenizer, TokenizeError, CONTEXT_LENGTH};

pub const EMBED_DIM: usize = 1152;
const IDLE_UNLOAD_SECONDS: i64 = 30 * 60;

#[derive(Debug, thiserror::Error)]
pub enum SiglipError {
    #[error("ort error: {0}")]
    Ort(#[from] ort::Error),
    #[error(transparent)]
    Tokenize(#[from] TokenizeError),
    #[error("model not loaded")]
    NotLoaded,
}

pub struct ModelPaths {
    pub image_onnx: PathBuf,
    pub text_onnx: PathBuf,
    pub tokenizer_json: PathBuf,
}

impl ModelPaths {
    pub fn all_exist(&self) -> bool {
        self.image_onnx.is_file() && self.text_onnx.is_file() && self.tokenizer_json.is_file()
    }
}

struct Loaded {
    image_session: Session,
    text_session: Session,
    tokenizer: SiglipTokenizer,
}

/// Lazily-loaded, idle-unloadable SigLIP2 model handle. One instance is
/// shared behind an `Arc` in `AppState`.
pub struct SiglipModel {
    paths: ModelPaths,
    loaded: Mutex<Option<Loaded>>,
    last_used_unix: AtomicI64,
    load_error: Mutex<Option<String>>,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// EP selection is env-gated rather than automatic-on-macOS for now:
/// spike S2 found CoreML's MLProgram format needs a *static*-batch export
/// to hit its good numbers (123ms/img vs MPS's 141ms) and to avoid
/// crashing — the dynamic-batch graphs used here/in CI don't qualify yet.
/// M9's model export pipeline will ship per-purpose static exports and
/// this can default to CoreML on macOS at that point.
///
/// Measured on an M4 Pro with the current dynamic-batch exports, via
/// `scripts/bench_index.py --compare-env LRG_ML_EP=coreml`: CoreML claims
/// only 341 of 1474 graph nodes across 57 partitions, and end-to-end
/// per-photo latency is unchanged (ratios 0.97x-1.13x, scattered both
/// ways, n=8-11 per format/size group). So enabling this before M9 buys
/// nothing — the partition boundaries eat whatever the accelerated nodes
/// save. Re-measure once static exports land, not before.
fn build_session(path: &Path) -> ort::Result<Session> {
    let mut builder = Session::builder()?;
    #[cfg(target_os = "macos")]
    if std::env::var("LRG_ML_EP").as_deref() == Ok("coreml") {
        use ort::execution_providers::coreml::{ComputeUnits, ModelFormat};
        use ort::execution_providers::CoreMLExecutionProvider;
        // `with_static_input_shapes` is load-bearing, not a tuning knob: the
        // exports here still have a dynamic batch dim, and without this the
        // EP hands those subgraphs to CoreML anyway, which compiles them and
        // then aborts the whole process inside MPSGraph ("has unbounded
        // dimension which is not supported" / "failed assertion `original
        // module failed verification'"). With it, the EP declines the
        // dynamic subgraphs and they fall back to the CPU EP registered
        // below instead of taking the server down.
        builder = builder.with_execution_providers([CoreMLExecutionProvider::default()
            .with_model_format(ModelFormat::MLProgram)
            .with_compute_units(ComputeUnits::All)
            .with_static_input_shapes(true)
            .with_model_cache_dir(std::env::temp_dir().join("lrg-coreml-cache").display())
            .build()])?;
    }
    // The CPU EP's default arena only ever grows (never returns memory to
    // the OS) — confirmed via per-photo RSS logging showing this same
    // pattern on lrg-ml::faces's sessions. CoreML falls back to CPU for
    // unsupported ops, so this matters on macOS too, not just the
    // CPU-only platforms. See faces::build_session for the full writeup.
    builder = builder
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])?;
    builder.commit_from_file(path)
}

impl SiglipModel {
    pub fn new(paths: ModelPaths) -> Self {
        SiglipModel {
            paths,
            loaded: Mutex::new(None),
            last_used_unix: AtomicI64::new(0),
            load_error: Mutex::new(None),
        }
    }

    pub fn is_cached(&self) -> bool {
        self.paths.all_exist()
    }

    fn ensure_loaded(&self) -> Result<(), SiglipError> {
        let mut guard = self.loaded.lock().unwrap();
        if guard.is_some() {
            self.last_used_unix.store(now_unix(), Ordering::Relaxed);
            return Ok(());
        }
        let result = (|| -> ort::Result<Loaded> {
            let image_session = build_session(&self.paths.image_onnx)?;
            let text_session = build_session(&self.paths.text_onnx)?;
            Ok(Loaded {
                image_session,
                text_session,
                tokenizer: SiglipTokenizer::from_file(&self.paths.tokenizer_json)
                    .map_err(|e| ort::Error::new(e.to_string()))?,
            })
        })();
        match result {
            Ok(loaded) => {
                *guard = Some(loaded);
                *self.load_error.lock().unwrap() = None;
                self.last_used_unix.store(now_unix(), Ordering::Relaxed);
                Ok(())
            }
            Err(e) => {
                *self.load_error.lock().unwrap() = Some(e.to_string());
                Err(e.into())
            }
        }
    }

    pub fn unload(&self) {
        let mut guard = self.loaded.lock().unwrap();
        if guard.take().is_some() {
            log::info!("Unloading SigLIP2 model due to inactivity...");
        }
    }

    /// Call periodically (e.g. every 60s) from a background task.
    pub fn unload_if_idle(&self) {
        let last = self.last_used_unix.load(Ordering::Relaxed);
        if last != 0 && now_unix() - last >= IDLE_UNLOAD_SECONDS {
            self.unload();
        }
    }

    pub fn status(&self) -> (&'static str, Option<String>) {
        if self.loaded.lock().unwrap().is_some() {
            ("loaded", None)
        } else if let Some(err) = self.load_error.lock().unwrap().clone() {
            ("failed", Some(err))
        } else {
            ("not_loaded", None)
        }
    }

    /// Embed one RGB8 image; NOT L2-normalized (callers normalize per the
    /// Python search/index call sites, which normalize after this call).
    pub fn embed_image(
        &self,
        pixels: &[u8],
        width: usize,
        height: usize,
    ) -> Result<Vec<f32>, SiglipError> {
        self.ensure_loaded()?;
        let mut guard = self.loaded.lock().unwrap();
        let loaded = guard.as_mut().ok_or(SiglipError::NotLoaded)?;

        let data = image_pre::preprocess(pixels, width, height);
        let input =
            Tensor::from_array(([1, 3, image_pre::IMAGE_SIZE, image_pre::IMAGE_SIZE], data))?;
        let outputs = loaded
            .image_session
            .run(ort::inputs!["pixel_values" => input])?;
        let (_, embedding) = outputs[0].try_extract_tensor::<f32>()?;
        Ok(embedding.to_vec())
    }

    /// Embed a batch of texts; NOT L2-normalized (see `embed_image`).
    pub fn embed_text(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SiglipError> {
        self.ensure_loaded()?;
        let mut guard = self.loaded.lock().unwrap();
        let loaded = guard.as_mut().ok_or(SiglipError::NotLoaded)?;

        let ids = loaded.tokenizer.encode_batch(texts)?;
        let batch = texts.len();
        let input = Tensor::from_array(([batch, CONTEXT_LENGTH], ids))?;
        let outputs = loaded
            .text_session
            .run(ort::inputs!["input_ids" => input])?;
        let (_, embedding) = outputs[0].try_extract_tensor::<f32>()?;
        Ok(embedding.chunks(EMBED_DIM).map(|c| c.to_vec()).collect())
    }
}

/// L2-normalize in place, matching `F.normalize(x, p=2, dim=1)`.
pub fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}
