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
    /// A name this model used to be offered under, still accepted by
    /// [`resolve_model`] so a choice the plugin saved earlier keeps working.
    /// Only the Hugging Face cache has one: its models used to be named after
    /// the snapshot hash.
    #[serde(skip)]
    pub alias: Option<String>,
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

/// `true` for a directory discovery must never offer: a download still being
/// staged (`.<name>.part`, or `<name>.part` from before staging moved to a
/// hidden name) or anything else hidden.
///
/// A staged snapshot satisfies [`is_model_dir`] as soon as `config.json` and
/// the first shard have arrived, so without this check a half-finished
/// download appeared as an installed model.
fn is_staging_or_hidden(dir: &Path) -> bool {
    let name = name_of(dir);
    name.starts_with('.') || name.ends_with(".part")
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
            if !path.is_dir() || is_staging_or_hidden(&path) {
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

/// The repo a Hugging Face cache snapshot belongs to, as `(org, name)`.
///
/// The cache stores `org/name` as `models--org--name/snapshots/<sha>/`. Only
/// the `--` after `models` and the first `--` after the org are separators:
/// Hugging Face does not allow `--` inside an org name, but a repo name may
/// contain it.
fn hf_cache_repo(snapshot: &Path) -> Option<(String, String)> {
    let snapshots = snapshot.parent()?;
    if snapshots.file_name()? != "snapshots" {
        return None;
    }
    let repo_dir = name_of(snapshots.parent()?);
    let rest = repo_dir.strip_prefix("models--")?;
    let (org, name) = rest.split_once("--")?;
    (!org.is_empty() && !name.is_empty()).then(|| (org.to_string(), name.to_string()))
}

/// The snapshot `refs/main` points at, for a snapshot inside the cache.
fn hf_cache_main_snapshot(snapshot: &Path) -> Option<String> {
    let repo_dir = snapshot.parent()?.parent()?;
    let sha = std::fs::read_to_string(repo_dir.join("refs").join("main")).ok()?;
    Some(sha.trim().to_string())
}

/// Hugging Face cache snapshots, one per repo, named after the repo.
///
/// A snapshot directory is named after its commit hash, which is what the
/// plugin used to show — `mlx: 4f1c…` tells nobody which model that is. The
/// repo's name part is used instead, the same name a download of that repo
/// gets, so the catalog's "installed" check recognises it too. When the cache
/// holds several snapshots of one repo, the one `refs/main` points at wins;
/// the others are older revisions of the same model.
fn hf_cache_models(root: &Path) -> Vec<LocalModel> {
    let mut by_repo: std::collections::BTreeMap<(String, String), PathBuf> =
        std::collections::BTreeMap::new();
    for dir in model_dirs_in(root, 3) {
        let Some(repo) = hf_cache_repo(&dir) else {
            // Something the user placed in the cache by hand; offer it under
            // its own name.
            by_repo.insert((String::new(), dir.display().to_string()), dir);
            continue;
        };
        let is_main = hf_cache_main_snapshot(&dir).is_some_and(|sha| sha == name_of(&dir));
        match by_repo.get(&repo) {
            Some(_) if !is_main => {}
            _ => {
                by_repo.insert(repo, dir);
            }
        }
    }
    by_repo
        .into_iter()
        .map(|((org, name), dir)| {
            if org.is_empty() {
                LocalModel {
                    name: name_of(&dir),
                    model_dir: dir,
                    source: "huggingface",
                    alias: None,
                }
            } else {
                LocalModel {
                    name,
                    alias: Some(name_of(&dir)),
                    model_dir: dir,
                    source: "huggingface",
                }
            }
        })
        .collect()
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
            alias: None,
        });
    }

    for dir in model_dirs_in(&paths.dir, 1) {
        found.push(LocalModel {
            name: name_of(&dir),
            model_dir: dir,
            source: "downloaded",
            alias: None,
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
                    alias: None,
                });
            }
        }
        let hf = home.join(".cache").join("huggingface").join("hub");
        if hf.is_dir() {
            found.extend(hf_cache_models(&hf));
        }
    }

    dedup_by_dir(found)
}

/// Drop every model whose directory was already found by an earlier, higher
/// priority source — `Vec::dedup_by` only merges neighbours, and the same
/// directory can be reached from sources that are not adjacent in the list
/// (an `LRG_MLX_MODEL_DIR` pointing into the LM Studio folder, say).
fn dedup_by_dir(found: Vec<LocalModel>) -> Vec<LocalModel> {
    let mut seen = std::collections::HashSet::new();
    found
        .into_iter()
        .filter(|model| seen.insert(model.model_dir.clone()))
        .collect()
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
        .or_else(|| available.iter().find(|m| m.alias.as_deref() == Some(name)))
        .or_else(|| available.iter().find(|m| m.model_dir == Path::new(name)))
        .cloned()
}

/// Where a catalog entry's snapshot ends up.
#[must_use]
pub fn destination_for(entry: &CatalogEntry) -> PathBuf {
    lrg_ml::model_paths::resolve_mlx().dir.join(entry.dir_name)
}

/// Where a snapshot bound for `destination` is assembled before it is moved
/// into place: a hidden sibling, which discovery skips.
///
/// This used to be `destination.with_extension("part")`, which treats
/// everything after the last dot as an extension — `Qwen2.5-VL-7B-Instruct-4bit`
/// staged into `Qwen2.part`, so two such models collided, and the visible name
/// let discovery offer the half-finished download as an installed model.
#[must_use]
pub fn staging_for(destination: &Path) -> PathBuf {
    destination.with_file_name(format!(".{}.part", name_of(destination)))
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

    fn write_model(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        std::fs::write(dir.join("model.safetensors"), b"w").unwrap();
    }

    /// A name with a dot in it used to stage into `<prefix>.part`, so two such
    /// models collided and the staging directory showed up as installed.
    #[test]
    fn staging_keeps_the_whole_name_and_is_hidden() {
        let destination = Path::new("/models/mlx/Qwen2.5-VL-7B-Instruct-4bit");
        assert_eq!(
            staging_for(destination),
            Path::new("/models/mlx/.Qwen2.5-VL-7B-Instruct-4bit.part")
        );
    }

    #[test]
    fn discovery_skips_downloads_still_being_staged() {
        let temp = tempfile::tempdir().unwrap();
        let finished = temp.path().join("gemma-4-e4b-it-4bit");
        write_model(&finished);
        // Both the current hidden staging name and the old visible one.
        write_model(&staging_for(
            &temp.path().join("Qwen2.5-VL-7B-Instruct-4bit"),
        ));
        write_model(&temp.path().join("Qwen2.part"));

        assert_eq!(model_dirs_in(temp.path(), 1), vec![finished]);
    }

    #[test]
    fn hugging_face_cache_models_are_named_after_their_repo() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp
            .path()
            .join("models--mlx-community--gemma-3-12b-it-qat-4bit");
        let old = repo.join("snapshots").join("1111");
        let main = repo.join("snapshots").join("2222");
        write_model(&old);
        write_model(&main);
        std::fs::create_dir_all(repo.join("refs")).unwrap();
        std::fs::write(repo.join("refs").join("main"), "2222\n").unwrap();

        let found = hf_cache_models(temp.path());
        assert_eq!(found.len(), 1, "one entry per repo, not per snapshot");
        assert_eq!(found[0].name, "gemma-3-12b-it-qat-4bit");
        assert_eq!(found[0].model_dir, main, "refs/main picks the snapshot");
        assert_eq!(found[0].alias.as_deref(), Some("2222"));
    }

    #[test]
    fn a_repo_name_may_contain_a_double_dash() {
        let snapshot = Path::new("/hub/models--org--name--with--dashes/snapshots/abc");
        assert_eq!(
            hf_cache_repo(snapshot),
            Some(("org".to_string(), "name--with--dashes".to_string()))
        );
        assert_eq!(hf_cache_repo(Path::new("/hub/plain/model")), None);
    }

    /// The same directory reached through two sources that are not neighbours
    /// in the list must still be offered once, from the first source.
    #[test]
    fn duplicates_are_dropped_even_when_not_adjacent() {
        let model = |name: &str, dir: &str, source| LocalModel {
            name: name.to_string(),
            model_dir: PathBuf::from(dir),
            source,
            alias: None,
        };
        let found = dedup_by_dir(vec![
            model("a", "/x/a", "env"),
            model("b", "/x/b", "downloaded"),
            model("a", "/x/a", "lmstudio"),
        ]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].source, "env");
    }
}
