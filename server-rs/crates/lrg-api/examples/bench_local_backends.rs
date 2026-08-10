//! Head-to-head benchmark: llama.cpp versus MLX on the same photos.
//!
//! Both backends get byte-identical prompts, the same JSON Schema and the same
//! decoded pixels, so the only variable is the inference stack. Run it with a
//! matched model pair — same family, same size, same quantization tier — or the
//! numbers mean nothing:
//!
//! ```bash
//! cd server-rs
//! LIBCLANG_PATH="$(xcode-select -p)/Toolchains/XcodeDefault.xctoolchain/usr/lib" \
//! LRG_MLX_SIDECAR=../native/mlx-sidecar/.build/xcode/Build/Products/Release/lrgenius-mlx \
//! cargo run --release -p lrg-api --features llamacpp --example bench_local_backends -- \
//!   --photos /path/to/photos \
//!   --gguf   /path/to/gemma-4-E2B-it-Q4_K_M.gguf \
//!   --mmproj /path/to/mmproj-F16.gguf \
//!   --mlx    /path/to/gemma-4-e2b-it-4bit
//! ```
//!
//! # What is and is not measured
//!
//! Model load is timed separately and excluded from the per-photo figures: it
//! is paid once per indexing run, not per photo, and including it would flatter
//! whichever backend happened to have warmer page cache.
//!
//! Each photo is timed as a separate single-photo request, because that is the
//! shape both backends actually see at `n_parallel = 1` — the default. There is
//! a separate batched pass for llama.cpp, since a real batch is the one thing
//! it can do that MLX cannot (see `lrg_mlx`'s module docs).
//!
//! Decoding is schema-constrained on both sides, matching production. That is
//! deliberate and it is not free: it is part of what each backend costs.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lrg_providers::local::{LocalImage, LocalRequest, SharedLocalEngine};

/// One photo's decoded pixels, decoded once and shared by both backends so
/// neither pays for JPEG decoding inside its timed region.
struct Photo {
    name: String,
    image: LocalImage,
}

struct Sample {
    name: String,
    elapsed: Duration,
    prompt_tokens: u32,
    completion_tokens: u32,
    ok: bool,
    parsed_as_json: bool,
}

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn required(name: &str) -> String {
    arg(name).unwrap_or_else(|| {
        eprintln!("missing required argument {name}");
        eprintln!("see the module docs at the top of this file for usage");
        std::process::exit(2);
    })
}

fn schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "caption": {"type": "string"},
            "keywords": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["caption", "keywords"],
    })
}

/// The prompt is split the way the real providers split it, so each backend is
/// handed the same run-constant and per-photo halves it would see in an
/// indexing run.
fn request_for(photo: &Photo) -> LocalRequest {
    LocalRequest {
        system_prompt: "You are a photo cataloguing assistant. Reply with JSON only.".to_string(),
        stable_prompt: "Describe the photograph in one sentence and list up to ten keywords \
                        describing its subject, setting and mood."
            .to_string(),
        per_photo_prompt: format!("File: {}. Captured 2026-08-10.", photo.name),
        image: Some(photo.image.clone()),
        schema: Some(schema()),
        max_tokens: 200,
        temperature: 0.0,
    }
}

fn load_photos(dir: &Path) -> Vec<Photo> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png"))
        })
        .collect();
    entries.sort();

    entries
        .iter()
        .map(|path| {
            let bytes = std::fs::read(path).expect("photo should be readable");
            let (width, height, rgb) =
                lrg_providers::image_encode::image_to_rgb(&bytes).expect("photo should decode");
            Photo {
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                image: LocalImage { width, height, rgb },
            }
        })
        .collect()
}

/// One throwaway inference, so the timed pass does not pay for it.
///
/// Both backends compile Metal pipelines on first use, and llama.cpp also
/// warms its mmproj graph. That cost is real but it is paid once per run, not
/// per photo, and leaving it inside the first sample would make whichever
/// backend went first look far worse than it is.
async fn warm_up(engine: &SharedLocalEngine, photo: &Photo) {
    let mut request = request_for(photo);
    request.max_tokens = 8;
    let _ = engine.generate(vec![request]).await;
}

/// Run every photo as its own single-photo request.
async fn run_sequential(engine: &SharedLocalEngine, photos: &[Photo]) -> Vec<Sample> {
    let mut samples = Vec::new();
    for photo in photos {
        let started = Instant::now();
        let mut results = engine.generate(vec![request_for(photo)]).await;
        let elapsed = started.elapsed();
        let result = results.pop();
        samples.push(match result {
            Some(Ok(response)) => Sample {
                name: photo.name.clone(),
                elapsed,
                prompt_tokens: response.prompt_tokens,
                completion_tokens: response.completion_tokens,
                ok: true,
                parsed_as_json: serde_json::from_str::<serde_json::Value>(response.text.trim())
                    .is_ok(),
            },
            other => {
                if let Some(Err(e)) = other {
                    eprintln!("  {} failed: {e}", photo.name);
                }
                Sample {
                    name: photo.name.clone(),
                    elapsed,
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    ok: false,
                    parsed_as_json: false,
                }
            }
        });
    }
    samples
}

/// Run the whole set as one batch, and report the wall time for the group.
async fn run_batched(engine: &SharedLocalEngine, photos: &[Photo]) -> (Duration, usize, u32) {
    let requests: Vec<LocalRequest> = photos.iter().map(request_for).collect();
    let started = Instant::now();
    let results = engine.generate(requests).await;
    let elapsed = started.elapsed();
    let succeeded = results.iter().filter(|r| r.is_ok()).count();
    let completion: u32 = results
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|r| r.completion_tokens)
        .sum();
    (elapsed, succeeded, completion)
}

fn report(label: &str, load: Duration, samples: &[Sample]) {
    let ok: Vec<&Sample> = samples.iter().filter(|s| s.ok).collect();
    println!("\n=== {label} ===");
    println!("  model load          {:>8.2} s", load.as_secs_f64());
    if ok.is_empty() {
        println!("  no photo succeeded");
        return;
    }

    println!(
        "  {:<20} {:>9} {:>8} {:>8} {:>7}",
        "photo", "seconds", "prompt", "output", "JSON"
    );
    for sample in samples {
        println!(
            "  {:<20} {:>9.2} {:>8} {:>8} {:>7}",
            sample.name,
            sample.elapsed.as_secs_f64(),
            sample.prompt_tokens,
            sample.completion_tokens,
            if !sample.ok {
                "FAIL"
            } else if sample.parsed_as_json {
                "ok"
            } else {
                "BAD"
            },
        );
    }

    let total: Duration = ok.iter().map(|s| s.elapsed).sum();
    let completion: u32 = ok.iter().map(|s| s.completion_tokens).sum();
    let mean = total.as_secs_f64() / ok.len() as f64;
    let mut sorted: Vec<f64> = ok.iter().map(|s| s.elapsed.as_secs_f64()).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("no NaN durations"));

    println!("  ------------------------------------------------------");
    println!("  photos ok           {:>8}/{}", ok.len(), samples.len());
    println!("  total               {:>8.2} s", total.as_secs_f64());
    println!("  mean per photo      {:>8.2} s", mean);
    println!("  median per photo    {:>8.2} s", sorted[sorted.len() / 2]);
    println!(
        "  output tok/s        {:>8.1}",
        f64::from(completion) / total.as_secs_f64()
    );
}

#[tokio::main]
async fn main() {
    let photos = load_photos(Path::new(&required("--photos")));
    assert!(!photos.is_empty(), "no photos found");
    println!(
        "Benchmarking {} photo(s): {}",
        photos.len(),
        photos
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let gguf = PathBuf::from(required("--gguf"));
    let mmproj = arg("--mmproj").map(PathBuf::from);
    let mlx_dir = PathBuf::from(required("--mlx"));

    // ---- MLX -------------------------------------------------------------
    let mlx_slot = lrg_api::mlx_engine::MlxEngineSlot::default();
    let started = Instant::now();
    let mlx: SharedLocalEngine = mlx_slot
        .get_or_load(&lrg_api::mlx_engine::MlxEngineSettings {
            model_dir: mlx_dir.clone(),
        })
        .await
        .expect("the MLX engine should load");
    let mlx_load = started.elapsed();
    warm_up(&mlx, &photos[0]).await;
    let mlx_samples = run_sequential(&mlx, &photos).await;
    let (mlx_batch, mlx_batch_ok, mlx_batch_tokens) = run_batched(&mlx, &photos).await;
    let mlx_batch_size = mlx.preferred_batch_size();

    // ---- llama.cpp, one sequence -----------------------------------------
    //
    // `n_parallel = 1` is the shipped default and the only setting that is a
    // like-for-like comparison against MLX: one photo in flight, one decode.
    let llama_slot = lrg_api::llm_engine::LlmEngineSlot::default();
    let solo = lrg_api::llm_engine::settings_for(
        gguf.clone(),
        mmproj.clone(),
        lrg_api::llm_engine::EngineOverrides {
            n_parallel: Some(1),
            ..Default::default()
        },
    );
    let started = Instant::now();
    let llama: SharedLocalEngine = llama_slot
        .get_or_load(&solo)
        .await
        .expect("the llama.cpp engine should load");
    let llama_load = started.elapsed();
    warm_up(&llama, &photos[0]).await;
    let llama_samples = run_sequential(&llama, &photos).await;

    report("MLX (sidecar)", mlx_load, &mlx_samples);
    report(
        "llama.cpp (in-process, n_parallel=1)",
        llama_load,
        &llama_samples,
    );

    // ---- llama.cpp, parallel sequences ------------------------------------
    //
    // Loaded separately because `n_parallel` cannot be changed on a live
    // context. This is the one thing llama.cpp can do that MLX cannot: share a
    // pinned prefix across sequences that decode together.
    let parallel_n = photos.len().min(4) as u32;
    let parallel = lrg_api::llm_engine::settings_for(
        gguf,
        mmproj,
        lrg_api::llm_engine::EngineOverrides {
            n_parallel: Some(parallel_n),
            ..Default::default()
        },
    );
    // Only this local `Arc` needs dropping — `get_or_load` unloads the slot's
    // own copy before it builds the replacement. `LlamaBackend::init()` is a
    // process-wide singleton that frees only on drop, so an engine that is
    // still alive anywhere blocks the next one from initializing.
    drop(llama);

    let llama_parallel: SharedLocalEngine = llama_slot
        .get_or_load(&parallel)
        .await
        .expect("the llama.cpp engine should reload with parallel sequences");
    warm_up(&llama_parallel, &photos[0]).await;
    let (lp_batch, lp_ok, lp_tokens) = run_batched(&llama_parallel, &photos).await;

    println!("\n=== whole set as one batch ===");
    println!(
        "  {:<38} {:>9} {:>7} {:>14}",
        "backend", "seconds", "ok", "output tok/s"
    );
    let row = |label: &str, elapsed: Duration, ok: usize, tokens: u32| {
        println!(
            "  {:<38} {:>9.2} {:>5}/{} {:>14.1}",
            label,
            elapsed.as_secs_f64(),
            ok,
            photos.len(),
            f64::from(tokens) / elapsed.as_secs_f64(),
        );
    };
    row(
        &format!("MLX (preferred_batch_size={mlx_batch_size})"),
        mlx_batch,
        mlx_batch_ok,
        mlx_batch_tokens,
    );
    row(
        &format!("llama.cpp (n_parallel={parallel_n})"),
        lp_batch,
        lp_ok,
        lp_tokens,
    );
}
