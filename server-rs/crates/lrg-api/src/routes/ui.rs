//! Browser UI — the pages the plugin opens instead of building the same thing
//! out of Lightroom's view factory, plus the action bridge they talk back
//! through ([`crate::ui_bridge`]).
//!
//! Paths here are spelled as the domain only — `"/ui/people"` — and
//! `build_router` nests them under [`crate::API_PREFIX`], so the served path is
//! `/v1/ui/people`, like every other domain.
//!
//! `/v1/ui/people` replaces the old LrView People dialog. That dialog had to
//! decode every face thumbnail to a temp file (`f:picture` takes paths, not
//! bytes), lay the grid out at construction time, and ask the user to close
//! and reopen the window after re-clustering, because an LrView tree cannot
//! be rebuilt. A page re-renders.
//!
//! `/v1/ui/search` is the browser search page (`TaskSearchBrowser.lua`): the
//! same `/v1/search` the Advanced Search dialog calls, re-run live as the
//! options change, with each result's stored preview
//! (`GET /v1/photos/thumbnail`). Its task posts Lightroom's current view and
//! selection as the page's context (`/v1/ui/search/context`), and turns the
//! ranked results into a collection when the page queues
//! `show_search_results`.
//!
//! The page is served from the backend's own origin, so its `fetch` calls
//! reach `/v1/faces/*` with no CORS setup, and it carries the plugin's `db_path`
//! in the query string exactly like the plugin's own requests do — the
//! auto-bind middleware treats both the same.
//!
//! Exposure note: these endpoints are reachable by any page in the user's
//! browser, the same as every other route on this port. The bridge cannot do
//! more than the plugin's own menu items already do (build a collection,
//! change the Library selection) and it only does that while the user has
//! the task open in Lightroom, so it adds no reach the backend did not
//! already have.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

use crate::state::AppState;
use crate::ui_bridge::{DEFAULT_PAGE, PAGES};

/// Baked into the binary rather than shipped alongside it: the plugin
/// launches the installed server from a fixed path, and a page that can go
/// missing (or go stale next to a freshly updated binary) is a support
/// problem the file size does not justify.
const PEOPLE_HTML: &str = include_str!("../ui/people.html");
const SEARCH_HTML: &str = include_str!("../ui/search.html");

/// Actions the plugin knows how to run, per page. A page from an older build
/// asking for something this server has never heard of is rejected at the
/// door, rather than sitting in the queue until the plugin drops it silently.
const KNOWN_ACTIONS: &[(&str, &str)] = &[
    ("people", "show_in_library"),
    // The ranked result list becomes a collection under "Search Results".
    ("search", "show_search_results"),
    // Read Lightroom's current view and selection again (the page's scopes).
    ("search", "refresh_search_context"),
];

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/people", get(people_page))
        .route("/ui/search", get(search_page))
        .route(
            "/ui/search/context",
            get(take_search_context).post(put_search_context),
        )
        .route("/ui/status", get(status))
        .route("/ui/actions", get(take_actions).post(queue_action))
}

/// The page a request is about: `page` from the query string or the body,
/// `"people"` when absent. `Err` is the 400 for an unknown one.
fn page_of(name: Option<&str>) -> Result<&'static str, Box<Response>> {
    let name = name.map(str::trim).filter(|n| !n.is_empty());
    match name {
        None => Ok(DEFAULT_PAGE),
        Some(n) => PAGES.iter().copied().find(|p| *p == n).ok_or_else(|| {
            Box::new(
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": format!("unknown page \"{n}\"")})),
                )
                    .into_response(),
            )
        }),
    }
}

async fn people_page(State(state): State<Arc<AppState>>) -> Html<&'static str> {
    state.ui_bridge.mark_page_seen("people");
    Html(PEOPLE_HTML)
}

async fn search_page(State(state): State<Arc<AppState>>) -> Html<&'static str> {
    state.ui_bridge.mark_page_seen("search");
    Html(SEARCH_HTML)
}

/// Page heartbeat. The page uses `plugin_connected` to tell the user that
/// Lightroom stopped listening — otherwise "Show in Lightroom" would look
/// like it worked and nothing would happen.
async fn status(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let page = match page_of(query.get("page").map(String::as_str)) {
        Ok(page) => page,
        Err(response) => return *response,
    };
    state.ui_bridge.mark_page_seen(page);
    Json(json!({
        "status": "success",
        "plugin_connected": state.ui_bridge.plugin_connected(page),
    }))
    .into_response()
}

/// Plugin poll. Drains whatever the page queued; `page_open` lets the plugin
/// report a browser that never came up instead of waiting forever.
async fn take_actions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let page = match page_of(query.get("page").map(String::as_str)) {
        Ok(page) => page,
        Err(response) => return *response,
    };
    let actions = state.ui_bridge.drain(page);
    Json(json!({
        "status": "success",
        "actions": actions,
        "page_open": state.ui_bridge.page_open(page),
    }))
    .into_response()
}

/// Whether `key` of `body` is a non-empty array of strings.
fn non_empty_ids(body: &Value, key: &str) -> bool {
    body.get(key)
        .and_then(Value::as_array)
        .is_some_and(|ids| !ids.is_empty() && ids.iter().all(Value::is_string))
}

async fn queue_action(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let page = match page_of(body.get("page").and_then(Value::as_str)) {
        Ok(page) => page,
        Err(response) => return *response,
    };
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
    if !KNOWN_ACTIONS.contains(&(page, action.as_str())) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("unknown action \"{action}\" for the {page} page")})),
        )
            .into_response();
    }
    let missing = match action.as_str() {
        "show_in_library" if !non_empty_ids(&body, "person_ids") => Some("person_ids"),
        "show_search_results" if !non_empty_ids(&body, "photo_ids") => Some("photo_ids"),
        _ => None,
    };
    if let Some(field) = missing {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("{action} needs a non-empty {field} array")})),
        )
            .into_response();
    }

    let plugin_connected = state.ui_bridge.plugin_connected(page);
    let queued = state.ui_bridge.enqueue(page, body);
    log::info!(
        "UI bridge queued \"{action}\" for {page} ({queued} pending, plugin_connected={plugin_connected})"
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

/// The plugin's half: what the search page cannot read for itself — the
/// catalog, Lightroom's current view and selection as photo IDs, the last
/// search options. Stored as sent (`ui_bridge::UiBridge::set_context`); the
/// page reads it back with GET.
async fn put_search_context(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Response {
    if !body.is_object() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "the search context must be a JSON object"})),
        )
            .into_response();
    }
    let revision = state.ui_bridge.set_context("search", body);
    Json(json!({"status": "success", "revision": revision})).into_response()
}

/// The page's half: the context the plugin stored, `null` before it has
/// stored one, and its revision (which a refresh raises).
async fn take_search_context(State(state): State<Arc<AppState>>) -> Json<Value> {
    state.ui_bridge.mark_page_seen("search");
    let (context, revision) = state.ui_bridge.context("search");
    Json(json!({
        "status": "success",
        "context": context,
        "revision": revision,
        "plugin_connected": state.ui_bridge.plugin_connected("search"),
    }))
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
    async fn search_page_is_served_as_html() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/ui/search")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        // The paths the page calls are the ones this server answers.
        for path in [
            "/v1/search",
            "/v1/search/genres",
            "/v1/photos/thumbnail",
            "/v1/ui/search/context",
            "/v1/ui/actions",
            "/v1/ui/status",
        ] {
            assert!(html.contains(path), "the page does not call {path}");
        }
    }

    #[tokio::test]
    async fn search_actions_go_to_the_search_queue_only() {
        let app = app();
        let queued = app
            .clone()
            .oneshot(post_action(json!({
                "page": "search",
                "action": "show_search_results",
                "photo_ids": ["p1", "p2"],
            })))
            .await
            .unwrap();
        assert_eq!(queued.status(), StatusCode::OK);

        // A People task polling does not take it.
        let people = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ui/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(body_json(people).await["actions"]
            .as_array()
            .unwrap()
            .is_empty());

        let search = app
            .oneshot(
                Request::builder()
                    .uri("/ui/actions?page=search")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let search = body_json(search).await;
        assert_eq!(search["actions"][0]["photo_ids"], json!(["p1", "p2"]));
        assert_eq!(search["page_open"], true);
    }

    #[tokio::test]
    async fn malformed_search_actions_and_unknown_pages_are_rejected() {
        let app = app();
        for body in [
            json!({"page": "search", "action": "show_search_results", "photo_ids": []}),
            json!({"page": "search", "action": "show_search_results", "photo_ids": [1]}),
            json!({"page": "search", "action": "show_in_library", "person_ids": ["x"]}),
            json!({"page": "people", "action": "show_search_results", "photo_ids": ["p"]}),
            json!({"page": "nowhere", "action": "show_in_library", "person_ids": ["x"]}),
        ] {
            let response = app
                .clone()
                .oneshot(post_action(body.clone()))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        }
        let refresh = app
            .clone()
            .oneshot(post_action(
                json!({"page": "search", "action": "refresh_search_context"}),
            ))
            .await
            .unwrap();
        assert_eq!(refresh.status(), StatusCode::OK);
        let unknown_page = app
            .oneshot(
                Request::builder()
                    .uri("/ui/status?page=nowhere")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown_page.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn the_search_context_goes_from_the_plugin_to_the_page() {
        let app = app();
        let empty = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ui/search/context")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let empty = body_json(empty).await;
        assert!(empty["context"].is_null());
        assert_eq!(empty["revision"], 0);

        let context = json!({
            "catalog_id": "cat-1",
            "scopes": {"selection": {"photo_ids": ["p1"]}},
        });
        let stored = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ui/search/context")
                    .header("content-type", "application/json")
                    .body(Body::from(context.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(stored).await["revision"], 1);

        let read = app
            .oneshot(
                Request::builder()
                    .uri("/ui/search/context")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let read = body_json(read).await;
        assert_eq!(read["context"], context);
        assert_eq!(read["revision"], 1);
        assert_eq!(read["plugin_connected"], true);
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
