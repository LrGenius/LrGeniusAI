//! Builds the OpenAI-flavor JSON schema describing the requested metadata
//! fields, for structured-output prompting.
//!
//! Both local engines constrain decoding with this schema — llama.cpp via
//! llguidance, the MLX sidecar via XGrammar — so its *shape* is a direct
//! output-token cost on every photo. Two things follow, and they are the
//! reason this file looks the way it does:
//!
//! * Keywords are a flat list of `{category, items}` groups, not a nested
//!   mirror of the taxonomy. The old shape put every category in `required`,
//!   so a model had to emit every branch — including the empty ones — for
//!   every photo. See [`crate::keyword_taxonomy`].
//! * Only fields the model must genuinely always produce go in `required`.
//!   An optional field listed as required cannot be omitted, which turns
//!   "omit this when it does not apply" prompt guidance into a dead letter.
//!
//! Note that `openai.rs` runs the result through
//! [`crate::schema_strict::make_schema_strict`], which forces *every*
//! property back into `required` because OpenAI's strict mode demands it.
//! That is intentional and only applies to that one provider; do not
//! "fix" the leaner `required` here to match it.

use serde_json::{json, Map, Value};

use crate::keyword_taxonomy::CategoryLabels;
use crate::types::MetadataGenerationRequest;

/// One keyword: a bare string, or an object when translations/aliases are
/// requested.
///
/// `aliases` and `synonym_aliases` are deliberately *not* required: the
/// prompt tells the model to omit them when no genuine synonym exists
/// (`prompts.rs`), and a schema that requires them makes that impossible —
/// the model then emits an empty array per keyword instead, which is the
/// most expensive way to say nothing.
fn keyword_leaf_item_schema(bilingual: bool, aliases: bool) -> Value {
    if !bilingual && !aliases {
        return json!({"type": "string"});
    }
    let mut properties = Map::new();
    let mut required = vec!["name".to_string()];
    properties.insert("name".into(), json!({"type": "string"}));
    if aliases {
        properties.insert(
            "aliases".into(),
            json!({"type": "array", "items": {"type": "string"}}),
        );
    }
    if bilingual {
        properties.insert(
            "synonyms".into(),
            json!({"type": "array", "items": {"type": "string"}}),
        );
        // The bilingual prompt asks for `synonyms` unconditionally, so this
        // one stays required — schema and prompt agree.
        required.push("synonyms".to_string());
        if aliases {
            properties.insert(
                "synonym_aliases".into(),
                json!({"type": "array", "items": {"type": "string"}}),
            );
        }
    }
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

/// The flat `{category, items}` group list. Only categories that actually
/// apply appear, because the array's length is the model's to choose.
///
/// `category` is an `enum` of the hybrid labels, so the grammar itself rules
/// out an invented category — no post-hoc validation needed.
fn keyword_groups_schema(labels: &CategoryLabels, bilingual: bool, aliases: bool) -> Value {
    let categories: Vec<&str> = labels.labels().collect();
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "category": {"type": "string", "enum": categories},
                "items": {"type": "array", "items": keyword_leaf_item_schema(bilingual, aliases)},
            },
            "required": ["category", "items"],
            "additionalProperties": false,
        }
    })
}

pub fn prepare_response_structure(request: &MetadataGenerationRequest) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();

    if request.generate_title {
        properties.insert("title".into(), json!({"type": "string"}));
        required.push("title");
    }
    if request.generate_caption {
        properties.insert("caption".into(), json!({"type": "string"}));
        required.push("caption");
    }
    // Alt text and caption are near-identical prose for the same photo, and
    // prose is the most expensive thing the model emits. When both are asked
    // for, generate the caption once and copy it into `alt_text` after
    // parsing (see the providers' response assembly). Alt text only gets its
    // own field when there is no caption to derive it from.
    if request.generate_alt_text && !request.generate_caption {
        properties.insert("alt_text".into(), json!({"type": "string"}));
        required.push("alt_text");
    }
    if request.generate_keywords {
        let keywords_schema = match &request.keyword_categories {
            Some(categories) => {
                let labels = CategoryLabels::from_categories(categories);
                if labels.is_empty() {
                    // A taxonomy that contains nothing usable is the same as
                    // no taxonomy at all.
                    json!({"type": "array", "items": keyword_leaf_item_schema(request.bilingual_keywords, request.generate_aliases)})
                } else {
                    keyword_groups_schema(
                        &labels,
                        request.bilingual_keywords,
                        request.generate_aliases,
                    )
                }
            }
            None => {
                json!({"type": "array", "items": keyword_leaf_item_schema(request.bilingual_keywords, request.generate_aliases)})
            }
        };
        properties.insert("keywords".into(), keywords_schema);
        required.push("keywords");
    }

    json!({"type": "object", "properties": properties, "required": required})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{KeywordCategories, KeywordTree};

    fn base_request() -> MetadataGenerationRequest {
        MetadataGenerationRequest::default()
    }

    #[test]
    fn simple_keyword_array_when_no_categories() {
        let mut req = base_request();
        req.generate_keywords = true;
        let schema = prepare_response_structure(&req);
        assert_eq!(schema["properties"]["keywords"]["type"], "array");
        assert_eq!(schema["properties"]["keywords"]["items"]["type"], "string");
        assert_eq!(schema["required"], json!(["keywords"]));
    }

    #[test]
    fn optional_alias_fields_are_not_required() {
        let mut req = base_request();
        req.generate_keywords = true;
        req.bilingual_keywords = true;
        req.generate_aliases = true;
        let schema = prepare_response_structure(&req);
        let item = &schema["properties"]["keywords"]["items"];
        assert_eq!(item["type"], "object");
        // All four fields remain available...
        for field in ["name", "aliases", "synonyms", "synonym_aliases"] {
            assert!(
                item["properties"].get(field).is_some(),
                "{field} should still be offered"
            );
        }
        // ...but only the two the prompt asks for unconditionally are required.
        assert_eq!(item["required"], json!(["name", "synonyms"]));
    }

    #[test]
    fn categories_become_a_flat_group_list_with_an_enum() {
        let mut req = base_request();
        req.generate_keywords = true;
        req.keyword_categories = Some(KeywordCategories::Flat(vec![
            "People".into(),
            "Places".into(),
        ]));
        let schema = prepare_response_structure(&req);
        let kw = &schema["properties"]["keywords"];
        assert_eq!(kw["type"], "array");
        assert_eq!(
            kw["items"]["properties"]["category"]["enum"],
            json!(["People", "Places"])
        );
        assert_eq!(kw["items"]["required"], json!(["category", "items"]));
    }

    #[test]
    fn nested_categories_flatten_to_hybrid_labels() {
        let mut req = base_request();
        req.generate_keywords = true;
        // "Family" is unique, so it stays bare rather than "People/Family".
        let tree: KeywordTree = KeywordTree(vec![(
            "People".to_string(),
            KeywordTree(vec![("Family".to_string(), KeywordTree(vec![]))]),
        )]);
        req.keyword_categories = Some(KeywordCategories::Nested(tree));
        let schema = prepare_response_structure(&req);
        let kw = &schema["properties"]["keywords"];
        assert_eq!(kw["type"], "array");
        assert_eq!(
            kw["items"]["properties"]["category"]["enum"],
            json!(["Family"])
        );
    }

    #[test]
    fn no_empty_category_containers_remain_in_the_schema() {
        // The regression this whole shape exists to prevent: a taxonomy must
        // never turn into per-category properties that the model has to fill.
        let mut req = base_request();
        req.generate_keywords = true;
        req.keyword_categories = Some(KeywordCategories::Flat(
            (0..40).map(|i| format!("Cat{i}")).collect(),
        ));
        let schema = prepare_response_structure(&req);
        let kw = &schema["properties"]["keywords"];
        assert_eq!(kw["type"], "array");
        assert!(
            kw.get("properties").is_none(),
            "categories must not become required object properties"
        );
    }

    #[test]
    fn alt_text_is_derived_from_caption_when_both_are_requested() {
        let mut req = base_request();
        req.generate_caption = true;
        req.generate_alt_text = true;
        let schema = prepare_response_structure(&req);
        assert_eq!(schema["required"], json!(["caption"]));
        assert!(schema["properties"].get("alt_text").is_none());
    }

    #[test]
    fn alt_text_keeps_its_own_field_without_a_caption() {
        let mut req = base_request();
        req.generate_title = true;
        req.generate_alt_text = true;
        let schema = prepare_response_structure(&req);
        assert_eq!(schema["required"], json!(["title", "alt_text"]));
        assert!(schema["properties"].get("caption").is_none());
    }
}
