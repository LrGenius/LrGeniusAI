//! Spike S2: benchmark the exported SigLIP2 image tower under ort with
//! the CoreML or CPU execution provider.
//!
//! Usage: bench_image <model.onnx> <coreml|cpu> [batch] [iters]

use std::time::Instant;

use ort::execution_providers::{CPUExecutionProvider, CoreMLExecutionProvider};
use ort::session::Session;
use ort::value::Tensor;

fn main() -> ort::Result<()> {
    let mut args = std::env::args().skip(1);
    let model_path = args.next().expect("model path required");
    let ep = args.next().unwrap_or_else(|| "cpu".into());
    let batch: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let iters: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(10);

    let builder = Session::builder()?;
    let mut builder = match ep.as_str() {
        "coreml" => {
            use ort::execution_providers::coreml::{ComputeUnits, ModelFormat};
            builder.with_execution_providers([CoreMLExecutionProvider::default()
                .with_model_format(ModelFormat::MLProgram)
                .with_compute_units(ComputeUnits::All)
                .with_static_input_shapes(true)
                .with_model_cache_dir(std::env::temp_dir().join("lrg-coreml-cache").display())
                .build()
                .error_on_failure()])?
        }
        _ => builder.with_execution_providers([CPUExecutionProvider::default().build()])?,
    };

    let load_start = Instant::now();
    let mut session = builder.commit_from_file(&model_path)?;
    println!(
        "session loaded in {:.1}s (ep={ep})",
        load_start.elapsed().as_secs_f32()
    );

    // Deterministic pseudo-random pixels; values in a plausible normalized range.
    let n = batch * 3 * 384 * 384;
    let data: Vec<f32> = (0..n)
        .map(|i| ((i as f32 * 0.618_034).fract() - 0.5) * 2.0)
        .collect();
    let input = Tensor::from_array(([batch, 3, 384, 384], data))?;

    // Warmup (first CoreML run includes model compilation).
    let warmup_start = Instant::now();
    session.run(ort::inputs!["pixel_values" => input.clone()])?;
    println!(
        "first run (incl. EP compile): {:.1}s",
        warmup_start.elapsed().as_secs_f32()
    );
    for _ in 0..2 {
        session.run(ort::inputs!["pixel_values" => input.clone()])?;
    }

    let t0 = Instant::now();
    for _ in 0..iters {
        session.run(ort::inputs!["pixel_values" => input.clone()])?;
    }
    let per_batch = t0.elapsed().as_secs_f64() / iters as f64;
    println!(
        "ort {ep} batch={batch}: {:.0} ms/batch, {:.0} ms/img",
        per_batch * 1000.0,
        per_batch * 1000.0 / batch as f64
    );
    Ok(())
}
