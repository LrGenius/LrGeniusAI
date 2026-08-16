//! BioCLIP 2 image preprocessing.
//!
//! **Deliberately different from [`crate::image_pre`] in three ways.** Do not
//! "unify" them — they preprocess for different checkpoints, and every
//! difference here is load-bearing:
//!
//! | | SigLIP2 (`image_pre`) | BioCLIP 2 (here) |
//! |---|---|---|
//! | size | 384 | **224** |
//! | geometry | shortest-edge resize + center crop | **squash** to 224x224 |
//! | filter | bicubic | **bilinear** |
//! | domain | resize on `u8`, then scale | **scale to `[0,1]` first, resize on `f32`** |
//!
//! mean/std are the same OpenAI-CLIP constants in both, reused from
//! `image_pre` rather than re-typed.
//!
//! The source of truth is pybioclip, which overrides open_clip's own
//! transform for TreeOfLife models (`predict.py`, `BaseClassifier.
//! load_pretrained_model`: `self.preprocess = preprocess_img if self.model_str
//! in TOL_MODELS else preprocess`) with:
//!
//! ```python
//! transforms.Compose([
//!     transforms.ToTensor(),
//!     transforms.Resize((224, 224), antialias=True),
//!     transforms.Normalize(mean=(0.48145466, 0.4578275, 0.40821073),
//!                          std=(0.26862954, 0.26130258, 0.27577711)),
//! ])
//! ```
//!
//! Note `ToTensor()` comes *before* `Resize` — hence the float-domain resize
//! ([`resize_plane_f32`]) instead of the `u8` path SigLIP2 uses. `Resize` with
//! a *tuple* target does not preserve aspect ratio, so a 3:2 frame is squashed
//! rather than cropped: the whole organism stays in frame, which is the point
//! for identification, and it is what the checkpoint was trained on.
//!
//! torchvision's `antialias=True` bilinear and Pillow's bilinear widen the
//! kernel support by the downscale factor the same way, so sharing
//! `pil_resample`'s coefficients is the right approximation — but it is an
//! approximation. `tests/bioclip_goldens.rs` is what bounds the residual.

use lrg_imaging::pil_resample::{resize_plane_f32, Filter};

use crate::image_pre::{MEAN, STD};

pub const IMAGE_SIZE: usize = 224;

/// Preprocess one RGB8 image into an NCHW f32 buffer (1x3x224x224):
/// scale to `[0,1]`, squash-resize to 224x224 (bilinear, antialiased),
/// per-channel OpenAI-CLIP normalize.
pub fn preprocess(pixels: &[u8], width: usize, height: usize) -> Vec<f32> {
    debug_assert_eq!(pixels.len(), width * height * 3);

    let mut out = vec![0.0f32; 3 * IMAGE_SIZE * IMAGE_SIZE];
    let mut plane = vec![0.0f32; width * height];
    for c in 0..3 {
        // `ToTensor()`: u8 -> f32 scaled to [0, 1], per channel plane.
        for (i, v) in plane.iter_mut().enumerate() {
            *v = pixels[i * 3 + c] as f32 / 255.0;
        }
        let resized = resize_plane_f32(
            &plane,
            width,
            height,
            IMAGE_SIZE,
            IMAGE_SIZE,
            Filter::Bilinear,
        );
        let base = c * IMAGE_SIZE * IMAGE_SIZE;
        for (i, v) in resized.iter().enumerate() {
            out[base + i] = (v - MEAN[c]) / STD[c];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_shape_and_normalization() {
        // A uniform mid-grey image resizes to itself, so every output value
        // is the normalized constant — which pins the channel ordering, the
        // [0,1] scaling and the mean/std application all at once.
        let pixels = vec![128u8; 40 * 30 * 3];
        let out = preprocess(&pixels, 40, 30);
        assert_eq!(out.len(), 3 * IMAGE_SIZE * IMAGE_SIZE);
        for c in 0..3 {
            let expected = (128.0 / 255.0 - MEAN[c]) / STD[c];
            let base = c * IMAGE_SIZE * IMAGE_SIZE;
            for &v in &out[base..base + IMAGE_SIZE * IMAGE_SIZE] {
                assert!((v - expected).abs() < 1e-4, "got {v}, want {expected}");
            }
        }
    }

    #[test]
    fn squashes_rather_than_crops() {
        // A wide frame with a distinctive right edge: under a center crop the
        // right edge would be discarded, under a squash it survives. Red on
        // the left half, blue on the right.
        let (w, h) = (400, 100);
        let mut pixels = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                if x < w / 2 {
                    pixels[i] = 255;
                } else {
                    pixels[i + 2] = 255;
                }
            }
        }
        let out = preprocess(&pixels, w, h);
        let red_plane = &out[0..IMAGE_SIZE * IMAGE_SIZE];
        let blue_plane = &out[2 * IMAGE_SIZE * IMAGE_SIZE..];
        let row = IMAGE_SIZE / 2;
        // Left stays red, right stays blue — nothing was cropped away.
        assert!(red_plane[row * IMAGE_SIZE + 10] > red_plane[row * IMAGE_SIZE + 210]);
        assert!(blue_plane[row * IMAGE_SIZE + 210] > blue_plane[row * IMAGE_SIZE + 10]);
    }
}
