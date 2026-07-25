//! Port of `providers/chatgpt.py::_make_schema_strict`: recursively
//! forces OpenAI's structured-output constraints (every object needs
//! `additionalProperties: false` and every property in `required`).

use serde_json::Value;

pub fn make_schema_strict(schema: &Value) -> Value {
    let Value::Object(obj) = schema else {
        return schema.clone();
    };
    let mut out = obj.clone();
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
}
