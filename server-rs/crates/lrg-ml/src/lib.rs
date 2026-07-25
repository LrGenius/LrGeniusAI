//! ONNX inference: SigLIP2 image/text embedding via `ort`. Face pipeline
//! (SCRFD + ArcFace) lands in M5.

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
