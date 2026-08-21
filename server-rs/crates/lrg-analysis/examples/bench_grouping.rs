//! Timing harness for `group_and_sort_images`.
//!
//! Grouping used to be a full O(n^2) sweep with a 1152-dimensional f64 cosine
//! per pair, run inline on a tokio worker thread, so a large cull was the
//! dominant cost of `/v1/cull/grade`. This measures the sweep at several catalog sizes
//! so the scaling curve is visible rather than assumed.
//!
//! ```text
//! cargo run --release -p lrg-analysis --example bench_grouping [sizes...]
//! ```
//!
//! The synthetic set models a real shoot: bursts of 8 frames a second apart,
//! with several seconds of gap between bursts, and near-identical embeddings
//! inside a burst.

use std::time::Instant;

use lrg_analysis::grouping::{group_and_sort_images, GroupingInput};
use serde_json::{Map, Value};

const EMBED_DIM: usize = 1152;
const BURST_LEN: usize = 8;

fn make_records(n: usize) -> Vec<GroupingInput> {
    (0..n)
        .map(|i| {
            let burst = i / BURST_LEN;
            let within = i % BURST_LEN;
            // 1s between frames in a burst, 30s between bursts.
            let capture_time = (burst * 30 + within) as f64;

            // Frames in a burst share a hash; bursts differ.
            let phash = (burst as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);

            // Embedding dominated by the burst identity, with a small
            // per-frame perturbation so distances are not degenerate.
            let mut embedding = vec![0.0f32; EMBED_DIM];
            for (k, slot) in embedding.iter_mut().enumerate() {
                let base = ((burst * 31 + k) as f32 * 0.017).sin();
                *slot = base + (within as f32) * 0.0001;
            }

            let mut metadata = Map::new();
            metadata.insert(
                "cull_sharpness".into(),
                Value::from(0.5 + (within as f64) * 0.01),
            );
            metadata.insert("cull_exposure".into(), Value::from(0.7));
            metadata.insert("cull_noise".into(), Value::from(0.2));

            GroupingInput {
                photo_id: format!("p{i}"),
                filename: format!("IMG_{i:06}.CR3"),
                capture_time: Some(capture_time),
                embedding: Some(embedding),
                phash: Some(phash),
                metadata,
            }
        })
        .collect()
}

fn main() {
    let sizes: Vec<usize> = {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.is_empty() {
            vec![500, 2000, 5000, 10000]
        } else {
            args.iter().filter_map(|a| a.parse().ok()).collect()
        }
    };

    println!(
        "{:>8}  {:>12}  {:>10}  {:>12}",
        "photos", "elapsed", "groups", "photos/s"
    );
    for n in sizes {
        let records = make_records(n);
        let t0 = Instant::now();
        let groups = group_and_sort_images(records, None, None, Some(1), "default");
        let dt = t0.elapsed();
        println!(
            "{:>8}  {:>12?}  {:>10}  {:>12.0}",
            n,
            dt,
            groups.len(),
            n as f64 / dt.as_secs_f64()
        );
    }
}
