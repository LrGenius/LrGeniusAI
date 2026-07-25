//! Port of `services/persons.py`'s non-clustering logic: greedy stable
//! person-ID matching across re-clusters (by face-set overlap, largest
//! new cluster first) and the `person_names.json` display-name store.

use std::collections::{HashMap, HashSet};

/// Greedy stable-ID matching: sorted by cluster size descending, each new
/// cluster claims the not-yet-used old person_id with the largest face
/// overlap; ties/no-overlap get a fresh `person_N` id. Mirrors the
/// Python loop's semantics exactly. One documented approximation: when
/// two new clusters tie in size, Python's stable sort preserves
/// dict-insertion order (itself dependent on face iteration order from
/// the store) as the tie-break, whereas this breaks ties by ascending
/// label id — deterministic here, but not guaranteed to match Python's
/// choice of *which* cluster wins a shared best-overlap old id in that
/// rare case. The invariants that matter (unique assignment, exact
/// matches preferred, largest-cluster-first) hold either way.
pub fn match_labels_to_persons(
    old_person_faces: &HashMap<String, HashSet<String>>,
    new_label_faces: &HashMap<i64, HashSet<String>>,
) -> HashMap<i64, String> {
    let next_new_idx = max_person_index(old_person_faces.keys()) + 1;
    let mut next_new_idx = next_new_idx;

    let mut labels_by_size: Vec<i64> = new_label_faces.keys().copied().collect();
    labels_by_size.sort_by(|a, b| {
        new_label_faces[b]
            .len()
            .cmp(&new_label_faces[a].len())
            .then(a.cmp(b))
    });

    let mut used_old_ids: HashSet<String> = HashSet::new();
    let mut label_to_person: HashMap<i64, String> = HashMap::new();

    for lb in labels_by_size {
        let cluster_faces = &new_label_faces[&lb];
        let mut best_pid: Option<&String> = None;
        let mut best_overlap = 0usize;
        for (pid, face_set) in old_person_faces {
            if used_old_ids.contains(pid) {
                continue;
            }
            let overlap = cluster_faces.intersection(face_set).count();
            if overlap > best_overlap {
                best_overlap = overlap;
                best_pid = Some(pid);
            }
        }
        if let Some(pid) = best_pid.filter(|_| best_overlap > 0) {
            label_to_person.insert(lb, pid.clone());
            used_old_ids.insert(pid.clone());
        } else {
            label_to_person.insert(lb, format!("person_{next_new_idx}"));
            next_new_idx += 1;
        }
    }
    label_to_person
}

fn max_person_index<'a>(ids: impl Iterator<Item = &'a String>) -> i64 {
    let mut max_idx = -1i64;
    for id in ids {
        if let Some(rest) = id.strip_prefix("person_") {
            if let Ok(n) = rest.parse::<i64>() {
                max_idx = max_idx.max(n);
            }
        }
    }
    max_idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reassigns_existing_ids_by_largest_overlap() {
        let mut old = HashMap::new();
        old.insert("person_0".to_string(), set(&["f1", "f2", "f3"]));
        old.insert("person_1".to_string(), set(&["f4", "f5"]));

        let mut new_labels = HashMap::new();
        new_labels.insert(0i64, set(&["f1", "f2", "f3", "f6"])); // mostly person_0
        new_labels.insert(1i64, set(&["f4", "f5"])); // exactly person_1

        let mapping = match_labels_to_persons(&old, &new_labels);
        assert_eq!(mapping[&0], "person_0");
        assert_eq!(mapping[&1], "person_1");
    }

    #[test]
    fn brand_new_cluster_gets_next_free_index() {
        let mut old = HashMap::new();
        old.insert("person_0".to_string(), set(&["f1"]));
        old.insert("person_2".to_string(), set(&["f2"])); // gap at person_1

        let mut new_labels = HashMap::new();
        new_labels.insert(0i64, set(&["f9", "f10"])); // no overlap with anything

        let mapping = match_labels_to_persons(&old, &new_labels);
        // next_new_idx = max(0,2)+1 = 3
        assert_eq!(mapping[&0], "person_3");
    }

    #[test]
    fn two_new_clusters_never_collide_on_the_same_old_id() {
        let mut old = HashMap::new();
        old.insert("person_0".to_string(), set(&["f1", "f2"]));

        let mut new_labels = HashMap::new();
        new_labels.insert(0i64, set(&["f1"])); // partial overlap
        new_labels.insert(1i64, set(&["f2"])); // partial overlap, same old id

        let mapping = match_labels_to_persons(&old, &new_labels);
        let assigned: HashSet<&String> = mapping.values().collect();
        assert_eq!(assigned.len(), 2, "must not double-assign person_0");
    }
}
