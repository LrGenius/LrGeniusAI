//! Where the SigLIP-calibrated thresholds fall on a real catalog.
//!
//! Several thresholds were tuned against one embedding model and mean nothing
//! on another: the organism gate in front of BioCLIP, the burst and duplicate
//! distances in grouping, and the keyword clustering similarity. This reads
//! the vectors already stored for a catalog, embeds the fixed texts with the
//! model the environment points at, and prints the numbers those thresholds
//! are compared with — run it once per model and compare.
//!
//! ```bash
//! cd server-rs
//! # old model: the default files in ~/.cache/lrgenius/models, or LRG_SIGLIP_*
//! cargo run --release -p lrg-api --example siglip_thresholds -- /path/to/lrgenius-lance
//! ```
//!
//! Point it at a **copy** of the lance directory when a backend is running.
//! The stored vectors must have been made by the model the environment names,
//! or every number is noise. It prints how many vectors each `embedding_model`
//! holds so a mismatch is visible; only the text tower is run, so the old
//! image tower is not needed to measure an old catalog.

use std::collections::BTreeMap;
use std::path::PathBuf;

use lrg_ml::clip_iqa::{IqaPrompts, PromptSet};
use lrg_ml::siglip::{l2_normalize, SiglipModel};
use lrg_store::{Store, StoreRecord, IMAGE_TABLE};
use serde_json::Value;

/// Cosine distance between two normalized vectors.
fn dist(a: &[f32], b: &[f32]) -> f64 {
    1.0 - a
        .iter()
        .zip(b)
        .map(|(x, y)| *x as f64 * *y as f64)
        .sum::<f64>()
}

fn percentiles(label: &str, mut v: Vec<f64>) {
    if v.is_empty() {
        println!("  {label:34} (none)");
        return;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    println!(
        "  {label:34} n={:<4} min={:.3} p10={:.3} p25={:.3} med={:.3} p75={:.3} p90={:.3} max={:.3}",
        v.len(),
        v[0],
        at(0.10),
        at(0.25),
        at(0.5),
        at(0.75),
        at(0.90),
        v[v.len() - 1]
    );
}

fn num(m: &serde_json::Map<String, Value>, key: &str) -> Option<f64> {
    m.get(key).and_then(Value::as_f64)
}

#[tokio::main]
async fn main() {
    let root: PathBuf = std::env::args()
        .nth(1)
        .expect("usage: siglip_thresholds <lrgenius-lance dir>")
        .into();
    let store = Store::open(&root).await.expect("cannot open the store");
    let records: Vec<StoreRecord> = store
        .scan_all(IMAGE_TABLE)
        .await
        .expect("cannot read the image table")
        .into_iter()
        .filter(|r| r.vector.is_some())
        .collect();

    let mut by_model: BTreeMap<String, usize> = BTreeMap::new();
    for r in &records {
        let m = r
            .metadata
            .get("embedding_model")
            .and_then(Value::as_str)
            .unwrap_or("(none)");
        *by_model.entry(m.to_string()).or_default() += 1;
    }
    println!("stored vectors by embedding_model: {by_model:?}");
    println!("this build's model: {}", lrg_ml::siglip::MODEL_ID);

    let mut rows: Vec<(StoreRecord, Vec<f32>)> = records
        .into_iter()
        .map(|r| {
            let mut v = r.vector.clone().unwrap();
            l2_normalize(&mut v);
            (r, v)
        })
        .collect();
    rows.sort_by(|a, b| {
        let ta = num(&a.0.metadata, "capture_time").unwrap_or(f64::MAX);
        let tb = num(&b.0.metadata, "capture_time").unwrap_or(f64::MAX);
        ta.partial_cmp(&tb).unwrap()
    });

    let model = SiglipModel::new(lrg_ml::model_paths::resolve());

    // --- Prompt-set scores, and the organism gate against BioCLIP's verdict.
    println!("\nprompt-set scores (0..1) over the catalog; the organism gate default is DEFAULT_SPECIES_PREFILTER_THRESHOLD");
    for set in PromptSet::ALL {
        let texts: Vec<String> = set.texts().map(str::to_string).collect();
        let embeddings = model.embed_text(&texts).expect("text tower");
        let lookup: BTreeMap<&str, &[f32]> = texts
            .iter()
            .map(String::as_str)
            .zip(embeddings.iter().map(Vec::as_slice))
            .collect();
        let prompts =
            IqaPrompts::from_lookup(set, |t| lookup.get(t).copied()).expect("prompts incomplete");
        let scores: Vec<f64> = rows.iter().filter_map(|(_, v)| prompts.score(v)).collect();
        percentiles(set.as_str(), scores);

        if set == PromptSet::Organism {
            // Scores of named photos, e.g. to see where a known false
            // positive sits: SIGLIP_THRESHOLDS_IDS=meta1:abc,meta1:def
            if let Ok(ids) = std::env::var("SIGLIP_THRESHOLDS_IDS") {
                for id in ids.split(',') {
                    if let Some((_, v)) = rows.iter().find(|(r, _)| r.id == id.trim()) {
                        println!(
                            "    organism score {:.3}  {id}",
                            prompts.score(v).unwrap_or(f64::NAN)
                        );
                    }
                }
            }
            let (mut with_species, mut without) = (Vec::new(), Vec::new());
            for (r, v) in &rows {
                let Some(score) = prompts.score(v) else {
                    continue;
                };
                let identified = r
                    .metadata
                    .get("species_rank")
                    .and_then(Value::as_str)
                    .is_some_and(|rank| rank != "none");
                if identified {
                    with_species.push(score);
                } else {
                    without.push(score);
                }
            }
            let below = |v: &[f64], t: f64| v.iter().filter(|s| **s < t).count();
            percentiles("organism, BioCLIP identified", with_species.clone());
            percentiles("organism, nothing identified", without.clone());
            for t in [0.05, 0.08, 0.10, 0.12, 0.15, 0.20, 0.25, 0.35, 0.50] {
                println!(
                    "    gate {t:.2}: would skip {}/{} identified photos, {}/{} others",
                    below(&with_species, t),
                    with_species.len(),
                    below(&without, t),
                    without.len()
                );
            }
        }
    }

    // --- Genre: which photos land where, so the prompts can be judged by eye.
    {
        let texts: Vec<String> = lrg_ml::genre::texts().map(str::to_string).collect();
        let embeddings = model.embed_text(&texts).expect("text tower");
        let lookup: BTreeMap<&str, &[f32]> = texts
            .iter()
            .map(String::as_str)
            .zip(embeddings.iter().map(Vec::as_slice))
            .collect();
        let classifier = lrg_ml::genre::GenreClassifier::from_lookup(|t| lookup.get(t).copied())
            .expect("genre prompts incomplete");
        let mut by_genre: BTreeMap<&str, Vec<(String, f64)>> = BTreeMap::new();
        for (r, v) in &rows {
            if let Some(res) = classifier.classify(v) {
                by_genre
                    .entry(res.id)
                    .or_default()
                    .push((r.id.clone(), res.confidence));
            }
        }
        // Named photos, to see how known cases are classified:
        // SIGLIP_THRESHOLDS_IDS=meta1:abc,meta1:def
        if let Ok(ids) = std::env::var("SIGLIP_THRESHOLDS_IDS") {
            for id in ids.split(',') {
                if let Some((_, v)) = rows.iter().find(|(r, _)| r.id == id.trim()) {
                    if let Some(res) = classifier.classify(v) {
                        println!("    genre {:.2} {:14} {id}", res.confidence, res.id);
                    }
                }
            }
        }
        println!("\ngenre assignment (photo ids per genre, with the winner's softmax share)");
        for (genre, mut photos) in by_genre {
            photos.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            println!("  {genre:14} {:3} photos", photos.len());
            for (id, conf) in photos.iter().take(4) {
                println!("      {conf:.2} {id}");
            }
        }
    }

    // --- Grouping distances.
    println!("\ncosine distances (grouping: burst <= 0.12, duplicate 0.02..0.08)");
    let mut consecutive = Vec::new();
    let mut within_3s = Vec::new();
    for w in rows.windows(2) {
        let d = dist(&w[0].1, &w[1].1);
        consecutive.push(d);
        if let (Some(a), Some(b)) = (
            num(&w[0].0.metadata, "capture_time"),
            num(&w[1].0.metadata, "capture_time"),
        ) {
            if (b - a).abs() <= 3.0 {
                within_3s.push(d);
            }
        }
    }
    percentiles("neighbours in capture order", consecutive);
    percentiles("neighbours within 3 s", within_3s);
    let nearest: Vec<f64> = (0..rows.len())
        .map(|i| {
            (0..rows.len())
                .filter(|j| *j != i)
                .map(|j| dist(&rows[i].1, &rows[j].1))
                .fold(f64::MAX, f64::min)
        })
        .collect();
    percentiles("nearest other photo", nearest);
    let pairs_below = |t: f64| {
        let mut n = 0;
        for i in 0..rows.len() {
            for j in i + 1..rows.len() {
                if dist(&rows[i].1, &rows[j].1) <= t {
                    n += 1;
                }
            }
        }
        n
    };
    for t in [
        0.01, 0.02, 0.03, 0.04, 0.05, 0.06, 0.08, 0.10, 0.12, 0.14, 0.16, 0.20,
    ] {
        println!("    pairs with distance <= {t:.2}: {}", pairs_below(t));
    }

    // --- Keyword clustering similarity (text vs text).
    println!("\nkeyword text similarity (clustering threshold 0.85 with LLM, 0.88 without)");
    let related: &[(&str, &str)] = &[
        ("dog", "dogs"),
        ("puppy", "dog"),
        ("sea", "ocean"),
        ("car", "automobile"),
        ("flower", "blossom"),
        ("mountain", "mountains"),
        ("forest", "woods"),
        ("sunset", "dusk"),
    ];
    let unrelated: &[(&str, &str)] = &[
        ("dog", "car"),
        ("sea", "mountain"),
        ("flower", "building"),
        ("sunset", "portrait"),
        ("forest", "street"),
        ("puppy", "ocean"),
        ("car", "flower"),
        ("woods", "city"),
    ];
    let mut words: Vec<String> = related
        .iter()
        .chain(unrelated)
        .flat_map(|(a, b)| [a.to_string(), b.to_string()])
        .collect();
    words.sort();
    words.dedup();
    let embeddings = model.embed_text(&words).expect("text tower");
    let sim = |a: &str, b: &str| {
        let ia = words.iter().position(|w| w == a).unwrap();
        let ib = words.iter().position(|w| w == b).unwrap();
        let (mut x, mut y) = (embeddings[ia].clone(), embeddings[ib].clone());
        l2_normalize(&mut x);
        l2_normalize(&mut y);
        1.0 - dist(&x, &y)
    };
    percentiles(
        "related pairs",
        related.iter().map(|(a, b)| sim(a, b)).collect(),
    );
    percentiles(
        "unrelated pairs",
        unrelated.iter().map(|(a, b)| sim(a, b)).collect(),
    );
    for (a, b) in related.iter().chain(unrelated) {
        println!("    {a:10} {b:12} {:.3}", sim(a, b));
    }
}
