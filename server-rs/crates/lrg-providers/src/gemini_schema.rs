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

use crate::keyword_taxonomy::{CategoryLabels, KeywordLeafEncoding};
use crate::types::MetadataGenerationRequest;

/// See `schema.rs::keyword_leaf_item_schema` for the positional layout and
/// what it saves. `minItems`/`maxItems` are part of the OpenAPI subset Gemini
/// accepts, so the two-group bound survives here — unlike on the OpenAI path,
/// where `schema_strict` has to drop it.
fn gemini_keyword_leaf_item_schema(encoding: KeywordLeafEncoding) -> Value {
    let string_list = json!({"type": "ARRAY", "minItems": 1, "items": {"type": "STRING"}});
    match encoding {
        KeywordLeafEncoding::Plain => json!({"type": "STRING"}),
        KeywordLeafEncoding::Aliased => string_list,
        KeywordLeafEncoding::Translated => json!({
            "type": "ARRAY", "minItems": 2, "maxItems": 2, "items": {"type": "STRING"}
        }),
        KeywordLeafEncoding::TranslatedAliased => json!({
            "type": "ARRAY", "minItems": 2, "maxItems": 2, "items": string_list
        }),
    }
}

/// The flat `{category, items}` group list, Gemini-flavored.
fn gemini_keyword_groups_schema(labels: &CategoryLabels, encoding: KeywordLeafEncoding) -> Value {
    let categories: Vec<&str> = labels.labels().collect();
    json!({
        "type": "ARRAY",
        "items": {
            "type": "OBJECT",
            "properties": {
                "category": {"type": "STRING", "enum": categories},
                "items": {
                    "type": "ARRAY",
                    "items": gemini_keyword_leaf_item_schema(encoding),
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
        let encoding =
            KeywordLeafEncoding::for_request(request.bilingual_keywords, request.generate_aliases);
        let flat_list =
            || json!({"type": "ARRAY", "items": gemini_keyword_leaf_item_schema(encoding)});
        let keywords_schema = match &request.keyword_categories {
            Some(categories) => {
                let labels = CategoryLabels::from_categories(categories);
                if labels.is_empty() {
                    flat_list()
                } else {
                    gemini_keyword_groups_schema(&labels, encoding)
                }
            }
            None => flat_list(),
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
    fn translation_with_aliases_is_two_positional_groups() {
        let mut req = base_request();
        req.generate_keywords = true;
        req.bilingual_keywords = true;
        req.generate_aliases = true;
        let schema = prepare_gemini_response_schema(&req);
        let item = &schema["properties"]["keywords"]["items"];
        assert_eq!(item["type"], "ARRAY");
        // Gemini takes minItems/maxItems, so the two-group bound survives here
        // even though `schema_strict` has to drop it on the OpenAI path.
        assert_eq!(
            (&item["minItems"], &item["maxItems"]),
            (&json!(2), &json!(2))
        );
        assert_eq!(item["items"]["type"], "ARRAY");
        assert_eq!(item["items"]["items"]["type"], "STRING");
    }

    #[test]
    fn gemini_leaf_shape_matches_the_openai_builder() {
        // The two builders differ only in dialect. A photo must come back the
        // same shape whichever provider answered it, or `normalize` would need
        // to know who was asked.
        for (bilingual, aliases) in [(false, false), (false, true), (true, false), (true, true)] {
            let mut req = base_request();
            req.generate_keywords = true;
            req.bilingual_keywords = bilingual;
            req.generate_aliases = aliases;
            let gemini = &prepare_gemini_response_schema(&req)["properties"]["keywords"];
            let openai = &crate::schema::prepare_response_structure(&req)["properties"]["keywords"];
            assert_eq!(
                gemini["items"]["minItems"], openai["items"]["minItems"],
                "bilingual={bilingual} aliases={aliases}"
            );
            assert_eq!(
                gemini["items"]["maxItems"], openai["items"]["maxItems"],
                "bilingual={bilingual} aliases={aliases}"
            );
        }
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
