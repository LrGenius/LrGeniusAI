//! Browser UI — the pages the plugin opens instead of building the same thing
//! out of Lightroom's view factory, plus the action bridge they talk back
//! through ([`crate::ui_bridge`]).
//!
//! `/ui/people` replaces the old LrView People dialog. That dialog had to
//! decode every face thumbnail to a temp file (`f:picture` takes paths, not
//! bytes), lay the grid out at construction time, and ask the user to close
//! and reopen the window after re-clustering, because an LrView tree cannot
//! be rebuilt. A page re-renders.
//!
//! The page is served from the backend's own origin, so its `fetch` calls
//! reach `/faces/*` with no CORS setup, and it carries the plugin's `db_path`
//! in the query string exactly like the plugin's own requests do — the
//! auto-bind middleware treats both the same.
//!
//! Exposure note: these endpoints are reachable by any page in the user's
//! browser, the same as every other route on this port. The bridge cannot do
//! more than the plugin's own menu items already do (build a collection,
//! change the Library selection) and it only does that while the user has
//! the task open in Lightroom, so it adds no reach the backend did not
//! already have.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

use crate::state::AppState;

/// Baked into the binary rather than shipped alongside it: the plugin
/// launches the installed server from a fixed path, and a page that can go
/// missing (or go stale next to a freshly updated binary) is a support
/// problem the file size does not justify.
const PEOPLE_HTML: &str = include_str!("../ui/people.html");

/// Actions the plugin knows how to run. A page from an older build asking
/// for something this server has never heard of is rejected at the door,
/// rather than sitting in the queue until the plugin drops it silently.
const KNOWN_ACTIONS: &[&str] = &["show_in_library"];

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/people", get(people_page))
        .route("/ui/status", get(status))
        .route("/ui/actions", get(take_actions).post(queue_action))
}

async fn people_page(State(state): State<Arc<AppState>>) -> Html<&'static str> {
    state.ui_bridge.mark_page_seen();
    Html(PEOPLE_HTML)
}

/// Page heartbeat. The page uses `plugin_connected` to tell the user that
/// Lightroom stopped listening — otherwise "Show in Lightroom" would look
/// like it worked and nothing would happen.
async fn status(State(state): State<Arc<AppState>>) -> Json<Value> {
    state.ui_bridge.mark_page_seen();
    Json(json!({
        "status": "success",
        "plugin_connected": state.ui_bridge.plugin_connected(),
    }))
}

/// Plugin poll. Drains whatever the page queued; `page_open` lets the plugin
/// report a browser that never came up instead of waiting forever.
async fn take_actions(State(state): State<Arc<AppState>>) -> Json<Value> {
    let actions = state.ui_bridge.drain();
    Json(json!({
        "status": "success",
        "actions": actions,
        "page_open": state.ui_bridge.page_open(),
    }))
}

async fn queue_action(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    // Owned, because the body itself is handed to the queue below.
    let Some(action) = body
        .get("action")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "action is required"})),
        )
            .into_response();
    };
    if !KNOWN_ACTIONS.contains(&action.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("unknown action \"{action}\"")})),
        )
            .into_response();
    }
    if action == "show_in_library"
        && body
            .get("person_ids")
            .and_then(Value::as_array)
            .is_none_or(|ids| ids.is_empty())
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "show_in_library needs a non-empty person_ids array"})),
        )
            .into_response();
    }

    let plugin_connected = state.ui_bridge.plugin_connected();
    let queued = state.ui_bridge.enqueue(body);
    log::info!(
        "UI bridge queued \"{action}\" ({queued} pending, plugin_connected={plugin_connected})"
    );

    Json(json!({
        "status": "success",
        "queued": queued,
        // Reported from *before* the enqueue on purpose: it answers "will
        // anything pick this up", which the enqueue itself does not change.
        "plugin_connected": plugin_connected,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;

    fn app() -> Router {
        let state = Arc::new(AppState::new(None, true));
        router().with_state(state)
    }

    async fn body_json(response: Response) -> Value {
        let bytes = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn post_action(body: Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/ui/actions")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn people_page_is_served_as_html() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/ui/people")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("text/html; charset=utf-8")
        );
    }

    #[tokio::test]
    async fn queued_action_comes_back_to_the_plugin_once() {
        let app = app();
        let queued = app
            .clone()
            .oneshot(post_action(
                json!({"action": "show_in_library", "person_ids": ["person_0"]}),
            ))
            .await
            .unwrap();
        assert_eq!(queued.status(), StatusCode::OK);

        let drained = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ui/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let drained = body_json(drained).await;
        assert_eq!(drained["actions"].as_array().unwrap().len(), 1);
        assert_eq!(drained["actions"][0]["person_ids"][0], "person_0");
        assert_eq!(drained["page_open"], true);

        let again = app
            .oneshot(
                Request::builder()
                    .uri("/ui/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(body_json(again).await["actions"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn unknown_and_malformed_actions_are_rejected() {
        let app = app();
        for body in [
            json!({"person_ids": ["person_0"]}),
            json!({"action": "rm_-rf", "person_ids": ["person_0"]}),
            json!({"action": "show_in_library", "person_ids": []}),
        ] {
            let response = app.clone().oneshot(post_action(body)).await.unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }

        let drained = app
            .oneshot(
                Request::builder()
                    .uri("/ui/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(body_json(drained).await["actions"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn status_reports_the_plugin_once_it_has_polled() {
        let app = app();
        let before = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ui/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(before).await["plugin_connected"], false);

        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/ui/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let after = app
            .oneshot(
                Request::builder()
                    .uri("/ui/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(after).await["plugin_connected"], true);
    }
}
