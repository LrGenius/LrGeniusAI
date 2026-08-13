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

use crate::keyword_taxonomy::{CategoryLabels, KeywordLeafEncoding};
use crate::types::MetadataGenerationRequest;

/// One keyword, positional rather than named once anything beyond the bare
/// term is asked for. See [`KeywordLeafEncoding`] for what each slot means and
/// why the field names moved into the prompt.
///
/// `minItems`/`maxItems` are what make the positional reading safe: they stop
/// a model from returning one group where two are meant, which would silently
/// file a translation as an alias. `schema_strict` drops them again for
/// OpenAI, whose strict mode rejects the keywords outright — the decoder in
/// `normalize.rs` stays lenient for exactly that reason.
fn keyword_leaf_item_schema(encoding: KeywordLeafEncoding) -> Value {
    let string_list = json!({"type": "array", "minItems": 1, "items": {"type": "string"}});
    match encoding {
        KeywordLeafEncoding::Plain => json!({"type": "string"}),
        // The term first, then any further terms for it. No upper bound: how
        // many alternatives a keyword genuinely has is the model's call.
        KeywordLeafEncoding::Aliased => string_list,
        // Exactly the term and its translation.
        KeywordLeafEncoding::Translated => json!({
            "type": "array", "minItems": 2, "maxItems": 2, "items": {"type": "string"}
        }),
        // One group per language, each shaped like `Aliased`.
        KeywordLeafEncoding::TranslatedAliased => json!({
            "type": "array", "minItems": 2, "maxItems": 2, "items": string_list
        }),
    }
}

/// The flat `{category, items}` group list. Only categories that actually
/// apply appear, because the array's length is the model's to choose.
///
/// `category` is an `enum` of the hybrid labels, so the grammar itself rules
/// out an invented category — no post-hoc validation needed.
///
/// Keying the object by category name instead would save the `"category"` and
/// `"items"` labels, but it cannot ship: `schema_strict::make_schema_strict`
/// forces every property into `required` for OpenAI, so every category in the
/// taxonomy would become mandatory — the exact failure this flat shape was
/// introduced to fix. It is worth roughly 9 tokens per group against the
/// leaf's ~200 per photo, so the trade is not close.
fn keyword_groups_schema(labels: &CategoryLabels, encoding: KeywordLeafEncoding) -> Value {
    let categories: Vec<&str> = labels.labels().collect();
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "category": {"type": "string", "enum": categories},
                "items": {"type": "array", "items": keyword_leaf_item_schema(encoding)},
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
        let encoding =
            KeywordLeafEncoding::for_request(request.bilingual_keywords, request.generate_aliases);
        let flat_list = || json!({"type": "array", "items": keyword_leaf_item_schema(encoding)});
        let keywords_schema = match &request.keyword_categories {
            Some(categories) => {
                let labels = CategoryLabels::from_categories(categories);
                if labels.is_empty() {
                    // A taxonomy that contains nothing usable is the same as
                    // no taxonomy at all.
                    flat_list()
                } else {
                    keyword_groups_schema(&labels, encoding)
                }
            }
            None => flat_list(),
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
    fn aliases_alone_are_one_flat_string_list() {
        let mut req = base_request();
        req.generate_keywords = true;
        req.generate_aliases = true;
        let item = &prepare_response_structure(&req)["properties"]["keywords"]["items"];
        assert_eq!(item["type"], "array");
        assert_eq!(item["items"]["type"], "string");
        // No upper bound: a keyword may have any number of further terms.
        assert_eq!(item["minItems"], json!(1));
        assert!(item.get("maxItems").is_none());
    }

    #[test]
    fn translation_alone_is_a_pair_of_strings() {
        let mut req = base_request();
        req.generate_keywords = true;
        req.bilingual_keywords = true;
        let item = &prepare_response_structure(&req)["properties"]["keywords"]["items"];
        assert_eq!(item["items"]["type"], "string");
        assert_eq!(
            (&item["minItems"], &item["maxItems"]),
            (&json!(2), &json!(2))
        );
    }

    #[test]
    fn translation_with_aliases_is_two_groups_of_strings() {
        let mut req = base_request();
        req.generate_keywords = true;
        req.bilingual_keywords = true;
        req.generate_aliases = true;
        let item = &prepare_response_structure(&req)["properties"]["keywords"]["items"];
        // Exactly two groups — one per language. Without the bound a model
        // could return a single group and its translation would be read as an
        // alias of the keyword.
        assert_eq!(
            (&item["minItems"], &item["maxItems"]),
            (&json!(2), &json!(2))
        );
        assert_eq!(item["items"]["type"], "array");
        assert_eq!(item["items"]["items"]["type"], "string");
        assert_eq!(item["items"]["minItems"], json!(1));
        // The named form is gone: it cost 546 output tokens where this costs
        // 344 for the same nine keywords.
        assert!(item.get("properties").is_none());
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
