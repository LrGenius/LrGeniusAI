//! Which MLX models exist, where they are, and how to get one.
//!
//! The MLX counterpart to [`crate::llm_models`], and deliberately shaped like
//! it — same three sources in the same priority order, same `resolve_model`
//! contract — so the routes can treat the two backends symmetrically.
//!
//! The one structural difference drives everything else here: **an MLX model is
//! a directory, not a file.** A GGUF is one artifact (plus an mmproj); an MLX
//! model is a Hugging Face repo snapshot — `config.json`, one or more
//! safetensors shards, and the tokenizer files. So discovery looks for
//! directories containing a `config.json`, and downloading means enumerating a
//! repo's file list rather than fetching two known names.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// A model the user could download.
///
/// No sha256, for the same reason as the GGUF catalog: Hugging Face repos are
/// mutable, so a digest pinned at compile time would either go stale and block
/// legitimate downloads or be quietly ignored.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    pub id: &'static str,
    pub label: &'static str,
    /// Hugging Face repo, e.g. `mlx-community/gemma-4-e4b-it-4bit`.
    pub repo: &'static str,
    pub revision: &'static str,
    /// Directory name the snapshot is stored under. Kept explicit rather than
    /// derived from `repo` so that renaming an upstream repo does not orphan
    /// everyone's already-downloaded copy.
    pub dir_name: &'static str,
    /// Approximate total download, for the UI to show before committing.
    pub approx_bytes: u64,
    /// Rough floor for comfortable use; the UI warns below it.
    pub min_ram_gb: u32,
}

/// Deliberately short, and every entry must be an **ungated** repo — a gated
/// one needs a Hugging Face token the plugin cannot supply.
///
/// `approx_bytes` is the measured sum of the files this code actually fetches
/// (i.e. after `is_skippable_repo_file`), not a round estimate, so the progress
/// bar's denominator matches what is downloaded.
///
/// All are vision models: MLX is offered for photo analysis, and a text-only
/// entry here would look selectable and then fail on the first photo. Text-only
/// MLX models still work if the user points `LRG_MLX_MODEL_DIR` at one, which
/// is enough for keyword clustering.
pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: "mlx-gemma4-e2b",
        label: "Gemma 4 E2B (fast, lower quality)",
        repo: "mlx-community/gemma-4-e2b-it-4bit",
        revision: "main",
        dir_name: "gemma-4-e2b-it-4bit",
        approx_bytes: 3_583_086_498,
        min_ram_gb: 8,
    },
    CatalogEntry {
        id: "mlx-gemma4-e4b",
        label: "Gemma 4 E4B (recommended)",
        repo: "mlx-community/gemma-4-e4b-it-4bit",
        revision: "main",
        dir_name: "gemma-4-e4b-it-4bit",
        approx_bytes: 5_179_239_349,
        min_ram_gb: 16,
    },
    CatalogEntry {
        id: "mlx-gemma4-12b",
        label: "Gemma 4 12B (highest quality, needs more RAM)",
        // Google's quantization-aware-trained 4-bit release, same rationale as
        // the GGUF QAT build in llm_models.rs. Larger on disk than that GGUF
        // (11.0 GB vs. 7.15 GB) for the same parameter count, so it gets a
        // higher RAM floor here.
        repo: "mlx-community/gemma-4-12B-it-qat-4bit",
        revision: "main",
        dir_name: "gemma-4-12B-it-qat-4bit",
        approx_bytes: 11_020_138_609,
        min_ram_gb: 32,
    },
    CatalogEntry {
        id: "mlx-ministral3-8b",
        label: "Ministral 3 8B (balanced alternative to Gemma)",
        // `model_type: mistral3` with a Pixtral vision tower, which
        // mlx-swift-lm's VLM factory registers, so the sidecar loads it as a
        // vision model rather than falling back to the text-only factory.
        repo: "mlx-community/Ministral-3-8B-Instruct-2512-4bit",
        revision: "main",
        dir_name: "Ministral-3-8B-Instruct-2512-4bit",
        approx_bytes: 5_630_652_158,
        min_ram_gb: 16,
    },
    CatalogEntry {
        id: "mlx-qwen3vl-4b",
        label: "Qwen3-VL 4B (balanced alternative to Gemma)",
        repo: "lmstudio-community/Qwen3-VL-4B-Instruct-MLX-4bit",
        revision: "main",
        dir_name: "Qwen3-VL-4B-Instruct-MLX-4bit",
        approx_bytes: 3_109_735_659,
        min_ram_gb: 8,
    },
];

#[must_use]
pub fn catalog_entry(id: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|e| e.id == id)
}

/// A usable MLX model found on disk.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LocalModel {
    /// What the plugin sends back as `model` — the directory name, which is
    /// also what the user sees.
    pub name: String,
    pub model_dir: PathBuf,
    /// Where it was found, for the UI.
    pub source: &'static str,
}

/// `true` if `dir` looks like an MLX model snapshot.
///
/// `config.json` plus at least one safetensors shard is the minimum any of the
/// factories in mlx-swift-lm can load. Checking for the weights as well as the
/// config matters: an interrupted download leaves the small JSON files behind
/// long before the multi-gigabyte shards arrive, and offering that as a
/// loadable model produces a baffling failure minutes later.
#[must_use]
pub fn is_model_dir(dir: &Path) -> bool {
    if !dir.join("config.json").is_file() {
        return false;
    }
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "safetensors")
        })
    })
}

fn model_dirs_in(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((current, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if is_model_dir(&path) {
                found.push(path);
            } else if depth < max_depth {
                stack.push((path, depth + 1));
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

fn name_of(dir: &Path) -> String {
    dir.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Every MLX model we can offer without downloading anything.
///
/// LM Studio nests models as `<publisher>/<repo>/`, and it has shipped an MLX
/// engine on Apple silicon for a long time, so a user who already pulled an MLX
/// model there needs no second copy. The Hugging Face CLI cache is scanned too:
/// `huggingface-cli download` is the most common way to get one of these by
/// hand, and its `models--org--repo/snapshots/<sha>/` layout is exactly what
/// the depth allowance is for.
#[must_use]
pub fn discover_local_models() -> Vec<LocalModel> {
    let paths = lrg_ml::model_paths::resolve_mlx();
    let mut found = Vec::new();

    // An explicit override wins and is always offered, even outside any
    // scanned directory.
    if let Some(model_dir) = paths.model_dir.filter(|p| is_model_dir(p)) {
        found.push(LocalModel {
            name: name_of(&model_dir),
            model_dir,
            source: "env",
        });
    }

    for dir in model_dirs_in(&paths.dir, 1) {
        found.push(LocalModel {
            name: name_of(&dir),
            model_dir: dir,
            source: "downloaded",
        });
    }

    if let Some(home) = home() {
        let lmstudio = home.join(".lmstudio").join("models");
        if lmstudio.is_dir() {
            for dir in model_dirs_in(&lmstudio, 3) {
                found.push(LocalModel {
                    name: name_of(&dir),
                    model_dir: dir,
                    source: "lmstudio",
                });
            }
        }
        let hf = home.join(".cache").join("huggingface").join("hub");
        if hf.is_dir() {
            for dir in model_dirs_in(&hf, 3) {
                found.push(LocalModel {
                    name: name_of(&dir),
                    model_dir: dir,
                    source: "huggingface",
                });
            }
        }
    }

    // A model reachable by two routes should still appear once.
    found.dedup_by(|a, b| a.model_dir == b.model_dir);
    found
}

/// Find the model the request asked for.
///
/// Matched against the directory name, then against a full path, so both "what
/// the dropdown showed" and "a path the user typed" work. An empty name takes
/// the first available model, which is what makes `LRG_MLX_MODEL_DIR` usable
/// with no UI at all.
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
        .or_else(|| available.iter().find(|m| m.model_dir == Path::new(name)))
        .cloned()
}

/// Where a catalog entry's snapshot ends up.
#[must_use]
pub fn destination_for(entry: &CatalogEntry) -> PathBuf {
    lrg_ml::model_paths::resolve_mlx().dir.join(entry.dir_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_are_unique_and_resolvable() {
        for entry in CATALOG {
            assert_eq!(catalog_entry(entry.id).map(|e| e.id), Some(entry.id));
        }
        let ids: std::collections::HashSet<_> = CATALOG.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), CATALOG.len(), "catalog ids must be unique");
    }

    /// The ids must not collide with the GGUF catalog's: both appear in the
    /// same plugin dropdown, and `/v1/llm/downloads` picks an entry by id
    /// alone.
    #[test]
    fn catalog_ids_do_not_collide_with_the_gguf_catalog() {
        let gguf: std::collections::HashSet<_> =
            crate::llm_models::CATALOG.iter().map(|e| e.id).collect();
        for entry in CATALOG {
            assert!(
                !gguf.contains(entry.id),
                "{} is ambiguous across the two catalogs",
                entry.id
            );
        }
    }

    #[test]
    fn catalog_files_land_in_the_discovery_directory() {
        let root = lrg_ml::model_paths::resolve_mlx().dir;
        for entry in CATALOG {
            // Discovery scans this root, so a downloaded model must appear in
            // `/models` without any extra bookkeeping.
            assert_eq!(destination_for(entry).parent(), Some(root.as_path()));
        }
    }

    #[test]
    fn a_directory_needs_both_a_config_and_weights_to_count() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("model");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!is_model_dir(&dir), "an empty directory is not a model");

        std::fs::write(dir.join("config.json"), "{}").unwrap();
        assert!(
            !is_model_dir(&dir),
            "config.json alone is what a half-finished download looks like"
        );

        std::fs::write(dir.join("model.safetensors"), b"weights").unwrap();
        assert!(is_model_dir(&dir));
    }

    #[test]
    fn discovery_descends_into_nested_layouts() {
        let temp = tempfile::tempdir().unwrap();
        // The Hugging Face cache layout: models--org--repo/snapshots/<sha>/.
        let nested = temp
            .path()
            .join("models--mlx-community--gemma-4-e4b-it-4bit")
            .join("snapshots")
            .join("abc123");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("config.json"), "{}").unwrap();
        std::fs::write(nested.join("model.safetensors"), b"w").unwrap();

        let found = model_dirs_in(temp.path(), 3);
        assert_eq!(found, vec![nested]);
    }

    /// Once a directory is recognised as a model, its subdirectories must not
    /// be searched again — otherwise a model containing, say, an `onnx/`
    /// export would be offered twice.
    #[test]
    fn discovery_does_not_descend_into_a_model_it_already_found() {
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("outer");
        let inner = outer.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        for dir in [&outer, &inner] {
            std::fs::write(dir.join("config.json"), "{}").unwrap();
            std::fs::write(dir.join("model.safetensors"), b"w").unwrap();
        }

        assert_eq!(model_dirs_in(temp.path(), 3), vec![outer]);
    }
}
