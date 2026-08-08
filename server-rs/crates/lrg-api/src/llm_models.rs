//! Which GGUF models exist, where they are, and how to get one.
//!
//! Three sources, in priority order:
//!
//! 1. **Explicit env overrides** (`LRG_LLAMA_MODEL_GGUF` / `LRG_LLAMA_MMPROJ_GGUF`) —
//!    the escape hatch for testing and for users with their own weights.
//! 2. **On-disk discovery** — our own model directory plus the places LM Studio
//!    and Ollama keep GGUFs, so a user who already downloaded a vision model
//!    needs no second copy.
//! 3. **The curated catalog** — a short list of known-good vision models that
//!    can be downloaded from Hugging Face.
//!
//! GitHub release assets cap at 2 GB per file, which a 4B Q4 model exceeds, so
//! unlike the SigLIP2 assets these are fetched from Hugging Face directly.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// A model the user could download. Sizes and digests are filled in from the
/// HTTP response rather than hard-coded, because pinning a sha256 we cannot
/// verify at build time would be security theatre — see the note on
/// [`crate::routes::llm`].
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    pub id: &'static str,
    pub label: &'static str,
    /// Hugging Face repo, e.g. `ggml-org/SmolVLM-500M-Instruct-GGUF`.
    pub repo: &'static str,
    pub revision: &'static str,
    pub model_file: &'static str,
    pub mmproj_file: &'static str,
    /// Approximate total download, for the UI to show before committing.
    pub approx_bytes: u64,
    /// Rough floor for comfortable use; the UI warns below it.
    pub min_ram_gb: u32,
}

/// Deliberately short. Every entry must be an **ungated** repo — a gated one
/// needs a Hugging Face token, which the plugin has no way to supply.
pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: "smolvlm-500m",
        label: "SmolVLM 500M (fast, low quality — good for testing)",
        repo: "ggml-org/SmolVLM-500M-Instruct-GGUF",
        revision: "main",
        model_file: "SmolVLM-500M-Instruct-Q8_0.gguf",
        mmproj_file: "mmproj-SmolVLM-500M-Instruct-Q8_0.gguf",
        approx_bytes: 700 * 1024 * 1024,
        min_ram_gb: 4,
    },
    CatalogEntry {
        id: "qwen2.5-vl-3b",
        label: "Qwen2.5-VL 3B (balanced)",
        repo: "ggml-org/Qwen2.5-VL-3B-Instruct-GGUF",
        revision: "main",
        model_file: "Qwen2.5-VL-3B-Instruct-Q4_K_M.gguf",
        mmproj_file: "mmproj-Qwen2.5-VL-3B-Instruct-f16.gguf",
        approx_bytes: 3_400 * 1024 * 1024,
        min_ram_gb: 8,
    },
    CatalogEntry {
        id: "qwen2.5-vl-7b",
        label: "Qwen2.5-VL 7B (best quality, needs more RAM)",
        repo: "ggml-org/Qwen2.5-VL-7B-Instruct-GGUF",
        revision: "main",
        model_file: "Qwen2.5-VL-7B-Instruct-Q4_K_M.gguf",
        mmproj_file: "mmproj-Qwen2.5-VL-7B-Instruct-f16.gguf",
        approx_bytes: 6_500 * 1024 * 1024,
        min_ram_gb: 16,
    },
    // The current generation. All three are 4-bit and ungated, and
    // `approx_bytes` is the measured model + mmproj total rather than an
    // estimate. `lmstudio-community` is preferred over the other mirrors
    // because its mmproj files are named after the model — `mmproj-BF16.gguf`
    // on its own is easy to pair with the wrong weights once several models
    // share a directory.
    CatalogEntry {
        id: "gemma4-e4b",
        label: "Gemma 4 E4B (recommended)",
        repo: "lmstudio-community/gemma-4-E4B-it-GGUF",
        revision: "main",
        model_file: "gemma-4-E4B-it-Q4_K_M.gguf",
        mmproj_file: "mmproj-gemma-4-E4B-it-BF16.gguf",
        approx_bytes: 6_326_843_776,
        min_ram_gb: 16,
    },
    CatalogEntry {
        id: "gemma4-12b",
        label: "Gemma 4 12B (highest quality, needs more RAM)",
        // Google's quantization-aware-trained 4-bit release. QAT is Q4_0 rather
        // than Q4_K_M, but it is trained at 4-bit instead of rounded down to it,
        // so it holds up better than a post-training quantization of the same
        // size — which matters more at 12B than the K-quant packing does.
        repo: "lmstudio-community/gemma-4-12B-it-QAT-GGUF",
        revision: "main",
        model_file: "gemma-4-12B-it-QAT-Q4_0.gguf",
        mmproj_file: "mmproj-gemma-4-12B-it-QAT-BF16.gguf",
        approx_bytes: 7_150_994_336,
        min_ram_gb: 24,
    },
    CatalogEntry {
        id: "qwen3.5-9b",
        label: "Qwen3.5 9B (balanced alternative to Gemma)",
        repo: "lmstudio-community/Qwen3.5-9B-GGUF",
        revision: "main",
        model_file: "Qwen3.5-9B-Q4_K_M.gguf",
        mmproj_file: "mmproj-Qwen3.5-9B-BF16.gguf",
        approx_bytes: 6_548_748_736,
        min_ram_gb: 16,
    },
];

#[must_use]
pub fn catalog_entry(id: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|e| e.id == id)
}

#[must_use]
pub fn hf_url(repo: &str, revision: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/{revision}/{file}?download=true")
}

/// A usable model found on disk.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LocalModel {
    /// What the plugin sends back as `model` — the file name, which is also
    /// what the user sees.
    pub name: String,
    pub model_path: PathBuf,
    /// `None` means text-only: usable for keyword clustering but not for
    /// analysing photos.
    pub mmproj_path: Option<PathBuf>,
    /// Where it was found, for the UI ("already installed" vs "from LM Studio").
    pub source: &'static str,
}

/// `true` for files that are a projector rather than a text model.
///
/// The convention across every GGUF publisher is an `mmproj` prefix or infix;
/// there is no metadata flag we can read without opening the file.
fn is_mmproj(file_name: &str) -> bool {
    file_name.to_ascii_lowercase().contains("mmproj")
}

/// Pair each text model with a projector from the same directory.
///
/// Publishers name the pair consistently (`X.gguf` ↔ `mmproj-X.gguf`), so
/// prefer a projector whose name shares the model's stem; otherwise fall back
/// to the only projector present. Guessing across directories would be worse
/// than offering the model as text-only.
fn pair_models(files: &[PathBuf], source: &'static str) -> Vec<LocalModel> {
    let (projectors, models): (Vec<&PathBuf>, Vec<&PathBuf>) = files
        .iter()
        .partition(|p| is_mmproj(&p.file_name().unwrap_or_default().to_string_lossy()));

    models
        .into_iter()
        .map(|model_path| {
            let stem = model_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            let matching = projectors
                .iter()
                .find(|p| {
                    let name = p.file_name().unwrap_or_default().to_string_lossy();
                    let name = name.to_ascii_lowercase();
                    name.contains(stem.as_str())
                })
                .or(if projectors.len() == 1 {
                    projectors.first()
                } else {
                    None
                });
            LocalModel {
                name: model_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                model_path: (*model_path).clone(),
                mmproj_path: matching.map(|p| (*p).clone()),
                source,
            }
        })
        .collect()
}

fn gguf_files_in(dir: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((current, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < max_depth {
                    stack.push((path, depth + 1));
                }
            } else if path.extension().is_some_and(|e| e == "gguf") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Every model we can offer without downloading anything.
///
/// LM Studio nests models as `<publisher>/<repo>/<file>.gguf`, hence the depth
/// allowance. Ollama is deliberately not scanned: its blobs are content-addressed
/// (`sha256-…` with no extension), so they cannot be identified as GGUF by name
/// and pairing a projector to a model is not possible without parsing its
/// manifests.
#[must_use]
pub fn discover_local_models() -> Vec<LocalModel> {
    let paths = lrg_ml::model_paths::resolve_llm();
    let mut found = Vec::new();

    // Explicit overrides win and are always offered, even outside any scanned
    // directory.
    if let Some(model_path) = paths.model_gguf.filter(|p| p.is_file()) {
        found.push(LocalModel {
            name: model_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            mmproj_path: paths.mmproj_gguf.filter(|p| p.is_file()),
            model_path,
            source: "env",
        });
    }

    found.extend(pair_models(&gguf_files_in(&paths.dir, 1), "downloaded"));

    if let Some(home) = home() {
        let lmstudio = home.join(".lmstudio").join("models");
        if lmstudio.is_dir() {
            found.extend(pair_models(&gguf_files_in(&lmstudio, 3), "lmstudio"));
        }
    }

    // A model reachable by two routes should still appear once.
    found.dedup_by(|a, b| a.model_path == b.model_path);
    found
}

/// Find the model the request asked for.
///
/// `name` is matched against the file name, then against a full path, so both
/// "what the dropdown showed" and "a path the user typed" work. An empty name
/// takes the first available model, which is what makes the env-override path
/// usable without any UI.
#[must_use]
pub fn resolve_model(name: &str) -> Option<LocalModel> {
    let available = discover_local_models();
    let name = name.trim();
    if name.is_empty() {
        return available.into_iter().next();
    }
    available
        .iter()
        .find(|m| m.name == name)
        .or_else(|| available.iter().find(|m| m.model_path == Path::new(name)))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projectors_are_recognised_by_name() {
        assert!(is_mmproj("mmproj-model-f16.gguf"));
        assert!(is_mmproj("Qwen2.5-VL-mmproj.gguf"));
        assert!(!is_mmproj("Qwen2.5-VL-3B-Instruct-Q4_K_M.gguf"));
    }

    #[test]
    fn pairs_a_projector_with_its_matching_model() {
        let files = vec![
            PathBuf::from("/m/SmolVLM-500M-Instruct-Q8_0.gguf"),
            PathBuf::from("/m/mmproj-SmolVLM-500M-Instruct-Q8_0.gguf"),
        ];
        let paired = pair_models(&files, "downloaded");
        assert_eq!(paired.len(), 1, "the projector is not a model of its own");
        assert_eq!(paired[0].name, "SmolVLM-500M-Instruct-Q8_0.gguf");
        assert_eq!(
            paired[0].mmproj_path,
            Some(PathBuf::from("/m/mmproj-SmolVLM-500M-Instruct-Q8_0.gguf"))
        );
    }

    #[test]
    fn picks_the_right_projector_when_several_models_share_a_directory() {
        let files = vec![
            PathBuf::from("/m/alpha-Q4.gguf"),
            PathBuf::from("/m/beta-Q4.gguf"),
            PathBuf::from("/m/mmproj-alpha-Q4.gguf"),
            PathBuf::from("/m/mmproj-beta-Q4.gguf"),
        ];
        let mut paired = pair_models(&files, "downloaded");
        paired.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(paired.len(), 2);
        assert_eq!(
            paired[0].mmproj_path,
            Some(PathBuf::from("/m/mmproj-alpha-Q4.gguf"))
        );
        assert_eq!(
            paired[1].mmproj_path,
            Some(PathBuf::from("/m/mmproj-beta-Q4.gguf"))
        );
    }

    #[test]
    fn a_model_with_no_projector_is_offered_as_text_only() {
        let files = vec![PathBuf::from("/m/text-only-Q4.gguf")];
        let paired = pair_models(&files, "downloaded");
        assert_eq!(paired.len(), 1);
        assert!(paired[0].mmproj_path.is_none());
    }

    #[test]
    fn catalog_ids_are_unique_and_resolvable() {
        for entry in CATALOG {
            assert_eq!(catalog_entry(entry.id).map(|e| e.id), Some(entry.id));
            assert!(
                entry.mmproj_file.contains("mmproj"),
                "{} must name a real projector so discovery pairs it correctly",
                entry.id
            );
        }
        let ids: std::collections::HashSet<_> = CATALOG.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), CATALOG.len(), "catalog ids must be unique");
    }

    #[test]
    fn hf_url_points_at_the_resolve_endpoint() {
        let url = hf_url("org/repo", "main", "model.gguf");
        assert!(url.starts_with("https://huggingface.co/org/repo/resolve/main/model.gguf"));
    }
}
