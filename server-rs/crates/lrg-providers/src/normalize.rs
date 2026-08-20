//! Port of `providers/base.py`'s keyword normalization:
//! `_clean_string_list`, `_normalize_keyword_leaf`,
//! `_normalize_keywords_structure`. Cleans up whatever shape the LLM
//! returned (flat list, per-category dict, bilingual/alias objects)
//! into a canonical form: trimmed, de-duplicated case-insensitively,
//! empty containers dropped.

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::keyword_taxonomy::{CategoryLabels, KeywordLeafEncoding};
use crate::types::{KeywordCategories, MetadataGenerationRequest};

/// Turn one positional keyword leaf back into the named form the rest of this
/// module works in (`{name, synonyms, aliases, synonym_aliases}`).
///
/// This has to run *before* [`normalize_keywords_structure`], which treats
/// every array as a container and would otherwise scatter one keyword's terms
/// across several keywords.
///
/// Deliberately lenient in both directions. A model that ignored the schema
/// and sent the old named object, or a bare string, is passed straight
/// through — and it has to be, because `schema_strict` strips the
/// `minItems`/`maxItems` bound for OpenAI, so on that path nothing enforces
/// the group count. Under-filled shapes degrade to the next simpler reading
/// rather than being dropped: half an answer beats none.
fn expand_leaf(value: &Value, encoding: KeywordLeafEncoding) -> Value {
    let Value::Array(slots) = value else {
        // A string, or an object from a model that answered in the old shape.
        return value.clone();
    };
    if encoding == KeywordLeafEncoding::Plain {
        return value.clone();
    }

    // `[["Berg","Gipfel"],["mountain","peak"]]` — one group per language.
    if encoding == KeywordLeafEncoding::TranslatedAliased
        && slots.iter().any(|s| matches!(s, Value::Array(_)))
    {
        let group = |i: usize| match slots.get(i) {
            Some(Value::Array(terms)) => terms.clone(),
            _ => Vec::new(),
        };
        let (primary, secondary) = (group(0), group(1));
        let Some(name) = primary.first() else {
            return Value::Null;
        };
        let mut out = Map::new();
        out.insert("name".into(), name.clone());
        if primary.len() > 1 {
            out.insert("aliases".into(), Value::Array(primary[1..].to_vec()));
        }
        if let Some((first, rest)) = secondary.split_first() {
            out.insert("synonyms".into(), Value::Array(vec![first.clone()]));
            if !rest.is_empty() {
                out.insert("synonym_aliases".into(), Value::Array(rest.to_vec()));
            }
        }
        return Value::Object(out);
    }

    // A flat string list. Which field the tail belongs to is not visible in
    // the data — `["Berg","Gipfel"]` and `["Berg","mountain"]` are the same
    // JSON — so it is decided by what was asked for, never by inspection.
    let Some((name, rest)) = slots.split_first() else {
        return Value::Null;
    };
    let mut out = Map::new();
    out.insert("name".into(), name.clone());
    if !rest.is_empty() {
        let field = match encoding {
            KeywordLeafEncoding::Aliased => "aliases",
            // Includes a `TranslatedAliased` answer that arrived flat: the
            // translation is the one thing that must not be lost.
            _ => "synonyms",
        };
        out.insert(field.into(), Value::Array(rest.to_vec()));
    }
    Value::Object(out)
}

/// `expand_leaf` across a list of keywords, dropping what it could not read.
fn expand_leaf_list(value: &Value, encoding: KeywordLeafEncoding) -> Value {
    let Value::Array(items) = value else {
        return value.clone();
    };
    Value::Array(
        items
            .iter()
            .map(|item| expand_leaf(item, encoding))
            .filter(|item| !item.is_null())
            .collect(),
    )
}

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

/// Insert `items` at `path`, creating intermediate objects and merging into
/// an array that is already there.
fn insert_at_path(root: &mut Map<String, Value>, path: &[String], items: Vec<Value>) {
    let Some((leaf, parents)) = path.split_last() else {
        return;
    };
    let mut cursor = root;
    for segment in parents {
        let slot = cursor
            .entry(segment.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        // A taxonomy where one path is both a leaf and a parent is malformed;
        // preferring the parent keeps the deeper keywords rather than the
        // shallower ones.
        if !slot.is_object() {
            *slot = Value::Object(Map::new());
        }
        cursor = slot.as_object_mut().expect("set to an object just above");
    }
    match cursor
        .entry(leaf.clone())
        .or_insert_with(|| Value::Array(Vec::new()))
    {
        Value::Array(existing) => existing.extend(items),
        other => *other = Value::Array(items),
    }
}

/// Rebuild the nested category tree from the flat `[{category, items}]`
/// groups the model now returns.
///
/// The taxonomy is known input, so the model is asked only *which* category
/// applies, never to re-emit the tree around it — that redundancy was pure
/// output tokens. See [`crate::keyword_taxonomy`].
///
/// Anything that is not the group shape passes through untouched: a bare
/// keyword list (no taxonomy configured), or an older nested object from a
/// model that ignored the schema. A category that does not resolve keeps its
/// keywords under its own name rather than dropping them — the same thing
/// that used to happen when a model invented a category.
pub fn rebuild_keyword_groups(
    value: &Value,
    labels: &CategoryLabels,
    encoding: KeywordLeafEncoding,
) -> Value {
    let Value::Array(groups) = value else {
        return value.clone();
    };
    // A bare keyword list is an array too. Only treat this as groups when the
    // entries actually look like groups.
    let looks_like_groups = groups.iter().any(|g| {
        g.as_object()
            .is_some_and(|o| o.contains_key("category") && o.contains_key("items"))
    });
    if !looks_like_groups {
        return value.clone();
    }

    let mut root = Map::new();
    for group in groups {
        let Some(obj) = group.as_object() else {
            continue;
        };
        let Some(category) = obj.get("category").and_then(Value::as_str) else {
            continue;
        };
        let items = match obj.get("items") {
            Some(items @ Value::Array(_)) => match expand_leaf_list(items, encoding) {
                Value::Array(items) => items,
                _ => continue,
            },
            _ => continue,
        };
        if items.is_empty() {
            continue;
        }
        match labels.path_for(category) {
            Some(path) => insert_at_path(&mut root, path, items),
            None => {
                let fallback = category.trim();
                if !fallback.is_empty() {
                    insert_at_path(&mut root, &[fallback.to_string()], items);
                }
            }
        }
    }
    Value::Object(root)
}

/// Alt text for the response, derived from the caption when the model was
/// not asked for it separately.
///
/// Alt text and caption are near-identical prose about the same photo, and
/// prose is the most expensive thing a model emits. When both are requested
/// the schema asks only for `caption` (see `schema.rs`) and the caption is
/// reused here. An explicitly returned `alt_text` still wins, so a model that
/// volunteers one — or a provider that ignored the schema — loses nothing.
pub fn alt_text_from(
    parsed: &Value,
    generate_alt_text: bool,
    caption: Option<&String>,
) -> Option<String> {
    if !generate_alt_text {
        return None;
    }
    parsed
        .get("alt_text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| caption.cloned())
}

/// The one entry point providers should call for the `keywords` field:
/// expand the positional leaves, rebuild the tree from flat groups (when a
/// taxonomy was requested), then normalize.
///
/// `encoding` must be the one `schema.rs` built the request with — it is what
/// says whether the tail of `["Berg","Gipfel"]` is an alias or a translation,
/// and no amount of looking at the value can tell.
pub fn normalize_keywords(
    value: &Value,
    categories: Option<&KeywordCategories>,
    encoding: KeywordLeafEncoding,
) -> Value {
    match categories {
        Some(categories) => {
            let labels = CategoryLabels::from_categories(categories);
            if labels.is_empty() {
                return normalize_keywords_structure(&expand_leaf_list(value, encoding));
            }
            normalize_keywords_structure(&rebuild_keyword_groups(value, &labels, encoding))
        }
        None => normalize_keywords_structure(&expand_leaf_list(value, encoding)),
    }
}

/// Names the fields the caller asked for that the model did not deliver.
///
/// This is the metadata path's "degraded success": `success: true` with a
/// caption the user requested and did not get. Nothing about the response
/// distinguishes that from a photo the model simply had nothing to say about,
/// and until this existed nothing reported it — the field was written as
/// `warning: None` by every provider and the count of indexed photos went up
/// either way.
///
/// Returns `None` when everything requested came back, so the common case
/// stays quiet. Keywords count as missing when the container is empty, not
/// just when the key is absent: an empty array is what a model returns when it
/// declines, and `finish_one` drops it without storing anything.
///
/// Deliberately one shared function rather than a copy per provider. The five
/// providers already parse their responses six near-identical ways, and a
/// warning only some of them emit would be worse than none — the user would
/// learn that Gemini sometimes skips captions and never that Ollama does too.
pub fn missing_field_warning(
    request: &MetadataGenerationRequest,
    keywords: Option<&Value>,
    caption: Option<&String>,
    title: Option<&String>,
    alt_text: Option<&String>,
) -> Option<String> {
    fn empty(text: Option<&String>) -> bool {
        text.map(|s| s.trim().is_empty()).unwrap_or(true)
    }
    let keywords_empty = match keywords {
        None => true,
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
        Some(_) => false,
    };

    let mut missing = Vec::new();
    if request.generate_keywords && keywords_empty {
        missing.push("keywords");
    }
    if request.generate_caption && empty(caption) {
        missing.push("caption");
    }
    if request.generate_title && empty(title) {
        missing.push("title");
    }
    if request.generate_alt_text && empty(alt_text) {
        missing.push("alt text");
    }
    if missing.is_empty() {
        return None;
    }
    // Phrased as something to act on rather than as an internal state: the
    // two fixes that actually work are a different model and a longer token
    // budget, so the message names them.
    Some(format!(
        "the model returned no {} for this photo — the rest was kept. \
         A stronger model, or a higher Max Tokens setting, usually fixes this.",
        join_human(&missing)
    ))
}

/// "a", "a and b", "a, b and c" — so the warning reads like a sentence.
fn join_human(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::KeywordTree;
    use serde_json::json;

    fn taxonomy() -> KeywordCategories {
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
        ]))
    }

    #[test]
    fn groups_rebuild_into_the_nested_tree() {
        let raw = json!([
            {"category": "Wasser", "items": ["See", "Fluss"]},
            {"category": "Natur/Landschaft", "items": ["Berg"]},
        ]);
        let out = normalize_keywords(&raw, Some(&taxonomy()), KeywordLeafEncoding::Plain);
        assert_eq!(
            out,
            json!({"Natur": {"Wasser": ["See", "Fluss"], "Landschaft": ["Berg"]}})
        );
    }

    #[test]
    fn ambiguous_leaf_names_land_in_the_right_branch() {
        let raw = json!([
            {"category": "Natur/Landschaft", "items": ["Berg"]},
            {"category": "Reise/Landschaft", "items": ["Hotel"]},
        ]);
        let out = normalize_keywords(&raw, Some(&taxonomy()), KeywordLeafEncoding::Plain);
        assert_eq!(out["Natur"]["Landschaft"], json!(["Berg"]));
        assert_eq!(out["Reise"]["Landschaft"], json!(["Hotel"]));
    }

    #[test]
    fn repeated_categories_merge_and_empty_groups_vanish() {
        let raw = json!([
            {"category": "Wasser", "items": ["See"]},
            {"category": "Wasser", "items": ["Fluss"]},
            {"category": "Natur/Landschaft", "items": []},
        ]);
        let out = normalize_keywords(&raw, Some(&taxonomy()), KeywordLeafEncoding::Plain);
        assert_eq!(out, json!({"Natur": {"Wasser": ["See", "Fluss"]}}));
    }

    #[test]
    fn an_unknown_category_keeps_its_keywords() {
        let raw = json!([{"category": "Erfunden", "items": ["Etwas"]}]);
        let out = normalize_keywords(&raw, Some(&taxonomy()), KeywordLeafEncoding::Plain);
        assert_eq!(out, json!({"Erfunden": ["Etwas"]}));
    }

    #[test]
    fn a_bare_keyword_list_passes_through_untouched() {
        let raw = json!(["Berg", "See"]);
        let out = normalize_keywords(&raw, Some(&taxonomy()), KeywordLeafEncoding::Plain);
        assert_eq!(out, json!(["Berg", "See"]));
    }

    #[test]
    fn a_model_that_returns_the_old_nested_shape_still_works() {
        let raw = json!({"Natur": {"Wasser": ["See"]}});
        let out = normalize_keywords(&raw, Some(&taxonomy()), KeywordLeafEncoding::Plain);
        assert_eq!(out, json!({"Natur": {"Wasser": ["See"]}}));
    }

    #[test]
    fn leaf_objects_inside_groups_are_normalized() {
        let raw = json!([
            {"category": "Wasser", "items": [{"name": "See", "synonyms": ["lake", "See"]}]},
        ]);
        let out = normalize_keywords(&raw, Some(&taxonomy()), KeywordLeafEncoding::Plain);
        assert_eq!(out["Natur"]["Wasser"][0]["name"], "See");
        assert_eq!(out["Natur"]["Wasser"][0]["synonyms"], json!(["lake"]));
    }

    #[test]
    fn positional_leaf_expands_into_both_languages() {
        let raw = json!([{
            "category": "Wasser",
            "items": [[["See", "Gewässer"], ["lake", "pond"]]],
        }]);
        let out = normalize_keywords(
            &raw,
            Some(&taxonomy()),
            KeywordLeafEncoding::TranslatedAliased,
        );
        let leaf = &out["Natur"]["Wasser"][0];
        assert_eq!(leaf["name"], "See");
        assert_eq!(leaf["aliases"], json!(["Gewässer"]));
        assert_eq!(leaf["synonyms"], json!(["lake"]));
        assert_eq!(leaf["synonym_aliases"], json!(["pond"]));
    }

    #[test]
    fn the_same_flat_pair_reads_differently_per_encoding() {
        // `["See","lake"]` carries no hint of what its tail is. Only the
        // encoding the request was built with can say, which is why it is
        // threaded through rather than sniffed.
        let raw = json!([{"category": "Wasser", "items": [["See", "lake"]]}]);
        let aliased = normalize_keywords(&raw, Some(&taxonomy()), KeywordLeafEncoding::Aliased);
        assert_eq!(aliased["Natur"]["Wasser"][0]["aliases"], json!(["lake"]));
        assert!(aliased["Natur"]["Wasser"][0].get("synonyms").is_none());

        let translated =
            normalize_keywords(&raw, Some(&taxonomy()), KeywordLeafEncoding::Translated);
        assert_eq!(
            translated["Natur"]["Wasser"][0]["synonyms"],
            json!(["lake"])
        );
        assert!(translated["Natur"]["Wasser"][0].get("aliases").is_none());
    }

    #[test]
    fn a_leaf_that_lost_its_second_group_keeps_the_translation() {
        // `schema_strict` drops minItems/maxItems for OpenAI, so a flat answer
        // where two groups were asked for is possible. The translation is the
        // thing that must not be misfiled as an alias.
        let raw = json!([{"category": "Wasser", "items": [["See", "lake"]]}]);
        let out = normalize_keywords(
            &raw,
            Some(&taxonomy()),
            KeywordLeafEncoding::TranslatedAliased,
        );
        assert_eq!(out["Natur"]["Wasser"][0]["synonyms"], json!(["lake"]));
    }

    #[test]
    fn a_model_answering_in_the_old_named_shape_is_still_understood() {
        // Nothing forces a cloud model to follow the positional layout.
        let raw = json!([{
            "category": "Wasser",
            "items": [{"name": "See", "synonyms": ["lake"]}],
        }]);
        let out = normalize_keywords(
            &raw,
            Some(&taxonomy()),
            KeywordLeafEncoding::TranslatedAliased,
        );
        assert_eq!(out["Natur"]["Wasser"][0]["name"], "See");
        assert_eq!(out["Natur"]["Wasser"][0]["synonyms"], json!(["lake"]));
    }

    #[test]
    fn positional_leaves_dedupe_like_the_named_ones() {
        // Models pad the alias slot by repeating the keyword — measured on
        // gemma-4 with `[["Astronautin","Astronaut"],["astronaut","astronaut"]]`.
        let raw = json!([{
            "category": "Wasser",
            "items": [[["See", "see"], ["lake", "lake"]]],
        }]);
        let out = normalize_keywords(
            &raw,
            Some(&taxonomy()),
            KeywordLeafEncoding::TranslatedAliased,
        );
        let leaf = &out["Natur"]["Wasser"][0];
        assert!(leaf.get("aliases").is_none(), "self-repeat is not an alias");
        assert_eq!(leaf["synonyms"], json!(["lake"]));
        assert!(leaf.get("synonym_aliases").is_none());
    }

    #[test]
    fn positional_leaves_survive_without_a_taxonomy() {
        let raw = json!([["Berg", "mountain"], ["See", "lake"]]);
        let out = normalize_keywords(&raw, None, KeywordLeafEncoding::Translated);
        assert_eq!(out[0]["name"], "Berg");
        assert_eq!(out[0]["synonyms"], json!(["mountain"]));
        assert_eq!(out[1]["name"], "See");
    }

    #[test]
    fn an_empty_positional_leaf_is_dropped_not_kept_as_a_blank() {
        let raw = json!([{"category": "Wasser", "items": [[], ["See", "lake"]]}]);
        let out = normalize_keywords(&raw, Some(&taxonomy()), KeywordLeafEncoding::Translated);
        assert_eq!(out["Natur"]["Wasser"].as_array().map(Vec::len), Some(1));
        assert_eq!(out["Natur"]["Wasser"][0]["name"], "See");
    }

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

    /// A request that asked for everything, so each case below only has to say
    /// what came back.
    fn asked_for_everything() -> MetadataGenerationRequest {
        MetadataGenerationRequest {
            generate_keywords: true,
            generate_caption: true,
            generate_title: true,
            generate_alt_text: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_complete_response_warns_about_nothing() {
        let got = missing_field_warning(
            &asked_for_everything(),
            Some(&json!(["berg"])),
            Some(&"a caption".to_string()),
            Some(&"a title".to_string()),
            Some(&"alt text".to_string()),
        );
        assert_eq!(got, None);
    }

    #[test]
    fn a_field_that_was_not_requested_is_not_missing() {
        // The all-false default: nothing was asked for, so nothing can be
        // absent — this is the path a keywords-only run takes.
        let got = missing_field_warning(
            &MetadataGenerationRequest::default(),
            None,
            None,
            None,
            None,
        );
        assert_eq!(got, None);
    }

    #[test]
    fn a_requested_field_that_came_back_empty_is_reported() {
        let got = missing_field_warning(
            &asked_for_everything(),
            Some(&json!(["berg"])),
            Some(&"   ".to_string()),
            Some(&"a title".to_string()),
            Some(&"alt text".to_string()),
        )
        .expect("whitespace is not a caption");
        assert!(got.contains("caption"), "{got}");
        assert!(!got.contains("title"), "{got}");
    }

    #[test]
    fn empty_keyword_containers_count_as_missing() {
        // An empty array is what a model returns when it declines, and
        // `finish_one` stores nothing for it — so it must not read as success.
        for empty in [json!([]), json!({})] {
            let got = missing_field_warning(
                &asked_for_everything(),
                Some(&empty),
                Some(&"c".to_string()),
                Some(&"t".to_string()),
                Some(&"a".to_string()),
            )
            .unwrap_or_else(|| panic!("{empty} should count as missing keywords"));
            assert!(got.contains("keywords"), "{got}");
        }
    }

    #[test]
    fn several_missing_fields_read_as_a_sentence() {
        let got = missing_field_warning(&asked_for_everything(), None, None, None, None)
            .expect("everything is missing");
        assert!(
            got.contains("keywords, caption, title and alt text"),
            "{got}"
        );
    }
}
