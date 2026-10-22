//! The MCP (Model Context Protocol) server: lets an AI agent — Claude Code,
//! Claude Desktop, LM Studio, Cursor, any MCP client — use LrGeniusAI's index
//! and drive Lightroom through the plugin.
//!
//! Served at `/mcp` on the backend's own port as Streamable HTTP, so there is
//! no second process to run. The tools call the backend's REST routes
//! in-process ([`api`]) and reach Lightroom through the agent bridge
//! (`lrg_api::agent_bridge`). [`stdio`] is a small proxy for clients that only
//! start local servers as a command (Claude Desktop).
//!
//! Exposure: rmcp only answers requests whose `Host` is this computer (the
//! defence against DNS rebinding), and Origin validation is enforced with an
//! empty allow-list, so no web page in the user's browser can call it. MCP
//! clients are not browsers and send no `Origin`.
//!
//! On top of that every request needs the agent token as a bearer token. That
//! keeps out what the loopback rules cannot: other accounts on the same
//! computer (127.0.0.1 is shared) and local programs that are not the user's
//! AI app. The plugin keeps the token in the system's password storage and
//! hands it over when its agent bridge registers (see
//! `lrg_api::agent_bridge::McpAccess`); the user copies it from the Plug-in
//! Manager into their AI app's setup.

pub mod api;
pub mod server;
pub mod sets;
pub mod stdio;
mod tools_dev;
mod tools_index;
mod tools_lightroom;

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header::AUTHORIZATION, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use lrg_api::agent_bridge::McpAccess;
use lrg_api::state::AppState;

pub use server::{LrGeniusMcp, Shared, MAX_PHOTOS_PER_CALL};

/// Where the endpoint is mounted. Unversioned like `/ping`: MCP negotiates
/// its own protocol version.
pub const MCP_PATH: &str = "/mcp";

/// Builds the `/mcp` router.
///
/// `api` is the backend's own router (`lrg_api::build_router`), which the
/// tools call in-process. `shutdown` ends every MCP session when cancelled:
/// a client's open event stream would otherwise hold the graceful shutdown
/// open indefinitely.
pub fn router(
    state: Arc<AppState>,
    api: axum::Router,
    shutdown: CancellationToken,
) -> axum::Router {
    let state_for_auth = state.clone();
    let shared = Arc::new(Shared {
        state,
        api: api::Api::new(api),
        sets: sets::ResultSets::new(),
    });
    let config = StreamableHttpServerConfig::default()
        .enforce_origin_validation()
        .with_cancellation_token(shutdown);
    let service: StreamableHttpService<LrGeniusMcp, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(LrGeniusMcp::new(shared.clone())),
            Arc::new(LocalSessionManager::default()),
            config,
        );
    axum::Router::new()
        .nest_service(MCP_PATH, service)
        .layer(middleware::from_fn_with_state(
            state_for_auth,
            require_agent_token,
        ))
}

const NO_TOKEN: &str = "LrGeniusAI's agent bridge is not running, so there is no agent token to \
                        check yours against. Open Lightroom Classic with the LrGeniusAI plugin (the \
                        bridge starts with it), or turn the bridge on under Plug-in Manager > \
                        LrGeniusAI > AI Agents.";

const WRONG_TOKEN: &str = "Missing or wrong LrGeniusAI agent token. Copy the setup for your \
                           AI app from Lightroom: Plug-in Manager > LrGeniusAI > AI Agents. \
                           (The token changes when a new one is made there.)";

/// The bearer token of a request, if it carries one.
fn bearer(request: &Request) -> Option<&str> {
    let value = request.headers().get(AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

/// Lets a request through to the MCP service only with the agent token.
///
/// 403 rather than 401: a 401 sends MCP clients into OAuth discovery, which
/// this endpoint does not offer, and buries the message below a confusing
/// "authorization failed".
async fn require_agent_token(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let message = match state.agent_bridge.check_mcp_token(bearer(&request)) {
        McpAccess::Granted => return next.run(request).await,
        McpAccess::NoToken => NO_TOKEN,
        McpAccess::Denied => WRONG_TOKEN,
    };
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": {"code": -32001, "message": message},
        })),
    )
        .into_response()
}
