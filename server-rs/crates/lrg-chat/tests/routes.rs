//! The chat's HTTP routes through the real router: who may open a chat, who
//! may use one, and what the page gets.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use lrg_api::agent_bridge::{PluginInfo, RegisterRequest};
use lrg_api::state::AppState;

fn app() -> (Arc<AppState>, axum::Router) {
    let state = Arc::new(AppState::new(None, true));
    let api = lrg_api::build_router(state.clone());
    let router = lrg_chat::router(state.clone(), api, CancellationToken::new());
    (state, router)
}

/// Plays the plugin registering its agent bridge; returns the agent token.
fn register(state: &AppState) -> String {
    state
        .agent_bridge
        .register_with(RegisterRequest {
            info: PluginInfo::default(),
            ..Default::default()
        })
        .expect("nothing newer is registered")
        .mcp_token
}

fn request(method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:19819")
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap()
}

async fn json_of(response: Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn open(router: &axum::Router, agent_token: &str) -> (String, String) {
    let response = router
        .clone()
        .oneshot(request(
            "POST",
            "/v1/chat/sessions",
            Some(agent_token),
            Some(json!({"provider": "llamacpp", "model": "no-such-model"})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    let (id, token) = (
        body["session_id"].as_str().unwrap().to_string(),
        body["token"].as_str().unwrap().to_string(),
    );
    assert_eq!(
        body["page"],
        format!("/v1/ui/chat#session={id}&token={token}")
    );
    (id, token)
}

#[tokio::test]
async fn opening_a_chat_takes_the_agent_token() {
    let (state, router) = app();
    let body = Some(json!({"provider": "mlx", "model": "m"}));

    let response = router
        .clone()
        .oneshot(request("POST", "/v1/chat/sessions", None, body.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let message = json_of(response).await["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(message.contains("agent bridge is not running"), "{message}");

    let agent_token = register(&state);
    let response = router
        .clone()
        .oneshot(request("POST", "/v1/chat/sessions", Some("wrong"), body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = router
        .clone()
        .oneshot(request(
            "POST",
            "/v1/chat/sessions",
            Some(&agent_token),
            Some(json!({"provider": "openai", "model": "gpt"})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    open(&router, &agent_token).await;
}

#[tokio::test]
async fn a_chat_answers_only_its_own_token_from_its_own_origin() {
    let (state, router) = app();
    let agent_token = register(&state);
    let (id, token) = open(&router, &agent_token).await;
    let uri = format!("/v1/chat/sessions/{id}");

    for presented in [None, Some("wrong"), Some(agent_token.as_str())] {
        let response = router
            .clone()
            .oneshot(request("GET", &uri, presented, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{presented:?}");
    }

    let mut foreign = request("GET", &uri, Some(&token), None);
    foreign
        .headers_mut()
        .insert("origin", "http://evil.example".parse().unwrap());
    let response = router.clone().oneshot(foreign).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let mut own = request("GET", &uri, Some(&token), None);
    own.headers_mut()
        .insert("origin", "http://127.0.0.1:19819".parse().unwrap());
    let response = router.clone().oneshot(own).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let info = json_of(response).await;
    assert_eq!(info["model"], "no-such-model");
    assert_eq!(info["permissions"]["mode"], "auto_read");
    let tools = info["tools"].as_array().unwrap();
    assert!(tools
        .iter()
        .any(|t| t["name"] == "update_photos" && t["class"] == "destructive"));
    assert!(tools.iter().all(|t| t["name"] != "eval_lua"));
}

#[tokio::test]
async fn the_event_stream_replays_what_happened() {
    let (state, router) = app();
    let agent_token = register(&state);
    let (id, token) = open(&router, &agent_token).await;

    // Warming up a model that does not exist is reported on the stream.
    let response = router
        .clone()
        .oneshot(request(
            "GET",
            &format!("/v1/chat/sessions/{id}/events"),
            Some(&token),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    let mut body = response.into_body();
    let frame = tokio::time::timeout(Duration::from_secs(10), body.frame())
        .await
        .expect("an event arrives")
        .unwrap()
        .unwrap();
    let text = String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap();
    assert!(text.starts_with("data: "), "{text}");
    let event: Value = serde_json::from_str(text.trim().trim_start_matches("data: ")).unwrap();
    assert_eq!(event["type"], "error");
    assert_eq!(event["seq"], 1);
    let message = event["message"].as_str().unwrap();
    assert!(message.contains("no-such-model"), "{message}");
}

#[tokio::test]
async fn permissions_can_be_changed_and_a_closed_chat_is_gone() {
    let (state, router) = app();
    let agent_token = register(&state);
    let (id, token) = open(&router, &agent_token).await;
    let uri = format!("/v1/chat/sessions/{id}/permissions");

    let response = router
        .clone()
        .oneshot(request(
            "PUT",
            &uri,
            Some(&token),
            Some(json!({"mode": "auto_safe", "tools": {"update_photos": "ask"}, "bulk_limit": 50})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["saved"], false, "no index is bound in this test");
    assert_eq!(body["permissions"]["mode"], "auto_safe");
    assert_eq!(body["permissions"]["tools"]["update_photos"], "ask");

    let response = router
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("/v1/chat/sessions/{id}"),
            Some(&token),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = router
        .clone()
        .oneshot(request("GET", &uri, Some(&token), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_empty_message_or_an_unknown_approval_is_refused() {
    let (state, router) = app();
    let agent_token = register(&state);
    let (id, token) = open(&router, &agent_token).await;

    let response = router
        .clone()
        .oneshot(request(
            "POST",
            &format!("/v1/chat/sessions/{id}/messages"),
            Some(&token),
            Some(json!({"text": "   "})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = router
        .clone()
        .oneshot(request(
            "POST",
            &format!("/v1/chat/sessions/{id}/approvals/call_9"),
            Some(&token),
            Some(json!({"decision": "once"})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn the_page_is_served_with_strict_headers() {
    let (_, router) = app();
    let response = router
        .oneshot(request("GET", "/v1/ui/chat", None, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let csp = response.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        csp.contains("default-src 'none'") && csp.contains("frame-ancestors 'none'"),
        "{csp}"
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let page = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(page.contains("Chat with your catalog"));
}
