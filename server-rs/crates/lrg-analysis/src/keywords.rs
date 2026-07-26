//! Port of `services/keywords.py`'s pure logic: the complete-linkage
//! clique clustering over a keyword cosine-similarity matrix, and the
//! recursive keyword-structure merge-replacement used by
//! `apply_keyword_merges`. Embedding generation (CLIP) and LLM cluster
//! validation are I/O and live in `lrg-api`/`lrg-providers`.
//!
//! Also carries `flatten_keywords_to_string`, ported from
//! `services/index.py::_flatten_keywords` (a different Python module,
//! but the same "keyword structure" domain) — the `flattened_keywords`
//! field it produces is what search substring-matches against and what
//! `replace_in_flattened_keywords` above rewrites during merges.

use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// Normalize + dedupe (case-insensitive) a raw keyword name list, matching
/// `_parse_cluster_request`'s uniqueness pass.
pub fn dedupe_keywords(names: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for name in names {
        let trimmed = name.trim();
        let norm = trimmed.to_lowercase();
        if !norm.is_empty() && seen.insert(norm) {
            unique.push(trimmed.to_string());
        }
    }
    unique
}

/// Clamp the similarity threshold the same way `_parse_cluster_request`
/// does: `max(0.5, min(threshold, 1.0))`.
pub fn clamp_threshold(threshold: f64) -> f64 {
    threshold.clamp(0.5, 1.0)
}

/// Port of `_run_clustering`'s adjacency/clique-building core: given a
/// square cosine-similarity matrix (row-major, `n*n`) and a threshold,
/// find complete-linkage cliques (a candidate joins a cluster only when
/// it's above threshold with *every* existing member — not just one,
/// which is what prevents union-find's transitive-closure chaining).
///
/// Returns groups of >=2 original indices, each sorted ascending.
pub fn cluster_by_similarity(sim_matrix: &[f64], n: usize, threshold: f64) -> Vec<Vec<usize>> {
    assert_eq!(sim_matrix.len(), n * n);
    let sim = |i: usize, j: usize| sim_matrix[i * n + j];

    let mut adj: HashMap<usize, HashSet<usize>> = (0..n).map(|i| (i, HashSet::new())).collect();
    for i in 0..n {
        for j in (i + 1)..n {
            if sim(i, j) >= threshold {
                adj.get_mut(&i).unwrap().insert(j);
                adj.get_mut(&j).unwrap().insert(i);
            }
        }
    }

    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(adj[&i].len()));

    let mut assigned: HashSet<usize> = HashSet::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();

    for &seed in &order {
        if assigned.contains(&seed) || adj[&seed].is_empty() {
            continue;
        }
        let mut cluster: HashSet<usize> = HashSet::from([seed]);
        let mut candidates: Vec<usize> = adj[&seed].iter().copied().collect();
        candidates.sort_by_key(|&i| (std::cmp::Reverse(adj[&i].len()), i));
        for candidate in candidates {
            if assigned.contains(&candidate) || cluster.contains(&candidate) {
                continue;
            }
            if cluster.iter().all(|&m| adj[&m].contains(&candidate)) {
                cluster.insert(candidate);
            }
        }
        if cluster.len() >= 2 {
            let mut members: Vec<usize> = cluster.into_iter().collect();
            members.sort_unstable();
            assigned.extend(members.iter().copied());
            groups.push(members);
        }
    }

    groups
}

/// Port of `_replace_in_keyword_structure`: recursively replace keyword
/// names (case-insensitive lookup) throughout a list/object/string
/// keyword structure, deduping list entries that collide after
/// replacement. `merge_map` keys must already be lowercased.
/// Returns `(new_value, changed)`.
pub fn replace_in_keyword_structure(
    value: &Value,
    merge_map: &HashMap<String, String>,
) -> (Value, bool) {
    match value {
        Value::Array(items) => {
            let mut new_list = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            let mut changed = false;
            for item in items {
                let (new_item, item_changed) = replace_in_keyword_structure(item, merge_map);
                if item_changed {
                    changed = true;
                }
                let norm = new_item.as_str().map(str::to_lowercase);
                match &norm {
                    Some(n) if seen.contains(n) => {
                        changed = true; // duplicate removed
                    }
                    _ => {
                        if let Some(n) = norm {
                            seen.insert(n);
                        }
                        new_list.push(new_item);
                    }
                }
            }
            (Value::Array(new_list), changed)
        }
        Value::Object(map) => {
            let mut new_map = serde_json::Map::new();
            let mut changed = false;
            for (k, v) in map {
                let (new_v, sub_changed) = replace_in_keyword_structure(v, merge_map);
                if sub_changed {
                    changed = true;
                }
                new_map.insert(k.clone(), new_v);
            }
            (Value::Object(new_map), changed)
        }
        Value::String(s) => match merge_map.get(&s.to_lowercase()) {
            Some(replacement) if replacement != s => (Value::String(replacement.clone()), true),
            _ => (value.clone(), false),
        },
        other => (other.clone(), false),
    }
}

/// Build the lowercased duplicate->canonical map from `{duplicate, canonical}`
/// pairs, matching `apply_keyword_merges`'s validation (skip empty/no-op pairs).
pub fn build_merge_map(merges: &[(String, String)]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (dup, can) in merges {
        let dup = dup.trim();
        let can = can.trim();
        if !dup.is_empty() && !can.is_empty() && dup.to_lowercase() != can.to_lowercase() {
            map.insert(dup.to_lowercase(), can.to_string());
        }
    }
    map
}

/// Port of the `flattened_keywords` comma-separated-string branch of
/// `apply_keyword_merges`. Returns `(new_value, changed)`.
pub fn replace_in_flattened_keywords(
    flat: &str,
    merge_map: &HashMap<String, String>,
) -> (String, bool) {
    if flat.is_empty() {
        return (String::new(), false);
    }
    let mut new_parts: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut changed = false;
    for part in flat.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        let replacement = merge_map
            .get(&p.to_lowercase())
            .cloned()
            .unwrap_or_else(|| p.to_string());
        let norm = replacement.to_lowercase();
        if seen.insert(norm) {
            if replacement != p {
                changed = true;
            }
            new_parts.push(replacement);
        } else {
            changed = true; // deduplicated
        }
    }
    (new_parts.join(", "), changed)
}

fn normalize_keyword_text(value: &Value, seen: &mut HashSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    let push_unique = |out: &mut Vec<String>, seen: &mut HashSet<String>, text: &str| {
        let text = text.trim();
        if !text.is_empty() && seen.insert(text.to_lowercase()) {
            out.push(text.to_string());
        }
    };
    match value {
        Value::String(s) => push_unique(&mut out, seen, s),
        Value::Object(map) => {
            if let Some(name) = map.get("name").and_then(Value::as_str) {
                push_unique(&mut out, seen, name);
            }
            for field in ["synonyms", "aliases", "synonym_aliases"] {
                if let Some(Value::Array(bucket)) = map.get(field) {
                    for entry in bucket {
                        if let Some(s) = entry.as_str() {
                            push_unique(&mut out, seen, s);
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}

fn collect_nested_keywords(
    map: &Map<String, Value>,
    seen: &mut HashSet<String>,
    out: &mut Vec<String>,
) {
    for value in map.values() {
        match value {
            Value::Array(items) => {
                for kw in items {
                    out.extend(normalize_keyword_text(kw, seen));
                }
            }
            Value::Object(m) if !m.is_empty() => {
                if m.contains_key("name") {
                    out.extend(normalize_keyword_text(value, seen));
                } else {
                    collect_nested_keywords(m, seen, out);
                }
            }
            _ => out.extend(normalize_keyword_text(value, seen)),
        }
    }
}

/// Port of `services/index.py::_flatten_keywords`: flatten a flat list,
/// nested category dict, or already-flat string of keywords (plain
/// strings or `{name, synonyms, aliases, synonym_aliases}` objects) into
/// a single deduped (case-insensitive), comma-separated display string.
pub fn flatten_keywords_to_string(keywords: &Value) -> String {
    let is_falsy = match keywords {
        Value::Null => true,
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::String(s) => s.is_empty(),
        Value::Bool(false) => true,
        _ => false,
    };
    if is_falsy {
        return String::new();
    }
    if let Value::String(s) = keywords {
        return s.clone();
    }

    let mut seen = HashSet::new();
    match keywords {
        Value::Array(items) => {
            let mut flattened = Vec::new();
            for kw in items {
                flattened.extend(normalize_keyword_text(kw, &mut seen));
            }
            flattened.join(", ")
        }
        Value::Object(map) => {
            let mut all = Vec::new();
            collect_nested_keywords(map, &mut seen, &mut all);
            all.join(", ")
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dedupe_keywords_is_case_insensitive_and_trims() {
        let input = vec![
            "Cat".to_string(),
            " cat ".to_string(),
            "Dog".to_string(),
            "".to_string(),
        ];
        assert_eq!(
            dedupe_keywords(&input),
            vec!["Cat".to_string(), "Dog".to_string()]
        );
    }

    #[test]
    fn clamp_threshold_bounds_to_range() {
        assert_eq!(clamp_threshold(0.1), 0.5);
        assert_eq!(clamp_threshold(1.5), 1.0);
        assert_eq!(clamp_threshold(0.9), 0.9);
    }

    #[test]
    fn clique_clustering_requires_mutual_similarity_to_all_members() {
        // 0-1, 0-2, 1-2 all similar -> one triangle cluster.
        // 3 is similar to 0 only, so it must NOT join {0,1,2}.
        let n = 4;
        #[rustfmt::skip]
        let sim = vec![
            1.0, 0.9, 0.9, 0.9,
            0.9, 1.0, 0.9, 0.1,
            0.9, 0.9, 1.0, 0.1,
            0.9, 0.1, 0.1, 1.0,
        ];
        let groups = cluster_by_similarity(&sim, n, 0.85);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], vec![0, 1, 2]);
    }

    #[test]
    fn clique_clustering_ignores_below_threshold_pairs() {
        let n = 3;
        #[rustfmt::skip]
        let sim = vec![
            1.0, 0.5, 0.5,
            0.5, 1.0, 0.5,
            0.5, 0.5, 1.0,
        ];
        let groups = cluster_by_similarity(&sim, n, 0.85);
        assert!(groups.is_empty());
    }

    #[test]
    fn replace_in_keyword_structure_dedupes_and_replaces_case_insensitively() {
        let merge_map: HashMap<String, String> = [("automobile".to_string(), "Car".to_string())]
            .into_iter()
            .collect();
        let value = json!(["Car", "Automobile", "Truck"]);
        let (result, changed) = replace_in_keyword_structure(&value, &merge_map);
        assert!(changed);
        assert_eq!(result, json!(["Car", "Truck"]));
    }

    #[test]
    fn replace_in_keyword_structure_recurses_into_nested_objects() {
        let merge_map: HashMap<String, String> = [("kitty".to_string(), "Cat".to_string())]
            .into_iter()
            .collect();
        let value = json!({"animals": ["Kitty", "Dog"]});
        let (result, changed) = replace_in_keyword_structure(&value, &merge_map);
        assert!(changed);
        assert_eq!(result, json!({"animals": ["Cat", "Dog"]}));
    }

    #[test]
    fn build_merge_map_skips_empty_and_noop_pairs() {
        let merges = vec![
            ("Automobile".to_string(), "Car".to_string()),
            ("".to_string(), "Car".to_string()),
            ("Same".to_string(), "same".to_string()),
        ];
        let map = build_merge_map(&merges);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("automobile"), Some(&"Car".to_string()));
    }

    #[test]
    fn flattened_keywords_replace_and_dedupe() {
        let merge_map: HashMap<String, String> = [("automobile".to_string(), "Car".to_string())]
            .into_iter()
            .collect();
        let (result, changed) = replace_in_flattened_keywords("Car, Automobile, Truck", &merge_map);
        assert!(changed);
        assert_eq!(result, "Car, Truck");
    }

    #[test]
    fn flattened_keywords_no_change_when_nothing_matches() {
        let merge_map: HashMap<String, String> = HashMap::new();
        let (result, changed) = replace_in_flattened_keywords("Car, Truck", &merge_map);
        assert!(!changed);
        assert_eq!(result, "Car, Truck");
    }

    #[test]
    fn flatten_keywords_empty_and_string_passthrough() {
        assert_eq!(flatten_keywords_to_string(&Value::Null), "");
        assert_eq!(flatten_keywords_to_string(&json!([])), "");
        assert_eq!(flatten_keywords_to_string(&json!({})), "");
        assert_eq!(flatten_keywords_to_string(&json!("Car, Dog")), "Car, Dog");
    }

    #[test]
    fn flatten_keywords_flat_list_of_strings() {
        assert_eq!(
            flatten_keywords_to_string(&json!(["Car", "Dog", "Car"])),
            "Car, Dog"
        );
    }

    #[test]
    fn flatten_keywords_structured_objects_include_synonyms_and_aliases() {
        let value = json!([
            {"name": "Auto", "synonyms": ["Car"], "aliases": ["Kraftfahrzeug"]},
            "Dog",
        ]);
        assert_eq!(
            flatten_keywords_to_string(&value),
            "Auto, Car, Kraftfahrzeug, Dog"
        );
    }

    #[test]
    fn flatten_keywords_nested_category_dict_recurses() {
        let value = json!({
            "People": {"Family": ["Mom", "Dad"]},
            "Places": ["Beach"],
        });
        let result = flatten_keywords_to_string(&value);
        let mut parts: Vec<&str> = result.split(", ").collect();
        parts.sort_unstable();
        assert_eq!(parts, vec!["Beach", "Dad", "Mom"]);
    }

    #[test]
    fn flatten_keywords_dict_leaf_with_name_is_not_recursed_into() {
        // A dict value that itself looks like a {name, synonyms} keyword
        // object (not a further category) must be treated as a leaf.
        let value = json!({"Category": {"name": "Sunset", "synonyms": ["Dusk"]}});
        assert_eq!(flatten_keywords_to_string(&value), "Sunset, Dusk");
    }
}
