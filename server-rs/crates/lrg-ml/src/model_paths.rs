//! Resolves where the SigLIP2 ONNX models + tokenizer live on disk.
//!
//! Interim convention for M4: explicit env vars, falling back to a
//! `~/.cache/lrgenius/models/` directory. Production distribution
//! (bundled fp16 assets, download-on-first-run, signed release assets)
//! is M9 scope — this will change once that lands.

use std::path::PathBuf;

use crate::faces::FaceModelPaths;
use crate::siglip::ModelPaths;

fn default_models_dir() -> PathBuf {
    dirs_next_home()
        .join(".cache")
        .join("lrgenius")
        .join("models")
}

fn dirs_next_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn env_or_default(var: &str, default: PathBuf) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or(default)
}

pub fn resolve() -> ModelPaths {
    let dir = default_models_dir();
    ModelPaths {
        image_onnx: env_or_default("LRG_SIGLIP_IMAGE_ONNX", dir.join("siglip2_image.onnx")),
        text_onnx: env_or_default("LRG_SIGLIP_TEXT_ONNX", dir.join("siglip2_text.onnx")),
        tokenizer_json: env_or_default("LRG_SIGLIP_TOKENIZER", dir.join("tokenizer.json")),
    }
}

/// buffalo_l's own directory layout is the real InsightFace convention
/// (`INSIGHTFACE_ROOT`, default `~/.insightface`) — unlike SigLIP2, no
/// interim convention is needed since these ONNX files are already
/// exactly what production downloads and uses.
pub fn resolve_face() -> FaceModelPaths {
    let root = std::env::var_os("INSIGHTFACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_next_home().join(".insightface"));
    let dir = root.join("models").join("buffalo_l");
    FaceModelPaths {
        det_onnx: dir.join("det_10g.onnx"),
        rec_onnx: dir.join("w600k_r50.onnx"),
    }
}
