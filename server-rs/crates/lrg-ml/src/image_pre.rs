//! SigLIP2 image preprocessing.
//!
//! **Empirically verified, not spec-derived**: the model's own
//! `open_clip_config.json` `preprocess_cfg` (mean/std 0.5, `resize_mode:
//! squash`) is *not* what actually runs. `server_lifecycle.py` loads the
//! model via `open_clip.create_model_and_transforms(CLIP_MODEL_NAME,
//! pretrained=<local safetensors path>)` — passing a bare architecture
//! name plus a local checkpoint path, not an `hf-hub:...` spec — and this
//! path does not consult the checkpoint's `preprocess_cfg` JSON at all.
//! The actual transform (confirmed by printing `preprocess` from that
//! exact call, and by `metadata.py`'s `image_processor(image)` call site)
//! is torchvision's classic OpenAI-CLIP pipeline:
//!
//!   `Resize(384, bicubic, antialias) -> CenterCrop(384) -> ToTensor()
//!    -> Normalize(OPENAI_DATASET_MEAN, OPENAI_DATASET_STD)`
//!
//! i.e. aspect-preserving shortest-edge resize + center crop, and the
//! *original OpenAI CLIP* per-channel mean/std — not 0.5/0.5. Verified
//! against real torch output (`testdata/siglip_goldens.json`); do not
//! "fix" this back to the config.json values without re-checking both.

use lrg_imaging::pil_resample::{resize_plane, Filter};

pub const IMAGE_SIZE: usize = 384;
// Exact OPENAI_DATASET_MEAN/STD constants from open_clip — kept at full
// precision to match torchvision's f32 cast bit-for-bit.
// Shared with `bioclip_pre`: BioCLIP 2 normalizes with the same OpenAI-CLIP
// constants even though every other part of its transform differs.
#[allow(clippy::excessive_precision)]
pub(crate) const MEAN: [f32; 3] = [0.48145466, 0.4578275, 0.40821073];
#[allow(clippy::excessive_precision)]
pub(crate) const STD: [f32; 3] = [0.26862954, 0.26130258, 0.27577711];

/// `torchvision.transforms.functional._compute_resized_output_size` for a
/// single-int target (shortest edge -> target, aspect preserved, `int()`
/// truncation matching Python).
fn resized_output_size(w: usize, h: usize, target: usize) -> (usize, usize) {
    if w <= h {
        (target, (target * h) / w)
    } else {
        ((target * w) / h, target)
    }
}

/// Python `round()` (round-half-to-even) applied to `a / 2.0`, matching
/// `center_crop`'s `crop_top/crop_left` computation.
fn half_round_even(diff: i64) -> i64 {
    ((diff as f64) / 2.0).round_ties_even() as i64
}

/// Preprocess one RGB8 image into an NCHW f32 buffer (1x3x384x384):
/// shortest-edge resize to 384 (bicubic), center crop to 384x384,
/// per-channel OpenAI-CLIP normalize.
pub fn preprocess(pixels: &[u8], width: usize, height: usize) -> Vec<f32> {
    debug_assert_eq!(pixels.len(), width * height * 3);
    let (new_w, new_h) = resized_output_size(width, height, IMAGE_SIZE);

    let mut resized_planes = [Vec::new(), Vec::new(), Vec::new()];
    for (c, plane_out) in resized_planes.iter_mut().enumerate() {
        let plane: Vec<u8> = (0..width * height).map(|i| pixels[i * 3 + c]).collect();
        *plane_out = resize_plane(&plane, width, height, new_w, new_h, Filter::Bicubic);
    }

    // Center crop to IMAGE_SIZE x IMAGE_SIZE. Pad with zeros first if the
    // resized image is smaller than the crop in either dimension (matches
    // torchvision's `pad` step; only reachable for degenerate inputs).
    let (pad_w, pad_h) = (
        IMAGE_SIZE.saturating_sub(new_w),
        IMAGE_SIZE.saturating_sub(new_h),
    );
    let (padded_w, padded_h) = (new_w + pad_w, new_h + pad_h);
    let pad_left = pad_w / 2;
    let pad_top = pad_h / 2;

    let crop_left = pad_left + half_round_even(padded_w as i64 - IMAGE_SIZE as i64).max(0) as usize;
    let crop_top = pad_top + half_round_even(padded_h as i64 - IMAGE_SIZE as i64).max(0) as usize;

    let mut out = vec![0.0f32; 3 * IMAGE_SIZE * IMAGE_SIZE];
    for (c, plane) in resized_planes.iter().enumerate() {
        let base = c * IMAGE_SIZE * IMAGE_SIZE;
        for y in 0..IMAGE_SIZE {
            for x in 0..IMAGE_SIZE {
                let src_x = crop_left + x;
                let src_y = crop_top + y;
                let v = if src_x >= pad_left
                    && src_x < pad_left + new_w
                    && src_y >= pad_top
                    && src_y < pad_top + new_h
                {
                    plane[(src_y - pad_top) * new_w + (src_x - pad_left)] as f32 / 255.0
                } else {
                    0.0 // zero-fill for the padded border
                };
                out[base + y * IMAGE_SIZE + x] = (v - MEAN[c]) / STD[c];
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resized_output_size_matches_torchvision() {
        // Verified against real torchvision output for a 4:3 landscape.
        assert_eq!(resized_output_size(640, 480, 384), (512, 384));
        assert_eq!(resized_output_size(480, 640, 384), (384, 512));
        assert_eq!(resized_output_size(384, 384, 384), (384, 384));
    }

    #[test]
    fn output_shape_and_finite() {
        let pixels = vec![128u8; 100 * 60 * 3];
        let out = preprocess(&pixels, 100, 60);
        assert_eq!(out.len(), 3 * IMAGE_SIZE * IMAGE_SIZE);
        assert!(out.iter().all(|v| v.is_finite()));
    }
}
