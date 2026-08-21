//! `db_path` auto-bind middleware — port of the Flask `before_request`
//! hook in `geniusai_server.py`. If a request carries `db_path` (JSON body
//! or query string) and the backend isn't bound to it yet (e.g. after an
//! unexpected restart), bind transparently before the route runs.

use std::sync::Arc;

use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{header::CONTENT_TYPE, Method},
    middleware::Next,
    response::Response,
};

use crate::state::AppState;

/// Endpoints that don't need (and shouldn't pay the cost of) binding:
/// liveness checks and the explicit bind route, which handles db_path itself.
///
/// Matched against the full request path, so these carry the `/v1` prefix that
/// `build_router` nests them under. `/ping` is part of the unversioned
/// bootstrap contract and stays at the root.
const BYPASS_PATHS: &[&str] = &["/ping", "/v1/db/bind"];

/// JSON bodies are buffered up to this limit for the db_path peek. Larger
/// bodies (and all non-JSON bodies, e.g. multipart uploads) are passed
/// through untouched and rely on query-string db_path instead.
const MAX_PEEK_BODY_BYTES: usize = 64 * 1024 * 1024;

pub async fn auto_bind_db_path(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if BYPASS_PATHS.contains(&request.uri().path()) {
        return next.run(request).await;
    }

    let mut db_path = None;
    let mut request = request;

    let is_json_body = matches!(
        request.method(),
        &Method::POST | &Method::PUT | &Method::PATCH
    ) && request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"));

    if is_json_body {
        let (parts, body) = request.into_parts();
        match axum::body::to_bytes(body, MAX_PEEK_BODY_BYTES).await {
            Ok(bytes) => {
                db_path = extract_db_path_from_json(&bytes);
                request = Request::from_parts(parts, Body::from(bytes));
            }
            Err(_) => {
                // Body over the peek limit or unreadable: cannot restore it,
                // so this arm must not consume it in the first place. to_bytes
                // only fails after consuming, so surface a clear error.
                return Response::builder()
                    .status(413)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"error": "request body too large"}"#))
                    .unwrap();
            }
        }
    }

    if db_path.is_none() {
        db_path = query_param(request.uri().query(), "db_path");
    }

    if let Some(path) = db_path {
        // Don't fail the request here — let the route's own error handling
        // report underlying failures with more context (Python parity).
        if let Err(e) = state.ensure_db_path(&path).await {
            log::error!("Auto-bind to db_path {path} failed: {e}");
        }
    }

    next.run(request).await
}

fn extract_db_path_from_json(bytes: &Bytes) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    value
        .as_object()?
        .get("db_path")?
        .as_str()
        .map(str::to_string)
}

fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        if it.next() == Some(key) {
            let raw = it.next().unwrap_or("");
            return percent_decode(raw);
        }
    }
    None
}

fn percent_decode(raw: &str) -> Option<String> {
    let raw = raw.replace('+', " ");
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            let byte = u8::from_str_radix(hex, 16).ok()?;
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_param_extracts_and_decodes() {
        assert_eq!(
            query_param(
                Some("db_path=%2FUsers%2Fbm%2Fcat%20dir%2Flrgenius.db&x=1"),
                "db_path"
            ),
            Some("/Users/bm/cat dir/lrgenius.db".to_string())
        );
        assert_eq!(query_param(Some("a=1&b=2"), "db_path"), None);
        assert_eq!(query_param(None, "db_path"), None);
    }

    #[test]
    fn json_peek_reads_db_path_only_from_objects() {
        let bytes = Bytes::from(r#"{"db_path": "/x/lrgenius.db", "other": 1}"#);
        assert_eq!(
            extract_db_path_from_json(&bytes),
            Some("/x/lrgenius.db".to_string())
        );
        assert_eq!(extract_db_path_from_json(&Bytes::from("[1,2]")), None);
        assert_eq!(extract_db_path_from_json(&Bytes::from("not json")), None);
    }
}
