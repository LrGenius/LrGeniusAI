//! Who may use the backend: the access check in front of every route.
//!
//! Every request has to carry the backend's access token
//! ([`lrg_common::api_token`]) as `Authorization: Bearer <token>`. That keeps
//! out what listening on 127.0.0.1 alone cannot: a web page in the user's
//! browser (DNS rebinding makes the browser treat 127.0.0.1 as the page's own
//! site), other accounts on the same computer, and — when the backend listens
//! on the network for a plug-in on another computer — everyone else there.
//!
//! On top of the token, two checks that cost nothing:
//! - a backend that listens only on this computer answers only requests
//!   addressed to it by that name (`Host`), which turns a rebinding page away
//!   before it reaches any route;
//! - a request a page from another site sent (`Origin`) is refused even with
//!   the token. The backend's own pages (`/v1/ui/*`) are the same site.
//!
//! Some routes are let through without the token:
//! - `/ping`, `/version`, `/version/check`: a plug-in has to be able to find
//!   out what backend it is talking to, and that it is too old for it, before
//!   it can know how to get in;
//! - `/mcp` and `/v1/chat/*`: they check their own tokens (the agent token,
//!   a chat's own);
//! - the HTML of the browser pages. A browser cannot add a header when it
//!   opens an address, and the page holds nothing; it gets the token from the
//!   address's `#fragment` and sends it with every request of its own.
//!
//! This is applied around the whole app, outside the router the MCP tools and
//! the chat call in-process: those calls never cross the network.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// What the plug-in recognises a refused token by (`error_code` in the body).
/// The status alone would not do: the agent bridge's routes answer 401 for a
/// session that has to register again.
pub const ERROR_CODE: &str = "api_token";

const WRONG_TOKEN: &str = "This request did not carry the backend's access token, or carried \
                           a different one. The LrGeniusAI plug-in sends it by itself when the \
                           backend runs on the same computer; an older plug-in does not, so \
                           update the plug-in. For a backend on another computer, run \
                           `geniusai-server token` there and paste what it prints under \
                           Plug-in Manager > LrGeniusAI > Backend Server > Access token.";

const NOT_THIS_COMPUTER: &str = "This backend only answers requests addressed to this computer \
                                 (127.0.0.1 or localhost).";

const FOREIGN_PAGE: &str = "This backend does not accept requests from other web pages.";

pub struct ApiAuth {
    token: String,
    loopback_only: bool,
}

impl ApiAuth {
    /// `listen_host` is the address the backend listens on
    /// (`GENIUSAI_HOST`): only one on this computer turns on the `Host` check.
    pub fn new(token: String, listen_host: &str) -> Self {
        Self {
            token,
            loopback_only: is_loopback_host(listen_host),
        }
    }
}

/// Whether a `Host` header (or a listen address) names this computer:
/// `localhost` or a loopback address, with or without a port. An absent Host
/// (HTTP/1.0, some test clients) counts as local.
pub fn is_loopback_host(host: &str) -> bool {
    if host.is_empty() {
        return true;
    }
    let name = if let Some(rest) = host.strip_prefix('[') {
        // [::1]:19819
        rest.split(']').next().unwrap_or("")
    } else if host.matches(':').count() > 1 {
        // A bare IPv6 address, as a listen address is written.
        host
    } else {
        host.rsplit_once(':')
            .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
            .map_or(host, |(name, _)| name)
    };
    let name = name.to_ascii_lowercase();
    name == "localhost"
        || name
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Routes that need no token; see the module docs for why each.
fn exempt(method: &Method, path: &str) -> bool {
    matches!(path, "/ping" | "/version" | "/version/check")
        || path == "/mcp"
        || path.starts_with("/mcp/")
        || path.starts_with("/v1/chat/")
        || (method == Method::GET && matches!(path, "/v1/ui/people" | "/v1/ui/chat"))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

/// A page from another site sent this. Browsers send `Origin` with every
/// cross-site request and with same-site POSTs; the plug-in sends none.
fn foreign_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) else {
        return false;
    };
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    let site = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"));
    site != Some(host)
}

fn refuse(message: &str, code: Option<&str>) -> Response {
    let mut body = json!({"error": message});
    if let Some(code) = code {
        body["error_code"] = json!(code);
    }
    (StatusCode::FORBIDDEN, Json(body)).into_response()
}

pub async fn require_api_token(
    State(auth): State<Arc<ApiAuth>>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    if auth.loopback_only {
        let host = headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default();
        if !is_loopback_host(host) {
            return refuse(NOT_THIS_COMPUTER, None);
        }
    }
    if exempt(request.method(), request.uri().path()) {
        return next.run(request).await;
    }
    if foreign_origin(headers) {
        return refuse(FOREIGN_PAGE, None);
    }
    match bearer(headers) {
        Some(token) if lrg_common::api_token::matches(&auth.token, token) => {
            next.run(request).await
        }
        _ => refuse(WRONG_TOKEN, Some(ERROR_CODE)),
    }
}

/// Puts the check in front of `app`.
pub fn protect(app: axum::Router, auth: ApiAuth) -> axum::Router {
    app.layer(axum::middleware::from_fn_with_state(
        Arc::new(auth),
        require_api_token,
    ))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::{get, post};
    use axum::Router;
    use tower::ServiceExt;

    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn app(listen_host: &str) -> Router {
        let routes = Router::new()
            .route("/ping", get(|| async { "pong" }))
            .route("/v1/db/stats", get(|| async { "stats" }))
            .route("/v1/ui/people", get(|| async { "<html>" }))
            .route("/v1/ui/actions", post(|| async { "queued" }))
            .route("/v1/chat/sessions", post(|| async { "chat" }))
            .route("/mcp", post(|| async { "mcp" }));
        protect(routes, ApiAuth::new(TOKEN.to_string(), listen_host))
    }

    async fn status(app: Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    fn get_req(path: &str, host: &str, token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri(path).header("host", host);
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn the_api_needs_the_token() {
        let app = app("127.0.0.1");
        let (code, body) = status(
            app.clone(),
            get_req("/v1/db/stats", "127.0.0.1:19819", None),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN);
        assert_eq!(body["error_code"], ERROR_CODE);

        let wrong = "fedcba9876543210fedcba9876543210";
        let (code, _) = status(
            app.clone(),
            get_req("/v1/db/stats", "127.0.0.1:19819", Some(wrong)),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN);

        let (code, _) = status(app, get_req("/v1/db/stats", "127.0.0.1:19819", Some(TOKEN))).await;
        assert_eq!(code, StatusCode::OK);
    }

    #[tokio::test]
    async fn bootstrap_pages_and_self_guarded_routes_need_no_token() {
        let app = app("127.0.0.1");
        for path in ["/ping", "/v1/ui/people"] {
            let (code, _) = status(app.clone(), get_req(path, "localhost:19819", None)).await;
            assert_eq!(code, StatusCode::OK, "{path}");
        }
        for path in ["/mcp", "/v1/chat/sessions"] {
            let request = Request::builder()
                .method("POST")
                .uri(path)
                .header("host", "127.0.0.1:19819")
                .body(Body::empty())
                .unwrap();
            let (code, _) = status(app.clone(), request).await;
            assert_eq!(code, StatusCode::OK, "{path}");
        }
    }

    #[tokio::test]
    async fn a_loopback_backend_turns_away_other_host_names() {
        // DNS rebinding: the page's own name, resolved to 127.0.0.1.
        let (code, _) = status(
            app("127.0.0.1"),
            get_req("/ping", "evil.example:19819", None),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN);
        // Listening on the network, other names are how it is reached.
        let (code, _) = status(app("0.0.0.0"), get_req("/ping", "nas.local:19819", None)).await;
        assert_eq!(code, StatusCode::OK);
        let (code, _) = status(
            app("0.0.0.0"),
            get_req("/v1/db/stats", "nas.local:19819", None),
        )
        .await;
        assert_eq!(code, StatusCode::FORBIDDEN);
        let (code, _) = status(
            app("0.0.0.0"),
            get_req("/v1/db/stats", "nas.local:19819", Some(TOKEN)),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
    }

    #[tokio::test]
    async fn only_the_backends_own_pages_may_send_from_a_browser() {
        let post_from = |origin: &str| {
            Request::builder()
                .method("POST")
                .uri("/v1/ui/actions")
                .header("host", "127.0.0.1:19819")
                .header("origin", origin)
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap()
        };
        let (code, _) = status(app("127.0.0.1"), post_from("http://127.0.0.1:19819")).await;
        assert_eq!(code, StatusCode::OK);
        let (code, _) = status(app("127.0.0.1"), post_from("https://evil.example")).await;
        assert_eq!(code, StatusCode::FORBIDDEN);
        let (code, _) = status(app("127.0.0.1"), post_from("null")).await;
        assert_eq!(code, StatusCode::FORBIDDEN);
    }

    #[test]
    fn loopback_names() {
        for host in [
            "",
            "localhost",
            "LOCALHOST:19819",
            "127.0.0.1",
            "127.0.0.1:19819",
            "[::1]:19819",
            "::1",
        ] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "0.0.0.0",
            "::",
            "192.168.1.5:19819",
            "evil.example",
            "127.0.0.1.evil.example",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }
}
