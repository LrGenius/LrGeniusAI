//! The OpenAI-compatible client against a fake server on a local port: what it
//! sends, and how it copes with the ways real servers differ.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use lrg_providers::openai_compatible::OpenAiCompatibleProvider;
use lrg_providers::types::MetadataGenerationRequest;

/// One request as the fake saw it: its `Authorization` header and body.
type Seen = (Option<String>, Value);

#[derive(Clone, Default)]
struct Fake {
    /// Every request, in order.
    seen: Arc<Mutex<Vec<Seen>>>,
    /// Answer `json_schema` requests with a 400, like LiteLLM without
    /// `drop_params`.
    reject_schema: bool,
    /// Refuse requests without this bearer key.
    require_key: Option<&'static str>,
}

impl Fake {
    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn auth_of(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

async fn chat(
    State(fake): State<Fake>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let auth = auth_of(&headers);
    fake.seen.lock().unwrap().push((auth.clone(), body.clone()));
    if let Some(key) = fake.require_key {
        if auth.as_deref() != Some(format!("Bearer {key}").as_str()) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": {"message": "Invalid API key"}})),
            );
        }
    }
    let format = body["response_format"]["type"].as_str().unwrap_or("none");
    if fake.reject_schema && format == "json_schema" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "response_format json_schema is not supported"}})),
        );
    }
    let answer = json!({
        "keywords": ["harbour", "boats"],
        "caption": "Boats in a harbour",
        "title": "Harbour",
    })
    .to_string();
    // Without a schema to hold it to, a model wraps its answer in prose.
    let content = if format == "json_schema" {
        answer
    } else {
        format!("Here you go:\n```json\n{answer}\n```")
    };
    (
        StatusCode::OK,
        Json(json!({
            "choices": [{"finish_reason": "stop", "message": {"content": content}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5},
        })),
    )
}

async fn models(State(fake): State<Fake>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    fake.seen
        .lock()
        .unwrap()
        .push((auth_of(&headers), json!("models")));
    (
        StatusCode::OK,
        Json(json!({"data": [
            {"id": "vision-model", "architecture": {"input_modalities": ["text", "image"]}},
            {"id": "text-only", "architecture": {"input_modalities": ["text"]}},
        ]})),
    )
}

/// Start the fake and return its address the way a user would type it.
async fn serve(fake: Fake) -> String {
    let app = Router::new()
        .route("/v1/chat/completions", post(chat))
        .route("/v1/models", get(models))
        .with_state(fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("127.0.0.1:{port}")
}

fn photo_request(model: &str) -> MetadataGenerationRequest {
    MetadataGenerationRequest {
        uuid: "photo-1".to_string(),
        model: model.to_string(),
        // JPEG magic: passed through as-is, never decoded.
        image_data: vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0],
        generate_keywords: true,
        generate_caption: true,
        generate_title: true,
        temperature: 0.2,
        ..Default::default()
    }
}

#[tokio::test]
async fn the_key_is_sent_only_when_there_is_one() {
    let fake = Fake::default();
    let address = serve(fake.clone()).await;

    let with_key = OpenAiCompatibleProvider::custom(&address, Some("sk-test".to_string())).unwrap();
    let without_key = OpenAiCompatibleProvider::custom(&address, Some("  ".to_string())).unwrap();
    assert!(
        with_key
            .generate_metadata(&photo_request("m-key"))
            .await
            .success
    );
    assert!(
        without_key
            .generate_metadata(&photo_request("m-key"))
            .await
            .success
    );

    let auth: Vec<Option<String>> = fake.requests().into_iter().map(|(a, _)| a).collect();
    assert_eq!(auth, vec![Some("Bearer sk-test".to_string()), None]);
}

#[tokio::test]
async fn the_photo_goes_as_a_data_uri_with_the_schema() {
    let fake = Fake::default();
    let address = serve(fake.clone()).await;
    let provider = OpenAiCompatibleProvider::custom(&address, None).unwrap();

    let response = provider.generate_metadata(&photo_request("m-shape")).await;
    assert!(response.success, "{:?}", response.error);
    assert_eq!(response.title.as_deref(), Some("Harbour"));
    assert!(response.warning.is_none(), "{:?}", response.warning);

    let (_, body) = &fake.requests()[0];
    assert_eq!(body["response_format"]["type"], "json_schema");
    let image = body["messages"][1]["content"][1]["image_url"]["url"]
        .as_str()
        .unwrap();
    assert!(image.starts_with("data:image/jpeg;base64,"), "{image}");
}

/// A server that rejects `json_schema` is asked again with a looser format,
/// and that is remembered: the next photo — through a new provider instance,
/// as every photo gets — costs one request, not two.
#[tokio::test]
async fn a_rejected_schema_falls_back_once_and_is_remembered() {
    let fake = Fake {
        reject_schema: true,
        ..Default::default()
    };
    let address = serve(fake.clone()).await;

    let first = OpenAiCompatibleProvider::custom(&address, None)
        .unwrap()
        .generate_metadata(&photo_request("m-fallback"))
        .await;
    assert!(first.success, "{:?}", first.error);
    assert_eq!(first.caption.as_deref(), Some("Boats in a harbour"));
    let warning = first.warning.unwrap_or_default();
    assert!(warning.contains("cannot constrain"), "{warning}");
    assert_eq!(fake.requests().len(), 2);

    let second = OpenAiCompatibleProvider::custom(&address, None)
        .unwrap()
        .generate_metadata(&photo_request("m-fallback"))
        .await;
    assert!(second.success, "{:?}", second.error);
    // Every photo answered in the looser format says so: a later run must be
    // told too, and the plugin deduplicates and caps repeated warnings.
    assert!(second
        .warning
        .unwrap_or_default()
        .contains("cannot constrain"));
    let requests = fake.requests();
    assert_eq!(requests.len(), 3, "the second photo needs one request");
    assert_eq!(requests[2].1["response_format"]["type"], "json_object");
}

#[tokio::test]
async fn a_rejected_key_says_so() {
    let fake = Fake {
        require_key: Some("right"),
        ..Default::default()
    };
    let address = serve(fake).await;
    let provider = OpenAiCompatibleProvider::custom(&address, Some("wrong".to_string())).unwrap();

    let response = provider.generate_metadata(&photo_request("m-auth")).await;
    assert!(!response.success);
    let error = response.error.unwrap_or_default();
    assert!(error.contains("rejected the API key"), "{error}");
}

#[tokio::test]
async fn only_models_that_take_photos_are_listed() {
    let fake = Fake::default();
    let address = serve(fake.clone()).await;
    let provider = OpenAiCompatibleProvider::custom(&address, Some("sk-list".to_string())).unwrap();

    assert_eq!(
        provider.list_models_checked().await,
        Ok(vec!["vision-model".to_string()])
    );
    assert_eq!(
        fake.requests()[0].0.as_deref(),
        Some("Bearer sk-list"),
        "listing sends the key too"
    );
}
