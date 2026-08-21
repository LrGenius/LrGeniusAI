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

#[derive(Debug, Clone, Copy, PartialEq)]
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

/// One initial cluster handed to [`agglomerative_seeded`] — a group of points
/// that is already decided and must stay together.
///
/// This is how manual assignments enter clustering. A person the user built by
/// hand in the People UI arrives as one seed, so the algorithm can only grow it
/// or leave it alone; it can never take it apart.
pub struct Seed {
    /// Indices into the caller's embedding list, used to measure this seed's
    /// linkage distance to the others. Sub-sample large seeds down to
    /// [`MAX_SEED_REPS`] before building them: complete linkage between two
    /// seeds costs `|a| * |b|` distance computations, so two hand-curated
    /// people of a few thousand faces each would otherwise dominate the run.
    pub reps: Vec<usize>,
    /// Seeds carrying different `Some(group)` values are never merged, whatever
    /// their distance. Two people the user has separately confirmed are not one
    /// person, and no threshold should be able to say otherwise.
    pub group: Option<usize>,
}

/// The most members of one seed that take part in a linkage measurement.
///
/// Complete linkage only needs the *furthest* pair, and a sample of this size
/// finds a near-worst pair reliably enough; the alternative is a quadratic blow-up
/// the moment someone merges two large people together.
pub const MAX_SEED_REPS: usize = 32;

/// Agglomerative clustering that starts from pre-formed groups instead of
/// singletons, honouring must-link (within a seed) and cannot-link (between
/// seeds with different `group`s) constraints.
///
/// Returns one label per *seed*, not per point. `None` above
/// [`AGGLOMERATIVE_MAX_POINTS`] seeds, exactly like [`agglomerative`].
pub fn agglomerative_seeded(
    embeddings: &[Vec<f32>],
    seeds: &[Seed],
    distance_threshold: f64,
    linkage: Linkage,
) -> Option<Vec<i64>> {
    let n = seeds.len();
    if n == 0 {
        return Some(Vec::new());
    }
    if n == 1 {
        return Some(vec![0]);
    }
    if n > AGGLOMERATIVE_MAX_POINTS {
        return None;
    }

    // Linkage distance between every pair of seeds, measured over their
    // representatives. Row-parallel: nothing here is written twice.
    let upper: Vec<Vec<f64>> = (0..n)
        .into_par_iter()
        .map(|i| {
            (0..n)
                .map(|j| {
                    if j > i {
                        seed_distance(embeddings, &seeds[i], &seeds[j], linkage)
                    } else {
                        0.0
                    }
                })
                .collect()
        })
        .collect();
    let mut dist: Vec<Vec<f64>> = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            dist[i][j] = upper[i][j];
            dist[j][i] = upper[i][j];
        }
    }

    let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    let mut weight: Vec<f64> = seeds.iter().map(|s| s.reps.len().max(1) as f64).collect();
    let mut group: Vec<Option<usize>> = seeds.iter().map(|s| s.group).collect();
    let mut alive: Vec<bool> = vec![true; n];

    loop {
        let mut best = (f64::INFINITY, usize::MAX, usize::MAX);
        for i in 0..n {
            if !alive[i] {
                continue;
            }
            for j in (i + 1)..n {
                if !alive[j] || !may_merge(group[i], group[j]) {
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
        let (_, i, j) = best;

        let (wi, wj) = (weight[i], weight[j]);
        for k in 0..n {
            if !alive[k] || k == i || k == j {
                continue;
            }
            let new_d = match linkage {
                Linkage::Complete => dist[i][k].max(dist[j][k]),
                Linkage::Average => (wi * dist[i][k] + wj * dist[j][k]) / (wi + wj),
            };
            dist[i][k] = new_d;
            dist[k][i] = new_d;
        }
        let moved = std::mem::take(&mut clusters[j]);
        clusters[i].extend(moved);
        weight[i] = wi + wj;
        // The merged cluster inherits whichever side was pinned; `may_merge`
        // guarantees at most one of them was.
        group[i] = group[i].or(group[j]);
        alive[j] = false;
    }

    let mut labels = vec![0i64; n];
    let mut next_label = 0i64;
    for c in 0..n {
        if !alive[c] {
            continue;
        }
        for &seed in &clusters[c] {
            labels[seed] = next_label;
        }
        next_label += 1;
    }
    Some(labels)
}

fn may_merge(a: Option<usize>, b: Option<usize>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

fn seed_distance(embeddings: &[Vec<f32>], a: &Seed, b: &Seed, linkage: Linkage) -> f64 {
    // Complete takes the running max (distances are non-negative, so 0.0 is a
    // safe identity); Average accumulates a sum.
    let mut acc = 0.0f64;
    let mut pairs = 0usize;
    for &i in &a.reps {
        for &j in &b.reps {
            let d = euclidean(&embeddings[i], &embeddings[j]);
            match linkage {
                Linkage::Complete => acc = acc.max(d),
                Linkage::Average => acc += d,
            }
            pairs += 1;
        }
    }
    if pairs == 0 {
        return f64::INFINITY;
    }
    match linkage {
        Linkage::Complete => acc,
        Linkage::Average => acc / pairs as f64,
    }
}

/// A cheap linear pre-partition ("canopy"): walks the points once, joining each
/// to the first running centroid within `threshold` and starting a new one
/// otherwise. Returns one canopy id per point.
///
/// Used only to keep [`agglomerative_seeded`] under its size cap on large
/// catalogs. The threshold is deliberately loose — its job is to separate
/// people who are obviously nothing alike, so that the real, chaining-resistant
/// pass runs inside each canopy. Order matters (it is greedy), so callers pass
/// points best-first.
pub fn canopy_partition(points: &[Vec<f32>], threshold: f64, order: &[usize]) -> Vec<usize> {
    let mut canopy_of = vec![usize::MAX; points.len()];
    let mut centroids: Vec<Vec<f32>> = Vec::new();
    let mut counts: Vec<f64> = Vec::new();

    for &i in order {
        let mut best = (f64::INFINITY, usize::MAX);
        for (c, centroid) in centroids.iter().enumerate() {
            let d = euclidean(&points[i], centroid);
            if d < best.0 {
                best = (d, c);
            }
        }
        if best.1 != usize::MAX && best.0 <= threshold {
            let c = best.1;
            let n = counts[c];
            for (slot, v) in centroids[c].iter_mut().zip(&points[i]) {
                *slot = ((*slot as f64 * n + *v as f64) / (n + 1.0)) as f32;
            }
            counts[c] = n + 1.0;
            canopy_of[i] = c;
        } else {
            canopy_of[i] = centroids.len();
            centroids.push(points[i].clone());
            counts.push(1.0);
        }
    }
    canopy_of
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
    fn seeded_agglomerative_keeps_a_seed_together_and_apart() {
        // Four near-identical points. Two are pinned to different groups, so
        // no distance can merge them; the unpinned pair joins whichever it is
        // allowed to.
        let points = vec![
            vec![1.0, 0.0],
            vec![0.999, 0.001],
            vec![0.998, 0.002],
            vec![0.997, 0.003],
        ];
        let seeds = vec![
            Seed {
                reps: vec![0],
                group: Some(0),
            },
            Seed {
                reps: vec![1],
                group: Some(1),
            },
            Seed {
                reps: vec![2],
                group: None,
            },
            Seed {
                reps: vec![3],
                group: None,
            },
        ];
        let labels = agglomerative_seeded(&points, &seeds, 1.0, Linkage::Complete).unwrap();
        assert_ne!(labels[0], labels[1], "two pinned groups must never merge");
        assert_eq!(
            labels.iter().collect::<HashSet<_>>().len(),
            2,
            "everything else is within the threshold, so two clusters is the answer"
        );
    }

    #[test]
    fn a_seed_with_many_members_is_measured_by_its_furthest_representative() {
        // Complete linkage across seeds: seed A spans a wide arc, so its
        // distance to B is set by A's far end, not its near one.
        let points = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![0.1, 0.995]];
        let seeds = vec![
            Seed {
                reps: vec![0, 1],
                group: None,
            },
            Seed {
                reps: vec![2],
                group: None,
            },
        ];
        // point 2 is close to point 1 but far from point 0.
        let close = agglomerative_seeded(&points, &seeds, 0.2, Linkage::Complete).unwrap();
        assert_ne!(
            close[0], close[1],
            "the furthest pair is well over 0.2 apart"
        );
        let loose = agglomerative_seeded(&points, &seeds, 2.0, Linkage::Complete).unwrap();
        assert_eq!(loose[0], loose[1]);
    }

    #[test]
    fn canopy_partition_separates_two_distant_blobs() {
        let points = vec![
            vec![0.0, 0.0],
            vec![0.05, 0.0],
            vec![10.0, 0.0],
            vec![10.05, 0.0],
        ];
        let order: Vec<usize> = (0..points.len()).collect();
        let canopies = canopy_partition(&points, 1.0, &order);
        assert_eq!(canopies[0], canopies[1]);
        assert_eq!(canopies[2], canopies[3]);
        assert_ne!(canopies[0], canopies[2]);
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
