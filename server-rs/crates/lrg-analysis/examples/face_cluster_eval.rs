//! Scores face clustering against known identities, for a change to its policy.
//!
//! ```text
//! cargo run --release -p lrg-ml --example face_state_probe -- --embed people/*/*.jpg > faces.jsonl
//! cargo run --release -p lrg-analysis --example face_cluster_eval -- faces.jsonl
//! ```
//!
//! A face's identity is the folder its photo is in, and only the largest face of
//! each photo counts, so a photo of two people does not read as a mistake. The
//! score is BCubed: precision is how much of a face's cluster is the same person,
//! recall how much of that person is in the face's cluster. A face left unassigned
//! is a cluster of its own, which is what it is to whoever browses the People page.

use std::collections::HashMap;
use std::path::Path;

use lrg_analysis::face_cluster::{cluster_faces, ClusterParams, FaceInput};
use serde_json::Value;

struct Scored {
    precision: f64,
    recall: f64,
    clusters: usize,
    unassigned: usize,
    held_back: usize,
    attached: usize,
}

fn score(faces: &[FaceInput], identity: &[String], params: &ClusterParams) -> Scored {
    let outcome = cluster_faces(faces, params);
    let label = |i: usize| {
        outcome.labels[i]
            .map(|l| l as i64)
            .unwrap_or(-(i as i64) - 1)
    };
    let (mut precision, mut recall) = (0.0, 0.0);
    for i in 0..faces.len() {
        let same_cluster = |j: &usize| label(*j) == label(i);
        let same_person = |j: &usize| identity[*j] == identity[i];
        let in_cluster = (0..faces.len()).filter(same_cluster).count() as f64;
        let of_person = (0..faces.len()).filter(same_person).count() as f64;
        let both = (0..faces.len())
            .filter(|j| same_cluster(j) && same_person(j))
            .count() as f64;
        precision += both / in_cluster;
        recall += both / of_person;
    }
    let n = faces.len().max(1) as f64;
    Scored {
        precision: precision / n,
        recall: recall / n,
        clusters: outcome.clusters.len(),
        unassigned: outcome.labels.iter().filter(|l| l.is_none()).count(),
        held_back: outcome.stats.low_quality,
        attached: outcome.stats.attached,
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("faces.jsonl");
    let text = std::fs::read_to_string(path).expect("readable file");
    let mut largest: HashMap<String, Value> = HashMap::new();
    for line in text.lines() {
        let row: Value = serde_json::from_str(line).expect("one JSON object per line");
        let file = row["file"].as_str().unwrap_or_default().to_string();
        let area = |v: &Value| v["area_ratio"].as_f64().unwrap_or(0.0);
        if largest
            .get(&file)
            .is_none_or(|kept| area(kept) < area(&row))
        {
            largest.insert(file, row);
        }
    }
    let mut rows: Vec<(String, Value)> = largest.into_iter().collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    let identity: Vec<String> = rows
        .iter()
        .map(|(file, _)| {
            Path::new(file)
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        })
        .collect();
    let number = |v: &Value, key: &str| v[key].as_f64();
    let faces: Vec<FaceInput> = rows
        .iter()
        .map(|(file, row)| FaceInput {
            id: file.clone(),
            embedding: row["embedding"]
                .as_array()
                .expect("run the probe with --embed")
                .iter()
                .filter_map(|v| v.as_f64().map(|x| x as f32))
                .collect(),
            det_score: number(row, "det_score"),
            area_ratio: number(row, "area_ratio"),
            sharpness: number(row, "sharpness"),
            occlusion: number(row, "occlusion"),
            pinned: None,
        })
        .collect();
    println!(
        "{} faces of {} people",
        faces.len(),
        identity
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
    );
    println!("threshold max_occl clusters unassigned held attached precision recall f1");
    for threshold in [0.4, 0.5, 0.6] {
        for max_occlusion in [0.55, 0.7, 0.85, 1.0] {
            let mut params = ClusterParams::defaults();
            params.cosine_threshold = threshold;
            params.gate.max_occlusion = max_occlusion;
            let s = score(&faces, &identity, &params);
            println!(
                "{threshold:9.2} {max_occlusion:8.2} {:8} {:10} {:4} {:8} {:9.3} {:6.3} {:.3}",
                s.clusters,
                s.unassigned,
                s.held_back,
                s.attached,
                s.precision,
                s.recall,
                2.0 * s.precision * s.recall / (s.precision + s.recall).max(1e-9),
            );
        }
    }
}
