//! Port of `providers/chatgpt.py::_make_schema_strict`: recursively
//! forces OpenAI's structured-output constraints (every object needs
//! `additionalProperties: false` and every property in `required`).

use serde_json::Value;

/// Validation keywords OpenAI's strict mode does not accept. Sending one is
/// not ignored — the whole request is rejected — so they are stripped here
/// rather than left out of `schema.rs`, where they do real work: the local
/// engines and Gemini all enforce them, and they are what stops a positional
/// keyword leaf from arriving with the wrong number of groups. The decoder in
/// `normalize.rs` is lenient precisely because this path cannot keep them.
const UNSUPPORTED_BY_STRICT_MODE: [&str; 2] = ["minItems", "maxItems"];

pub fn make_schema_strict(schema: &Value) -> Value {
    let Value::Object(obj) = schema else {
        return schema.clone();
    };
    let mut out = obj.clone();
    for key in UNSUPPORTED_BY_STRICT_MODE {
        out.remove(key);
    }
    let schema_type = out.get("type").and_then(Value::as_str).map(str::to_string);
    let has_properties = out.contains_key("properties");
    let has_items = out.contains_key("items");

    if schema_type.as_deref() == Some("object") || has_properties {
        out.insert("type".into(), Value::String("object".into()));
        out.insert("additionalProperties".into(), Value::Bool(false));

        if let Some(Value::Object(properties)) = out.get("properties").cloned() {
            if !properties.is_empty() {
                let mut required: Vec<Value> = match out.get("required") {
                    Some(Value::Array(r)) => r.clone(),
                    _ => Vec::new(),
                };
                for key in properties.keys() {
                    if !required.iter().any(|r| r.as_str() == Some(key)) {
                        required.push(Value::String(key.clone()));
                    }
                }
                out.insert("required".into(), Value::Array(required));

                let mut new_props = serde_json::Map::new();
                for (k, v) in properties {
                    new_props.insert(k, make_schema_strict(&v));
                }
                out.insert("properties".into(), Value::Object(new_props));
            }
        }
    } else if schema_type.as_deref() == Some("array") || has_items {
        out.insert("type".into(), Value::String("array".into()));
        if let Some(items) = out.get("items").cloned() {
            out.insert("items".into(), make_schema_strict(&items));
        }
    }

    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn object_gets_additional_properties_false_and_full_required() {
        let schema = json!({"type": "object", "properties": {"title": {"type": "string"}}});
        let strict = make_schema_strict(&schema);
        assert_eq!(strict["additionalProperties"], json!(false));
        assert_eq!(strict["required"], json!(["title"]));
    }

    #[test]
    fn nested_objects_and_arrays_recurse() {
        let schema = json!({
            "type": "object",
            "properties": {
                "keywords": {"type": "array", "items": {"type": "object", "properties": {"name": {"type": "string"}}}}
            }
        });
        let strict = make_schema_strict(&schema);
        let item = &strict["properties"]["keywords"]["items"];
        assert_eq!(item["additionalProperties"], json!(false));
        assert_eq!(item["required"], json!(["name"]));
    }

    #[test]
    fn existing_required_entries_are_preserved_not_duplicated() {
        let schema = json!({"type": "object", "properties": {"a": {"type": "string"}, "b": {"type": "string"}}, "required": ["a"]});
        let strict = make_schema_strict(&schema);
        assert_eq!(strict["required"], json!(["a", "b"]));
    }

    #[test]
    fn item_count_bounds_are_stripped_at_every_depth() {
        // OpenAI rejects the whole request over an unsupported keyword, so one
        // survivor anywhere is a failed call, not a loosened constraint.
        let schema = json!({
            "type": "object",
            "properties": {
                "keywords": {
                    "type": "array",
                    "items": {
                        "type": "array", "minItems": 2, "maxItems": 2,
                        "items": {"type": "array", "minItems": 1, "items": {"type": "string"}}
                    }
                }
            }
        });
        let strict = make_schema_strict(&schema);
        let leaf = &strict["properties"]["keywords"]["items"];
        assert!(leaf.get("minItems").is_none());
        assert!(leaf.get("maxItems").is_none());
        assert!(leaf["items"].get("minItems").is_none());
        // Stripping the bounds must not disturb the shape itself.
        assert_eq!(leaf["items"]["items"]["type"], "string");
    }

    #[test]
    fn the_real_bilingual_schema_survives_strict_mode() {
        use crate::schema::prepare_response_structure;
        use crate::types::MetadataGenerationRequest;

        let mut req = MetadataGenerationRequest {
            generate_keywords: true,
            bilingual_keywords: true,
            generate_aliases: true,
            ..Default::default()
        };
        req.generate_title = true;
        let strict = make_schema_strict(&prepare_response_structure(&req));
        let json = serde_json::to_string(&strict).expect("schema serializes");
        for key in UNSUPPORTED_BY_STRICT_MODE {
            assert!(!json.contains(key), "{key} must not reach OpenAI");
        }
    }
}
