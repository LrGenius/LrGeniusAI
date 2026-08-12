//! Gemini's dedicated response-schema builder. Kept separate from
//! `schema.rs` (the OpenAI-flavor builder) because Gemini's structured-output
//! schema uses uppercase type names and omits `additionalProperties`/
//! top-level `required` — it is not simply a mechanical case change of the
//! OpenAI shape, so it is built fresh rather than derived via `edit_recipe`'s
//! generic OpenAI->Gemini converter.
//!
//! The response *shape* mirrors `schema.rs` exactly — flat `{category, items}`
//! keyword groups, lean `required` — so a photo produces the same JSON
//! whichever provider answered. See `schema.rs` for why the shape is what it
//! is.

use serde_json::{json, Map, Value};

use crate::keyword_taxonomy::CategoryLabels;
use crate::types::MetadataGenerationRequest;

/// See `schema.rs::keyword_leaf_item_schema` — `aliases` and
/// `synonym_aliases` stay optional so the prompt's "omit when absent"
/// guidance can actually be followed.
fn gemini_keyword_leaf_item_schema(bilingual: bool, aliases: bool) -> Value {
    if !bilingual && !aliases {
        return json!({"type": "STRING"});
    }
    let mut properties = Map::new();
    let mut required = vec!["name".to_string()];
    properties.insert("name".into(), json!({"type": "STRING"}));
    if aliases {
        properties.insert(
            "aliases".into(),
            json!({"type": "ARRAY", "items": {"type": "STRING"}}),
        );
    }
    if bilingual {
        properties.insert(
            "synonyms".into(),
            json!({"type": "ARRAY", "items": {"type": "STRING"}}),
        );
        required.push("synonyms".to_string());
        if aliases {
            properties.insert(
                "synonym_aliases".into(),
                json!({"type": "ARRAY", "items": {"type": "STRING"}}),
            );
        }
    }
    json!({"type": "OBJECT", "properties": properties, "required": required})
}

/// The flat `{category, items}` group list, Gemini-flavored.
fn gemini_keyword_groups_schema(labels: &CategoryLabels, bilingual: bool, aliases: bool) -> Value {
    let categories: Vec<&str> = labels.labels().collect();
    json!({
        "type": "ARRAY",
        "items": {
            "type": "OBJECT",
            "properties": {
                "category": {"type": "STRING", "enum": categories},
                "items": {
                    "type": "ARRAY",
                    "items": gemini_keyword_leaf_item_schema(bilingual, aliases),
                },
            },
            "required": ["category", "items"],
        }
    })
}

pub fn prepare_gemini_response_schema(request: &MetadataGenerationRequest) -> Value {
    let mut properties = Map::new();

    if request.generate_title {
        properties.insert("title".into(), json!({"type": "STRING"}));
    }
    if request.generate_caption {
        properties.insert("caption".into(), json!({"type": "STRING"}));
    }
    // Derived from `caption` when both are requested — see `schema.rs`.
    if request.generate_alt_text && !request.generate_caption {
        properties.insert("alt_text".into(), json!({"type": "STRING"}));
    }
    if request.generate_keywords {
        let keywords_schema = match &request.keyword_categories {
            Some(categories) => {
                let labels = CategoryLabels::from_categories(categories);
                if labels.is_empty() {
                    json!({"type": "ARRAY", "items": gemini_keyword_leaf_item_schema(request.bilingual_keywords, request.generate_aliases)})
                } else {
                    gemini_keyword_groups_schema(
                        &labels,
                        request.bilingual_keywords,
                        request.generate_aliases,
                    )
                }
            }
            None => {
                json!({"type": "ARRAY", "items": gemini_keyword_leaf_item_schema(request.bilingual_keywords, request.generate_aliases)})
            }
        };
        properties.insert("keywords".into(), keywords_schema);
    }

    json!({"type": "OBJECT", "properties": properties})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{KeywordCategories, KeywordTree};

    fn base_request() -> MetadataGenerationRequest {
        MetadataGenerationRequest::default()
    }

    #[test]
    fn top_level_has_no_required_array() {
        let mut req = base_request();
        req.generate_title = true;
        let schema = prepare_gemini_response_schema(&req);
        assert_eq!(schema["type"], "OBJECT");
        assert!(schema.get("required").is_none());
        assert_eq!(schema["properties"]["title"]["type"], "STRING");
    }

    #[test]
    fn simple_keyword_array_uses_uppercase_types() {
        let mut req = base_request();
        req.generate_keywords = true;
        let schema = prepare_gemini_response_schema(&req);
        assert_eq!(schema["properties"]["keywords"]["type"], "ARRAY");
        assert_eq!(schema["properties"]["keywords"]["items"]["type"], "STRING");
    }

    #[test]
    fn optional_alias_fields_are_not_required() {
        let mut req = base_request();
        req.generate_keywords = true;
        req.bilingual_keywords = true;
        req.generate_aliases = true;
        let schema = prepare_gemini_response_schema(&req);
        let item = &schema["properties"]["keywords"]["items"];
        assert_eq!(item["type"], "OBJECT");
        assert_eq!(item["required"], json!(["name", "synonyms"]));
        assert!(item["properties"].get("aliases").is_some());
        assert!(item["properties"].get("synonym_aliases").is_some());
    }

    #[test]
    fn nested_categories_flatten_to_a_group_list_with_an_enum() {
        let mut req = base_request();
        req.generate_keywords = true;
        let tree: KeywordTree = KeywordTree(vec![(
            "People".to_string(),
            KeywordTree(vec![("Family".to_string(), KeywordTree(vec![]))]),
        )]);
        req.keyword_categories = Some(KeywordCategories::Nested(tree));
        let schema = prepare_gemini_response_schema(&req);
        let kw = &schema["properties"]["keywords"];
        assert_eq!(kw["type"], "ARRAY");
        assert_eq!(
            kw["items"]["properties"]["category"]["enum"],
            json!(["Family"])
        );
        assert!(kw.get("properties").is_none());
    }

    #[test]
    fn alt_text_is_derived_from_caption_when_both_are_requested() {
        let mut req = base_request();
        req.generate_caption = true;
        req.generate_alt_text = true;
        let schema = prepare_gemini_response_schema(&req);
        assert!(schema["properties"].get("caption").is_some());
        assert!(schema["properties"].get("alt_text").is_none());
    }
}
