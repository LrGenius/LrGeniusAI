//! Metadata helpers ported from `services/chroma.py`: catalog_ids
//! (JSON-list-string) handling and the photo_id/uuid identity fields.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

pub const CATALOG_IDS_FIELD: &str = "catalog_ids";
pub const PHOTO_ID_FIELD: &str = "photo_id";
pub const LEGACY_UUID_FIELD: &str = "uuid";
pub const HAS_EMBEDDING_FIELD: &str = "has_embedding";

/// Parse catalog_ids from metadata (JSON list string or plain list).
pub fn parse_catalog_ids(metadata: &Map<String, Value>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Some(raw) = metadata.get(CATALOG_IDS_FIELD) else {
        return out;
    };
    let items: Vec<Value> = match raw {
        Value::Array(list) => list.clone(),
        Value::String(s) => serde_json::from_str::<Value>(s)
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    for item in items {
        if let Some(s) = item.as_str() {
            if !s.is_empty() {
                out.insert(s.to_string());
            }
        }
    }
    out
}

/// Serialize to the sorted JSON-list-string format Chroma metadata used.
pub fn serialize_catalog_ids(ids: &BTreeSet<String>) -> String {
    serde_json::to_string(&ids.iter().collect::<Vec<_>>()).unwrap_or_else(|_| "[]".to_string())
}

/// Port of `_ensure_photo_metadata`: stamp photo_id and keep the legacy
/// uuid field for older clients/filters.
pub fn ensure_photo_metadata(photo_id: &str, metadata: &mut Map<String, Value>) {
    metadata.insert(PHOTO_ID_FIELD.into(), Value::String(photo_id.to_string()));
    if !metadata.contains_key(LEGACY_UUID_FIELD) {
        metadata.insert(
            LEGACY_UUID_FIELD.into(),
            Value::String(photo_id.to_string()),
        );
    }
}

/// Port of `_normalize_photo_id`.
pub fn normalize_photo_id(photo_id: Option<&str>, legacy_uuid: Option<&str>) -> Option<String> {
    let pid = photo_id.or(legacy_uuid)?.trim();
    if pid.is_empty() {
        None
    } else {
        Some(pid.to_string())
    }
}

/// `has_embedding` defaults to true when absent (matches
/// `get_all_image_ids`'s `metadata.get("has_embedding", True)`).
pub fn has_embedding(metadata: &Map<String, Value>) -> bool {
    metadata
        .get(HAS_EMBEDDING_FIELD)
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// Keys holding AI-generated metadata, cleared by /v1/photos/metadata/remove while
/// the photo stays indexed (port of AI_METADATA_KEYS).
///
/// `species_checked` is in here on purpose: clearing AI metadata has to mean
/// "recompute", and leaving the done-marker behind would make
/// `/v1/index/unprocessed` report the photo as finished forever.
pub const AI_METADATA_KEYS: [&str; 22] = [
    "title",
    "caption",
    "keywords",
    "alt_text",
    "model",
    "run_date",
    "tokens_used",
    "flattened_keywords",
    "edit_recipe",
    "edit_summary",
    "edit_warnings",
    "edit_model",
    "edit_provider",
    "edit_run_date",
    "species_taxonomy",
    "species_rank",
    "species_scientific_name",
    "species_common_name",
    "species_confidence",
    "species_alternatives",
    "species_checked",
    "species_model",
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_ids_roundtrip() {
        let mut m = Map::new();
        m.insert("catalog_ids".into(), json!("[\"b\", \"a\"]"));
        let ids = parse_catalog_ids(&m);
        assert_eq!(ids.len(), 2);
        assert_eq!(serialize_catalog_ids(&ids), "[\"a\",\"b\"]");

        // Plain-list form and garbage are tolerated.
        m.insert("catalog_ids".into(), json!(["x", ""]));
        assert_eq!(parse_catalog_ids(&m).len(), 1);
        m.insert("catalog_ids".into(), json!("not json"));
        assert!(parse_catalog_ids(&m).is_empty());
    }

    #[test]
    fn ensure_photo_metadata_stamps_identity() {
        let mut m = Map::new();
        ensure_photo_metadata("p1", &mut m);
        assert_eq!(m["photo_id"], json!("p1"));
        assert_eq!(m["uuid"], json!("p1"));

        // Existing legacy uuid is preserved.
        let mut m2 = Map::new();
        m2.insert("uuid".into(), json!("old-uuid"));
        ensure_photo_metadata("p1", &mut m2);
        assert_eq!(m2["uuid"], json!("old-uuid"));
    }

    #[test]
    fn has_embedding_defaults_true() {
        assert!(has_embedding(&Map::new()));
        let mut m = Map::new();
        m.insert("has_embedding".into(), json!(false));
        assert!(!has_embedding(&m));
    }

    #[test]
    fn normalize_photo_id_trims_and_falls_back() {
        assert_eq!(normalize_photo_id(Some(" a "), None), Some("a".into()));
        assert_eq!(normalize_photo_id(None, Some("u")), Some("u".into()));
        assert_eq!(normalize_photo_id(Some("  "), None), None);
        assert_eq!(normalize_photo_id(None, None), None);
    }
}
