//! Port of `providers/gemini.py`'s dedicated Gemini response-schema
//! builder (`_prepare_gemini_response_schema` / `_build_nested_gemini_keyword_schema`
//! / `_gemini_keyword_leaf_item_schema`). Kept separate from `schema.rs`
//! (the OpenAI-flavor builder) because Gemini's structured-output schema
//! uses uppercase type names and omits `additionalProperties`/top-level
//! `required` — it is not simply a mechanical case change of the OpenAI
//! shape, so it is built fresh rather than derived via `edit_recipe`'s
//! generic OpenAI->Gemini converter.

use serde_json::{json, Map, Value};

use crate::types::{KeywordCategories, MetadataGenerationRequest};

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
        required.push("aliases".to_string());
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
            required.push("synonym_aliases".to_string());
        }
    }
    json!({"type": "OBJECT", "properties": properties, "required": required})
}

fn build_nested_gemini_keyword_schema(
    tree: &crate::types::KeywordTree,
    bilingual: bool,
    aliases: bool,
) -> Value {
    let mut properties = Map::new();
    for (name, children) in tree.iter() {
        let field = if !children.is_empty() {
            build_nested_gemini_keyword_schema(children, bilingual, aliases)
        } else {
            json!({"type": "ARRAY", "items": gemini_keyword_leaf_item_schema(bilingual, aliases)})
        };
        properties.insert(name.clone(), field);
    }
    json!({"type": "OBJECT", "properties": properties})
}

/// Port of `_prepare_gemini_response_schema`.
pub fn prepare_gemini_response_schema(request: &MetadataGenerationRequest) -> Value {
    let mut properties = Map::new();

    if request.generate_title {
        properties.insert("title".into(), json!({"type": "STRING"}));
    }
    if request.generate_caption {
        properties.insert("caption".into(), json!({"type": "STRING"}));
    }
    if request.generate_alt_text {
        properties.insert("alt_text".into(), json!({"type": "STRING"}));
    }
    if request.generate_keywords {
        let keywords_schema = match &request.keyword_categories {
            Some(KeywordCategories::Nested(tree)) => build_nested_gemini_keyword_schema(
                tree,
                request.bilingual_keywords,
                request.generate_aliases,
            ),
            Some(KeywordCategories::Flat(list)) => {
                let mut props = Map::new();
                for category in list {
                    props.insert(
                        category.clone(),
                        json!({"type": "ARRAY", "items": gemini_keyword_leaf_item_schema(request.bilingual_keywords, request.generate_aliases)}),
                    );
                }
                json!({"type": "OBJECT", "properties": props})
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
    use crate::types::KeywordTree;

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
    fn bilingual_and_aliases_add_required_leaf_fields() {
        let mut req = base_request();
        req.generate_keywords = true;
        req.bilingual_keywords = true;
        req.generate_aliases = true;
        let schema = prepare_gemini_response_schema(&req);
        let item = &schema["properties"]["keywords"]["items"];
        assert_eq!(item["type"], "OBJECT");
        let required: Vec<&str> = item["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            required,
            vec!["name", "aliases", "synonyms", "synonym_aliases"]
        );
    }

    #[test]
    fn nested_categories_recurse_without_additional_properties() {
        let mut req = base_request();
        req.generate_keywords = true;
        let tree: KeywordTree = KeywordTree(vec![(
            "People".to_string(),
            KeywordTree(vec![("Family".to_string(), KeywordTree(vec![]))]),
        )]);
        req.keyword_categories = Some(KeywordCategories::Nested(tree));
        let schema = prepare_gemini_response_schema(&req);
        let people = &schema["properties"]["keywords"]["properties"]["People"];
        assert_eq!(people["type"], "OBJECT");
        assert!(people.get("additionalProperties").is_none());
        assert_eq!(people["properties"]["Family"]["type"], "ARRAY");
    }
}
