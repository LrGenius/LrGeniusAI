//! ONNX inference: SigLIP2 image/text embedding via `ort`. Face pipeline
//! (SCRFD + ArcFace) lands in M5.

pub mod image_pre;
pub mod model_paths;
pub mod siglip;
pub mod text_pre;
