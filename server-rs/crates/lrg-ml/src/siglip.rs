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

use std::path::PathBuf;

use ort::session::Session;
use ort::value::Tensor;

use crate::naflex_pre;
use crate::text_pre::{SiglipTokenizer, TokenizeError, CONTEXT_LENGTH};
use crate::tower::{build_session, NotLoaded, Tower};

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

#[derive(Debug, thiserror::Error)]
pub enum SiglipError {
    #[error("ort error: {0}")]
    Ort(#[from] ort::Error),
    #[error(transparent)]
    Tokenize(#[from] TokenizeError),
    #[error("model not loaded")]
    NotLoaded,
}

impl From<NotLoaded> for SiglipError {
    fn from(_: NotLoaded) -> Self {
        SiglipError::NotLoaded
    }
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

/// Lazily-loaded, idle-unloadable SigLIP2 model handle. One instance is
/// shared behind an `Arc` in `AppState`.
pub struct SiglipModel {
    paths: ModelPaths,
    image: Tower<Session>,
    text: Tower<TextTower>,
}

impl SiglipModel {
    pub fn new(paths: ModelPaths) -> Self {
        SiglipModel {
            paths,
            image: Tower::new("SigLIP2"),
            text: Tower::new("SigLIP2"),
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
