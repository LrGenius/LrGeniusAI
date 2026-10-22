//! The chat's HTTP routes, written as served: `lrg-server` merges this router
//! at the root, beside `/mcp`, rather than under `lrg_api`'s `/v1` nesting.
//!
//! Two secrets guard them. Opening a chat takes the agent token, which only
//! the plugin and the user's own AI apps have — the same check as `/mcp`.
//! Everything after that takes the chat's own token, which the plugin hands
//! the page in the URL fragment (fragments are never sent to a server, so it
//! stays out of logs). Unlike `/v1/ui/people`, these routes can change the
//! catalog whenever Lightroom is open, so they cannot be open to any page in
//! the browser the way the rest of `/v1` is.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;

use lrg_api::agent_bridge::McpAccess;
use lrg_api::routes::llm::engine_overrides_from_json;
use lrg_providers::cloud_chat::build_chat_model;
use lrg_providers::openai_compatible::ServerUrl;
use lrg_providers::provider::ProviderSelection;

use crate::events::Sequenced;
use crate::permissions::Permissions;
use crate::session::{Approval, ChatSession, CloudModel, LocalModels, ModelSource};
use crate::ChatHub;

const NO_BRIDGE: &str = "LrGeniusAI's agent bridge is not running, so a chat cannot be opened. \
                         Turn it on under Plug-in Manager > LrGeniusAI > AI Agents.";

const WRONG_AGENT_TOKEN: &str = "Missing or wrong LrGeniusAI agent token. Open the chat from \
                                 Lightroom: Library > Plug-in Extras > LrGeniusAI.";

const WRONG_CHAT: &str = "This chat has ended or the link is not valid. Open the chat again from \
                          Lightroom: Library > Plug-in Extras > LrGeniusAI.";

/// Baked into the binary, like the People page: a page that could go missing
/// or go stale next to an updated binary is a support problem the size does
/// not justify.
const CHAT_HTML: &str = include_str!("ui/chat.html");

pub(crate) fn router(hub: Arc<ChatHub>) -> Router {
    Router::new()
        .route("/v1/ui/chat", get(chat_page))
        .route("/v1/chat/sessions", post(create_session))
        .route(
            "/v1/chat/sessions/{id}",
            get(session_info).delete(close_session),
        )
        .route("/v1/chat/sessions/{id}/events", get(events))
        .route("/v1/chat/sessions/{id}/messages", post(send_message))
        .route("/v1/chat/sessions/{id}/approvals/{call_id}", post(approve))
        .route("/v1/chat/sessions/{id}/stop", post(stop))
        .route(
            "/v1/chat/sessions/{id}/permissions",
            get(get_permissions).put(put_permissions),
        )
        .with_state(hub)
}

/// The page holds no data of its own — everything comes through the routes
/// below, with the token from its address — so it is served to anyone. The
/// headers keep it from loading anything from elsewhere or being framed.
async fn chat_page() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
                 connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'",
            ),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        CHAT_HTML,
    )
        .into_response()
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": message.into()}))).into_response()
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

/// A request from a page served anywhere but here is refused even with the
/// right token. Browsers send `Origin` with every cross-origin request and
/// with same-origin POSTs; a request without one is a same-origin GET or not
/// from a browser at all.
fn foreign_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) else {
        return false;
    };
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    origin.strip_prefix("http://") != Some(host)
}

/// A page request turned away: a wrong token, a foreign origin, or a chat
/// that has ended. All three get the same answer, so a probe learns nothing.
struct Refused;

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        error(StatusCode::FORBIDDEN, WRONG_CHAT)
    }
}

/// The session a page request is for, once its token checks out.
fn authorized(hub: &ChatHub, id: &str, headers: &HeaderMap) -> Result<Arc<ChatSession>, Refused> {
    if foreign_origin(headers) {
        return Err(Refused);
    }
    let session = hub
        .session(id)
        .filter(|s| bearer(headers).is_some_and(|t| s.token_matches(t)))
        .ok_or(Refused)?;
    session.touch();
    Ok(session)
}

async fn create_session(
    State(hub): State<Arc<ChatHub>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    match hub.state.agent_bridge.check_mcp_token(bearer(&headers)) {
        McpAccess::Granted => {}
        McpAccess::NoToken => return error(StatusCode::FORBIDDEN, NO_BRIDGE),
        McpAccess::Denied => return error(StatusCode::FORBIDDEN, WRONG_AGENT_TOKEN),
    }
    let text = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let provider = text("provider").unwrap_or_default().to_lowercase();
    let Some(model) = text("model") else {
        return error(
            StatusCode::BAD_REQUEST,
            "No AI model is chosen for the chat. Pick one in the plugin settings.",
        );
    };

    let models: Box<dyn ModelSource> = if matches!(provider.as_str(), "llamacpp" | "mlx") {
        Box::new(LocalModels {
            state: hub.state.clone(),
            provider,
            model,
            overrides: engine_overrides_from_json(&body),
        })
    } else {
        // The key stays in this session's memory: it is never logged, never
        // written anywhere and never handed to the page.
        let selection = ProviderSelection {
            name: provider.clone(),
            api_key: text("api_key"),
            server_url: text("server_url"),
            ollama_base_url: text("ollama_base_url"),
            lmstudio_base_url: text("lmstudio_base_url"),
            local_engine: None,
        };
        match build_chat_model(&selection, &model) {
            Ok(chat_model) => Box::new(CloudModel {
                model: chat_model,
                recipient: recipient(&selection),
            }),
            Err(message) => return error(StatusCode::BAD_REQUEST, message),
        }
    };
    let session = match ChatSession::open(
        hub.state.clone(),
        hub.api.clone(),
        models,
        hub.permissions.clone(),
    )
    .await
    {
        Ok(session) => session,
        Err(message) => return error(StatusCode::INTERNAL_SERVER_ERROR, message),
    };
    if let Err(message) = hub.insert(session.clone()) {
        return error(StatusCode::SERVICE_UNAVAILABLE, message);
    }
    session.warm_up();
    log::info!("Chat {} opened", session.id);
    Json(json!({
        "session_id": session.id,
        "token": session.token(),
        "page": format!("/v1/ui/chat#session={}&token={}", session.id, session.token()),
    }))
    .into_response()
}

/// Who a cloud chat's conversation goes to, as the page names them; `None`
/// for a server on this computer.
fn recipient(selection: &ProviderSelection) -> Option<String> {
    match selection.name.as_str() {
        "anthropic" => Some("Anthropic".into()),
        "gemini" => Some("Google".into()),
        "chatgpt" | "openai" => Some("OpenAI".into()),
        "openai_compatible" => selection
            .server_url
            .as_deref()
            .and_then(|url| ServerUrl::parse(url).ok())
            .map(|url| format!("your AI server ({})", url.label)),
        // LM Studio and Ollama: the server the plugin reaches on this
        // computer unless the user moved it, and then it is theirs.
        _ => None,
    }
}

async fn session_info(
    State(hub): State<Arc<ChatHub>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match authorized(&hub, &id, &headers) {
        Ok(session) => Json(session.info()).into_response(),
        Err(refused) => refused.into_response(),
    }
}

async fn close_session(
    State(hub): State<Arc<ChatHub>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match authorized(&hub, &id, &headers) {
        Ok(session) => {
            session.stop();
            hub.remove(&id);
            log::info!("Chat {id} closed");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(refused) => refused.into_response(),
    }
}

#[derive(Deserialize)]
struct EventsQuery {
    #[serde(default)]
    after: u64,
}

fn sse_event(event: &Sequenced) -> Event {
    Event::default().data(serde_json::to_string(event).unwrap_or_else(|_| "{}".into()))
}

async fn events(
    State(hub): State<Arc<ChatHub>>,
    Path(id): Path<String>,
    Query(query): Query<EventsQuery>,
    headers: HeaderMap,
) -> Response {
    let session = match authorized(&hub, &id, &headers) {
        Ok(session) => session,
        Err(refused) => return refused.into_response(),
    };
    let (past, receiver) = session.events.subscribe(query.after);
    let past: Vec<Event> = past.iter().map(sse_event).collect();
    // A page that fell too far behind is told to reconnect, and catches up
    // from the log; the stream ends there.
    let live = futures_util::stream::unfold(Some(receiver), |receiver| async move {
        let mut receiver = receiver?;
        match receiver.recv().await {
            Ok(event) => Some((sse_event(&event), Some(receiver))),
            Err(RecvError::Lagged(_)) => Some((Event::default().event("resync").data("{}"), None)),
            Err(RecvError::Closed) => None,
        }
    });
    let stream = futures_util::stream::iter(past)
        .chain(live)
        .map(Ok::<_, Infallible>)
        // An open stream would otherwise hold the backend's graceful
        // shutdown open indefinitely, as `/mcp`'s would.
        .take_until(hub.shutdown.clone().cancelled_owned());
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[derive(Deserialize)]
struct MessageBody {
    text: String,
}

async fn send_message(
    State(hub): State<Arc<ChatHub>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<MessageBody>,
) -> Response {
    let session = match authorized(&hub, &id, &headers) {
        Ok(session) => session,
        Err(refused) => return refused.into_response(),
    };
    let text = body.text.trim();
    if text.is_empty() {
        return error(StatusCode::BAD_REQUEST, "The message is empty.");
    }
    match session.send(text.to_string()) {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(message) => error(StatusCode::CONFLICT, message),
    }
}

async fn approve(
    State(hub): State<Arc<ChatHub>>,
    Path((id, call_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(approval): Json<Approval>,
) -> Response {
    let session = match authorized(&hub, &id, &headers) {
        Ok(session) => session,
        Err(refused) => return refused.into_response(),
    };
    match session.approve(&call_id, approval) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(message) => error(StatusCode::CONFLICT, message),
    }
}

async fn stop(
    State(hub): State<Arc<ChatHub>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match authorized(&hub, &id, &headers) {
        Ok(session) => {
            session.stop();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(refused) => refused.into_response(),
    }
}

async fn get_permissions(
    State(hub): State<Arc<ChatHub>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match authorized(&hub, &id, &headers) {
        Ok(session) => Json(session.permissions()).into_response(),
        Err(refused) => refused.into_response(),
    }
}

async fn put_permissions(
    State(hub): State<Arc<ChatHub>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(permissions): Json<Permissions>,
) -> Response {
    let session = match authorized(&hub, &id, &headers) {
        Ok(session) => session,
        Err(refused) => return refused.into_response(),
    };
    match session.set_permissions(permissions) {
        Ok(saved) => {
            Json(json!({"saved": saved, "permissions": session.permissions()})).into_response()
        }
        Err(message) => error(StatusCode::INTERNAL_SERVER_ERROR, message),
    }
}
