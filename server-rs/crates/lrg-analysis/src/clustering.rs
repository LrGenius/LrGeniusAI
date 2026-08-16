//! Face clustering — port of `services/persons.py::run_clustering`'s
//! math: agglomerative hierarchical clustering (complete/average
//! linkage, cut at a distance threshold, every point assigned — matches
//! sklearn's `AgglomerativeClustering(n_clusters=None,
//! distance_threshold=...)`) and DBSCAN (matches
//! sklearn's `DBSCAN(eps=..., min_samples=...)`, noise labeled -1).
//!
//! Distances are Euclidean (L2) on the raw embedding vectors, matching
//! `metric="euclidean"` — callers convert the API's cosine-distance
//! threshold via `cosine_to_l2` first (unit-vector identity:
//! `L2 = sqrt(2 * cosine_distance)`).

use std::collections::HashSet;

use rayon::prelude::*;

pub fn cosine_to_l2(cosine_distance: f64) -> f64 {
    (2.0 * cosine_distance).sqrt()
}

fn euclidean_squared(a: &[f32], b: &[f32]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let d = (*x - *y) as f64;
            d * d
        })
        .sum::<f64>()
}

fn euclidean(a: &[f32], b: &[f32]) -> f64 {
    euclidean_squared(a, b).sqrt()
}

/// Above this many faces, [`agglomerative`] is refused rather than attempted.
///
/// It materialises an n×n `f64` matrix: 30k faces is 7 GB before any clustering
/// happens, and the merge loop on top of that is O(n³). Returning an error the
/// caller can explain beats allocating until the machine gives up — this
/// codebase has a history of exactly that failure mode during indexing.
pub const AGGLOMERATIVE_MAX_POINTS: usize = 6000;

#[derive(Clone, Copy, PartialEq)]
pub enum Linkage {
    Complete,
    Average,
}

/// Agglomerative clustering with a distance-threshold cutoff. Returns
/// one label per input point (all points assigned; no noise concept).
///
/// O(n³) in time and O(n²) in memory, so it is capped at
/// [`AGGLOMERATIVE_MAX_POINTS`] and returns `None` above that. Callers should
/// fall back to [`dbscan`], which is linear in memory.
pub fn agglomerative(
    embeddings: &[Vec<f32>],
    distance_threshold: f64,
    linkage: Linkage,
) -> Option<Vec<i64>> {
    let n = embeddings.len();
    if n == 0 {
        return Some(Vec::new());
    }
    if n == 1 {
        return Some(vec![0]);
    }
    if n > AGGLOMERATIVE_MAX_POINTS {
        return None;
    }

    // clusters[c] = member point indices; dist[i][j] = linkage distance
    // between active clusters i and j (upper-triangle only used).
    let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    let mut dist: Vec<Vec<f64>> = vec![vec![0.0; n]; n];
    // Row-parallel: each row only reads `embeddings`, so there is nothing to
    // synchronise, and this is the single most expensive phase.
    let upper: Vec<Vec<f64>> = (0..n)
        .into_par_iter()
        .map(|i| {
            (0..n)
                .map(|j| {
                    if j > i {
                        euclidean(&embeddings[i], &embeddings[j])
                    } else {
                        0.0
                    }
                })
                .collect()
        })
        .collect();
    for i in 0..n {
        for j in (i + 1)..n {
            let d = upper[i][j];
            dist[i][j] = d;
            dist[j][i] = d;
        }
    }
    let mut alive: Vec<bool> = vec![true; n];

    loop {
        // Find the globally closest pair of active clusters.
        let mut best = (f64::INFINITY, usize::MAX, usize::MAX);
        for i in 0..n {
            if !alive[i] {
                continue;
            }
            for j in (i + 1)..n {
                if !alive[j] {
                    continue;
                }
                if dist[i][j] < best.0 {
                    best = (dist[i][j], i, j);
                }
            }
        }
        if best.1 == usize::MAX || best.0 > distance_threshold {
            break;
        }
        let (d_ij, i, j) = best;
        let _ = d_ij;

        // Merge j into i; update distances to all other active clusters
        // via the Lance-Williams formulas for complete/average linkage.
        let ni = clusters[i].len() as f64;
        let nj = clusters[j].len() as f64;
        for k in 0..n {
            if !alive[k] || k == i || k == j {
                continue;
            }
            let d_ik = dist[i][k];
            let d_jk = dist[j][k];
            let new_d = match linkage {
                Linkage::Complete => d_ik.max(d_jk),
                Linkage::Average => (ni * d_ik + nj * d_jk) / (ni + nj),
            };
            dist[i][k] = new_d;
            dist[k][i] = new_d;
        }
        let moved = std::mem::take(&mut clusters[j]);
        clusters[i].extend(moved);
        alive[j] = false;
    }

    let mut labels = vec![0i64; n];
    let mut next_label = 0i64;
    for c in 0..n {
        if !alive[c] {
            continue;
        }
        for &point in &clusters[c] {
            labels[point] = next_label;
        }
        next_label += 1;
    }
    Some(labels)
}

/// Classic DBSCAN (brute-force region query), matching sklearn's
/// `metric="euclidean"` semantics: noise points labeled -1, cluster ids
/// assigned in the order clusters are discovered.
pub fn dbscan(embeddings: &[Vec<f32>], eps: f64, min_samples: usize) -> Vec<i64> {
    let n = embeddings.len();
    if n == 0 {
        return Vec::new();
    }

    // Squared distances against a squared threshold: the same comparison
    // without n² square roots. Parallel because each row is independent, and
    // this loop dominates the whole algorithm.
    let eps_squared = eps * eps;
    let neighbors: Vec<Vec<usize>> = (0..n)
        .into_par_iter()
        .map(|i| {
            (0..n)
                .filter(|&j| {
                    j != i && euclidean_squared(&embeddings[i], &embeddings[j]) <= eps_squared
                })
                .collect()
        })
        .collect();

    const UNVISITED: i64 = -2;
    const NOISE: i64 = -1;
    let mut labels = vec![UNVISITED; n];
    let mut cluster_id = 0i64;

    for i in 0..n {
        if labels[i] != UNVISITED {
            continue;
        }
        // Core-point check: self + neighbors >= min_samples (sklearn's convention).
        if neighbors[i].len() + 1 < min_samples {
            labels[i] = NOISE;
            continue;
        }
        labels[i] = cluster_id;
        let mut queue: Vec<usize> = neighbors[i].clone();
        // `queue.contains` was a linear scan of a Vec that grows to the size of
        // the cluster, making expansion O(cluster²) on its own — the dominant
        // cost once one person has a few thousand faces.
        let mut queued: HashSet<usize> = queue.iter().copied().collect();
        let mut qi = 0;
        while qi < queue.len() {
            let q = queue[qi];
            qi += 1;
            if labels[q] == NOISE {
                labels[q] = cluster_id;
            }
            if labels[q] != UNVISITED {
                continue;
            }
            labels[q] = cluster_id;
            if neighbors[q].len() + 1 >= min_samples {
                for &nb in &neighbors[q] {
                    if queued.insert(nb) {
                        queue.push(nb);
                    }
                }
            }
        }
        cluster_id += 1;
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Partition equivalence: same set of same-cluster / different-cluster
    /// pairs, ignoring arbitrary label numbering (which sklearn and this
    /// implementation are not guaranteed to agree on). Noise (-1) must
    /// match exactly since it isn't an arbitrary label.
    fn assert_same_partition(got: &[i64], want: &[i64]) {
        let n = got.len();
        for i in 0..n {
            for j in (i + 1)..n {
                let got_noise = got[i] < 0 || got[j] < 0;
                let want_noise = want[i] < 0 || want[j] < 0;
                if got_noise || want_noise {
                    assert_eq!(
                        (got[i] < 0, got[j] < 0),
                        (want[i] < 0, want[j] < 0),
                        "noise mismatch at ({i},{j}): got {got:?} want {want:?}"
                    );
                    continue;
                }
                assert_eq!(
                    got[i] == got[j],
                    want[i] == want[j],
                    "pair ({i},{j}) partition mismatch: got {got:?} want {want:?}"
                );
            }
        }
    }

    fn load_goldens() -> serde_json::Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/clustering_goldens.json"
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn agglomerative_matches_sklearn() {
        let goldens = load_goldens();
        let embeddings: Vec<Vec<f32>> = goldens["embeddings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                row.as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap() as f32)
                    .collect()
            })
            .collect();

        for (linkage_name, linkage) in [
            ("complete", Linkage::Complete),
            ("average", Linkage::Average),
        ] {
            for threshold in [0.3, 0.6] {
                let key = format!("agg_{linkage_name}_{threshold}");
                let want: Vec<i64> = goldens["results"][&key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_i64().unwrap())
                    .collect();
                let got = agglomerative(&embeddings, threshold, linkage)
                    .expect("golden fixture is far below AGGLOMERATIVE_MAX_POINTS");
                assert_same_partition(&got, &want);
            }
        }
    }

    #[test]
    fn dbscan_matches_sklearn() {
        let goldens = load_goldens();
        let embeddings: Vec<Vec<f32>> = goldens["embeddings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                row.as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap() as f32)
                    .collect()
            })
            .collect();

        for eps in [0.3, 0.6] {
            for min_samples in [2usize, 3] {
                let key = format!("dbscan_{eps}_{min_samples}");
                let want: Vec<i64> = goldens["results"][&key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_i64().unwrap())
                    .collect();
                let got = dbscan(&embeddings, eps, min_samples);
                assert_same_partition(&got, &want);
            }
        }
    }

    #[test]
    fn cosine_to_l2_identity() {
        assert!((cosine_to_l2(0.5) - 1.0).abs() < 1e-9);
        assert_eq!(cosine_to_l2(0.0), 0.0);
    }

    #[test]
    fn single_point_gets_label_zero() {
        assert_eq!(
            agglomerative(&[vec![1.0, 0.0]], 0.5, Linkage::Complete),
            Some(vec![0])
        );
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert_eq!(agglomerative(&[], 0.5, Linkage::Complete), Some(Vec::new()));
        assert!(dbscan(&[], 0.5, 2).is_empty());
    }

    #[test]
    fn agglomerative_declines_rather_than_allocating_gigabytes() {
        // The n×n f64 matrix is the reason: at 30k faces it is 7 GB, allocated
        // before a single distance is compared. The caller falls back to
        // DBSCAN, which is linear in memory.
        let too_many = vec![vec![0.0f32; 2]; AGGLOMERATIVE_MAX_POINTS + 1];
        assert!(agglomerative(&too_many, 0.5, Linkage::Complete).is_none());
    }

    #[test]
    fn dbscan_expands_a_large_dense_cluster() {
        // Regression for the frontier check: `queue.contains` was a linear
        // scan of a Vec that grows with the cluster, so expanding one big
        // cluster was quadratic on its own. Every point here is within eps of
        // every other, which is the worst case for that loop.
        let points: Vec<Vec<f32>> = (0..400).map(|i| vec![i as f32 * 1e-4, 0.0]).collect();
        let labels = dbscan(&points, 1.0, 2);
        assert_eq!(labels.len(), 400);
        assert!(
            labels.iter().all(|&l| l == 0),
            "one dense blob must come back as exactly one cluster"
        );
    }
}
