//! ONNX inference: SigLIP2 image/text embedding via `ort`. Face pipeline
//! (SCRFD + ArcFace) lands in M5.

/// Session config key that turns off MLAS's KleidiAI convolution kernels
/// (onnxruntime >= 1.25).
///
/// Those kernels leak: `ArmKleidiAI::MlasConv` allocates a workspace with
/// `operator new` on every convolution and never frees it. Measured on
/// arm64 macOS with `malloc_history`, a face-detection indexing run
/// retained ~31 MB per photo, 89% of it under that one frame — a 14k-photo
/// catalog reached a 45 GB footprint on a 24 GB machine. The allocations
/// are orphaned rather than owned by the session, so dropping and
/// rebuilding the session does not reclaim them; the only fix available
/// from the API side is to not take the KleidiAI path at all.
///
/// The key is ignored on non-arm64 builds, where these kernels don't exist.
pub const DISABLE_KLEIDIAI: &str = "mlas.disable_kleidiai";

pub mod arcface;
pub mod cv2_resize;
pub mod face_quality;
pub mod faces;
pub mod image_pre;
pub mod model_paths;
pub mod scrfd;
pub mod siglip;
pub mod text_pre;
pub mod umeyama;
