//! ort-backed SigLIP2 (`google/siglip2-so400m-patch16-naflex`) image + text
//! towers. The image tower takes NaFlex patches (see [`crate::naflex_pre`]),
//! so a photo keeps its aspect ratio instead of losing its edges to a square
//! center crop.
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

use crate::naflex_pre;
use crate::text_pre::{SiglipTokenizer, TokenizeError, CONTEXT_LENGTH};

pub const EMBED_DIM: usize = 1152;

/// Identifier for the embedding function this build writes.
///
/// A compile-time constant, like `lrg_ml::faces::MODEL_ID` and unlike the
/// BioCLIP head's `model_id`, which reads its version off the labels file: the
/// ONNX assets carry no version to read, and the release tag they are published
/// under is a different crate's constant.
///
/// **Bump this whenever a change would make new embeddings incomparable with
/// stored ones** — different weights, an export at a different optimization
/// level, a change to [`crate::naflex_pre::preprocess`] (its patch budget
/// included), or a different tokenizer. A stored vector only means something next to the function that
/// produced it, and `/v1/index/unprocessed` re-queues photos whose stored
/// `embedding_model` is not this value.
///
/// History: `siglip2-so400m-384/fp16` was the fixed-resolution
/// `ViT-SO400M-16-SigLIP2-384` with a 384x384 center crop
/// ([`LEGACY_MODEL_ID`]).
pub const MODEL_ID: &str = "siglip2-so400m-naflex-576/fp16";

/// The only SigLIP2 export that shipped before NaFlex, and so the identity of
/// every stored vector that carries no `embedding_model` at all: that field
/// was added while this model was the current one. The store upgrade stamps
/// it on those rows (`lrg_store::upgrade`); stamping [`MODEL_ID`] instead
/// would label 384 center-crop vectors as NaFlex and they would never be
/// re-embedded.
pub const LEGACY_MODEL_ID: &str = "siglip2-so400m-384/fp16";

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

/// The text tower's session, together with the tokenizer that feeds it.
///
/// The tokenizer lives here rather than beside the paths because nothing but
/// the text path reads it, and keeping it here is what lets the two towers be
/// loaded and dropped independently.
struct TextTower {
    session: Session,
    tokenizer: SiglipTokenizer,
}

/// One lazily-loaded, independently idle-unloaded tower.
///
/// The image and text towers each hold one of these. They used to share a
/// single `Option<Loaded>` that owned both sessions behind one mutex and one
/// idle clock, so anything that touched either tower built both: an
/// embeddings-only run paid for the text tower it never calls, and a semantic
/// search paid for the image tower. Nothing about either tower's cost changes
/// here — only which of them a call makes resident.
struct Tower<T> {
    loaded: Mutex<Option<T>>,
    /// This tower's own clock. A shared one meant image work refreshing the
    /// text tower's idle timer, so a tower nothing ever called could not be
    /// dropped while the other one was busy.
    last_used_unix: AtomicI64,
    load_error: Mutex<Option<String>>,
}

impl<T> Tower<T> {
    fn new() -> Self {
        Tower {
            loaded: Mutex::new(None),
            last_used_unix: AtomicI64::new(0),
            load_error: Mutex::new(None),
        }
    }

    /// Runs `f` against this tower's loaded value, building it first when it is
    /// not there yet.
    ///
    /// `name` reaches the failure log and is only for the human reading it: a
    /// load that fails on a background path has to say which tower it was.
    /// `build` fails with an ort error, `f` with a `SiglipError`, so the
    /// tokenizer's own error type reaches the caller unchanged.
    fn with<R>(
        &self,
        name: &str,
        build: impl FnOnce() -> ort::Result<T>,
        f: impl FnOnce(&mut T) -> Result<R, SiglipError>,
    ) -> Result<R, SiglipError> {
        let mut guard = self.loaded.lock().unwrap();
        if guard.is_none() {
            match build() {
                Ok(value) => {
                    *guard = Some(value);
                    *self.load_error.lock().unwrap() = None;
                }
                Err(e) => {
                    log::warn!("Could not load the SigLIP2 {name} tower: {e}");
                    *self.load_error.lock().unwrap() = Some(e.to_string());
                    return Err(e.into());
                }
            }
        }
        self.last_used_unix.store(now_unix(), Ordering::Relaxed);
        let loaded = guard.as_mut().ok_or(SiglipError::NotLoaded)?;
        f(loaded)
    }

    /// Drops this tower's session(s). Safe to call when nothing is loaded.
    ///
    /// The reason is a parameter because this is reached two ways — the
    /// `/unload` route and the idle sweep — and the log line used to say
    /// "due to inactivity" for both. A real `/unload` from the plugin then
    /// looked like an idle timeout in the log, which is the kind of small lie
    /// that costs an hour the next time someone reads it.
    fn unload_with_reason(&self, name: &str, reason: &str) {
        let mut guard = self.loaded.lock().unwrap();
        if guard.take().is_some() {
            log::info!("Unloading SigLIP2 {name} tower ({reason})");
        }
    }

    fn unload_if_idle(&self, name: &str) {
        let last = self.last_used_unix.load(Ordering::Relaxed);
        if last != 0 && now_unix() - last >= IDLE_UNLOAD_SECONDS {
            self.unload_with_reason(name, "idle");
        }
    }

    fn status(&self) -> (&'static str, Option<String>) {
        if self.loaded.lock().unwrap().is_some() {
            ("loaded", None)
        } else if let Some(err) = self.load_error.lock().unwrap().clone() {
            ("failed", Some(err))
        } else {
            ("not_loaded", None)
        }
    }
}

/// Lazily-loaded, idle-unloadable SigLIP2 model handle. One instance is
/// shared behind an `Arc` in `AppState`.
pub struct SiglipModel {
    paths: ModelPaths,
    image: Tower<Session>,
    text: Tower<TextTower>,
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
/// Optimization level pinned per [`crate::OPTIMIZATION_LEVEL`] — onnxruntime
/// segfaults loading `siglip2_text.onnx` at its own `ORT_ENABLE_ALL` default,
/// and triples the image tower's resident memory.
fn build_session(path: &Path) -> ort::Result<Session> {
    let mut builder = Session::builder()?.with_optimization_level(crate::OPTIMIZATION_LEVEL)?;
    #[cfg(target_os = "macos")]
    if std::env::var("LRG_ML_EP").as_deref() == Ok("coreml") {
        use ort::ep::coreml::{ComputeUnits, ModelFormat};
        use ort::ep::CoreML;
        builder = builder.with_execution_providers([CoreML::default()
            .with_model_format(ModelFormat::MLProgram)
            .with_compute_units(ComputeUnits::All)
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
    // KleidiAI is left enabled: it is worth ~4x on this tower and no longer
    // leaks on onnxruntime 1.28. See `crate::DISABLE_KLEIDIAI`.
    crate::apply_kleidiai_policy(builder)?.commit_from_file(path)
}

impl SiglipModel {
    pub fn new(paths: ModelPaths) -> Self {
        SiglipModel {
            paths,
            image: Tower::new(),
            text: Tower::new(),
        }
    }

    pub fn is_cached(&self) -> bool {
        self.paths.all_exist()
    }

    /// Identifier for the embedding function this build writes.
    ///
    /// Read by `/v1/index/unprocessed`, which runs *before* any indexing, so it
    /// is a constant rather than anything read off the loaded session — asking
    /// the model would mean loading it just to ask.
    pub fn model_id(&self) -> &'static str {
        MODEL_ID
    }

    /// Drops both towers. Safe to call when nothing is loaded.
    ///
    /// Both, unconditionally: `/server/unload` promises to free what the
    /// process holds, and leaving one tower resident because the caller
    /// happened to name the other would break that promise.
    pub fn unload(&self) {
        self.image.unload_with_reason("image", "requested");
        self.text.unload_with_reason("text", "requested");
    }

    /// Drops the text tower alone, leaving the image tower as it is.
    ///
    /// For a caller that loaded the text tower for one batch of fixed prompts
    /// and knows nothing else is about to need it: left to the idle sweep, it
    /// would stay resident for half an hour of an indexing run that never
    /// calls it again.
    pub fn unload_text(&self) {
        self.text.unload_with_reason("text", "no longer needed");
    }

    /// Call periodically (e.g. every 60s) from a background task.
    pub fn unload_if_idle(&self) {
        self.image.unload_if_idle("image");
        self.text.unload_if_idle("text");
    }

    /// The pair's status, which is what `/health`'s `clip_model` field has
    /// always described.
    ///
    /// A failure on either tower reports `"failed"` even when the other one is
    /// resident. The plug-in reads this field as "the AI model failed to load",
    /// and since the towers now load separately, a broken tower must not be
    /// able to hide behind a working one. [`Self::image_status`] and
    /// [`Self::text_status`] say which tower it was.
    pub fn status(&self) -> (&'static str, Option<String>) {
        let (image, image_error) = self.image.status();
        let (text, text_error) = self.text.status();
        if image == "failed" || text == "failed" {
            ("failed", image_error.or(text_error))
        } else if image == "loaded" || text == "loaded" {
            ("loaded", None)
        } else {
            ("not_loaded", None)
        }
    }

    /// The image tower alone — what indexing and the training path embed with.
    pub fn image_status(&self) -> (&'static str, Option<String>) {
        self.image.status()
    }

    /// The text tower alone — what search, keyword lookup and the prompt sets
    /// embed with.
    pub fn text_status(&self) -> (&'static str, Option<String>) {
        self.text.status()
    }

    /// Embed one RGB8 image; NOT L2-normalized (callers normalize per the
    /// Python search/index call sites, which normalize after this call).
    ///
    /// Loads the image tower and nothing else: before the towers were split,
    /// this call also brought the text tower resident.
    pub fn embed_image(
        &self,
        pixels: &[u8],
        width: usize,
        height: usize,
    ) -> Result<Vec<f32>, SiglipError> {
        self.image.with(
            "image",
            || build_session(&self.paths.image_onnx),
            |session| {
                let pre =
                    naflex_pre::preprocess(pixels, width, height, naflex_pre::MAX_NUM_PATCHES);
                // `pos_interp_h`/`pos_interp_w` carry the position-embedding
                // resize; see `NaflexInput::position_interpolation` for why
                // the graph does not compute it itself.
                let (rows, cols) = pre.position_interpolation();
                let n = pre.max_num_patches;
                let pixel_values =
                    Tensor::from_array(([1, n, naflex_pre::PATCH_DIM], pre.pixel_values))?;
                let mask = Tensor::from_array(([1, n], pre.attention_mask))?;
                let interp_h = Tensor::from_array(([pre.grid_h, naflex_pre::POSITION_GRID], rows))?;
                let interp_w = Tensor::from_array(([pre.grid_w, naflex_pre::POSITION_GRID], cols))?;
                let outputs = session.run(ort::inputs![
                    "pixel_values" => pixel_values,
                    "pixel_attention_mask" => mask,
                    "pos_interp_h" => interp_h,
                    "pos_interp_w" => interp_w,
                ])?;
                let (_, embedding) = outputs[0].try_extract_tensor::<f32>()?;
                Ok(embedding.to_vec())
            },
        )
    }

    /// Embed a batch of texts; NOT L2-normalized (see `embed_image`).
    ///
    /// Loads the text tower and nothing else, tokenizer included.
    pub fn embed_text(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SiglipError> {
        self.text.with(
            "text",
            || {
                let session = build_session(&self.paths.text_onnx)?;
                let tokenizer = SiglipTokenizer::from_file(&self.paths.tokenizer_json)
                    .map_err(|e| ort::Error::new(e.to_string()))?;
                Ok(TextTower { session, tokenizer })
            },
            |tower| {
                let ids = tower.tokenizer.encode_batch(texts)?;
                let batch = texts.len();
                let input = Tensor::from_array(([batch, CONTEXT_LENGTH], ids))?;
                let outputs = tower.session.run(ort::inputs!["input_ids" => input])?;
                let (_, embedding) = outputs[0].try_extract_tensor::<f32>()?;
                Ok(embedding.chunks(EMBED_DIM).map(|c| c.to_vec()).collect())
            },
        )
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
