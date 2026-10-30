//! AuraFace recognition (`auraface_glintr100.onnx`) — fal.ai's AuraFace-v1,
//! an ArcFace-style ResNet-100 trained on commercially usable data and
//! published under Apache-2.0 (huggingface.co/fal/AuraFace-v1, file
//! `glintr100.onnx`, renamed on download so its role is obvious next to the
//! other models).
//!
//! An alternative to FaceNet ([`crate::facenet`]) that the user opts into: it
//! separates identities more sharply — the point of offering it — at roughly
//! three times FaceNet's size and per-face cost. Both produce 512-d unit
//! vectors, so the store cannot tell them apart by shape; every row carries
//! [`crate::faces::Recognizer::model_id`] instead, exactly as for the FaceNet
//! switch.
//!
//! Preprocessing is InsightFace's for its ArcFace-family ONNX models, which is
//! how AuraFace is published to be used (`FaceAnalysis(name="auraface")`):
//! the five-landmark similarity warp onto the 112x112 ArcFace template
//! ([`crate::umeyama::norm_crop_112`] — the very crop InsightFace's
//! `norm_crop` makes), RGB order (InsightFace feeds `blobFromImages` a BGR
//! image with `swapRB=True`), and `(v - 127.5) / 127.5`, which is what
//! `ArcFaceONNX` picks for a graph that does not normalise its own input.
//! `scripts/auraface_reference.py` produces the reference embedding the
//! golden test checks this whole chain against.

pub const INPUT_SIZE: usize = 112;
pub const EMBED_DIM: usize = 512;

const INPUT_MEAN: f32 = 127.5;
const INPUT_STD: f32 = 127.5;

/// Aligned 112x112 RGB crop -> NCHW blob (1x3x112x112).
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
    fn blob_is_rgb_and_scaled_to_unit_range() {
        let mut px = vec![0u8; INPUT_SIZE * INPUT_SIZE * 3];
        px[0] = 255; // R
        px[1] = 127; // G
        px[2] = 0; // B
        let blob = to_blob(&px);
        let n = INPUT_SIZE * INPUT_SIZE;
        assert!((blob[0] - 1.0).abs() < 1e-6);
        assert!((blob[n] - (127.0 - 127.5) / 127.5).abs() < 1e-6);
        assert!((blob[2 * n] + 1.0).abs() < 1e-6);
    }
}
