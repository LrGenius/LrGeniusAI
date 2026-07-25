//! API contract tests for the M1 surface, asserting the exact response
//! shapes the Lightroom plugin (APISearchIndex.lua) depends on.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

use lrg_api::state::AppState;

fn app(state: Arc<AppState>) -> axum::Router {
    lrg_api::build_router(state)
}

fn fresh_app() -> (axum::Router, Arc<AppState>) {
    let state = Arc::new(AppState::new(None, false));
    (app(state.clone()), state)
}

async fn body_string(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_str(&body_string(response).await).unwrap()
}

#[tokio::test]
async fn ping_returns_plain_text_pong() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(Request::get("/ping").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        content_type.starts_with("text/plain"),
        "plugin expects a plain text pong, got {content_type}"
    );
    assert_eq!(body_string(response).await, "pong");
}

#[tokio::test]
async fn version_returns_backend_info() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(Request::get("/version").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert!(json["backend_version"].is_string());
    assert!(json["backend_release_tag"].is_string());
    assert!(json["backend_build"].is_number());
}

#[tokio::test]
async fn version_check_dev_fallback() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::post("/version/check")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"plugin_version": "9.9.9"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["compatible"], serde_json::json!(true));
}

#[tokio::test]
async fn initialize_requires_db_path() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::post("/initialize")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"], serde_json::json!("db_path is required"));
}

#[tokio::test]
async fn initialize_writes_handshake_files_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db");
    let db_path_str = db_path.to_str().unwrap();

    let (app, state) = fresh_app();
    let request = |path: &str| {
        Request::post("/initialize")
            .header("content-type", "application/json")
            .body(Body::from(format!(r#"{{"db_path": "{path}"}}"#)))
            .unwrap()
    };

    let response = app.clone().oneshot(request(db_path_str)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["status"], serde_json::json!("success"));
    assert_eq!(json["db_path"], serde_json::json!(db_path_str));

    // Handshake files land in the catalog dir (parent of db_path).
    let pid = std::fs::read_to_string(dir.path().join("lrgenius-server.pid")).unwrap();
    assert_eq!(pid, std::process::id().to_string());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("lrgenius-server.OK")).unwrap(),
        "OK\n"
    );
    assert_eq!(state.db_path(), Some(db_path.clone()));

    // Second call with the same path: already_initialized.
    let response = app.oneshot(request(db_path_str)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["status"], serde_json::json!("already_initialized"));
}

#[tokio::test]
async fn middleware_auto_binds_db_path_from_json_body() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db");
    let db_path_str = db_path.to_str().unwrap();

    let (app, state) = fresh_app();
    // Any non-bypass endpoint carrying db_path should transparently bind.
    let response = app
        .oneshot(
            Request::post("/version/check")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"plugin_version": "9.9.9", "db_path": "{db_path_str}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.db_path(), Some(db_path));
}

#[tokio::test]
async fn middleware_auto_binds_db_path_from_query_string() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db");

    let (app, state) = fresh_app();
    let uri = format!(
        "/health?db_path={}",
        db_path.to_str().unwrap().replace('/', "%2F")
    );
    let response = app
        .oneshot(Request::get(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.db_path(), Some(db_path));
}

#[tokio::test]
async fn shutdown_and_restart_respond_before_exiting() {
    let (app, _) = fresh_app();
    let response = app
        .clone()
        .oneshot(Request::post("/shutdown").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(
        json["status"],
        serde_json::json!("Server is shutting down...")
    );

    let response = app
        .oneshot(Request::post("/restart").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let json = body_json(response).await;
    assert_eq!(json["status"], serde_json::json!("Restarting..."));
}

#[tokio::test]
async fn health_reports_model_states() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["clip_model"], serde_json::json!("not_loaded"));
    assert!(json["clip_error"].is_null());
}

#[tokio::test]
async fn unknown_raw_log_type_returns_404() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::get("/logs/raw/nonsense")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn check_unprocessed_reports_only_missing_work() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db");
    let db_path_str = db_path.to_str().unwrap().to_string();

    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    let store = state.store().unwrap();

    // p1: fully processed (embedding + cull_phash). p2: metadata-only.
    let mut done = serde_json::Map::new();
    done.insert("has_embedding".into(), serde_json::json!(true));
    done.insert("cull_phash".into(), serde_json::json!("abcd0123abcd0123"));
    let mut pending = serde_json::Map::new();
    pending.insert("has_embedding".into(), serde_json::json!(false));
    store
        .upsert(
            lrg_store::IMAGE_TABLE,
            &[
                lrg_store::StoreRecord {
                    id: "p1".into(),
                    vector: Some(vec![0.1; 1152]),
                    metadata: done,
                },
                lrg_store::StoreRecord {
                    id: "p2".into(),
                    vector: None,
                    metadata: pending,
                },
            ],
        )
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::post("/index/check-unprocessed")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"photo_ids": ["p1", "p2", "p3"], "tasks": "embeddings",
                        "regenerate_metadata": "false", "db_path": "{db_path_str}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    // p1 is complete; p2 lacks an embedding; p3 is unknown.
    assert_eq!(json["photo_ids"], serde_json::json!(["p2", "p3"]));
    assert_eq!(json["uuids"], json["photo_ids"]);
}
