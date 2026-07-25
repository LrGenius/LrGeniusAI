//! Resolves where the SigLIP2 ONNX models + tokenizer live on disk.
//!
//! Interim convention for M4: explicit env vars, falling back to a
//! `~/.cache/lrgenius/models/` directory. Production distribution
//! (bundled fp16 assets, download-on-first-run, signed release assets)
//! is M9 scope — this will change once that lands.

use std::path::PathBuf;

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
