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
    // Dev fallback (backend "dev" + plugin placeholder 9.9.9 accepted) only
    // applies when this binary was actually built without LRG_BACKEND_*
    // baked in (e.g. the release workflow sets those from the git tag before
    // running this same test suite, so 9.9.9 is correctly incompatible
    // there). Derive the expectation from the backend's own reported
    // version instead of assuming a dev build.
    let (app, _) = fresh_app();

    let version_response = app
        .clone()
        .oneshot(Request::get("/version").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let version_json = body_json(version_response).await;
    let backend_version = version_json["backend_version"].as_str().unwrap();
    let backend_release_tag = version_json["backend_release_tag"].as_str().unwrap();
    let is_dev_backend = backend_version.to_lowercase().contains("dev")
        || backend_release_tag.to_lowercase().contains("dev");
    let plugin_version = if is_dev_backend {
        "9.9.9".to_string()
    } else {
        backend_version.to_string()
    };

    let response = app
        .oneshot(
            Request::post("/version/check")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"plugin_version": "{plugin_version}"}}"#
                )))
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
            .body(Body::from(
                serde_json::json!({ "db_path": path }).to_string(),
            ))
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
                .body(Body::from(
                    serde_json::json!({
                        "plugin_version": "9.9.9",
                        "db_path": db_path_str,
                    })
                    .to_string(),
                ))
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
                .body(Body::from(
                    serde_json::json!({
                        "photo_ids": ["p1", "p2", "p3"],
                        "tasks": "embeddings",
                        "regenerate_metadata": "false",
                        "db_path": db_path_str,
                    })
                    .to_string(),
                ))
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

// ---------------------------------------------------------------------------
// /cull + /group_similar
//
// These had no contract coverage at all, which is how several of the fields
// below drifted: `debug` was always serialized even though the plugin never
// reads it, and the `warning` string was driven by whether SigLIP happened to
// be resident in RAM rather than by whether the photos had embeddings.
// ---------------------------------------------------------------------------

/// Seeds `count` photos one second apart with identical pHashes, i.e. an
/// obvious burst that grouping must collapse into a single group.
async fn seed_burst(state: &Arc<AppState>, count: usize, with_embedding: bool) {
    let store = state.store().unwrap();
    let records: Vec<lrg_store::StoreRecord> = (0..count)
        .map(|i| {
            let mut meta = serde_json::Map::new();
            meta.insert(
                "filename".into(),
                serde_json::json!(format!("burst{i}.jpg")),
            );
            meta.insert("capture_time".into(), serde_json::json!(1_700_000_000 + i));
            meta.insert("cull_phash".into(), serde_json::json!("ffffffffffffffff"));
            // Descending sharpness so the winner is deterministic: photo 0.
            meta.insert(
                "cull_sharpness".into(),
                serde_json::json!(0.9 - 0.1 * i as f64),
            );
            meta.insert("cull_exposure".into(), serde_json::json!(0.8));
            meta.insert("cull_noise".into(), serde_json::json!(0.1));
            lrg_store::StoreRecord {
                id: format!("burst{i}"),
                vector: with_embedding.then(|| vec![0.5f32; 1152]),
                metadata: meta,
            }
        })
        .collect();
    store
        .upsert(lrg_store::IMAGE_TABLE, &records)
        .await
        .unwrap();
}

async fn cull_request(app: axum::Router, body: serde_json::Value) -> serde_json::Value {
    let response = app
        .oneshot(
            Request::post("/cull")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

#[tokio::test]
async fn cull_returns_summary_and_group_shape_the_plugin_reads() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    seed_burst(&state, 3, true).await;

    let json = cull_request(
        app,
        serde_json::json!({
            "photo_ids": ["burst0", "burst1", "burst2"],
            "db_path": db_path_str,
        }),
    )
    .await;

    assert_eq!(json["status"], "success");
    // TaskCullPhotos.lua reads every one of these summary fields.
    let summary = &json["summary"];
    assert_eq!(summary["group_count"], 1);
    assert_eq!(summary["pick_count"], 1);
    assert_eq!(summary["culling_preset"], "default");
    assert_eq!(summary["unindexed_count"], 0);

    let group = &json["groups"][0];
    assert_eq!(group["group_size"], 3);
    assert_eq!(group["winner_photo_id"], "burst0", "sharpest must win");
    assert!(group["photo_ids"].as_array().unwrap().len() == 3);

    // Per-photo fields the plugin writes into catalog metadata.
    let winner = &group["photos"][0];
    assert_eq!(winner["winner"], true);
    assert_eq!(winner["rank"], 1);
    assert!(winner["cull_score"].is_number());
    assert!(winner["reason_codes"].is_array());
    assert!(winner["explanation"].is_string());
    assert!(winner["metrics"].is_object());
}

#[tokio::test]
async fn cull_omits_debug_block_unless_requested() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    seed_burst(&state, 3, true).await;

    let json = cull_request(
        app.clone(),
        serde_json::json!({"photo_ids": ["burst0", "burst1", "burst2"], "db_path": db_path_str}),
    )
    .await;
    assert!(
        json["groups"][0]["debug"].is_null(),
        "debug must be off by default: it is O(k^2) floats the plugin never reads"
    );

    let json = cull_request(
        app,
        serde_json::json!({
            "photo_ids": ["burst0", "burst1", "burst2"],
            "include_debug": true,
            "db_path": db_path_str,
        }),
    )
    .await;
    let debug = &json["groups"][0]["debug"];
    assert!(debug["thresholds"].is_object());
    assert!(debug["pairwise_distances"].is_array());
    assert_eq!(debug["culling_preset"], "default");
}

#[tokio::test]
async fn cull_warns_about_unindexed_photos_instead_of_dropping_them_silently() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    seed_burst(&state, 2, true).await;

    let json = cull_request(
        app,
        serde_json::json!({
            "photo_ids": ["burst0", "burst1", "never-indexed"],
            "db_path": db_path_str,
        }),
    )
    .await;

    assert_eq!(json["summary"]["unindexed_count"], 1);
    let warning = json["warning"].as_str().unwrap_or_default();
    assert!(
        warning.contains("not been analyzed"),
        "expected an unindexed-photo warning, got {warning:?}"
    );
}

#[tokio::test]
async fn cull_warning_tracks_stored_embeddings_not_model_residency() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    // Embeddings present, SigLIP definitely not resident in this test process.
    seed_burst(&state, 3, true).await;

    let json = cull_request(
        app,
        serde_json::json!({"photo_ids": ["burst0", "burst1", "burst2"], "db_path": db_path_str}),
    )
    .await;

    assert!(
        json["warning"].is_null(),
        "an idle-unloaded model must not be reported as broken grouping, got {:?}",
        json["warning"]
    );
}

#[tokio::test]
async fn cull_warns_when_no_embeddings_are_stored() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    seed_burst(&state, 3, false).await;

    let json = cull_request(
        app,
        serde_json::json!({"photo_ids": ["burst0", "burst1", "burst2"], "db_path": db_path_str}),
    )
    .await;

    let warning = json["warning"].as_str().unwrap_or_default();
    assert!(
        warning.contains("perceptual hashes"),
        "expected a pHash-only fallback warning, got {warning:?}"
    );
    // Grouping still works from pHash + capture time.
    assert_eq!(json["summary"]["group_count"], 1);
}

#[tokio::test]
async fn cull_rejects_unknown_preset_with_the_available_list() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::post("/cull")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"photo_ids": ["a"], "culling_preset": "nope"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    let presets = json["available_presets"].as_array().unwrap();
    assert!(presets.iter().any(|p| p == "portrait"), "got {presets:?}");
}

#[tokio::test]
async fn group_similar_requires_photo_ids() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::post("/group_similar")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
