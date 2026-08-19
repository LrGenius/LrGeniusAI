//! Hybrid category labels for the flat keyword-group response shape.
//!
//! The old schema mirrored the whole category tree as nested objects with
//! every category in `required`, so a model had to emit every branch —
//! including the empty ones — for every photo. Those empty containers are
//! pure output tokens, and output tokens are the most expensive thing a
//! local engine does.
//!
//! The response shape is now a flat list of `{category, items}` groups, so
//! only categories that actually apply show up. `category` is constrained to
//! the labels built here, which are *hybrid*: a leaf category is named by its
//! bare name when that name is unique in the taxonomy, and by its full
//! `Parent/Child` path only when the bare name would be ambiguous. That keeps
//! the common case short without ever losing which branch was meant.
//!
//! The taxonomy is known input, so the tree is rebuilt server-side in
//! [`crate::normalize`] rather than asked back from the model.

use std::collections::HashMap;

use crate::types::{KeywordCategories, KeywordTree};

/// Separator between path segments in a full-path label. Also the character
/// a model is most likely to reach for unprompted, which makes the lenient
/// lookups in [`CategoryLabels::path_for`] more forgiving.
pub const PATH_SEPARATOR: &str = "/";

/// How one keyword is encoded in the model's answer.
///
/// Named field are readable but expensive: a keyword with a translation and
/// aliases costs 93 characters as an object and 39 as positional arrays, and
/// the difference is decoded one forward pass at a time. Measured on
/// gemma-4-e4b (same photo, same nine keywords): 546 output tokens for the
/// object form against 344 for the arrays — 2.4 seconds of decode on a single
/// photo. What the field names carried is explained once in the prompt
/// instead, where it rides along in the run-constant prefix and is prefilled
/// rather than decoded.
///
/// [`crate::schema`] builds the matching schema and [`crate::normalize`]
/// decodes the answer back into the named form; both derive the variant from
/// the same request flags via [`Self::for_request`], so neither has to guess
/// the shape from the data. That matters because `Aliased` and `Translated`
/// are both flat string arrays and are told apart only by which was asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeywordLeafEncoding {
    /// `"Berg"` — nothing but the keyword itself.
    Plain,
    /// `["Berg", "Gipfel"]` — the keyword, then any further terms for it.
    Aliased,
    /// `["Berg", "mountain"]` — the keyword and its translation.
    Translated,
    /// `[["Berg", "Gipfel"], ["mountain", "peak"]]` — one group per language,
    /// each holding the term first and any further terms after it.
    TranslatedAliased,
}

impl KeywordLeafEncoding {
    pub fn for_request(bilingual: bool, aliases: bool) -> Self {
        match (bilingual, aliases) {
            (false, false) => Self::Plain,
            (false, true) => Self::Aliased,
            (true, false) => Self::Translated,
            (true, true) => Self::TranslatedAliased,
        }
    }
}

/// The leaf categories of a taxonomy plus the label each is addressed by.
#[derive(Debug, Clone, Default)]
pub struct CategoryLabels {
    /// Label plus its full path, in taxonomy order. Order matters: it drives
    /// the schema `enum` and the prompt listing, and a stable order keeps the
    /// grammar cache keyed the same way across photos.
    entries: Vec<(String, Vec<String>)>,
    /// Lowercased lookup key -> index into `entries`. Holds both the hybrid
    /// label and the full path, so a model that volunteers the long form when
    /// the short one was offered still resolves.
    lookup: HashMap<String, usize>,
}

impl CategoryLabels {
    /// Build labels for `categories`. A flat category list is treated as a
    /// tree of depth one, which is exactly what it is.
    pub fn from_categories(categories: &KeywordCategories) -> Self {
        let mut paths: Vec<Vec<String>> = Vec::new();
        match categories {
            KeywordCategories::Flat(list) => {
                for name in list {
                    let name = name.trim();
                    if !name.is_empty() {
                        paths.push(vec![name.to_string()]);
                    }
                }
            }
            KeywordCategories::Nested(tree) => collect_leaves(tree, &mut Vec::new(), &mut paths),
        }

        // A bare leaf name is only usable when nothing else in the taxonomy
        // carries it. Count first, decide second.
        let mut leaf_name_counts: HashMap<String, usize> = HashMap::new();
        for path in &paths {
            if let Some(leaf) = path.last() {
                *leaf_name_counts.entry(leaf.to_lowercase()).or_insert(0) += 1;
            }
        }

        let mut entries: Vec<(String, Vec<String>)> = Vec::new();
        let mut used: HashMap<String, usize> = HashMap::new();
        for path in paths {
            let Some(leaf) = path.last() else { continue };
            let unique = leaf_name_counts
                .get(&leaf.to_lowercase())
                .copied()
                .unwrap_or(0)
                == 1;
            let mut label = if unique {
                leaf.clone()
            } else {
                path.join(PATH_SEPARATOR)
            };
            // A short label can still collide with another entry's full path
            // (a category literally named "Natur/Landschaft", say). Falling
            // back to the full path is enough because two leaves with the
            // same full path are the same leaf.
            if used.contains_key(&label.to_lowercase()) {
                label = path.join(PATH_SEPARATOR);
                if used.contains_key(&label.to_lowercase()) {
                    continue;
                }
            }
            used.insert(label.to_lowercase(), entries.len());
            entries.push((label, path));
        }

        let mut lookup: HashMap<String, usize> = HashMap::new();
        for (idx, (label, path)) in entries.iter().enumerate() {
            lookup.insert(label.to_lowercase(), idx);
            // The full path is always accepted, even when the short label was
            // the one offered. Never let it shadow an existing entry.
            lookup
                .entry(path.join(PATH_SEPARATOR).to_lowercase())
                .or_insert(idx);
        }

        CategoryLabels { entries, lookup }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The labels a model may emit, in taxonomy order.
    pub fn labels(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(label, _)| label.as_str())
    }

    /// Resolve whatever the model emitted back to a full category path.
    ///
    /// Accepts the hybrid label, the full path, and either with stray
    /// whitespace or separator padding — models are not reliable about those
    /// and a near-miss should not cost the photo its keywords.
    pub fn path_for(&self, label: &str) -> Option<&[String]> {
        let trimmed = label.trim().trim_matches('/').trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(idx) = self.lookup.get(&trimmed.to_lowercase()) {
            return Some(&self.entries[*idx].1);
        }
        // Tolerate alternative separators (" > ", " / ", "\") by normalizing
        // to the canonical one before a second lookup.
        let normalized = normalize_separators(trimmed);
        self.lookup
            .get(&normalized.to_lowercase())
            .map(|idx| self.entries[*idx].1.as_slice())
    }
}

/// Depth-first walk collecting the path to every leaf (a node with no
/// children), preserving taxonomy order.
fn collect_leaves(tree: &KeywordTree, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    for (name, children) in tree.iter() {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        prefix.push(name.to_string());
        if children.is_empty() {
            out.push(prefix.clone());
        } else {
            collect_leaves(children, prefix, out);
        }
        prefix.pop();
    }
}

/// Collapse the separators a model plausibly emits into [`PATH_SEPARATOR`].
fn normalize_separators(label: &str) -> String {
    // Segments are trimmed below, so " > " needs no separate pass.
    let unified = label.replace(['>', '\\'], "/");
    unified
        .split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join(PATH_SEPARATOR)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested() -> KeywordCategories {
        // Natur/Landschaft, Natur/Wasser, Reise/Landschaft, Menschen
        KeywordCategories::Nested(KeywordTree(vec![
            (
                "Natur".to_string(),
                KeywordTree(vec![
                    ("Landschaft".to_string(), KeywordTree(vec![])),
                    ("Wasser".to_string(), KeywordTree(vec![])),
                ]),
            ),
            (
                "Reise".to_string(),
                KeywordTree(vec![("Landschaft".to_string(), KeywordTree(vec![]))]),
            ),
            ("Menschen".to_string(), KeywordTree(vec![])),
        ]))
    }

    #[test]
    fn unique_leaf_names_stay_bare_ambiguous_ones_get_the_full_path() {
        let labels = CategoryLabels::from_categories(&nested());
        let got: Vec<&str> = labels.labels().collect();
        assert_eq!(
            got,
            vec!["Natur/Landschaft", "Wasser", "Reise/Landschaft", "Menschen"]
        );
    }

    #[test]
    fn resolves_short_label_full_path_and_alternative_separators() {
        let labels = CategoryLabels::from_categories(&nested());
        assert_eq!(labels.path_for("Wasser").unwrap(), &["Natur", "Wasser"]);
        // The long form is accepted even though "Wasser" was what we offered.
        assert_eq!(
            labels.path_for("Natur/Wasser").unwrap(),
            &["Natur", "Wasser"]
        );
        assert_eq!(
            labels.path_for(" natur > landschaft ").unwrap(),
            &["Natur", "Landschaft"]
        );
        assert!(labels.path_for("Erfunden").is_none());
    }

    #[test]
    fn flat_categories_are_a_depth_one_tree() {
        let categories = KeywordCategories::Flat(vec!["People".to_string(), "Places".to_string()]);
        let labels = CategoryLabels::from_categories(&categories);
        let got: Vec<&str> = labels.labels().collect();
        assert_eq!(got, vec!["People", "Places"]);
        assert_eq!(labels.path_for("places").unwrap(), &["Places"]);
    }

    #[test]
    fn duplicate_flat_categories_collapse_instead_of_duplicating_the_enum() {
        let categories = KeywordCategories::Flat(vec![
            "People".to_string(),
            "People".to_string(),
            "Places".to_string(),
        ]);
        let labels = CategoryLabels::from_categories(&categories);
        let got: Vec<&str> = labels.labels().collect();
        assert_eq!(got, vec!["People", "Places"]);
    }

    #[test]
    fn empty_and_blank_names_are_skipped() {
        let categories = KeywordCategories::Flat(vec!["  ".to_string(), "Real".to_string()]);
        let labels = CategoryLabels::from_categories(&categories);
        assert_eq!(labels.labels().collect::<Vec<_>>(), vec!["Real"]);
    }
}
