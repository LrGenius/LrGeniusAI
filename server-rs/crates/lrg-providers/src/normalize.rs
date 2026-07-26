//! Port of `providers/base.py`'s keyword normalization:
//! `_clean_string_list`, `_normalize_keyword_leaf`,
//! `_normalize_keywords_structure`. Cleans up whatever shape the LLM
//! returned (flat list, per-category dict, bilingual/alias objects)
//! into a canonical form: trimmed, de-duplicated case-insensitively,
//! empty containers dropped.

use std::collections::HashSet;

use serde_json::{Map, Value};

/// Port of `_clean_string_list`: trims, drops empties, de-dupes
/// case-insensitively (seeded with `reserved_lower`, e.g. the keyword's
/// own name so a keyword can't list itself as a synonym).
fn clean_string_list(value: Option<&Value>, reserved_lower: &[String]) -> Vec<String> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    let mut seen: HashSet<String> = reserved_lower.iter().cloned().collect();
    let mut cleaned = Vec::new();
    for item in items {
        let Some(text) = item.as_str() else { continue };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let lowered = text.to_lowercase();
        if seen.contains(&lowered) {
            continue;
        }
        seen.insert(lowered);
        cleaned.push(text.to_string());
    }
    cleaned
}

/// Port of `_normalize_keyword_leaf`: a leaf is either a plain string or
/// `{name, synonyms?, aliases?, synonym_aliases?}`. Returns `None` when
/// the leaf has no usable name.
pub fn normalize_keyword_leaf(value: &Value) -> Option<Value> {
    match value {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(Value::String(trimmed.to_string()))
            }
        }
        Value::Object(obj) => {
            let name = obj.get("name")?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            let name_lower = name.to_lowercase();
            let mut out = Map::new();
            out.insert("name".into(), json_str(name));

            let synonyms =
                clean_string_list(obj.get("synonyms"), std::slice::from_ref(&name_lower));
            if !synonyms.is_empty() {
                out.insert(
                    "synonyms".into(),
                    Value::Array(synonyms.iter().map(|s| json_str(s)).collect()),
                );
            }

            let aliases = clean_string_list(obj.get("aliases"), std::slice::from_ref(&name_lower));
            if !aliases.is_empty() {
                out.insert(
                    "aliases".into(),
                    Value::Array(aliases.iter().map(|s| json_str(s)).collect()),
                );
            }

            // synonym_aliases must not collide with the translation names themselves.
            let mut reserved: Vec<String> = vec![name_lower];
            reserved.extend(synonyms.iter().map(|s| s.to_lowercase()));
            let synonym_aliases = clean_string_list(obj.get("synonym_aliases"), &reserved);
            if !synonym_aliases.is_empty() {
                out.insert(
                    "synonym_aliases".into(),
                    Value::Array(synonym_aliases.iter().map(|s| json_str(s)).collect()),
                );
            }

            Some(Value::Object(out))
        }
        _ => None,
    }
}

fn json_str(s: &str) -> Value {
    Value::String(s.to_string())
}

fn is_empty_result(v: &Value) -> bool {
    matches!(v, Value::Null)
        || matches!(v, Value::Object(m) if m.is_empty())
        || matches!(v, Value::Array(a) if a.is_empty())
}

/// Port of `_normalize_keywords_structure`: recursively normalizes
/// whatever shape (list, nested-category dict, or a single leaf) the
/// model returned for the `keywords` field.
pub fn normalize_keywords_structure(value: &Value) -> Value {
    match value {
        Value::Array(items) => {
            let mut out = Vec::new();
            for item in items {
                if let Some(leaf) = normalize_keyword_leaf(item) {
                    out.push(leaf);
                } else if matches!(item, Value::Object(_) | Value::Array(_)) {
                    let nested = normalize_keywords_structure(item);
                    if !is_empty_result(&nested) {
                        out.push(nested);
                    }
                }
            }
            Value::Array(out)
        }
        Value::Object(obj) => {
            if let Some(Value::String(_)) = obj.get("name") {
                return normalize_keyword_leaf(value).unwrap_or(Value::Null);
            }
            let mut out = Map::new();
            for (key, item) in obj {
                let normalized = normalize_keywords_structure(item);
                if is_empty_result(&normalized) {
                    continue;
                }
                out.insert(key.clone(), normalized);
            }
            Value::Object(out)
        }
        other => normalize_keyword_leaf(other).unwrap_or(Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plain_string_list_normalizes_trimmed() {
        let input = json!(["  cat ", "", "dog"]);
        let out = normalize_keywords_structure(&input);
        assert_eq!(out, json!(["cat", "dog"]));
    }

    #[test]
    fn leaf_object_dedupes_case_insensitively_and_excludes_self() {
        let input = json!({"name": "Auto", "synonyms": ["Auto", "Fahrzeug", "fahrzeug"]});
        let out = normalize_keyword_leaf(&input).unwrap();
        assert_eq!(out["name"], "Auto");
        assert_eq!(out["synonyms"], json!(["Fahrzeug"]));
    }

    #[test]
    fn synonym_aliases_cannot_collide_with_synonyms_or_name() {
        let input = json!({
            "name": "Auto",
            "synonyms": ["Vehicle"],
            "synonym_aliases": ["Vehicle", "Car", "auto"]
        });
        let out = normalize_keyword_leaf(&input).unwrap();
        assert_eq!(out["synonym_aliases"], json!(["Car"]));
    }

    #[test]
    fn leaf_with_empty_name_is_dropped() {
        assert!(normalize_keyword_leaf(&json!({"name": "   "})).is_none());
        assert!(normalize_keyword_leaf(&json!({"synonyms": ["x"]})).is_none());
    }

    #[test]
    fn nested_category_dict_drops_empty_branches() {
        let input = json!({
            "People": ["Alice", "Bob"],
            "Places": [],
            "Empty": {}
        });
        let out = normalize_keywords_structure(&input);
        assert_eq!(out, json!({"People": ["Alice", "Bob"]}));
    }

    #[test]
    fn top_level_leaf_object_is_normalized_directly() {
        let input = json!({"name": "Sunset"});
        let out = normalize_keywords_structure(&input);
        assert_eq!(out, json!({"name": "Sunset"}));
    }
}
