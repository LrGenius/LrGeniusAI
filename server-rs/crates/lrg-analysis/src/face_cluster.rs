//! Turning a table of face embeddings into people.
//!
//! [`clustering`](crate::clustering) supplies the raw algorithms; this module
//! is the policy around them, and the policy is where clustering quality
//! actually comes from. Three things it does that a bare call to DBSCAN or
//! agglomerative does not:
//!
//! **It keeps low-quality faces out of cluster formation.** A tiny, blurry or
//! half-occluded crop still produces a 512-d unit vector, but that vector sits
//! nearer the middle of the embedding space than it does to the person it came
//! from. Left in, such faces act as bridges: A is close to the blur, the blur
//! is close to B, and a density-based algorithm walks straight from one person
//! to another. They are set aside during formation and attached afterwards to
//! whichever finished cluster they are close to — which can only ever put them
//! somewhere, never merge two people.
//!
//! **It never chains.** Complete linkage caps the distance between the two
//! furthest members of a cluster at the threshold, so "same person" stays a
//! statement about the whole group. DBSCAN's density-connectivity does not: it
//! is transitive, and one borderline pair is enough to join two people.
//! DBSCAN is still reachable, but it is no longer what an unqualified
//! "cluster faces" does.
//!
//! **It treats the user's decisions as facts.** A face the user assigned in
//! the People UI arrives here pinned. Pinned faces of one person enter as a
//! single pre-formed cluster (must-link) that the run can grow but not split,
//! and two different pinned people can never be merged into each other
//! (cannot-link), whatever the threshold says. Without this, every re-cluster
//! would quietly undo the corrections that motivated it.

use std::collections::HashMap;

use crate::clustering::{
    agglomerative_seeded, canopy_partition, cosine_to_l2, Linkage, Seed, AGGLOMERATIVE_MAX_POINTS,
    MAX_SEED_REPS,
};

/// One face row, reduced to what clustering needs.
///
/// The quality fields are optional because a face row written before those
/// measurements existed simply does not have them. `None` means "not measured",
/// which is a different thing from "measured and bad": the gate cannot judge
/// what it cannot see, so it lets such a face through rather than quietly
/// removing it from the run on no evidence.
#[derive(Debug, Clone, Default)]
pub struct FaceInput {
    pub id: String,
    /// L2-normalized identity embedding.
    pub embedding: Vec<f32>,
    pub det_score: Option<f64>,
    pub area_ratio: Option<f64>,
    pub sharpness: Option<f64>,
    pub occlusion: Option<f64>,
    /// The user's decision about this face, if they made one. `Some(person)`
    /// pins it to that person; `Some("")` pins it to *nobody*, which is how
    /// "this face is not them" is recorded. `None` leaves it to the algorithm.
    pub pinned: Option<String>,
}

/// What a face must look like to be allowed to *form* a person.
///
/// Deliberately stricter than detection's own bar (`DET_THRESH` = 0.6 in
/// `lrg_ml::yunet`): detection is answering "is this a face", which is a much
/// easier question than "is this crop good enough to tell two people apart".
/// Everything below the gate is still clustered — just afterwards, against
/// finished clusters, where a bad embedding can be wrong about one face
/// instead of wrong about a whole person.
#[derive(Debug, Clone, Copy)]
pub struct QualityGate {
    pub min_det_score: f64,
    /// Face box area as a fraction of the frame. 0.0008 is roughly a 100x100
    /// crop on a 12 MP file — below FaceNet's 160x160 input, so anything
    /// smaller is upsampled guesswork.
    pub min_area_ratio: f64,
    pub min_sharpness: f64,
    pub max_occlusion: f64,
}

impl QualityGate {
    pub const fn defaults() -> Self {
        QualityGate {
            min_det_score: 0.80,
            min_area_ratio: 0.0008,
            min_sharpness: 0.12,
            max_occlusion: 0.55,
        }
    }

    /// Why this face cannot form a person, or `None` if it can. An unmeasured
    /// field never rejects — see [`FaceInput`].
    pub fn reject(&self, face: &FaceInput) -> Option<&'static str> {
        if face.det_score.is_some_and(|v| v < self.min_det_score) {
            Some("low detection confidence")
        } else if face.area_ratio.is_some_and(|v| v < self.min_area_ratio) {
            Some("too small in frame")
        } else if face.sharpness.is_some_and(|v| v < self.min_sharpness) {
            Some("too blurry")
        } else if face.occlusion.is_some_and(|v| v > self.max_occlusion) {
            Some("mostly hidden")
        } else {
            None
        }
    }

    /// True when nothing about this face was measured, so the gate had no
    /// opinion either way. Callers report these — a run that leaned on
    /// unchecked faces produced a weaker answer than it looks like it did.
    pub fn unmeasured(face: &FaceInput) -> bool {
        face.det_score.is_none()
            && face.area_ratio.is_none()
            && face.sharpness.is_none()
            && face.occlusion.is_none()
    }
}

impl Default for QualityGate {
    fn default() -> Self {
        Self::defaults()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ClusterParams {
    /// Cosine distance. Two faces in one person are never further apart than
    /// this (complete linkage makes that a guarantee, not an average).
    pub cosine_threshold: f64,
    /// Clusters smaller than this are dissolved and their faces offered back
    /// to the attach pass. 1 keeps every cluster, including singletons.
    pub min_faces_per_person: usize,
    pub linkage: Linkage,
    pub gate: QualityGate,
    /// How close a gated-out face must be to a finished cluster's centroid,
    /// as a fraction of `cosine_threshold`. Below 1 on purpose: these are the
    /// embeddings that earned the least trust.
    pub attach_factor: f64,
}

impl ClusterParams {
    pub const fn defaults() -> Self {
        ClusterParams {
            cosine_threshold: 0.5,
            min_faces_per_person: 2,
            linkage: Linkage::Complete,
            gate: QualityGate::defaults(),
            attach_factor: 0.9,
        }
    }
}

impl Default for ClusterParams {
    fn default() -> Self {
        Self::defaults()
    }
}

/// One person this run produced.
#[derive(Debug, Clone)]
pub struct ClusterInfo {
    /// The person this cluster was pinned to, when the user built it by hand.
    /// Such a cluster keeps its identity across runs instead of going through
    /// overlap matching.
    pub pinned_person: Option<String>,
    pub size: usize,
    /// Mean cosine distance from a member to the cluster centroid — how tight
    /// this person is. Reported so the UI can show whether the threshold in
    /// use is doing anything sensible.
    pub spread: f64,
    /// Cosine distance to the nearest *other* cluster's centroid. A value at
    /// or under the threshold means these two would have merged if a handful
    /// of faces had fallen differently.
    pub nearest_other: f64,
}

#[derive(Debug, Clone, Default)]
pub struct ClusterStats {
    pub total: usize,
    /// Faces the user pinned to a person.
    pub pinned_faces: usize,
    /// Faces the user explicitly pinned to nobody.
    pub pinned_out: usize,
    /// Faces held back from forming people by [`QualityGate`].
    pub low_quality: usize,
    /// Faces that carry no quality measurements at all, so the gate could not
    /// judge them and let them through unchecked.
    pub unmeasured: usize,
    /// Held-back or dissolved faces that a finished cluster took in.
    pub attached: usize,
    /// Faces released by clusters too small to count as a person.
    pub dropped_small: usize,
    pub unassigned: usize,
    pub cluster_count: usize,
    /// How many canopies the pre-partition produced; 1 when it did not run.
    pub canopies: usize,
    /// Things that went less than perfectly and that the user can act on.
    /// The caller is expected to put these in its HTTP response, not just the
    /// log — a degraded clustering run is not a successful one.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ClusterOutcome {
    /// One entry per input face, in input order. `None` is "no person".
    pub labels: Vec<Option<usize>>,
    pub clusters: Vec<ClusterInfo>,
    pub stats: ClusterStats,
}

fn cosine_distance(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
    (1.0 - dot).clamp(0.0, 2.0)
}

/// Mean of the given rows, scaled back to unit length so that the
/// `L2 = sqrt(2 * cosine)` identity the thresholds rely on still holds.
fn unit_centroid(embeddings: &[&[f32]]) -> Vec<f32> {
    let Some(dim) = embeddings.first().map(|v| v.len()) else {
        return Vec::new();
    };
    let mut sum = vec![0.0f64; dim];
    for v in embeddings {
        for (slot, x) in sum.iter_mut().zip(v.iter()) {
            *slot += *x as f64;
        }
    }
    let norm = sum.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm <= 0.0 {
        return vec![0.0; dim];
    }
    sum.into_iter().map(|x| (x / norm) as f32).collect()
}

/// A face's own quality, used only to decide which members represent a seed
/// and in which order the greedy canopy pass sees things. Not a threshold, so
/// an unmeasured field falls back to a middling value rather than sinking the
/// face to the bottom of every ordering.
fn quality(face: &FaceInput) -> f64 {
    0.4 * face.det_score.unwrap_or(0.8)
        + 0.3 * face.sharpness.unwrap_or(0.5)
        + 0.2 * (1.0 - face.occlusion.unwrap_or(0.0))
        + 0.1 * (face.area_ratio.unwrap_or(0.02) / 0.12).clamp(0.0, 1.0)
}

pub fn cluster_faces(faces: &[FaceInput], params: &ClusterParams) -> ClusterOutcome {
    let mut stats = ClusterStats {
        total: faces.len(),
        canopies: 1,
        ..Default::default()
    };
    let mut labels: Vec<Option<usize>> = vec![None; faces.len()];
    if faces.is_empty() {
        return ClusterOutcome {
            labels,
            clusters: Vec::new(),
            stats,
        };
    }

    // ---- 1. Split the input by what the user decided and what quality allows.
    let mut pinned_groups: HashMap<String, Vec<usize>> = HashMap::new();
    let mut loose: Vec<usize> = Vec::new(); // unpinned, passes the gate
    let mut weak: Vec<usize> = Vec::new(); // unpinned, held back
    for (i, face) in faces.iter().enumerate() {
        match face.pinned.as_deref() {
            Some("") => stats.pinned_out += 1,
            Some(person) => {
                stats.pinned_faces += 1;
                pinned_groups.entry(person.to_string()).or_default().push(i);
            }
            None => {
                if QualityGate::unmeasured(face) {
                    stats.unmeasured += 1;
                }
                if params.gate.reject(face).is_some() {
                    stats.low_quality += 1;
                    weak.push(i);
                } else {
                    loose.push(i);
                }
            }
        }
    }

    // ---- 2. Build the seeds: one per pinned person, one per remaining face.
    let embeddings: Vec<Vec<f32>> = faces.iter().map(|f| f.embedding.clone()).collect();
    let mut seeds: Vec<Seed> = Vec::with_capacity(pinned_groups.len() + loose.len());
    let mut seed_members: Vec<Vec<usize>> = Vec::with_capacity(seeds.capacity());
    let mut seed_person: Vec<Option<String>> = Vec::with_capacity(seeds.capacity());

    // Sorted so a re-run over the same data produces the same clusters.
    let mut pinned_names: Vec<String> = pinned_groups.keys().cloned().collect();
    pinned_names.sort();
    for (group_idx, person) in pinned_names.iter().enumerate() {
        let members = pinned_groups.remove(person).unwrap_or_default();
        let mut by_quality = members.clone();
        by_quality.sort_by(|&a, &b| {
            quality(&faces[b])
                .partial_cmp(&quality(&faces[a]))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(faces[a].id.cmp(&faces[b].id))
        });
        by_quality.truncate(MAX_SEED_REPS);
        seeds.push(Seed {
            reps: by_quality,
            group: Some(group_idx),
        });
        seed_members.push(members);
        seed_person.push(Some(person.clone()));
    }
    for &i in &loose {
        seeds.push(Seed {
            reps: vec![i],
            group: None,
        });
        seed_members.push(vec![i]);
        seed_person.push(None);
    }

    let l2_threshold = cosine_to_l2(params.cosine_threshold);

    // ---- 3. Cluster the seeds, pre-partitioning first if there are too many
    //         for an n x n distance matrix.
    let seed_labels = if seeds.is_empty() {
        Vec::new()
    } else if seeds.len() <= AGGLOMERATIVE_MAX_POINTS {
        agglomerative_seeded(&embeddings, &seeds, l2_threshold, params.linkage)
            .expect("checked against AGGLOMERATIVE_MAX_POINTS above")
    } else {
        let centroids: Vec<Vec<f32>> = seeds
            .iter()
            .map(|s| {
                let rows: Vec<&[f32]> = s.reps.iter().map(|&i| embeddings[i].as_slice()).collect();
                unit_centroid(&rows)
            })
            .collect();
        // Biggest and best first: the greedy pass anchors canopies on the
        // seeds it can place most confidently.
        let mut order: Vec<usize> = (0..seeds.len()).collect();
        order.sort_by(|&a, &b| {
            seeds[b]
                .reps
                .len()
                .cmp(&seeds[a].reps.len())
                .then_with(|| {
                    quality(&faces[seeds[b].reps[0]])
                        .partial_cmp(&quality(&faces[seeds[a].reps[0]]))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| faces[seeds[a].reps[0]].id.cmp(&faces[seeds[b].reps[0]].id))
        });
        // Loose on purpose: this pass only has to keep people who look
        // nothing alike out of each other's way, so that the real,
        // chaining-resistant pass runs on a bounded number of seeds.
        let loose_cosine = (params.cosine_threshold * 1.75).min(0.75);
        let canopy_of = canopy_partition(&centroids, cosine_to_l2(loose_cosine), &order);
        let canopy_count = canopy_of.iter().copied().max().map_or(0, |m| m + 1);
        stats.canopies = canopy_count;
        stats.warnings.push(format!(
            "{} faces is more than the {AGGLOMERATIVE_MAX_POINTS}-group limit for exact \
             clustering, so they were pre-split into {canopy_count} coarse groups first. \
             People whose faces landed in different groups can come out as two entries; \
             merge them on this page and the merge will stick.",
            faces.len()
        ));

        let mut by_canopy: HashMap<usize, Vec<usize>> = HashMap::new();
        for (seed_idx, &c) in canopy_of.iter().enumerate() {
            by_canopy.entry(c).or_default().push(seed_idx);
        }
        let mut out = vec![0i64; seeds.len()];
        let mut next_label = 0i64;
        let mut canopy_ids: Vec<usize> = by_canopy.keys().copied().collect();
        canopy_ids.sort();
        let mut oversized = 0usize;
        for c in canopy_ids {
            let members = &by_canopy[&c];
            let sub: Vec<Seed> = members
                .iter()
                .map(|&s| Seed {
                    reps: seeds[s].reps.clone(),
                    group: seeds[s].group,
                })
                .collect();
            let sub_labels =
                match agglomerative_seeded(&embeddings, &sub, l2_threshold, params.linkage) {
                    Some(l) => l,
                    None => {
                        // One coarse group is still too big for the exact pass.
                        // The greedy centroid walk at the real threshold is a
                        // worse answer than complete linkage, but it is an
                        // answer, and it does not allocate an n x n matrix.
                        oversized += 1;
                        let sub_centroids: Vec<Vec<f32>> =
                            members.iter().map(|&s| centroids[s].clone()).collect();
                        let sub_order: Vec<usize> = {
                            let mut o: Vec<usize> = (0..members.len()).collect();
                            o.sort_by(|&a, &b| sub[b].reps.len().cmp(&sub[a].reps.len()));
                            o
                        };
                        canopy_partition(&sub_centroids, l2_threshold, &sub_order)
                            .into_iter()
                            .map(|v| v as i64)
                            .collect()
                    }
                };
            let mut remap: HashMap<i64, i64> = HashMap::new();
            for (&seed_idx, &lb) in members.iter().zip(sub_labels.iter()) {
                let global = *remap.entry(lb).or_insert_with(|| {
                    let l = next_label;
                    next_label += 1;
                    l
                });
                out[seed_idx] = global;
            }
        }
        if oversized > 0 {
            stats.warnings.push(format!(
                "{oversized} of those coarse groups were still too large to cluster exactly and \
                 fell back to an approximate pass. Their people are more likely to need \
                 merging by hand."
            ));
        }
        out
    };

    // ---- 4. Turn seed labels into clusters of faces.
    let cluster_count = seed_labels
        .iter()
        .copied()
        .max()
        .map_or(0, |m| m as usize + 1);
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); cluster_count];
    let mut pinned_of: Vec<Option<String>> = vec![None; cluster_count];
    for (seed_idx, &lb) in seed_labels.iter().enumerate() {
        let c = lb as usize;
        members[c].extend_from_slice(&seed_members[seed_idx]);
        if let Some(person) = &seed_person[seed_idx] {
            pinned_of[c] = Some(person.clone());
        }
    }

    // ---- 5. Dissolve clusters too small to be a person. A pinned cluster is
    //         the user's statement that this *is* a person, so it survives at
    //         any size.
    let mut leftovers: Vec<usize> = Vec::new();
    let mut kept: Vec<(Vec<usize>, Option<String>)> = Vec::new();
    for (c, member_list) in members.into_iter().enumerate() {
        let pinned = pinned_of[c].clone();
        if pinned.is_none() && member_list.len() < params.min_faces_per_person.max(1) {
            stats.dropped_small += member_list.len();
            leftovers.extend(member_list);
        } else {
            kept.push((member_list, pinned));
        }
    }

    // ---- 6. Attach the held-back and released faces to a finished cluster.
    let centroids: Vec<Vec<f32>> = kept
        .iter()
        .map(|(m, _)| {
            let rows: Vec<&[f32]> = m.iter().map(|&i| embeddings[i].as_slice()).collect();
            unit_centroid(&rows)
        })
        .collect();
    let attach_limit = params.cosine_threshold * params.attach_factor;
    for &i in weak.iter().chain(leftovers.iter()) {
        let mut best = (f64::INFINITY, usize::MAX);
        for (c, centroid) in centroids.iter().enumerate() {
            if centroid.is_empty() {
                continue;
            }
            let d = cosine_distance(&embeddings[i], centroid);
            if d < best.0 {
                best = (d, c);
            }
        }
        if best.1 != usize::MAX && best.0 <= attach_limit {
            kept[best.1].0.push(i);
            stats.attached += 1;
        }
    }

    // ---- 7. Labels and per-cluster diagnostics.
    let mut clusters: Vec<ClusterInfo> = Vec::with_capacity(kept.len());
    for (c, (member_list, pinned)) in kept.iter().enumerate() {
        for &i in member_list {
            labels[i] = Some(c);
        }
        let spread = if centroids[c].is_empty() {
            0.0
        } else {
            member_list
                .iter()
                .map(|&i| cosine_distance(&embeddings[i], &centroids[c]))
                .sum::<f64>()
                / member_list.len() as f64
        };
        clusters.push(ClusterInfo {
            pinned_person: pinned.clone(),
            size: member_list.len(),
            spread,
            nearest_other: f64::INFINITY,
        });
    }
    for a in 0..clusters.len() {
        for b in (a + 1)..clusters.len() {
            if centroids[a].is_empty() || centroids[b].is_empty() {
                continue;
            }
            let d = cosine_distance(&centroids[a], &centroids[b]);
            if d < clusters[a].nearest_other {
                clusters[a].nearest_other = d;
            }
            if d < clusters[b].nearest_other {
                clusters[b].nearest_other = d;
            }
        }
    }

    stats.cluster_count = clusters.len();
    stats.unassigned = labels.iter().filter(|l| l.is_none()).count();
    ClusterOutcome {
        labels,
        clusters,
        stats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit vector at `angle` radians in the first two dimensions. Cosine
    /// distance between two of them is `1 - cos(a - b)`, so tests can state
    /// the distance they want and get it.
    fn at(angle: f64) -> Vec<f32> {
        let mut v = vec![0.0f32; 8];
        v[0] = angle.cos() as f32;
        v[1] = angle.sin() as f32;
        v
    }

    fn good(id: &str, embedding: Vec<f32>) -> FaceInput {
        FaceInput {
            id: id.to_string(),
            embedding,
            det_score: Some(0.95),
            area_ratio: Some(0.05),
            sharpness: Some(0.6),
            occlusion: Some(0.1),
            pinned: None,
        }
    }

    fn bad(id: &str, embedding: Vec<f32>) -> FaceInput {
        FaceInput {
            sharpness: Some(0.01),
            ..good(id, embedding)
        }
    }

    fn groups(outcome: &ClusterOutcome, faces: &[FaceInput]) -> Vec<Vec<String>> {
        let mut by_label: HashMap<usize, Vec<String>> = HashMap::new();
        for (i, label) in outcome.labels.iter().enumerate() {
            if let Some(c) = label {
                by_label.entry(*c).or_default().push(faces[i].id.clone());
            }
        }
        let mut out: Vec<Vec<String>> = by_label
            .into_values()
            .map(|mut v| {
                v.sort();
                v
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn a_bridge_face_cannot_merge_two_people() {
        // Three points in a line, each step just inside the threshold. Density
        // connectivity would call this one person; complete linkage will not,
        // because the two ends are two steps apart.
        // One step is a cosine distance of 0.65; two steps is 1.76.
        let step = 0.35_f64.acos();
        let faces = vec![
            good("a", at(0.0)),
            good("a2", at(0.02)),
            good("bridge", at(step)),
            good("b", at(step * 2.0)),
            good("b2", at(step * 2.0 + 0.02)),
        ];
        let params = ClusterParams {
            cosine_threshold: 1.0 - (step.cos()) + 1e-6,
            min_faces_per_person: 1,
            ..ClusterParams::defaults()
        };
        let outcome = cluster_faces(&faces, &params);
        let a = outcome.labels[0].unwrap();
        let b = outcome.labels[3].unwrap();
        assert_ne!(
            a, b,
            "the two ends are further apart than the threshold and must not share a person"
        );
    }

    #[test]
    fn low_quality_faces_do_not_form_people_but_are_still_placed() {
        // One tight pair of good faces, plus a blurry face right next to them.
        // The blurry one must not be what makes a person, but it should still
        // end up with the person it is nearest to.
        let faces = vec![
            good("sharp1", at(0.0)),
            good("sharp2", at(0.05)),
            bad("blurry", at(0.03)),
        ];
        let params = ClusterParams {
            cosine_threshold: 0.3,
            min_faces_per_person: 2,
            ..ClusterParams::defaults()
        };
        let outcome = cluster_faces(&faces, &params);
        assert_eq!(outcome.stats.low_quality, 1);
        assert_eq!(outcome.stats.attached, 1);
        assert_eq!(
            groups(&outcome, &faces),
            vec![vec!["blurry", "sharp1", "sharp2"]]
        );
    }

    #[test]
    fn a_lone_blurry_face_cannot_invent_a_person() {
        let faces = vec![
            bad("blurry", at(0.0)),
            good("x", at(2.0)),
            good("y", at(2.01)),
        ];
        let params = ClusterParams {
            cosine_threshold: 0.2,
            min_faces_per_person: 2,
            ..ClusterParams::defaults()
        };
        let outcome = cluster_faces(&faces, &params);
        assert_eq!(outcome.labels[0], None);
        assert_eq!(outcome.stats.unassigned, 1);
    }

    #[test]
    fn pinned_faces_stay_with_their_person_however_far_apart_they_are() {
        // Two faces the user assigned to the same person, deliberately placed
        // much further apart than the threshold. A re-cluster must not undo it.
        let mut faces = vec![
            good("p1", at(0.0)),
            good("p2", at(1.2)),
            good("other", at(2.6)),
            good("other2", at(2.61)),
        ];
        faces[0].pinned = Some("person_7".into());
        faces[1].pinned = Some("person_7".into());
        let params = ClusterParams {
            cosine_threshold: 0.2,
            min_faces_per_person: 2,
            ..ClusterParams::defaults()
        };
        let outcome = cluster_faces(&faces, &params);
        assert_eq!(outcome.labels[0], outcome.labels[1]);
        let cluster = &outcome.clusters[outcome.labels[0].unwrap()];
        assert_eq!(cluster.pinned_person.as_deref(), Some("person_7"));
        assert_eq!(outcome.stats.pinned_faces, 2);
    }

    #[test]
    fn two_pinned_people_are_never_merged_into_one() {
        // Identical embeddings, so distance cannot be the thing keeping them
        // apart — only the cannot-link constraint can.
        let mut faces = vec![
            good("a", at(0.0)),
            good("a2", at(0.001)),
            good("b", at(0.002)),
            good("b2", at(0.003)),
        ];
        faces[0].pinned = Some("person_1".into());
        faces[1].pinned = Some("person_1".into());
        faces[2].pinned = Some("person_2".into());
        faces[3].pinned = Some("person_2".into());
        let params = ClusterParams {
            cosine_threshold: 0.9,
            min_faces_per_person: 1,
            ..ClusterParams::defaults()
        };
        let outcome = cluster_faces(&faces, &params);
        assert_ne!(outcome.labels[0], outcome.labels[2]);
        assert_eq!(outcome.clusters.len(), 2);
    }

    #[test]
    fn a_face_pinned_to_nobody_is_left_alone() {
        let mut faces = vec![good("a", at(0.0)), good("b", at(0.01)), good("c", at(0.02))];
        faces[2].pinned = Some(String::new());
        let params = ClusterParams {
            cosine_threshold: 0.5,
            min_faces_per_person: 2,
            ..ClusterParams::defaults()
        };
        let outcome = cluster_faces(&faces, &params);
        assert_eq!(outcome.labels[2], None);
        assert_eq!(outcome.stats.pinned_out, 1);
        assert_eq!(groups(&outcome, &faces), vec![vec!["a", "b"]]);
    }

    #[test]
    fn diagnostics_describe_the_clusters_that_came_out() {
        let faces = vec![
            good("a", at(0.0)),
            good("a2", at(0.05)),
            good("b", at(1.5)),
            good("b2", at(1.55)),
        ];
        let params = ClusterParams {
            cosine_threshold: 0.2,
            min_faces_per_person: 2,
            ..ClusterParams::defaults()
        };
        let outcome = cluster_faces(&faces, &params);
        assert_eq!(outcome.clusters.len(), 2);
        for cluster in &outcome.clusters {
            assert!(cluster.spread >= 0.0 && cluster.spread < 0.05);
            assert!(
                cluster.nearest_other > params.cosine_threshold,
                "two clearly different people should not sit inside the threshold"
            );
        }
    }

    /// A row from before the quality metrics existed carries none of them.
    /// Judging it as if every measurement came back zero would drop it out of
    /// the run entirely — on no evidence at all.
    #[test]
    fn unmeasured_faces_are_not_treated_as_bad_faces() {
        let faces = vec![
            FaceInput {
                id: "old1".into(),
                embedding: at(0.0),
                ..FaceInput::default()
            },
            FaceInput {
                id: "old2".into(),
                embedding: at(0.02),
                ..FaceInput::default()
            },
        ];
        let params = ClusterParams {
            cosine_threshold: 0.3,
            min_faces_per_person: 2,
            ..ClusterParams::defaults()
        };
        let outcome = cluster_faces(&faces, &params);
        assert_eq!(outcome.stats.low_quality, 0);
        assert_eq!(outcome.stats.unmeasured, 2);
        assert_eq!(groups(&outcome, &faces), vec![vec!["old1", "old2"]]);
    }

    #[test]
    fn empty_input_is_not_an_error() {
        let outcome = cluster_faces(&[], &ClusterParams::defaults());
        assert!(outcome.labels.is_empty());
        assert!(outcome.clusters.is_empty());
        assert_eq!(outcome.stats.total, 0);
    }
}
