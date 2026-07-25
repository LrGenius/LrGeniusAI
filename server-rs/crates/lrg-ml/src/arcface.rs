//! ArcFace recognition (buffalo_l `w600k_r50.onnx`) — port of
//! `insightface.model_zoo.arcface_onnx.ArcFaceONNX.get_feat`: align via
//! `norm_crop` (112x112), then
//! `cv2.dnn.blobFromImages(imgs, 1/127.5, (112,112), (127.5,...),
//! swapRB=True)`. Same RGB->"BGR via swapRB" channel reversal as SCRFD
//! (see scrfd.rs docs) — reproduced exactly, not corrected.

const INPUT_MEAN: f32 = 127.5;
const INPUT_STD: f32 = 127.5;
pub const EMBED_DIM: usize = 512;

/// Aligned 112x112 RGB crop -> NCHW blob (1x3x112x112) ready for
/// `w600k_r50.onnx`.
pub fn to_blob(aligned_rgb: &[u8]) -> Vec<f32> {
    const SIZE: usize = 112;
    let n = SIZE * SIZE;
    debug_assert_eq!(aligned_rgb.len(), n * 3);
    let mut out = vec![0.0f32; 3 * n];
    for i in 0..n {
        let r = aligned_rgb[i * 3] as f32;
        let g = aligned_rgb[i * 3 + 1] as f32;
        let b = aligned_rgb[i * 3 + 2] as f32;
        out[i] = (b - INPUT_MEAN) / INPUT_STD;
        out[n + i] = (g - INPUT_MEAN) / INPUT_STD;
        out[2 * n + i] = (r - INPUT_MEAN) / INPUT_STD;
    }
    out
}
