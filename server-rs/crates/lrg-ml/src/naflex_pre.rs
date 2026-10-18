//! SigLIP2 NaFlex image preprocessing.
//!
//! A port of transformers' `Siglip2ImageProcessorPil` — the PIL backend, not
//! the torchvision one, because [`resize_plane`] is a bit-exact port of
//! Pillow's 8-bit resize and the goldens (`testdata/naflex_pre_goldens.json`,
//! from `testdata/make_naflex_pre_goldens.py`) are generated with it:
//!
//!   resize so the image fits in at most `max_num_patches` 16x16 patches,
//!   aspect ratio kept, both sides a multiple of 16 (bilinear) ->
//!   rescale 1/255 -> normalize with mean = std = 0.5 ->
//!   cut into patches, row-major, each flattened (y, x, channel) ->
//!   zero-pad to `max_num_patches` and mark the real ones in a mask.
//!
//! This replaces the 384x384 center crop the fixed-resolution model used:
//! NaFlex sees the whole frame at its own aspect ratio, so a portrait or a
//! panorama no longer loses its edges before the model looks at it.
//!
//! The position embeddings are the one part of NaFlex that does not travel as
//! pixels. The model learned them on a 16x16 grid and resizes that grid to each
//! image's patch grid (`Siglip2VisionEmbeddings.resize_positional_embeddings`,
//! bilinear with antialias). A data-dependent `F.interpolate` does not survive
//! ONNX export, so the exported graph takes the resize as two small weight
//! matrices instead — [`NaflexInput::position_interpolation`] builds them —
//! and applies them to the grid it carries as an initializer.

use lrg_imaging::pil_resample::{resample_matrix, resize_plane, Filter};

/// Side of one square patch, in pixels.
pub const PATCH_SIZE: usize = 16;

/// Values per flattened patch: 16 x 16 pixels x 3 channels.
pub const PATCH_DIM: usize = PATCH_SIZE * PATCH_SIZE * 3;

/// How many patches an image is resized to fit, at most.
///
/// 576 is the token count of the fixed 384x384 model this replaces (24 x 24),
/// so indexing costs about what it did. The model accepts any value; changing
/// it changes every embedding, so it is part of `siglip::MODEL_ID`.
pub const MAX_NUM_PATCHES: usize = 576;

/// Side of the learned position-embedding grid (`sqrt(config.num_patches)`).
///
/// Fixed by the checkpoint, not a tuning knob: the exported graph's
/// interpolation inputs are `[grid_h, 16]` and `[grid_w, 16]`.
pub const POSITION_GRID: usize = 16;

const MEAN: f32 = 0.5;
const STD: f32 = 0.5;

/// One image, ready for the NaFlex image tower.
pub struct NaflexInput {
    /// `[max_num_patches, PATCH_DIM]`, row-major; rows past
    /// [`Self::num_patches`] are zero.
    pub pixel_values: Vec<f32>,
    /// `[max_num_patches]`: 1 for a real patch, 0 for padding.
    pub attention_mask: Vec<i64>,
    /// Patch rows.
    pub grid_h: usize,
    /// Patch columns.
    pub grid_w: usize,
    pub max_num_patches: usize,
}

impl NaflexInput {
    pub fn num_patches(&self) -> usize {
        self.grid_h * self.grid_w
    }

    /// The `[grid_h, POSITION_GRID]` and `[grid_w, POSITION_GRID]` matrices
    /// that resize the position-embedding grid to this image's patch grid.
    ///
    /// torch's antialiased bilinear resize is separable and uses Pillow's
    /// kernels, so `rows @ grid @ cols^T` per channel is the same resize the
    /// model ran in training.
    pub fn position_interpolation(&self) -> (Vec<f32>, Vec<f32>) {
        (
            resample_matrix(POSITION_GRID, self.grid_h, Filter::Bilinear),
            resample_matrix(POSITION_GRID, self.grid_w, Filter::Bilinear),
        )
    }
}

/// Port of transformers' `get_image_size_for_max_num_patches`: the largest
/// size, both sides a multiple of `patch_size` and at least one patch, that
/// keeps the aspect ratio and stays within `max_num_patches`.
///
/// Kept as the same binary search over the scale, in `f64`, rather than solved
/// in closed form: rounding up to whole patches makes the boundary a step
/// function, and only the same search lands on the same step.
pub fn target_size(
    height: usize,
    width: usize,
    patch_size: usize,
    max_num_patches: usize,
) -> (usize, usize) {
    const EPS: f64 = 1e-5;
    let scaled = |scale: f64, size: usize| -> usize {
        let s = size as f64 * scale;
        let s = (s / patch_size as f64).ceil() * patch_size as f64;
        (s.max(patch_size as f64)) as usize
    };

    let (mut scale_min, mut scale_max) = (EPS / 10.0, 100.0f64);
    while (scale_max - scale_min) >= EPS {
        let scale = (scale_min + scale_max) / 2.0;
        let th = scaled(scale, height);
        let tw = scaled(scale, width);
        let num_patches = (th as f64 / patch_size as f64) * (tw as f64 / patch_size as f64);
        if num_patches <= max_num_patches as f64 {
            scale_min = scale;
        } else {
            scale_max = scale;
        }
    }
    (scaled(scale_min, height), scaled(scale_min, width))
}

/// Preprocess one RGB8 image (`width * height * 3` bytes, interleaved).
pub fn preprocess(
    pixels: &[u8],
    width: usize,
    height: usize,
    max_num_patches: usize,
) -> NaflexInput {
    let (th, tw) = target_size(height, width, PATCH_SIZE, max_num_patches);
    preprocess_with_grid(
        pixels,
        width,
        height,
        th / PATCH_SIZE,
        tw / PATCH_SIZE,
        max_num_patches,
    )
}

/// [`preprocess`] with the patch grid chosen by the caller.
///
/// For a model whose authors pick the grid with their own rule rather than
/// transformers': AestheticSigLIP does (`crate::aesthetic::patch_grid`). The
/// resize, normalization and patch order are the same.
pub fn preprocess_with_grid(
    pixels: &[u8],
    width: usize,
    height: usize,
    grid_h: usize,
    grid_w: usize,
    max_num_patches: usize,
) -> NaflexInput {
    debug_assert_eq!(pixels.len(), width * height * 3);
    debug_assert!(grid_h * grid_w <= max_num_patches);
    let (th, tw) = (grid_h * PATCH_SIZE, grid_w * PATCH_SIZE);

    let mut planes = [Vec::new(), Vec::new(), Vec::new()];
    for (c, plane_out) in planes.iter_mut().enumerate() {
        let plane: Vec<u8> = (0..width * height).map(|i| pixels[i * 3 + c]).collect();
        *plane_out = resize_plane(&plane, width, height, tw, th, Filter::Bilinear);
    }

    let mut pixel_values = vec![0.0f32; max_num_patches * PATCH_DIM];
    let mut attention_mask = vec![0i64; max_num_patches];
    for gy in 0..grid_h {
        for gx in 0..grid_w {
            let patch = gy * grid_w + gx;
            attention_mask[patch] = 1;
            let out = &mut pixel_values[patch * PATCH_DIM..(patch + 1) * PATCH_DIM];
            for py in 0..PATCH_SIZE {
                let row = (gy * PATCH_SIZE + py) * tw + gx * PATCH_SIZE;
                for px in 0..PATCH_SIZE {
                    for (c, plane) in planes.iter().enumerate() {
                        // `rescale` multiplies in f64 and casts to f32;
                        // `normalize` then runs in f32.
                        let v = (plane[row + px] as f64 * (1.0 / 255.0)) as f32;
                        out[(py * PATCH_SIZE + px) * 3 + c] = (v - MEAN) / STD;
                    }
                }
            }
        }
    }

    NaflexInput {
        pixel_values,
        attention_mask,
        grid_h,
        grid_w,
        max_num_patches,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_size_fills_but_never_exceeds_the_budget() {
        for (h, w) in [(480, 640), (640, 480), (1000, 1000), (200, 3000), (37, 41)] {
            let (th, tw) = target_size(h, w, PATCH_SIZE, MAX_NUM_PATCHES);
            assert_eq!(th % PATCH_SIZE, 0);
            assert_eq!(tw % PATCH_SIZE, 0);
            assert!((th / PATCH_SIZE) * (tw / PATCH_SIZE) <= MAX_NUM_PATCHES);
        }
        // A square image uses the whole 24x24 budget.
        assert_eq!(
            target_size(1000, 1000, PATCH_SIZE, MAX_NUM_PATCHES),
            (384, 384)
        );
    }

    #[test]
    fn padding_is_zero_and_masked() {
        let pixels = vec![200u8; 300 * 100 * 3];
        let input = preprocess(&pixels, 300, 100, MAX_NUM_PATCHES);
        let n = input.num_patches();
        assert!(n < MAX_NUM_PATCHES);
        assert!(input.attention_mask[..n].iter().all(|m| *m == 1));
        assert!(input.attention_mask[n..].iter().all(|m| *m == 0));
        assert!(input.pixel_values[n * PATCH_DIM..]
            .iter()
            .all(|v| *v == 0.0));
        // 200/255 normalized with mean = std = 0.5.
        let expected = (200.0f32 / 255.0 - 0.5) / 0.5;
        assert!((input.pixel_values[0] - expected).abs() < 1e-6);
    }
}
