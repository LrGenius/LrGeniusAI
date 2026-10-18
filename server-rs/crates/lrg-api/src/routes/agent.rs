//! `/v1/agent/*` — the plugin's side of the agent bridge
//! ([`crate::agent_bridge`]): registering, claiming queued commands and
//! answering them. The MCP endpoint is the other side; it queues the commands.
//!
//! Exposure: unlike the rest of this API, these routes hand out and accept
//! work that changes the Lightroom catalog. So they turn away anything a web
//! page could send. A request with an `Origin` header comes from a browser —
//! Lightroom's `LrHttp` sends none — and a `Host` that is not this computer is
//! a DNS-rebinding page that resolved its own name to 127.0.0.1. Everything but
//! registration and the status read also needs the session token, which only
//! the registered plugin holds.

use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent_bridge::{
    PluginInfo, PluginOutcome, Progress, RegisterRequest, Superseded, PING_INTERVAL,
};
use crate::state::AppState;

/// The header the plugin sends its session token in.
pub const TOKEN_HEADER: &str = "x-lrgenius-bridge-token";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/agent/bridge",
            get(bridge_status).post(register).delete(unregister),
        )
        .route("/agent/commands", get(claim_commands))
        .route("/agent/commands/{id}/result", post(command_result))
        .route("/agent/commands/{id}/progress", post(command_progress))
}

/// `None` when the request may proceed.
fn refuse_browser_or_remote(headers: &HeaderMap) -> Option<Response> {
    if headers.contains_key(axum::http::header::ORIGIN) {
        return Some(forbidden(
            "The agent bridge does not accept requests from web pages.",
        ));
    }
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !is_loopback_host(host) {
        return Some(forbidden(
            "The agent bridge only answers requests addressed to this computer \
             (127.0.0.1 or localhost).",
        ));
    }
    None
}

/// `localhost`, `127.0.0.1`, `[::1]`, each with or without a port. An absent
/// Host (HTTP/1.0, some test clients) counts as local: the listener itself
/// only binds the loopback interface by default.
fn is_loopback_host(host: &str) -> bool {
    if host.is_empty() {
        return true;
    }
    let name = if let Some(rest) = host.strip_prefix('[') {
        // [::1]:19819
        rest.split(']').next().unwrap_or("")
    } else {
        host.rsplit_once(':')
            .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
            .map_or(host, |(name, _)| name)
    };
    matches!(
        name.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

fn forbidden(message: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({"error": message}))).into_response()
}

fn token(headers: &HeaderMap) -> &str {
    headers
        .get(TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

/// The plugin's token is not (or no longer) the live session — most often
/// because the backend restarted. `reregister` tells it to register again
/// instead of reporting a failure.
fn unknown_session() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "This agent bridge session is not known to the backend (it may have restarted).",
            "reregister": true,
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct RegisterBody {
    /// The plugin's LrSocket port. Absent when binding it failed; the plugin
    /// then works by asking every second.
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    previous_token: Option<String>,
    /// The agent token the plugin keeps; absent the first time.
    #[serde(default)]
    mcp_token: Option<String>,
    #[serde(default)]
    rotate_mcp_token: bool,
    /// When the plugin's bridge loop started; the newest one wins.
    #[serde(default)]
    started_at: Option<f64>,
    #[serde(flatten)]
    info: PluginInfo,
}

async fn register(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RegisterBody>,
) -> Response {
    if let Some(refused) = refuse_browser_or_remote(&headers) {
        return refused;
    }
    let registration = match state.agent_bridge.register_with(RegisterRequest {
        port: body.port,
        previous_token: body.previous_token,
        info: body.info,
        mcp_token: body.mcp_token,
        rotate_mcp_token: body.rotate_mcp_token,
        started_at: body.started_at,
    }) {
        Ok(registration) => registration,
        Err(Superseded) => return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "A newer LrGeniusAI agent bridge is registered (the plugin was reloaded \
                          or restarted). This one should stop.",
                "superseded": true,
            })),
        )
            .into_response(),
    };
    Json(json!({
        "status": "success",
        "token": registration.token,
        "session": registration.session,
        "resumed": registration.resumed,
        // Only in this answer to the plugin that registered, which keeps it
        // in its password storage and shows it to the user.
        "mcp_token": registration.mcp_token,
        // The plugin treats a connected socket that stayed silent for a few
        // of these as dead.
        "ping_interval_seconds": PING_INTERVAL.as_secs(),
        "server_version": lrg_common::version::backend_version(),
    }))
    .into_response()
}

async fn bridge_status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(refused) = refuse_browser_or_remote(&headers) {
        return refused;
    }
    let status = state.agent_bridge.status();
    let mut body = serde_json::to_value(status).unwrap_or(Value::Null);
    body["status"] = json!("success");
    Json(body).into_response()
}

async fn unregister(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(refused) = refuse_browser_or_remote(&headers) {
        return refused;
    }
    if !state.agent_bridge.unregister(token(&headers)) {
        return unknown_session();
    }
    Json(json!({"status": "success", "stopped": true})).into_response()
}

async fn claim_commands(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(refused) = refuse_browser_or_remote(&headers) {
        return refused;
    }
    match state.agent_bridge.claim(token(&headers)) {
        Some(commands) => Json(json!({"status": "success", "commands": commands})).into_response(),
        None => unknown_session(),
    }
}

async fn command_result(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<u64>,
    Json(outcome): Json<PluginOutcome>,
) -> Response {
    if let Some(refused) = refuse_browser_or_remote(&headers) {
        return refused;
    }
    match state.agent_bridge.complete(token(&headers), id, outcome) {
        Some(accepted) => {
            if !accepted {
                log::info!(
                    "Agent bridge: result for command {id} arrived after its caller stopped waiting"
                );
            }
            Json(json!({"status": "success", "accepted": accepted})).into_response()
        }
        None => unknown_session(),
    }
}

async fn command_progress(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<u64>,
    Json(progress): Json<Progress>,
) -> Response {
    if let Some(refused) = refuse_browser_or_remote(&headers) {
        return refused;
    }
    match state.agent_bridge.progress(token(&headers), id, progress) {
        Some(cancel) => Json(json!({"status": "success", "cancel": cancel})).into_response(),
        None => unknown_session(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;
    use crate::agent_bridge::CallOptions;

    fn app() -> (Router, Arc<AppState>) {
        let state = Arc::new(AppState::new(None, true));
        (router().with_state(state.clone()), state)
    }

    async fn body_json(response: Response) -> Value {
        let bytes = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn request(method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "127.0.0.1:19819");
        if let Some(token) = token {
            builder = builder.header(TOKEN_HEADER, token);
        }
        match body {
            Some(body) => builder
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        }
    }

    async fn register_plugin(app: &Router) -> String {
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                "/agent/bridge",
                None,
                Some(
                    json!({"plugin_version": "9.9.9", "catalog_id": "cat", "allow_changes": true}),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["ping_interval_seconds"], PING_INTERVAL.as_secs());
        assert_eq!(body["mcp_token"].as_str().unwrap().len(), 64);
        body["token"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn register_claim_and_answer_round_trip() {
        let (app, state) = app();
        let token = register_plugin(&app).await;

        let caller = {
            let state = state.clone();
            tokio::spawn(async move {
                state
                    .agent_bridge
                    .call(
                        "status",
                        json!({"k": "v"}),
                        CallOptions::read(Duration::from_secs(5)),
                    )
                    .await
            })
        };

        let command = loop {
            let response = app
                .clone()
                .oneshot(request("GET", "/agent/commands", Some(&token), None))
                .await
                .unwrap();
            let body = body_json(response).await;
            if let Some(first) = body["commands"].as_array().and_then(|c| c.first()) {
                break first.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(command["action"], "status");
        assert_eq!(command["args"]["k"], "v");

        let id = command["id"].as_u64().unwrap();
        let progress = app
            .clone()
            .oneshot(request(
                "POST",
                &format!("/agent/commands/{id}/progress"),
                Some(&token),
                Some(json!({"done": 1, "total": 2})),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(progress).await["cancel"], false);

        let answered = app
            .clone()
            .oneshot(request(
                "POST",
                &format!("/agent/commands/{id}/result"),
                Some(&token),
                Some(json!({"ok": true, "result": {"photos": 2}, "warnings": ["w"]})),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(answered).await["accepted"], true);

        let reply = caller.await.unwrap().unwrap();
        assert_eq!(reply.result["photos"], 2);
        assert_eq!(reply.warnings, vec!["w".to_string()]);

        let status = body_json(
            app.clone()
                .oneshot(request("GET", "/agent/bridge", None, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status["connected"], true);
        assert_eq!(status["plugin"]["catalog_id"], "cat");
        assert!(
            status.get("token").is_none() && !status.to_string().contains(&token),
            "the status read must never echo the session token"
        );
        assert!(
            !status.to_string().contains("mcp_token"),
            "the status read must never show the agent token"
        );
    }

    #[tokio::test]
    async fn a_wrong_token_is_told_to_register_again() {
        let (app, _) = app();
        register_plugin(&app).await;
        let response = app
            .oneshot(request("GET", "/agent/commands", Some("nope"), None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(response).await["reregister"], true);
    }

    #[tokio::test]
    async fn browser_and_rebinding_requests_are_refused() {
        let (app, _) = app();
        let mut from_page = request(
            "POST",
            "/agent/bridge",
            None,
            Some(json!({"allow_changes": true})),
        );
        from_page
            .headers_mut()
            .insert("origin", "https://example.com".parse().unwrap());
        let response = app.clone().oneshot(from_page).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let mut rebound = request("GET", "/agent/bridge", None, None);
        rebound
            .headers_mut()
            .insert("host", "evil.example:19819".parse().unwrap());
        let response = app.oneshot(rebound).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn unregister_stops_the_session() {
        let (app, state) = app();
        let token = register_plugin(&app).await;
        let response = app
            .clone()
            .oneshot(request("DELETE", "/agent/bridge", Some(&token), None))
            .await
            .unwrap();
        assert_eq!(body_json(response).await["stopped"], true);
        assert!(!state.agent_bridge.is_connected());
    }

    #[test]
    fn loopback_hosts_are_recognised() {
        for host in [
            "",
            "127.0.0.1",
            "127.0.0.1:19819",
            "localhost:19819",
            "LOCALHOST",
            "[::1]:19819",
        ] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "evil.example",
            "evil.example:19819",
            "127.0.0.1.nip.io:19819",
            "10.0.0.2:19819",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }
}
