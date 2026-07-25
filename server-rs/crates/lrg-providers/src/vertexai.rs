//! Port of `services/vertexai.py`: Google Vertex AI multimodal
//! embeddings (`multimodalembedding@001`), used optionally alongside
//! SigLIP2 and stored in a separate table. The Python code goes through
//! the `google-cloud-aiplatform` gRPC SDK; this calls the same model's
//! plain REST `:predict` endpoint instead, authenticating via
//! `gcp_auth` (Application Default Credentials — the same credential
//! chain `gcloud auth application-default login` sets up, or a service
//! account key via `GOOGLE_APPLICATION_CREDENTIALS`) rather than the
//! gRPC SDK's implicit auth. Not live-tested against a real GCP project
//! in this environment (no credentials or network egress available) —
//! the request/response shapes follow the documented REST contract.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

pub const VERTEX_EMBEDDING_DIM: usize = 1408;
const VERTEX_MODEL_NAME: &str = "multimodalembedding@001";
const AUTH_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

#[derive(Debug, thiserror::Error)]
pub enum VertexAiError {
    #[error("Vertex AI project not configured")]
    NotConfigured,
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
}

/// Best-effort MIME detection for the common formats the endpoint
/// supports — Lightroom-exported previews are JPEG, used as the safe
/// default like the Python version.
fn detect_mime_type(image_bytes: &[u8]) -> &'static str {
    if image_bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']) {
        "image/png"
    } else {
        "image/jpeg"
    }
}

/// Port of `_resolve_config`: explicit args win, then env vars, with
/// `VERTEX_LOCATION` defaulting to `us-central1`.
pub fn resolve_config(
    vertex_project_id: Option<&str>,
    vertex_location: Option<&str>,
) -> (Option<String>, String) {
    let project = vertex_project_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT").ok())
        .or_else(|| std::env::var("VERTEX_PROJECT_ID").ok());
    let location = vertex_location
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("VERTEX_LOCATION").ok())
        .unwrap_or_else(|| "us-central1".to_string());
    (project, location)
}

/// L2-normalize (Python normalizes because this store's cosine-distance
/// convention, like Chroma's, assumes unit vectors), returning `None`
/// for a near-zero vector instead of dividing by ~0.
fn l2_normalize(mut vec: Vec<f32>) -> Option<Vec<f32>> {
    let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm <= 1e-6 {
        return None;
    }
    for v in &mut vec {
        *v /= norm;
    }
    Some(vec)
}

/// Port of `_extract_embedding`: pull `predictions[0][field_name]` (or
/// `predictions[0][field_name]["values"]`) out of the REST response.
fn extract_embedding(prediction: &Value, field_name: &str) -> Option<Vec<f32>> {
    let field = prediction.get(field_name)?;
    let values = field.get("values").unwrap_or(field);
    let arr = values.as_array()?;
    if arr.is_empty() {
        return None;
    }
    arr.iter()
        .map(Value::as_f64)
        .collect::<Option<Vec<f64>>>()
        .map(|v| v.into_iter().map(|f| f as f32).collect())
}

pub struct VertexAiProvider {
    client: reqwest::Client,
    project: String,
    location: String,
}

impl VertexAiProvider {
    /// Resolves project/location the same way Python's
    /// `_get_vertex_client_and_endpoint` does; `None` when no project
    /// can be determined (matches `is_available() == False`).
    pub fn new(vertex_project_id: Option<&str>, vertex_location: Option<&str>) -> Option<Self> {
        let (project, location) = resolve_config(vertex_project_id, vertex_location);
        Some(VertexAiProvider {
            client: reqwest::Client::new(),
            project: project?,
            location,
        })
    }

    fn endpoint_url(&self) -> String {
        format!(
            "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/google/models/{VERTEX_MODEL_NAME}:predict",
            self.location, self.project, self.location
        )
    }

    async fn access_token(&self) -> Result<Arc<gcp_auth::Token>, VertexAiError> {
        let manager = gcp_auth::provider()
            .await
            .map_err(|e| VertexAiError::Auth(e.to_string()))?;
        manager
            .token(&[AUTH_SCOPE])
            .await
            .map_err(|e| VertexAiError::Auth(e.to_string()))
    }

    async fn predict(&self, instance: Value) -> Result<Value, VertexAiError> {
        let token = self.access_token().await?;
        let body = json!({
            "instances": [instance],
            "parameters": {"dimension": VERTEX_EMBEDDING_DIM},
        });
        let resp = self
            .client
            .post(self.endpoint_url())
            .bearer_auth(token.as_str())
            .json(&body)
            .timeout(Duration::from_secs(60))
            .send()
            .await?;
        Ok(resp.json::<Value>().await?)
    }

    /// Port of `get_image_embeddings`: one request per image (API
    /// limit), `None` per-image on any failure rather than aborting the
    /// batch.
    pub async fn get_image_embeddings(&self, images: &[Vec<u8>]) -> Vec<Option<Vec<f32>>> {
        let mut results = Vec::with_capacity(images.len());
        for img_bytes in images {
            let instance = json!({
                "image": {
                    "bytesBase64Encoded": base64_encode(img_bytes),
                    "mimeType": detect_mime_type(img_bytes),
                },
            });
            let embedding = match self.predict(instance).await {
                Ok(result) => result
                    .get("predictions")
                    .and_then(Value::as_array)
                    .and_then(|p| p.first())
                    .and_then(|p| extract_embedding(p, "imageEmbedding"))
                    .and_then(l2_normalize),
                Err(e) => {
                    log::warn!("Vertex embedding failed: {e}");
                    None
                }
            };
            results.push(embedding);
        }
        results
    }

    /// Port of `get_text_embedding`.
    pub async fn get_text_embedding(&self, text: &str) -> Option<Vec<f32>> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let instance = json!({"text": text});
        match self.predict(instance).await {
            Ok(result) => result
                .get("predictions")
                .and_then(Value::as_array)
                .and_then(|p| p.first())
                .and_then(|p| extract_embedding(p, "textEmbedding"))
                .and_then(l2_normalize),
            Err(e) => {
                log::warn!("Vertex text embedding failed: {e}");
                None
            }
        }
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_mime_type_recognizes_png_and_defaults_to_jpeg() {
        assert_eq!(
            detect_mime_type(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']),
            "image/png"
        );
        assert_eq!(detect_mime_type(&[0xFF, 0xD8, 0xFF]), "image/jpeg");
        assert_eq!(detect_mime_type(&[]), "image/jpeg");
    }

    #[test]
    fn resolve_config_prefers_explicit_args_over_env() {
        let (project, location) = resolve_config(Some("my-project"), Some("europe-west1"));
        assert_eq!(project.as_deref(), Some("my-project"));
        assert_eq!(location, "europe-west1");
    }

    #[test]
    fn resolve_config_defaults_location_when_unset() {
        // SAFETY: single-threaded test env var manipulation for this
        // specific isolated check; no other test reads these vars.
        unsafe {
            std::env::remove_var("VERTEX_LOCATION");
            std::env::remove_var("GOOGLE_CLOUD_PROJECT");
            std::env::remove_var("VERTEX_PROJECT_ID");
        }
        let (project, location) = resolve_config(None, None);
        assert_eq!(project, None);
        assert_eq!(location, "us-central1");
    }

    #[test]
    fn extract_embedding_handles_plain_array_and_values_wrapper() {
        let plain = json!({"imageEmbedding": [1.0, 2.0, 3.0]});
        assert_eq!(
            extract_embedding(&plain, "imageEmbedding"),
            Some(vec![1.0, 2.0, 3.0])
        );

        let wrapped = json!({"textEmbedding": {"values": [4.0, 5.0]}});
        assert_eq!(
            extract_embedding(&wrapped, "textEmbedding"),
            Some(vec![4.0, 5.0])
        );

        let missing = json!({});
        assert_eq!(extract_embedding(&missing, "imageEmbedding"), None);

        let empty = json!({"imageEmbedding": []});
        assert_eq!(extract_embedding(&empty, "imageEmbedding"), None);
    }

    #[test]
    fn l2_normalize_produces_unit_vector_and_rejects_near_zero() {
        let normalized = l2_normalize(vec![3.0, 4.0]).unwrap();
        assert!((normalized[0] - 0.6).abs() < 1e-6);
        assert!((normalized[1] - 0.8).abs() < 1e-6);

        assert_eq!(l2_normalize(vec![0.0, 0.0]), None);
    }

    #[test]
    fn provider_new_none_when_no_project_resolvable() {
        unsafe {
            std::env::remove_var("GOOGLE_CLOUD_PROJECT");
            std::env::remove_var("VERTEX_PROJECT_ID");
        }
        assert!(VertexAiProvider::new(None, None).is_none());
        assert!(VertexAiProvider::new(Some("proj"), None).is_some());
    }
}
