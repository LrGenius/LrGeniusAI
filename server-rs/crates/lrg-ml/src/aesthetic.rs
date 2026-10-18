//! ort-backed AestheticSigLIP (`somepago/AestheticSigLIP`, Apache-2.0): a
//! SigLIP 2 So400m NaFlex encoder fine-tuned to score a photo from 1 to 9.
//!
//! It is its own backbone, not a head on the search model's stored embedding:
//! the score comes from the encoder's pooled output plus the masked mean of two
//! intermediate layers, through an MLP over score buckets, and the score is the
//! expectation over the bucket centers. So scoring a photo is a second So400m
//! forward pass, at the 256 patches it was trained with (the search model uses
//! 576) — which is why the preprocessing here is its own, not
//! [`crate::siglip`]'s: another patch budget, and the authors' own rule for
//! choosing the grid ([`patch_grid`]).
//!
//! Exported by `scripts/export_aesthetic_siglip_fp16.py`, which pins the
//! upstream revision, the checkpoint hash and the code it imports.

use std::path::PathBuf;

use ort::session::Session;
use ort::value::Tensor;

use crate::naflex_pre::{self, NaflexInput};
use crate::tower::{build_session, NotLoaded, Tower};

/// Patches per image the model was trained with. Not a tuning knob: the
/// exported graph has this as its fixed input length.
pub const MAX_NUM_PATCHES: usize = 256;

/// Identifier for the scoring function this build writes, stored with every
/// score as `aesthetic_model` so a different export is recognised as stale.
///
/// **Bump this whenever a change would make new scores incomparable with
/// stored ones**: different weights, a different export precision, or a change
/// to [`patch_grid`] or the preprocessing it feeds.
pub const MODEL_ID: &str = "aesthetic-siglip2-naflex-256/fp16";

/// Lowest and highest score the model can produce: its first and last bucket
/// centers. The README says 1-10; the shipped checkpoint's buckets stop at 9.
pub const SCORE_MIN: f32 = 1.0;
pub const SCORE_MAX: f32 = 9.0;

#[derive(Debug, thiserror::Error)]
pub enum AestheticError {
    #[error("ort error: {0}")]
    Ort(#[from] ort::Error),
    #[error("model not loaded")]
    NotLoaded,
    #[error("the model returned no score")]
    NoScore,
}

impl From<NotLoaded> for AestheticError {
    fn from(_: NotLoaded) -> Self {
        AestheticError::NotLoaded
    }
}

pub struct AestheticPaths {
    pub onnx: PathBuf,
}

impl AestheticPaths {
    pub fn all_exist(&self) -> bool {
        self.onnx.is_file()
    }
}

/// The authors' patch-grid rule (`naflex.compute_patch_grid`), ported with the
/// same float arithmetic and tie-breaking: the grid that best keeps the aspect
/// ratio within `max_patches`, preferring more patches when equally good.
///
/// Not transformers' `get_image_size_for_max_num_patches`: the two pick
/// different grids for many shapes, and the model was trained on this one.
/// `testdata/aesthetic_pre_goldens.json` pins it against the Python original.
pub fn patch_grid(img_h: usize, img_w: usize, max_patches: usize) -> (usize, usize) {
    let aspect = img_h as f64 / img_w as f64;
    let (mut best_h, mut best_w, mut best_waste) = (1usize, 1usize, f64::INFINITY);
    let max_side = ((max_patches as f64 * aspect.max(1.0 / aspect)).sqrt() as usize) + 1;

    for h in 1..=max_side {
        let mut w = ((max_patches as f64 / h as f64) as usize).min(max_side);
        if w < 1 {
            continue;
        }
        if h * w > max_patches {
            w = max_patches / h;
        }
        if w < 1 {
            continue;
        }
        let cur_aspect = h as f64 / w as f64;
        let waste = (cur_aspect / aspect).ln().abs();
        // Prefer grids that use more patches (higher resolution).
        let score = waste - 0.001 * (h * w) as f64;
        if score < best_waste {
            best_waste = score;
            best_h = h;
            best_w = w;
        }
    }
    (best_h, best_w)
}

/// One RGB8 image (`width * height * 3` bytes, interleaved) as the aesthetic
/// tower takes it.
pub fn preprocess(pixels: &[u8], width: usize, height: usize) -> NaflexInput {
    let (grid_h, grid_w) = patch_grid(height, width, MAX_NUM_PATCHES);
    naflex_pre::preprocess_with_grid(pixels, width, height, grid_h, grid_w, MAX_NUM_PATCHES)
}

/// Lazily-loaded, idle-unloadable aesthetic scorer. One instance is shared
/// behind an `Arc` in `AppState`, like [`crate::siglip::SiglipModel`].
pub struct AestheticModel {
    paths: AestheticPaths,
    tower: Tower<Session>,
}

impl AestheticModel {
    pub fn new(paths: AestheticPaths) -> Self {
        AestheticModel {
            paths,
            tower: Tower::new("aesthetic"),
        }
    }

    pub fn is_cached(&self) -> bool {
        self.paths.all_exist()
    }

    /// Identifier for the scoring function this build writes. A constant, so
    /// `/v1/index/unprocessed` can compare against it without loading the model.
    pub fn model_id(&self) -> &'static str {
        MODEL_ID
    }

    /// Drops the session. Safe to call when nothing is loaded.
    pub fn unload(&self) {
        self.tower.unload_with_reason("scoring", "requested");
    }

    /// Call periodically (e.g. every 60s) from a background task.
    pub fn unload_if_idle(&self) {
        self.tower.unload_if_idle("scoring");
    }

    /// `("loaded" | "not_loaded" | "failed", error)`, as `/health` reports it.
    pub fn status(&self) -> (&'static str, Option<String>) {
        self.tower.status()
    }

    /// Scores one RGB8 image, `SCORE_MIN..=SCORE_MAX`, higher is better.
    pub fn score_image(
        &self,
        pixels: &[u8],
        width: usize,
        height: usize,
    ) -> Result<f32, AestheticError> {
        self.tower.with(
            "scoring",
            || build_session(&self.paths.onnx),
            |session| {
                let pre = preprocess(pixels, width, height);
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
                let (_, score) = outputs[0].try_extract_tensor::<f32>()?;
                score
                    .first()
                    .copied()
                    .map(|s| s.clamp(SCORE_MIN, SCORE_MAX))
                    .ok_or(AestheticError::NoScore)
            },
        )
    }
}
