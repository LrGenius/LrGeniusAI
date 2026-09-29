//! Checking a Hugging Face model the user typed in, before anything is
//! downloaded.
//!
//! The curated catalogs ([`crate::mlx_models`], [`crate::llm_models`]) only
//! list combinations that have been run. "Other model from Hugging Face…" lets
//! the user pick any repo instead, and a multi-gigabyte download that then
//! fails on the first photo is the worst way to learn that it cannot work. So
//! everything that can be known from a few small requests is checked first:
//! the repo exists and is not gated, it holds the right kind of weights, the
//! MLX engine knows its architecture and image processor, the tokenizer and
//! chat template are there, and — for a GGUF — which quantization and which
//! vision projector to take.
//!
//! The assessment is a pure function of the repo's metadata, its file list and
//! a few small JSON files ([`assess_mlx`], [`assess_gguf`]); [`check_repo`]
//! only fetches them. The Hugging Face address can be overridden with
//! `HF_ENDPOINT`, like the Hugging Face tools themselves, which is also how the
//! tests reach a fake.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::routes::llm::is_safe_repo_path;

pub const DEFAULT_HF_ENDPOINT: &str = "https://huggingface.co";

/// The Hugging Face server: `HF_ENDPOINT`, or huggingface.co.
#[must_use]
pub fn hf_endpoint() -> String {
    std::env::var("HF_ENDPOINT")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_HF_ENDPOINT.to_string())
}

/// Which local engine a model is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Mlx,
    Llamacpp,
}

/// Model types the pinned mlx-swift-lm can run as a **vision** model.
///
/// Copied from `VLMTypeRegistry` at the commit `native/mlx-sidecar/Package.swift`
/// pins (c97539da). Update it with every pin bump: a type missing here is
/// refused before download; a type listed here that the pin cannot run fails
/// only after it.
pub const MLX_VISION_MODEL_TYPES: &[&str] = &[
    "paligemma",
    "qwen2_vl",
    "qwen2_5_vl",
    "qwen3_vl",
    "qwen3_vl_moe",
    "qwen3_5",
    "qwen3_5_moe",
    "idefics3",
    "smolvlm",
    "gemma3",
    "gemma4",
    "gemma4_unified",
    "fastvlm",
    "llava_qwen2",
    "pixtral",
    "mistral3",
    "lfm2_vl",
    "lfm2-vl",
    "glm_ocr",
];

/// Image processors the same pin registers (`VLMProcessorTypeRegistry`).
pub const MLX_VISION_PROCESSORS: &[&str] = &[
    "PaliGemmaProcessor",
    "Qwen2VLProcessor",
    "Qwen2_5_VLProcessor",
    "Qwen3VLProcessor",
    "Idefics3Processor",
    "Gemma3Processor",
    "Gemma4Processor",
    "Gemma4UnifiedProcessor",
    "SmolVLMProcessor",
    "FastVLMProcessor",
    "PixtralProcessor",
    "Mistral3Processor",
    "Lfm2VlProcessor",
    "Glm46VProcessor",
];

/// Model types whose processor mlx-swift-lm picks itself, whatever the repo's
/// processor config names.
const MLX_PROCESSOR_OVERRIDES: &[&str] = &["mistral3", "gemma4_unified"];

/// Repos known to download fine and then fail to load, with the reason. See
/// "Known model incompatibilities" in `native/mlx-sidecar/README.md`.
const KNOWN_INCOMPATIBLE: &[(&str, &str)] = &[(
    "mlx-community/SmolVLM-256M-Instruct-8bit",
    "its vision tower does not match what the bundled MLX engine expects (shape mismatch on load)",
)];

/// Quantizations tried, in order, when a GGUF repo is given without one.
/// Q4_K_M is llama.cpp's own default for `-hf`.
const GGUF_QUANT_PREFERENCE: &[&str] = &[
    "q4_k_m", "q4_k_s", "q4_0", "iq4_xs", "iq4_nl", "q5_k_m", "q5_k_s", "q6_k", "q8_0",
];

/// What the user typed, taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInput {
    /// `org/name`.
    pub repo: String,
    pub revision: Option<String>,
    /// GGUF only: the quantization after a `:`, as in `llama-server -hf`.
    pub quant: Option<String>,
}

fn is_name_part(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 96
        && part != "."
        && part != ".."
        && !part.contains("..")
        && part
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Read a model name or a model page address.
///
/// Accepts `org/name`, `org/name:Q5_K_M`, `org/name@revision` and the
/// addresses the Hugging Face site shows (`https://huggingface.co/org/name`,
/// with `/tree/<rev>` or a file path after it, and `hf.co/…`).
pub fn parse_repo_input(input: &str) -> Result<RepoInput, String> {
    let original = input.trim();
    let not_a_name = || {
        format!(
            "\"{original}\" is not a Hugging Face model name. Paste it as org/name — for \
             example mlx-community/gemma-3-12b-it-qat-4bit — or paste the model page's address."
        )
    };
    let mut text = original;
    for prefix in [
        "https://huggingface.co/",
        "http://huggingface.co/",
        "https://www.huggingface.co/",
        "huggingface.co/",
        "https://hf.co/",
        "http://hf.co/",
        "hf.co/",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest;
            break;
        }
    }
    let text = text.split(['?', '#']).next().unwrap_or(text);
    let segments: Vec<&str> = text.split('/').filter(|s| !s.is_empty()).collect();
    if segments.len() < 2 {
        return Err(not_a_name());
    }
    let org = segments[0];
    let mut name = segments[1];
    let mut revision = None;
    if segments.len() > 2 {
        // /tree/<rev>, /blob/<rev>/file, /resolve/<rev>/file — anything else
        // after the name is not a model address.
        match (segments[2], segments.get(3)) {
            ("tree" | "blob" | "resolve", Some(rev)) => revision = Some((*rev).to_string()),
            _ => return Err(not_a_name()),
        }
    }
    let mut quant = None;
    if let Some((base, rev)) = name.split_once('@') {
        name = base;
        revision = Some(rev.to_string());
    }
    if let Some((base, q)) = name.split_once(':') {
        name = base;
        quant = Some(q.to_string()).filter(|q| !q.is_empty());
    }
    if !is_name_part(org) || !is_name_part(name) {
        return Err(not_a_name());
    }
    if let Some(rev) = &revision {
        if !is_name_part(rev) {
            return Err(not_a_name());
        }
    }
    if let Some(q) = &quant {
        if !q
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        {
            return Err(not_a_name());
        }
    }
    Ok(RepoInput {
        repo: format!("{org}/{name}"),
        revision,
        quant,
    })
}

/// One file to fetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepoFile {
    /// Path inside the repo.
    pub path: String,
    pub size: u64,
}

/// The result of a check: what would be downloaded, and what to tell the user.
#[derive(Debug, Clone, Serialize)]
pub struct RepoCheck {
    pub engine: Engine,
    pub repo: String,
    /// The commit the download is pinned to, so the files checked are the
    /// files fetched.
    pub revision: String,
    /// The folder the model is stored in.
    pub dir_name: String,
    /// The name the model is offered under once downloaded — the `model` to
    /// send with provider `mlx` or `llamacpp`.
    pub installed_name: String,
    pub files: Vec<RepoFile>,
    pub approx_bytes: u64,
    /// A rough estimate of the memory the model needs while running.
    pub est_ram_gb: f64,
    /// MLX: the architecture (`gemma3`).
    pub model_type: Option<String>,
    /// GGUF: the quantization picked (`Q4_K_M`).
    pub quant: Option<String>,
    pub already_installed: bool,
    pub warnings: Vec<String>,
}

/// Why a check failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// The repo cannot be used; the message says why and what to do instead.
    Unusable(String),
    /// Hugging Face could not be asked.
    Unreachable(String),
}

impl CheckError {
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            CheckError::Unusable(m) | CheckError::Unreachable(m) => m,
        }
    }
}

fn unusable(message: impl Into<String>) -> CheckError {
    CheckError::Unusable(message.into())
}

/// Refuse gated, private and disabled repos: their files need a signed-in
/// Hugging Face account, which the plugin cannot supply.
fn check_access(repo: &str, info: &Value) -> Result<(), CheckError> {
    let gated = match info.get("gated") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.is_empty() && s != "false",
        Some(_) => true,
    };
    if gated {
        return Err(unusable(format!(
            "{repo} is gated: Hugging Face only hands out its files after you accept its licence \
             and sign in, which LrGeniusAI cannot do. Look for an ungated copy — mlx-community, \
             lmstudio-community and ggml-org publish most models without a gate."
        )));
    }
    if info.get("private").and_then(Value::as_bool) == Some(true)
        || info.get("disabled").and_then(Value::as_bool) == Some(true)
    {
        return Err(unusable(format!(
            "{repo} is private or disabled on Hugging Face, so it cannot be downloaded."
        )));
    }
    Ok(())
}

fn has_extension(path: &str, extension: &str) -> bool {
    path.to_ascii_lowercase()
        .ends_with(&format!(".{extension}"))
}

fn top_level(path: &str) -> bool {
    !path.contains('/')
}

fn gb(bytes: u64) -> f64 {
    bytes as f64 / 1e9
}

/// A rough memory estimate: the weights plus a fifth for activations and the
/// KV cache, plus the runtime itself. Only good for a warning.
fn estimate_ram_gb(weight_bytes: u64) -> f64 {
    let estimate = gb(weight_bytes) * 1.2 + 1.5;
    (estimate * 10.0).round() / 10.0
}

const UNTESTED_WARNING: &str = "This model is not in LrGeniusAI's tested list. The checks passed, \
    but how well it follows the answer format is only known once you try it on a few photos.";

/// Decide whether an MLX repo can work, from its metadata and small files.
///
/// `config` is `config.json`; `processor` the processor config
/// (`preprocessor_config.json`, else `processor_config.json`), when the repo
/// has one; `tokenizer_config` is `tokenizer_config.json`.
pub fn assess_mlx(
    repo: &str,
    info: &Value,
    files: &[RepoFile],
    config: &Value,
    processor: Option<&Value>,
    tokenizer_config: Option<&Value>,
) -> Result<AssessedMlx, CheckError> {
    check_access(repo, info)?;
    if let Some((_, why)) = KNOWN_INCOMPATIBLE
        .iter()
        .find(|(r, _)| r.eq_ignore_ascii_case(repo))
    {
        return Err(unusable(format!(
            "{repo} does not work with LrGeniusAI: {why}."
        )));
    }

    let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let has = |name: &str| names.contains(&name);
    let top_weights = names
        .iter()
        .any(|n| top_level(n) && has_extension(n, "safetensors"));
    if !top_weights {
        if names.iter().any(|n| has_extension(n, "gguf")) {
            return Err(unusable(format!(
                "{repo} is a GGUF model (for llama.cpp), not an MLX one. On a Mac, look for the \
                 model's MLX version — usually from mlx-community."
            )));
        }
        return Err(unusable(format!(
            "{repo} has no MLX weights (.safetensors files), so the MLX engine cannot load it."
        )));
    }
    let is_mlx = info.get("library_name").and_then(Value::as_str) == Some("mlx")
        || info
            .get("tags")
            .and_then(Value::as_array)
            .is_some_and(|tags| tags.iter().any(|t| t.as_str() == Some("mlx")))
        || config.get("quantization").is_some()
        || config.get("quantization_config").is_some();
    if !is_mlx {
        return Err(unusable(format!(
            "{repo} is not an MLX conversion: full-size weights like these are far too large to \
             run here. Look for the model's MLX version, usually mlx-community/<name>-4bit."
        )));
    }
    if !has("config.json") {
        return Err(unusable(format!("{repo} has no config.json.")));
    }
    for required in ["tokenizer.json", "tokenizer_config.json"] {
        if !has(required) {
            return Err(unusable(format!(
                "{repo} has no {required}, which the MLX engine needs to read and write text."
            )));
        }
    }

    let model_type = config
        .get("model_type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !MLX_VISION_MODEL_TYPES.contains(&model_type.as_str()) {
        let text_only = model_type.ends_with("_text")
            || !(has("preprocessor_config.json") || has("processor_config.json"));
        return Err(unusable(if text_only {
            format!(
                "{repo} is a text-only model (type \"{model_type}\"): it cannot look at photos. \
                 Pick a vision model — Gemma 3/4, Qwen2.5-VL, Qwen3-VL, Mistral 3 or SmolVLM."
            )
        } else {
            format!(
                "{repo} uses the model type \"{model_type}\", which the bundled MLX engine cannot \
                 run as a vision model. Supported types: {}.",
                MLX_VISION_MODEL_TYPES.join(", ")
            )
        }));
    }

    let mut warnings = Vec::new();
    if !MLX_PROCESSOR_OVERRIDES.contains(&model_type.as_str()) {
        let Some(processor) = processor else {
            return Err(unusable(format!(
                "{repo} has no image processor config (preprocessor_config.json or \
                 processor_config.json), so the MLX engine could only load it as text-only."
            )));
        };
        match processor.get("processor_class").and_then(Value::as_str) {
            Some(class) if MLX_VISION_PROCESSORS.contains(&class) => {}
            Some(class) => {
                return Err(unusable(format!(
                    "{repo} uses the image processor \"{class}\", which the bundled MLX engine \
                     does not have."
                )))
            }
            None => warnings.push(format!(
                "{repo} does not name its image processor; the MLX engine will guess it from the \
                 model type."
            )),
        }
    }

    let has_template = tokenizer_config
        .and_then(|t| t.get("chat_template"))
        .is_some_and(|t| !t.is_null())
        || has("chat_template.jinja")
        || has("chat_template.json");
    if !has_template {
        return Err(unusable(format!(
            "{repo} has no chat template, so the MLX engine cannot turn a request into a prompt."
        )));
    }

    // What is fetched: the files mlx-swift-lm itself would download, minus
    // extra exports some repos carry alongside.
    let wanted: Vec<RepoFile> = files
        .iter()
        .filter(|f| {
            let lower = f.path.to_ascii_lowercase();
            (has_extension(&lower, "safetensors")
                || has_extension(&lower, "json")
                || has_extension(&lower, "jinja"))
                && !lower.starts_with("original/")
                && !lower.starts_with("onnx/")
        })
        .cloned()
        .collect();
    let approx_bytes: u64 = wanted.iter().map(|f| f.size).sum();
    let weight_bytes: u64 = wanted
        .iter()
        .filter(|f| has_extension(&f.path, "safetensors"))
        .map(|f| f.size)
        .sum();
    warnings.push(UNTESTED_WARNING.to_string());

    Ok(AssessedMlx {
        files: wanted,
        approx_bytes,
        est_ram_gb: estimate_ram_gb(weight_bytes),
        model_type,
        warnings,
    })
}

/// What [`assess_mlx`] found.
#[derive(Debug, Clone)]
pub struct AssessedMlx {
    pub files: Vec<RepoFile>,
    pub approx_bytes: u64,
    pub est_ram_gb: f64,
    pub model_type: String,
    pub warnings: Vec<String>,
}

/// The quantization a GGUF file name carries (`Q4_K_M`), if any.
fn quant_of(path: &str) -> Option<String> {
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = file.strip_suffix(".gguf").unwrap_or(file);
    stem.split(['-', '.'])
        .rev()
        .find(|segment| crate::llm_models::is_quant_tag(&segment.to_ascii_lowercase()))
        .map(str::to_string)
}

fn is_split_part(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = file.strip_suffix(".gguf").unwrap_or(file);
    stem.rsplit_once("-of-").is_some_and(|(head, total)| {
        total.len() == 5
            && total.bytes().all(|b| b.is_ascii_digit())
            && head.rsplit_once('-').is_some_and(|(_, part)| {
                part.len() == 5 && part.bytes().all(|b| b.is_ascii_digit())
            })
    })
}

/// What [`assess_gguf`] found.
#[derive(Debug, Clone)]
pub struct AssessedGguf {
    pub model: RepoFile,
    pub mmproj: RepoFile,
    pub quant: String,
    pub approx_bytes: u64,
    pub est_ram_gb: f64,
    pub warnings: Vec<String>,
}

/// Decide which model file and which vision projector of a GGUF repo to
/// take, or why none will do.
pub fn assess_gguf(
    repo: &str,
    info: &Value,
    files: &[RepoFile],
    wanted_quant: Option<&str>,
) -> Result<AssessedGguf, CheckError> {
    check_access(repo, info)?;
    let ggufs: Vec<&RepoFile> = files
        .iter()
        .filter(|f| has_extension(&f.path, "gguf"))
        .collect();
    if ggufs.is_empty() {
        let is_mlx = files.iter().any(|f| has_extension(&f.path, "safetensors"));
        return Err(unusable(if is_mlx {
            format!(
                "{repo} has no GGUF files — it looks like an MLX or full-size model. For llama.cpp, \
                 look for the model's GGUF version (from ggml-org, lmstudio-community, unsloth or \
                 bartowski)."
            )
        } else {
            format!("{repo} has no GGUF files, so llama.cpp cannot load it.")
        }));
    }
    let (projectors, models): (Vec<&RepoFile>, Vec<&RepoFile>) = ggufs
        .into_iter()
        .partition(|f| f.path.to_ascii_lowercase().contains("mmproj"));
    if models.is_empty() {
        return Err(unusable(format!(
            "{repo} only holds a vision projector, no model."
        )));
    }

    let available: Vec<String> = {
        let mut quants: Vec<String> = models.iter().filter_map(|f| quant_of(&f.path)).collect();
        quants.sort();
        quants.dedup();
        quants
    };
    let with_quant = |quant: &str| -> Vec<&RepoFile> {
        let mut found: Vec<&RepoFile> = models
            .iter()
            .copied()
            .filter(|f| quant_of(&f.path).is_some_and(|q| q.eq_ignore_ascii_case(quant)))
            .collect();
        found.sort_by(|a, b| a.path.cmp(&b.path));
        found
    };
    let chosen: Vec<&RepoFile> = match wanted_quant {
        Some(quant) => {
            let found = with_quant(quant);
            if found.is_empty() {
                return Err(unusable(format!(
                    "{repo} has no {quant} version. Available: {}.",
                    if available.is_empty() {
                        "none named by quantization".to_string()
                    } else {
                        available.join(", ")
                    }
                )));
            }
            found
        }
        None => match GGUF_QUANT_PREFERENCE
            .iter()
            .map(|q| with_quant(q))
            .find(|found| !found.is_empty())
        {
            Some(found) => found,
            None if models.len() == 1 => models.clone(),
            None => {
                return Err(unusable(format!(
                    "{repo} has several versions and none of the usual 4-bit ones. Add the one \
                     you want after a colon, e.g. {repo}:{}.",
                    available.first().map_or("Q8_0", String::as_str)
                )))
            }
        },
    };
    // The first part of a split model sorts first; a lone file is its own
    // first part.
    let model = chosen[0];
    if is_split_part(&model.path) {
        return Err(unusable(format!(
            "The {} version of {repo} is split into several files, which LrGeniusAI cannot \
             download yet. Pick a smaller quantization, e.g. {repo}:Q4_K_M.",
            quant_of(&model.path).unwrap_or_default()
        )));
    }

    let mmproj = {
        let mut sorted = projectors.clone();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));
        let lower = |f: &&RepoFile| f.path.to_ascii_lowercase();
        sorted
            .iter()
            .find(|f| lower(f).contains("f16") && !lower(f).contains("bf16"))
            .or_else(|| sorted.iter().find(|f| lower(f).contains("bf16")))
            .or_else(|| sorted.first())
            .copied()
    };
    let Some(mmproj) = mmproj else {
        return Err(unusable(format!(
            "{repo} has no vision projector (mmproj file), so the model cannot look at photos. \
             Look for a repo of this model that includes one — ggml-org and lmstudio-community \
             usually do."
        )));
    };

    let approx_bytes = model.size + mmproj.size;
    Ok(AssessedGguf {
        model: model.clone(),
        mmproj: mmproj.clone(),
        quant: quant_of(&model.path).unwrap_or_else(|| "unknown".to_string()),
        approx_bytes,
        est_ram_gb: estimate_ram_gb(model.size) + (gb(mmproj.size) * 10.0).round() / 10.0,
        warnings: vec![UNTESTED_WARNING.to_string()],
    })
}

/// Written next to a downloaded MLX model, so a later check can tell "already
/// installed" from "a different model with the same folder name".
pub const SOURCE_MARKER: &str = ".lrgenius-source.json";

fn marker_repo(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(SOURCE_MARKER)).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value
        .get("repo")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The folder a repo goes into under `root`: its name, or `org--name` when a
/// different model already has that name. `bool` is "already installed".
///
/// A folder whose source marker names this repo is reused whether or not the
/// model in it is complete: that is a download of the same repo that was
/// interrupted, and retrying into a second folder would leave the first one
/// behind — for a GGUF whose model file arrived but whose projector did not,
/// as a text-only copy of the model under the same name.
fn folder_for(root: &Path, repo: &str, installed: impl Fn(&Path) -> bool) -> (String, bool) {
    let (org, name) = repo.split_once('/').unwrap_or(("", repo));
    // Whether `dir` can hold this repo, and if so whether it already does.
    let usable = |dir: &Path| -> Option<bool> {
        if !dir.exists() {
            return Some(false);
        }
        match marker_repo(dir) {
            Some(marked) if marked.eq_ignore_ascii_case(repo) => Some(installed(dir)),
            Some(_) => None,
            // Unmarked: placed by hand or by an older version. Only a complete
            // model is taken to be this one; anything else is left alone.
            None => installed(dir).then_some(true),
        }
    };
    if let Some(done) = usable(&root.join(name)) {
        return (name.to_string(), done);
    }
    let qualified = format!("{org}--{name}");
    let done = usable(&root.join(&qualified)).unwrap_or(false);
    (qualified, done)
}

/// Record which repo a model folder holds; see [`folder_for`].
pub fn write_source_marker(dir: &Path, repo: &str, revision: &str) -> std::io::Result<()> {
    let marker = serde_json::json!({ "repo": repo, "revision": revision });
    std::fs::write(dir.join(SOURCE_MARKER), marker.to_string())
}

async fn get_json(client: &reqwest::Client, url: &str) -> Result<(u16, Value), CheckError> {
    let response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| {
            CheckError::Unreachable(format!(
                "Could not reach Hugging Face to check the model: {e}"
            ))
        })?;
    let status = response.status().as_u16();
    let value = response.json::<Value>().await.unwrap_or(Value::Null);
    Ok((status, value))
}

/// Fetch a small JSON file of the repo at `revision`; `None` if it is absent
/// or unreadable.
async fn repo_json(
    client: &reqwest::Client,
    endpoint: &str,
    repo: &str,
    revision: &str,
    file: &str,
) -> Result<Option<Value>, CheckError> {
    let url = crate::llm_models::hf_url_at(endpoint, repo, revision, file);
    let (status, value) = get_json(client, &url).await?;
    Ok((status == 200 && !value.is_null()).then_some(value))
}

/// Check `input` as a model for `engine`: fetch what is needed from Hugging
/// Face at `endpoint` and assess it.
///
/// `revision` pins the check to a commit — the download passes the one its
/// check returned, so what is fetched is what was checked.
pub async fn check_repo(
    client: &reqwest::Client,
    endpoint: &str,
    input: &str,
    engine: Engine,
    revision: Option<&str>,
) -> Result<RepoCheck, CheckError> {
    let mut parsed = parse_repo_input(input).map_err(CheckError::Unusable)?;
    if let Some(revision) = revision.map(str::trim).filter(|r| !r.is_empty()) {
        if !is_name_part(revision) {
            return Err(unusable(format!("\"{revision}\" is not a valid revision.")));
        }
        parsed.revision = Some(revision.to_string());
    }
    if engine == Engine::Mlx && parsed.quant.is_some() {
        return Err(unusable(
            "A quantization after a colon only applies to GGUF models. For MLX, the \
             quantization is part of the repo's name (…-4bit, …-8bit).",
        ));
    }
    let repo = &parsed.repo;
    let revision = parsed.revision.as_deref().unwrap_or("main");

    let (status, info) = get_json(
        client,
        &format!("{endpoint}/api/models/{repo}/revision/{revision}"),
    )
    .await?;
    match status {
        200 => {}
        401 | 403 | 404 => {
            return Err(unusable(format!(
                "Hugging Face has no public model named {repo}{}. Check the name, or paste the \
                 model page's address.",
                parsed
                    .revision
                    .as_deref()
                    .map(|r| format!(" at revision {r}"))
                    .unwrap_or_default()
            )))
        }
        s => {
            return Err(CheckError::Unreachable(format!(
                "Hugging Face could not answer for {repo} (HTTP {s}). Try again later."
            )))
        }
    }
    let sha = info
        .get("sha")
        .and_then(Value::as_str)
        .unwrap_or(revision)
        .to_string();
    // The canonical spelling, in case the user typed different case.
    let repo = info
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| id.eq_ignore_ascii_case(repo))
        .unwrap_or(repo)
        .to_string();

    let (status, tree) = get_json(
        client,
        &format!("{endpoint}/api/models/{repo}/tree/{sha}?recursive=true"),
    )
    .await?;
    if status != 200 {
        return Err(CheckError::Unreachable(format!(
            "Hugging Face could not list the files of {repo} (HTTP {status})."
        )));
    }
    let files: Vec<RepoFile> = tree
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("file"))
        .filter_map(|entry| {
            Some(RepoFile {
                path: entry.get("path").and_then(Value::as_str)?.to_string(),
                size: entry.get("size").and_then(Value::as_u64).unwrap_or(0),
            })
        })
        .collect();
    if let Some(bad) = files.iter().find(|f| !is_safe_repo_path(&f.path)) {
        return Err(unusable(format!(
            "{repo} lists a file at an unsafe path ({}); refusing to download it.",
            bad.path
        )));
    }

    match engine {
        Engine::Mlx => {
            let config = repo_json(client, endpoint, &repo, &sha, "config.json")
                .await?
                .unwrap_or(Value::Null);
            let has = |name: &str| files.iter().any(|f| f.path == name);
            let mut processor = None;
            for name in ["preprocessor_config.json", "processor_config.json"] {
                if has(name) {
                    let value = repo_json(client, endpoint, &repo, &sha, name).await?;
                    let names_class = value
                        .as_ref()
                        .is_some_and(|v| v.get("processor_class").is_some());
                    if processor.is_none() || names_class {
                        processor = value;
                    }
                    if names_class {
                        break;
                    }
                }
            }
            let tokenizer_config = if has("tokenizer_config.json") {
                repo_json(client, endpoint, &repo, &sha, "tokenizer_config.json").await?
            } else {
                None
            };
            let assessed = assess_mlx(
                &repo,
                &info,
                &files,
                &config,
                processor.as_ref(),
                tokenizer_config.as_ref(),
            )?;
            let root = lrg_ml::model_paths::resolve_mlx().dir;
            let (dir_name, already_installed) =
                folder_for(&root, &repo, crate::mlx_models::is_model_dir);
            Ok(RepoCheck {
                engine,
                repo,
                revision: sha,
                installed_name: dir_name.clone(),
                dir_name,
                files: assessed.files,
                approx_bytes: assessed.approx_bytes,
                est_ram_gb: assessed.est_ram_gb,
                model_type: Some(assessed.model_type),
                quant: None,
                already_installed,
                warnings: assessed.warnings,
            })
        }
        Engine::Llamacpp => {
            let assessed = assess_gguf(&repo, &info, &files, parsed.quant.as_deref())?;
            let root = lrg_ml::model_paths::resolve_llm().dir;
            let model_name = file_name(&assessed.model.path);
            let mmproj_name = file_name(&assessed.mmproj.path);
            let (dir_name, already_installed) = folder_for(&root, &repo, |dir| {
                dir.join(&model_name).is_file() && dir.join(&mmproj_name).is_file()
            });
            Ok(RepoCheck {
                engine,
                repo,
                revision: sha,
                dir_name,
                installed_name: model_name,
                files: vec![assessed.model, assessed.mmproj],
                approx_bytes: assessed.approx_bytes,
                est_ram_gb: assessed.est_ram_gb,
                model_type: None,
                quant: Some(assessed.quant),
                already_installed,
                warnings: assessed.warnings,
            })
        }
    }
}

/// The last path component of a repo path.
#[must_use]
pub fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Where a checked model's files go.
#[must_use]
pub fn destination_of(check: &RepoCheck) -> PathBuf {
    match check.engine {
        Engine::Mlx => lrg_ml::model_paths::resolve_mlx().dir.join(&check.dir_name),
        Engine::Llamacpp => lrg_ml::model_paths::resolve_llm().dir.join(&check.dir_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(path: &str, size: u64) -> RepoFile {
        RepoFile {
            path: path.to_string(),
            size,
        }
    }

    fn input(repo: &str, revision: Option<&str>, quant: Option<&str>) -> RepoInput {
        RepoInput {
            repo: repo.to_string(),
            revision: revision.map(str::to_string),
            quant: quant.map(str::to_string),
        }
    }

    #[test]
    fn names_and_page_addresses_are_understood() {
        for (text, expected) in [
            (
                "mlx-community/gemma-3-12b-it-qat-4bit",
                input("mlx-community/gemma-3-12b-it-qat-4bit", None, None),
            ),
            (
                " https://huggingface.co/mlx-community/gemma-3-12b-it-qat-4bit ",
                input("mlx-community/gemma-3-12b-it-qat-4bit", None, None),
            ),
            (
                "https://huggingface.co/ggml-org/gemma-3-12b-it-GGUF/tree/main",
                input("ggml-org/gemma-3-12b-it-GGUF", Some("main"), None),
            ),
            (
                "https://huggingface.co/ggml-org/gemma-3-12b-it-GGUF/blob/main/gemma-3-12b-it-Q4_K_M.gguf",
                input("ggml-org/gemma-3-12b-it-GGUF", Some("main"), None),
            ),
            (
                "hf.co/unsloth/gemma-3-12b-it-GGUF:Q5_K_M",
                input("unsloth/gemma-3-12b-it-GGUF", None, Some("Q5_K_M")),
            ),
            (
                "Qwen/Qwen2.5-VL-7B-Instruct@abc123",
                input("Qwen/Qwen2.5-VL-7B-Instruct", Some("abc123"), None),
            ),
        ] {
            assert_eq!(parse_repo_input(text), Ok(expected), "{text}");
        }
    }

    #[test]
    fn anything_else_is_refused() {
        for text in [
            "",
            "gemma3",
            "mlx-community/",
            "../etc/passwd",
            "org/../x",
            "org/name/extra",
            "https://example.com/org/name",
            "org/na me",
            "-org/name",
            "org/name:Q4;rm",
        ] {
            assert!(
                parse_repo_input(text).is_err(),
                "{text:?} should be refused"
            );
        }
    }

    fn mlx_info() -> Value {
        json!({"id": "mlx-community/gemma-3-12b-it-qat-4bit", "library_name": "mlx", "gated": false})
    }

    fn mlx_files() -> Vec<RepoFile> {
        vec![
            file("config.json", 5_000),
            file("model-00001-of-00002.safetensors", 5_000_000_000),
            file("model-00002-of-00002.safetensors", 3_000_000_000),
            file("model.safetensors.index.json", 90_000),
            file("tokenizer.json", 30_000_000),
            file("tokenizer_config.json", 1_000_000),
            file("preprocessor_config.json", 500),
            file("README.md", 2_000),
            file("original/consolidated.safetensors", 24_000_000_000),
        ]
    }

    fn gemma3_config() -> Value {
        json!({"model_type": "gemma3", "quantization": {"bits": 4, "group_size": 64}})
    }

    fn with_template() -> Value {
        json!({"chat_template": "{{ messages }}"})
    }

    /// The #341 case: a 4-bit Gemma 3 12B from mlx-community.
    #[test]
    fn a_supported_mlx_vision_model_passes() {
        let processor = json!({"processor_class": "Gemma3Processor"});
        let assessed = assess_mlx(
            "mlx-community/gemma-3-12b-it-qat-4bit",
            &mlx_info(),
            &mlx_files(),
            &gemma3_config(),
            Some(&processor),
            Some(&with_template()),
        )
        .expect("should pass");
        assert_eq!(assessed.model_type, "gemma3");
        // Weights, JSON and nothing else — no README, no original/ export.
        let paths: Vec<&str> = assessed.files.iter().map(|f| f.path.as_str()).collect();
        assert!(!paths.contains(&"README.md"));
        assert!(!paths.iter().any(|p| p.starts_with("original/")));
        assert_eq!(assessed.approx_bytes, 8_031_095_500);
        assert!(assessed.est_ram_gb > 9.0 && assessed.est_ram_gb < 13.0);
        assert!(assessed.warnings.iter().any(|w| w.contains("tested list")));
    }

    fn mlx_error(
        info: &Value,
        files: &[RepoFile],
        config: &Value,
        processor: Option<&Value>,
        tokenizer: Option<&Value>,
    ) -> String {
        assess_mlx("org/model", info, files, config, processor, tokenizer)
            .expect_err("should be refused")
            .message()
            .to_string()
    }

    #[test]
    fn mlx_models_that_cannot_work_are_refused_with_a_reason() {
        let processor = json!({"processor_class": "Gemma3Processor"});
        let tpl = with_template();
        let files = mlx_files();

        let gated = json!({"library_name": "mlx", "gated": "manual"});
        assert!(mlx_error(
            &gated,
            &files,
            &gemma3_config(),
            Some(&processor),
            Some(&tpl)
        )
        .contains("gated"));

        let text_only = json!({"model_type": "gemma3_text", "quantization": {}});
        assert!(mlx_error(
            &mlx_info(),
            &files,
            &text_only,
            Some(&processor),
            Some(&tpl)
        )
        .contains("text-only"));

        let unknown = json!({"model_type": "llava_next", "quantization": {}});
        assert!(
            mlx_error(&mlx_info(), &files, &unknown, Some(&processor), Some(&tpl))
                .contains("cannot run as a vision model")
        );

        let odd_processor = json!({"processor_class": "LlavaNextProcessor"});
        assert!(mlx_error(
            &mlx_info(),
            &files,
            &gemma3_config(),
            Some(&odd_processor),
            Some(&tpl)
        )
        .contains("LlavaNextProcessor"));

        assert!(
            mlx_error(&mlx_info(), &files, &gemma3_config(), None, Some(&tpl))
                .contains("image processor config")
        );

        assert!(mlx_error(
            &mlx_info(),
            &files,
            &gemma3_config(),
            Some(&processor),
            Some(&json!({}))
        )
        .contains("chat template"));

        let gguf_only = vec![file("model-Q4_K_M.gguf", 1)];
        assert!(
            mlx_error(&mlx_info(), &gguf_only, &gemma3_config(), None, None).contains("GGUF model")
        );

        let full_size = json!({"library_name": "transformers"});
        let plain_config = json!({"model_type": "gemma3"});
        assert!(mlx_error(
            &full_size,
            &files,
            &plain_config,
            Some(&processor),
            Some(&tpl)
        )
        .contains("not an MLX conversion"));
    }

    #[test]
    fn a_known_incompatible_repo_is_refused() {
        let err = assess_mlx(
            "mlx-community/SmolVLM-256M-Instruct-8bit",
            &mlx_info(),
            &mlx_files(),
            &json!({"model_type": "smolvlm", "quantization": {}}),
            Some(&json!({"processor_class": "SmolVLMProcessor"})),
            Some(&with_template()),
        )
        .err()
        .unwrap();
        assert!(err.message().contains("does not work"));
    }

    #[test]
    fn a_chat_template_file_counts_as_a_template() {
        let mut files = mlx_files();
        files.push(file("chat_template.jinja", 3_000));
        assert!(assess_mlx(
            "org/model",
            &mlx_info(),
            &files,
            &gemma3_config(),
            Some(&json!({"processor_class": "Gemma3Processor"})),
            Some(&json!({})),
        )
        .is_ok());
    }

    fn gguf_files() -> Vec<RepoFile> {
        vec![
            file("gemma-3-12b-it-Q4_K_M.gguf", 7_300_000_000),
            file("gemma-3-12b-it-Q8_0.gguf", 12_500_000_000),
            file("gemma-3-12b-it-BF16-00001-of-00002.gguf", 12_000_000_000),
            file("gemma-3-12b-it-BF16-00002-of-00002.gguf", 12_000_000_000),
            file("mmproj-model-bf16.gguf", 850_000_000),
            file("mmproj-model-f16.gguf", 850_000_000),
            file("README.md", 1),
        ]
    }

    fn gguf_info() -> Value {
        json!({"id": "ggml-org/gemma-3-12b-it-GGUF", "gated": false})
    }

    #[test]
    fn a_gguf_repo_gets_q4_k_m_and_the_f16_projector() {
        let assessed = assess_gguf(
            "ggml-org/gemma-3-12b-it-GGUF",
            &gguf_info(),
            &gguf_files(),
            None,
        )
        .expect("should pass");
        assert_eq!(assessed.model.path, "gemma-3-12b-it-Q4_K_M.gguf");
        assert_eq!(assessed.mmproj.path, "mmproj-model-f16.gguf");
        assert_eq!(assessed.quant, "Q4_K_M");
        assert_eq!(assessed.approx_bytes, 8_150_000_000);
    }

    #[test]
    fn a_requested_quantization_is_honoured_or_explained() {
        let assessed = assess_gguf("r/x", &gguf_info(), &gguf_files(), Some("q8_0")).unwrap();
        assert_eq!(assessed.model.path, "gemma-3-12b-it-Q8_0.gguf");

        let err = assess_gguf("r/x", &gguf_info(), &gguf_files(), Some("Q3_K_S"))
            .err()
            .unwrap();
        assert!(
            err.message().contains("Available: BF16, Q4_K_M, Q8_0"),
            "{err:?}"
        );

        let err = assess_gguf("r/x", &gguf_info(), &gguf_files(), Some("BF16"))
            .err()
            .unwrap();
        assert!(
            err.message().contains("split into several files"),
            "{err:?}"
        );
    }

    #[test]
    fn a_gguf_repo_without_a_projector_is_refused() {
        let files = vec![file("model-Q4_K_M.gguf", 1)];
        let err = assess_gguf("r/x", &gguf_info(), &files, None)
            .err()
            .unwrap();
        assert!(err.message().contains("no vision projector"));
    }

    #[test]
    fn an_mlx_repo_is_not_mistaken_for_a_gguf_one() {
        let err = assess_gguf("r/x", &gguf_info(), &mlx_files(), None)
            .err()
            .unwrap();
        assert!(err.message().contains("no GGUF files"));
    }

    #[test]
    fn quantization_is_read_from_the_file_name() {
        assert_eq!(
            quant_of("gemma-3-12b-it-Q4_K_M.gguf").as_deref(),
            Some("Q4_K_M")
        );
        assert_eq!(quant_of("sub/Model.Q5_K_S.gguf").as_deref(), Some("Q5_K_S"));
        assert_eq!(quant_of("model.gguf"), None);
        assert!(is_split_part("x-Q8_0-00001-of-00003.gguf"));
        assert!(!is_split_part("x-Q8_0.gguf"));
    }

    #[test]
    fn a_folder_taken_by_another_model_gets_the_org_name() {
        let root = tempfile::tempdir().unwrap();
        let is_dir = |p: &Path| p.is_dir();
        assert_eq!(
            folder_for(root.path(), "org/model", is_dir),
            ("model".to_string(), false)
        );

        let taken = root.path().join("model");
        std::fs::create_dir_all(&taken).unwrap();
        std::fs::write(taken.join(SOURCE_MARKER), r#"{"repo":"other/model"}"#).unwrap();
        assert_eq!(
            folder_for(root.path(), "org/model", is_dir),
            ("org--model".to_string(), false)
        );
        assert_eq!(
            folder_for(root.path(), "other/model", is_dir),
            ("model".to_string(), true)
        );
    }

    /// A retry after an interrupted download goes back into the same folder.
    #[test]
    fn an_interrupted_download_of_the_same_repo_is_resumed_in_its_folder() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("gemma-3-12b-it-GGUF");
        std::fs::create_dir_all(&folder).unwrap();
        write_source_marker(&folder, "ggml-org/gemma-3-12b-it-GGUF", "abc").unwrap();
        let complete = |_: &Path| false;
        assert_eq!(
            folder_for(root.path(), "ggml-org/gemma-3-12b-it-GGUF", complete),
            ("gemma-3-12b-it-GGUF".to_string(), false)
        );
        // An unmarked, incomplete folder is someone else's: not reused.
        let other = root.path().join("model");
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(
            folder_for(root.path(), "org/model", complete),
            ("org--model".to_string(), false)
        );
    }
}
