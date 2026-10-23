//! In-process calls into the backend's own HTTP API.
//!
//! The MCP tools reuse the REST routes rather than the functions behind them.
//! The routes are what the plugin calls, so a tool gets the same answer the
//! plugin's own menu item would — same catalog scoping, same warnings, same
//! middleware. The calls never touch the network: an axum `Router` is a
//! `tower::Service`, and `oneshot` drives one request through it in memory.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use serde_json::Value;
use tower::ServiceExt;

/// Responses are JSON documents of a few MB at most (a cull run over a large
/// selection); this only guards against something unexpected.
const MAX_RESPONSE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone)]
pub struct Api {
    router: Router,
}

/// A failed API call, with the backend's own `error` text when it sent one.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Api {
    pub fn new(router: Router) -> Self {
        Self { router }
    }

    pub async fn get(&self, path: &str) -> Result<Value, ApiError> {
        self.send(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value, ApiError> {
        self.send(Method::POST, path, Some(body)).await
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ApiError> {
        let builder = Request::builder()
            .method(method)
            .uri(path)
            // The agent routes refuse anything not addressed to this
            // computer; in-process calls are, by definition.
            .header("host", "127.0.0.1");
        let request = match body {
            Some(body) => builder
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => builder.body(Body::empty()),
        }
        .map_err(|e| ApiError {
            status: 0,
            message: format!("Could not build the request for {path}: {e}"),
        })?;

        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .map_err(|e| ApiError {
                status: 0,
                message: format!("{path} failed: {e}"),
            })?;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
            .await
            .map_err(|e| ApiError {
                status: status.as_u16(),
                message: format!("Could not read the response of {path}: {e}"),
            })?;
        let value: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).map_err(|e| ApiError {
                status: status.as_u16(),
                message: format!("{path} did not answer with JSON: {e}"),
            })?
        };
        if status.is_success() {
            return Ok(value);
        }
        let message = value
            .get("error")
            .and_then(Value::as_str)
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "{path} failed with HTTP {}",
                    status.canonical_reason().unwrap_or("error")
                )
            });
        Err(ApiError {
            status: status.as_u16(),
            message,
        })
    }
}

impl ApiError {
    pub fn is_not_found(&self) -> bool {
        self.status == StatusCode::NOT_FOUND.as_u16()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use axum::Json;
    use serde_json::json;

    fn router() -> Router {
        Router::new()
            .route("/ok", get(|| async { Json(json!({"results": [1, 2]})) }))
            .route(
                "/echo",
                post(|Json(v): Json<Value>| async move { Json(json!({"got": v})) }),
            )
            .route(
                "/fail",
                get(|| async {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error": "No search term provided"})),
                    )
                }),
            )
    }

    #[tokio::test]
    async fn get_post_and_errors() {
        let api = Api::new(router());
        assert_eq!(api.get("/ok").await.unwrap()["results"][1], 2);
        assert_eq!(
            api.post("/echo", json!({"a": 1})).await.unwrap()["got"]["a"],
            1
        );
        let err = api.get("/fail").await.unwrap_err();
        assert_eq!(err.status, 400);
        assert_eq!(err.message, "No search term provided");
        assert!(api.get("/missing").await.unwrap_err().is_not_found());
    }
}
