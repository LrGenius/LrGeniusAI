//! Measures whether a local vision model decides culling's close calls better
//! than the ranking does — the gate step 7 of the culling plan has to pass
//! before an "AI tiebreak" ships.
//!
//! ```sh
//! cargo run -q --release -p lrg-analysis --example cull_eval -- \
//!     testdata/cull_bench/fixtures --preset auto --close-calls calls.json
//! cargo run --release -p lrg-llama --example tiebreak_probe -- \
//!     --model model.gguf --mmproj mmproj.gguf --calls calls.json \
//!     --images ~/.cache/lrgenius/cull-bench/variants
//! ```
//!
//! Each close call whose two frames are labelled differently is shown to the
//! model as one picture, the two frames side by side, and it says which to
//! keep. Which frame is on the left alternates, so a model that always says
//! "left" scores half. Benchmark photo ids are `bench-<key>-<variant>`, found
//! under `--images` as `<key>/<variant>.jpg`.

use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use image::{Rgb, RgbImage as Canvas};
use lrg_llama::{GenerationRequest, LlamaEngine, LlamaEngineConfig, RgbImage};
use serde_json::{json, Value};

/// Each frame's long edge in the side-by-side picture.
const EDGE: u32 = 768;

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn path_of(images: &Path, photo_id: &str) -> PathBuf {
    let rest = photo_id.strip_prefix("bench-").unwrap_or(photo_id);
    let (key, variant) = rest.split_once('-').expect("bench-<key>-<variant>");
    images.join(key).join(format!("{variant}.jpg"))
}

fn side_by_side(left: &Path, right: &Path) -> RgbImage {
    let load = |p: &Path| {
        image::open(p)
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
            .resize(EDGE, EDGE, FilterType::Lanczos3)
            .to_rgb8()
    };
    let (a, b) = (load(left), load(right));
    let gap = 16;
    let (w, h) = (a.width() + gap + b.width(), a.height().max(b.height()));
    let mut canvas = Canvas::from_pixel(w, h, Rgb([255, 255, 255]));
    image::imageops::overlay(&mut canvas, &a, 0, 0);
    image::imageops::overlay(&mut canvas, &b, i64::from(a.width() + gap), 0);
    RgbImage::new(w, h, canvas.into_raw()).expect("buffer matches")
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let model = arg(&args, "--model").expect("--model GGUF");
    let mmproj = arg(&args, "--mmproj").expect("--mmproj GGUF");
    let calls: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string(arg(&args, "--calls").expect("--calls JSON")).unwrap(),
    )
    .unwrap();
    let images = PathBuf::from(arg(&args, "--images").expect("--images DIR"));
    let limit: usize = arg(&args, "--limit").map_or(usize::MAX, |n| n.parse().unwrap());

    let config = LlamaEngineConfig {
        model_path: model.into(),
        mmproj_path: Some(mmproj.into()),
        n_ctx: 8192,
        n_parallel: 1,
        ..Default::default()
    };
    let engine = tokio::task::spawn_blocking(move || LlamaEngine::new(config))
        .await
        .unwrap()
        .expect("engine loads");

    let schema = json!({
        "type": "object",
        "properties": {
            "keep": {"type": "string", "enum": ["A", "B"]},
            "reason": {"type": "string"}
        },
        "required": ["keep", "reason"]
    });
    let rank = |v: &Value| v.as_u64();
    let decided: Vec<&Value> = calls
        .iter()
        .filter(|c| matches!((rank(&c["pick_rank"]), rank(&c["runner_up_rank"])), (Some(a), Some(b)) if a != b))
        .take(limit)
        .collect();

    let (mut model_right, mut ranking_right, mut said_a) = (0usize, 0usize, 0usize);
    for (i, call) in decided.iter().enumerate() {
        let pick = call["pick"].as_str().unwrap();
        let runner = call["runner_up"].as_str().unwrap();
        let pick_better = rank(&call["pick_rank"]) < rank(&call["runner_up_rank"]);
        // The ranking's pick is on the left every other time.
        let (left, right) = if i % 2 == 0 {
            (pick, runner)
        } else {
            (runner, pick)
        };
        let picture = side_by_side(&path_of(&images, left), &path_of(&images, right));
        let request = GenerationRequest {
            system_prompt: "You are a professional photo editor culling a client's shoot."
                .to_string(),
            stable_prompt: "The picture shows two frames of the same burst side by side: frame A \
                            on the left, frame B on the right. Which one would you keep for the \
                            client? Judge, in this order: is the subject in focus and free of \
                            blur and shake; is it well exposed; is it clean rather than grainy; \
                            are the eyes open and the expression good. Answer as JSON with \
                            `keep` (\"A\" or \"B\") and a one-sentence `reason`."
                .to_string(),
            per_photo_prompt: String::new(),
            image: Some(picture),
            schema: Some(schema.clone()),
            max_tokens: 120,
            temperature: 0.0,
        };
        let out = match engine.generate(vec![request]).await {
            Ok(out) => out,
            Err(e) => {
                eprintln!("{pick}: {e}");
                continue;
            }
        };
        let text = match out.into_iter().next() {
            Some(Ok(o)) => o.text,
            Some(Err(e)) => {
                eprintln!("{pick}: {e}");
                continue;
            }
            None => continue,
        };
        let answer: Value = serde_json::from_str(&text).unwrap_or(json!({}));
        let keep_left = answer["keep"].as_str() == Some("A");
        said_a += usize::from(keep_left);
        let kept_pick = keep_left == (left == pick);
        let right_answer = kept_pick == pick_better;
        model_right += usize::from(right_answer);
        ranking_right += usize::from(pick_better);
        println!(
            "{:>3} {:<40} vs {:<40} model {} ranking {} | {}",
            i,
            pick,
            runner,
            if right_answer { "right" } else { "wrong" },
            if pick_better { "right" } else { "wrong" },
            answer["reason"].as_str().unwrap_or("")
        );
    }
    let n = decided.len().max(1) as f64;
    println!(
        "\n{} close calls with different labels: model right {} ({:.1} %), ranking right {} \
         ({:.1} %); the model said A (left) {:.1} % of the time",
        decided.len(),
        model_right,
        model_right as f64 * 100.0 / n,
        ranking_right,
        ranking_right as f64 * 100.0 / n,
        said_a as f64 * 100.0 / n
    );
}
