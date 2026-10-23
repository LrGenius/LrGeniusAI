//! The `/mcp` endpoint's gatekeeping, through the real router: the agent
//! token, the loopback-only Host check and the refusal of browser origins.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use lrg_api::agent_bridge::{PluginInfo, RegisterRequest};
use lrg_api::state::AppState;

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"auth-test","version":"1"}}}"#;

fn app() -> (Arc<AppState>, axum::Router) {
    let state = Arc::new(AppState::new(None, true));
    let api = lrg_api::build_router(state.clone());
    let router = lrg_mcp::router(state.clone(), api, CancellationToken::new());
    (state, router)
}

fn initialize(token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "127.0.0.1:19819")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(INITIALIZE)).unwrap()
}

async fn error_message(response: axum::response::Response) -> String {
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn register(state: &AppState) -> String {
    // `register_with` needs an `Arc<AgentBridge>`; AppState holds one.
    state
        .agent_bridge
        .register_with(RegisterRequest {
            info: PluginInfo::default(),
            ..Default::default()
        })
        .expect("nothing newer is registered")
        .mcp_token
}

#[tokio::test]
async fn without_the_bridge_mcp_explains_how_to_start_it() {
    let (_, app) = app();
    let response = app.oneshot(initialize(Some("anything"))).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(error_message(response)
        .await
        .contains("agent bridge is not running"));
}

#[tokio::test]
async fn a_missing_or_wrong_token_is_refused_and_the_right_one_admitted() {
    let (state, app) = app();
    let token = register(&state);

    let missing = app.clone().oneshot(initialize(None)).await.unwrap();
    assert_eq!(missing.status(), StatusCode::FORBIDDEN);
    assert!(error_message(missing).await.contains("Plug-in Manager"));

    let wrong = app
        .clone()
        .oneshot(initialize(Some("0".repeat(64).as_str())))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::FORBIDDEN);

    let right = app.oneshot(initialize(Some(&token))).await.unwrap();
    assert_eq!(right.status(), StatusCode::OK);
    assert!(
        right.headers().contains_key("mcp-session-id"),
        "the session starts once the token matches"
    );
}

#[tokio::test]
async fn browsers_and_rebinding_pages_are_refused_even_with_the_token() {
    let (state, app) = app();
    let token = register(&state);

    let mut from_page = initialize(Some(&token));
    from_page
        .headers_mut()
        .insert("origin", "https://example.com".parse().unwrap());
    let response = app.clone().oneshot(from_page).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let mut rebound = initialize(Some(&token));
    rebound
        .headers_mut()
        .insert("host", "evil.example:19819".parse().unwrap());
    let response = app.oneshot(rebound).await.unwrap();
    assert_ne!(response.status(), StatusCode::OK);
}
