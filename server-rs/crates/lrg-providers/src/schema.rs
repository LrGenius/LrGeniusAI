//! Port of `providers/base.py`'s `_prepare_response_structure` /
//! `_build_nested_keyword_schema` / `_keyword_leaf_item_schema`: builds
//! the OpenAI-flavor JSON schema describing the requested metadata
//! fields, for structured-output prompting.

use serde_json::{json, Map, Value};

use crate::types::{KeywordCategories, MetadataGenerationRequest};

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
        required.push("aliases".to_string());
    }
    if bilingual {
        properties.insert(
            "synonyms".into(),
            json!({"type": "array", "items": {"type": "string"}}),
        );
        required.push("synonyms".to_string());
        if aliases {
            properties.insert(
                "synonym_aliases".into(),
                json!({"type": "array", "items": {"type": "string"}}),
            );
            required.push("synonym_aliases".to_string());
        }
    }
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

fn build_nested_keyword_schema(
    tree: &crate::types::KeywordTree,
    bilingual: bool,
    aliases: bool,
) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (name, children) in tree.iter() {
        let field = if !children.is_empty() {
            build_nested_keyword_schema(children, bilingual, aliases)
        } else {
            json!({"type": "array", "items": keyword_leaf_item_schema(bilingual, aliases)})
        };
        properties.insert(name.clone(), field);
        if !required.contains(name) {
            required.push(name.clone());
        }
    }
    json!({"type": "object", "properties": properties, "additionalProperties": false, "required": required})
}

/// Port of `_prepare_response_structure`.
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
    if request.generate_alt_text {
        properties.insert("alt_text".into(), json!({"type": "string"}));
        required.push("alt_text");
    }
    if request.generate_keywords {
        let keywords_schema = match &request.keyword_categories {
            Some(KeywordCategories::Nested(tree)) => build_nested_keyword_schema(
                tree,
                request.bilingual_keywords,
                request.generate_aliases,
            ),
            Some(KeywordCategories::Flat(list)) => {
                let mut props = Map::new();
                let mut req = Vec::new();
                for category in list {
                    props.insert(
                        category.clone(),
                        json!({"type": "array", "items": keyword_leaf_item_schema(request.bilingual_keywords, request.generate_aliases)}),
                    );
                    if !req.contains(category) {
                        req.push(category.clone());
                    }
                }
                json!({"type": "object", "properties": props, "additionalProperties": false, "required": req})
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
    use crate::types::KeywordTree;

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
    fn bilingual_and_aliases_add_object_fields() {
        let mut req = base_request();
        req.generate_keywords = true;
        req.bilingual_keywords = true;
        req.generate_aliases = true;
        let schema = prepare_response_structure(&req);
        let item = &schema["properties"]["keywords"]["items"];
        assert_eq!(item["type"], "object");
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
    fn flat_categories_produce_per_category_arrays() {
        let mut req = base_request();
        req.generate_keywords = true;
        req.keyword_categories = Some(KeywordCategories::Flat(vec![
            "People".into(),
            "Places".into(),
        ]));
        let schema = prepare_response_structure(&req);
        let kw = &schema["properties"]["keywords"];
        assert_eq!(kw["type"], "object");
        assert!(kw["properties"]["People"]["type"] == "array");
        assert!(kw["properties"]["Places"]["type"] == "array");
    }

    #[test]
    fn nested_categories_recurse() {
        let mut req = base_request();
        req.generate_keywords = true;
        let tree: KeywordTree = KeywordTree(vec![(
            "People".to_string(),
            KeywordTree(vec![("Family".to_string(), KeywordTree(vec![]))]),
        )]);
        req.keyword_categories = Some(KeywordCategories::Nested(tree));
        let schema = prepare_response_structure(&req);
        let people = &schema["properties"]["keywords"]["properties"]["People"];
        assert_eq!(people["type"], "object");
        assert_eq!(people["properties"]["Family"]["type"], "array");
    }

    #[test]
    fn required_fields_follow_requested_flags() {
        let mut req = base_request();
        req.generate_title = true;
        req.generate_alt_text = true;
        let schema = prepare_response_structure(&req);
        assert_eq!(schema["required"], json!(["title", "alt_text"]));
        assert!(schema["properties"].get("caption").is_none());
    }
}
