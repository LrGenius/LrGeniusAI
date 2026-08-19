//! The wire types shared with `native/mlx-sidecar/Sources/lrgenius-mlx/Protocol.swift`.
//!
//! Both sides are hand-written rather than generated, so the field names here
//! are the contract: change one and you must change the other. The Swift side
//! spells every multi-word key in snake_case via explicit `CodingKeys` so these
//! structs need no rename attributes beyond the ones below.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct Request<'a> {
    pub id: u64,
    pub op: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_dir: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<Vec<GenerationSpec>>,
}

/// Where the raw pixels for one photo were staged.
///
/// Pixels travel via a file rather than inside the JSON: a decoded photo is
/// several megabytes and base64 would add a third again to every batch line.
#[derive(Debug, Serialize)]
pub struct ImagePayload {
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// 3 for RGB, 4 for RGBA. Always 3 today; sent explicitly so the reader
    /// never infers it from the file length.
    pub channels: u32,
}

#[derive(Debug, Serialize)]
pub struct GenerationSpec {
    pub system_prompt: String,
    pub stable_prompt: String,
    pub per_photo_prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImagePayload>,
    /// The schema is sent pre-serialized because XGrammar compiles from schema
    /// *text*; round-tripping it through a Swift dictionary and back would risk
    /// reordering keys and defeating the sidecar's grammar cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub max_tokens: u32,
    pub temperature: f32,
}

#[derive(Debug, Deserialize)]
pub struct EngineInfoPayload {
    pub model_path: String,
    pub model_name: String,
    pub supports_vision: bool,
}

#[derive(Debug, Deserialize)]
pub struct GenerationResultPayload {
    pub ok: bool,
    pub text: Option<String>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    #[serde(default)]
    pub prefix_reused: bool,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    pub error: Option<String>,
    pub info: Option<EngineInfoPayload>,
    pub results: Option<Vec<GenerationResultPayload>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Swift side keys off exactly these names; a rename here is a silent
    /// protocol break, so pin them.
    #[test]
    fn generation_spec_uses_the_agreed_key_names() {
        let spec = GenerationSpec {
            system_prompt: "sys".into(),
            stable_prompt: "stable".into(),
            per_photo_prompt: "photo".into(),
            image: Some(ImagePayload {
                path: "/tmp/x.rgb".into(),
                width: 2,
                height: 3,
                channels: 3,
            }),
            schema: Some("{}".into()),
            max_tokens: 64,
            temperature: 0.2,
        };
        let json = serde_json::to_value(&spec).unwrap();
        for key in [
            "system_prompt",
            "stable_prompt",
            "per_photo_prompt",
            "image",
            "schema",
            "max_tokens",
            "temperature",
        ] {
            assert!(json.get(key).is_some(), "missing key {key}");
        }
        assert_eq!(json["image"]["channels"], 3);
    }

    /// An absent image must be omitted, not sent as `null`: the Swift decoder
    /// treats the key's presence as "there is a photo here".
    #[test]
    fn an_absent_image_is_omitted_entirely() {
        let spec = GenerationSpec {
            system_prompt: String::new(),
            stable_prompt: String::new(),
            per_photo_prompt: String::new(),
            image: None,
            schema: None,
            max_tokens: 8,
            temperature: 0.0,
        };
        let json = serde_json::to_value(&spec).unwrap();
        assert!(json.get("image").is_none());
        assert!(json.get("schema").is_none());
    }

    #[test]
    fn a_response_without_optional_fields_still_decodes() {
        let response: Response = serde_json::from_str(r#"{"id":7,"ok":true}"#).unwrap();
        assert_eq!(response.id, 7);
        assert!(response.ok);
        assert!(response.results.is_none());
    }
}
