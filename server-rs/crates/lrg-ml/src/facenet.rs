//! FaceNet recognition (`facenet_vggface2.onnx`) — the Inception-ResNet-v1
//! from `facenet-pytorch`, VGGFace2 weights, exported by
//! `scripts/export_facenet.py`.
//!
//! Replaces InsightFace's ArcFace (`w600k_r50.onnx`). Both produce a
//! 512-d L2-normalized identity embedding, so `FACE_TABLE`'s width is
//! unchanged — which is exactly why stored rows carry
//! [`crate::faces::MODEL_ID`]: two embeddings of the same shape from
//! different spaces are indistinguishable to the store, and mixing them
//! would silently ruin clustering rather than fail loudly.
//!
//! Preprocessing is `facenet-pytorch`'s `fixed_image_standardization`:
//! RGB (no channel swap — unlike both InsightFace models), 160x160,
//! `(v - 127.5) / 128.0`.

pub const INPUT_SIZE: usize = 160;
pub const EMBED_DIM: usize = 512;

const INPUT_MEAN: f32 = 127.5;
const INPUT_STD: f32 = 128.0;

/// Aligned 160x160 RGB crop -> NCHW blob (1x3x160x160) ready for
/// `facenet_vggface2.onnx`.
pub fn to_blob(aligned_rgb: &[u8]) -> Vec<f32> {
    let n = INPUT_SIZE * INPUT_SIZE;
    debug_assert_eq!(aligned_rgb.len(), n * 3);
    let mut out = vec![0.0f32; 3 * n];
    for i in 0..n {
        out[i] = (aligned_rgb[i * 3] as f32 - INPUT_MEAN) / INPUT_STD;
        out[n + i] = (aligned_rgb[i * 3 + 1] as f32 - INPUT_MEAN) / INPUT_STD;
        out[2 * n + i] = (aligned_rgb[i * 3 + 2] as f32 - INPUT_MEAN) / INPUT_STD;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_is_rgb_and_standardized() {
        let mut px = vec![0u8; INPUT_SIZE * INPUT_SIZE * 3];
        px[0] = 255; // R
        px[1] = 127; // G
        px[2] = 0; // B
        let blob = to_blob(&px);
        let n = INPUT_SIZE * INPUT_SIZE;
        // No swapRB here: channel 0 stays R. Both InsightFace models reversed
        // the channels; carrying that habit over would cost accuracy quietly.
        assert!((blob[0] - (255.0 - 127.5) / 128.0).abs() < 1e-6);
        assert!((blob[n] - (127.0 - 127.5) / 128.0).abs() < 1e-6);
        assert!((blob[2 * n] - (0.0 - 127.5) / 128.0).abs() < 1e-6);
    }
}
