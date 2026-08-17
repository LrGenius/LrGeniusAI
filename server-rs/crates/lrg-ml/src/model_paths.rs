//! Resolves where the SigLIP2 ONNX models + tokenizer live on disk.
//!
//! Interim convention for M4: explicit env vars, falling back to a
//! `~/.cache/lrgenius/models/` directory. Production distribution
//! (bundled fp16 assets, download-on-first-run, signed release assets)
//! is M9 scope — this will change once that lands.

use std::path::PathBuf;

use crate::bioclip::BioclipModelPaths;
use crate::faces::FaceModelPaths;
use crate::siglip::ModelPaths;

fn default_models_dir() -> PathBuf {
    dirs_next_home()
        .join(".cache")
        .join("lrgenius")
        .join("models")
}

/// The home directory the model cache hangs off.
///
/// `USERPROFILE` is not optional politeness: a native Windows process has no
/// `HOME` (only Git Bash and friends set one), so without that fallback every
/// model on Windows resolved under `std::env::temp_dir()` — a location the OS
/// is entitled to empty, for a 3 GB download the user is asked to make once.
/// `lrg-api`'s `llm_models`/`mlx_models` already resolve their roots this way;
/// this brings the ONNX families in line with them.
fn dirs_next_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
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

/// Where GGUF weights for the in-process LLM live.
///
/// Same per-artifact env-var convention as SigLIP2, plus `dir` so the download
/// route and on-disk discovery agree on one location without each rebuilding
/// the path. Unlike the SigLIP2 files there is no fixed filename: which GGUF is
/// in play is a user choice, so only the directory is well-known.
#[derive(Debug, Clone)]
pub struct LlmModelPaths {
    /// Directory downloads land in and discovery scans.
    pub dir: PathBuf,
    /// Explicit override for the text model, from `LRG_LLAMA_MODEL_GGUF`.
    pub model_gguf: Option<PathBuf>,
    /// Explicit override for the multimodal projector, from
    /// `LRG_LLAMA_MMPROJ_GGUF`.
    pub mmproj_gguf: Option<PathBuf>,
}

pub fn resolve_llm() -> LlmModelPaths {
    LlmModelPaths {
        dir: env_or_default("LRG_LLAMA_MODEL_DIR", default_models_dir().join("llm")),
        model_gguf: std::env::var_os("LRG_LLAMA_MODEL_GGUF").map(PathBuf::from),
        mmproj_gguf: std::env::var_os("LRG_LLAMA_MMPROJ_GGUF").map(PathBuf::from),
    }
}

/// Where MLX models live.
///
/// Deliberately a *directory of directories*, unlike [`LlmModelPaths`]: an MLX
/// model is a Hugging Face repo snapshot (`config.json`, safetensors shards,
/// tokenizer files), not a single file, so there is nothing analogous to
/// `LRG_LLAMA_MODEL_GGUF` pointing at one artifact. `model_dir` is the override
/// for "use exactly this model", and `dir` is the root that downloads land in
/// and discovery scans.
#[derive(Debug, Clone)]
pub struct MlxModelPaths {
    /// Root directory holding one subdirectory per model.
    pub dir: PathBuf,
    /// Explicit override for a single model directory, from `LRG_MLX_MODEL_DIR`.
    pub model_dir: Option<PathBuf>,
}

pub fn resolve_mlx() -> MlxModelPaths {
    MlxModelPaths {
        dir: env_or_default("LRG_MLX_MODEL_ROOT", default_models_dir().join("mlx")),
        model_dir: std::env::var_os("LRG_MLX_MODEL_DIR").map(PathBuf::from),
    }
}

/// Where BioCLIP 2's image tower and its pruned Tree-of-Life head live.
///
/// Same per-artifact env-var convention as SigLIP2. Three files rather than
/// two because the zero-shot head is split: the embedding matrix is binary
/// (`bioclip2_taxa.bin`) and its labels are JSON (`bioclip2_taxa.json`) — see
/// `lrg-ml::bioclip` for the format and `scripts/bioclip_taxa_filter.toml`
/// for which taxa are in it.
pub fn resolve_bioclip() -> BioclipModelPaths {
    let dir = default_models_dir();
    BioclipModelPaths {
        image_onnx: env_or_default("LRG_BIOCLIP_IMAGE_ONNX", dir.join("bioclip2_image.onnx")),
        taxa_bin: env_or_default("LRG_BIOCLIP_TAXA_BIN", dir.join("bioclip2_taxa.bin")),
        taxa_json: env_or_default("LRG_BIOCLIP_TAXA_JSON", dir.join("bioclip2_taxa.json")),
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
