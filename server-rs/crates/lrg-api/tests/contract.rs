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
            Request::post("/v1/db/bind")
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
        Request::post("/v1/db/bind")
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
        "/v1/server/health?db_path={}",
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
        .oneshot(
            Request::post("/v1/server/restart")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let json = body_json(response).await;
    assert_eq!(json["status"], serde_json::json!("Restarting..."));
}

#[tokio::test]
async fn health_reports_model_states() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::get("/v1/server/health")
                .body(Body::empty())
                .unwrap(),
        )
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
            Request::get("/v1/server/logs/nonsense/raw")
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
            Request::post("/v1/index/unprocessed")
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
// /v1/cull/grade + /v1/cull/groups
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
            Request::post("/v1/cull/grade")
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
            Request::post("/v1/cull/grade")
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
            Request::post("/v1/cull/groups")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// The `cull` task is the fast ingest path. Its delta check cannot key off
/// FACE_TABLE rows (it writes none) and cannot treat `cull_faces_present:
/// false` as incomplete (that is the correct value for a photo with no faces).
#[tokio::test]
async fn check_unprocessed_understands_the_cull_task() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    let store = state.store().unwrap();

    let row = |phash: Option<&str>, faces_present: Option<bool>| {
        let mut m = serde_json::Map::new();
        if let Some(p) = phash {
            m.insert("cull_phash".into(), serde_json::json!(p));
        }
        if let Some(f) = faces_present {
            m.insert("cull_faces_present".into(), serde_json::json!(f));
        }
        m
    };

    store
        .upsert(
            lrg_store::IMAGE_TABLE,
            &[
                // Fully culled, and genuinely has no faces.
                lrg_store::StoreRecord {
                    id: "done_faceless".into(),
                    vector: None,
                    metadata: row(Some("abcd0123abcd0123"), Some(false)),
                },
                // Fully culled, has faces.
                lrg_store::StoreRecord {
                    id: "done_with_faces".into(),
                    vector: None,
                    metadata: row(Some("abcd0123abcd0124"), Some(true)),
                },
                // Has a pHash from an older index pass but never a cull face pass.
                lrg_store::StoreRecord {
                    id: "needs_face_pass".into(),
                    vector: None,
                    metadata: row(Some("abcd0123abcd0125"), None),
                },
                // Indexed for embeddings only, no cull data at all.
                lrg_store::StoreRecord {
                    id: "needs_everything".into(),
                    vector: Some(vec![0.1; 1152]),
                    metadata: row(None, None),
                },
            ],
        )
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::post("/v1/index/unprocessed")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "photo_ids": [
                            "done_faceless", "done_with_faces",
                            "needs_face_pass", "needs_everything", "unknown",
                        ],
                        "tasks": "cull",
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
    assert_eq!(
        json["photo_ids"],
        serde_json::json!(["needs_face_pass", "needs_everything", "unknown"]),
        "a faceless photo that has been culled must not be reported forever"
    );
}

/// Swapping the face detector/recognizer pair invalidates stored embeddings:
/// FaceNet and the retired ArcFace both emit 512-d unit vectors, so nothing
/// about a row's shape says which model wrote it. `check-unprocessed` must
/// therefore treat "checked by an older model" as "not checked", or those
/// photos would never be re-derived.
#[tokio::test]
async fn check_unprocessed_requeues_faces_from_a_superseded_model() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    let store = state.store().unwrap();

    let image_row = |model: Option<&str>| {
        let mut m = serde_json::Map::new();
        m.insert("faces_checked".into(), serde_json::json!(true));
        // `needs_cull_phash` is checked for *any* task, so without this every
        // row here would be reported outstanding regardless of its face state
        // and the assertion below would pass without testing anything.
        m.insert("cull_phash".into(), serde_json::json!("abcd0123abcd0123"));
        if let Some(model) = model {
            m.insert("face_model".into(), serde_json::json!(model));
        }
        m
    };
    let face_row = |id: &str, model: Option<&str>| {
        let mut m = serde_json::Map::new();
        m.insert("person_id".into(), serde_json::json!("person_0"));
        if let Some(model) = model {
            m.insert("face_model".into(), serde_json::json!(model));
        }
        lrg_store::StoreRecord {
            id: id.into(),
            vector: Some(vec![0.1; 512]),
            metadata: m,
        }
    };

    store
        .upsert(
            lrg_store::IMAGE_TABLE,
            &[
                lrg_store::StoreRecord {
                    id: "current".into(),
                    vector: None,
                    metadata: image_row(Some(lrg_ml::faces::MODEL_ID)),
                },
                lrg_store::StoreRecord {
                    id: "old_model".into(),
                    vector: None,
                    metadata: image_row(Some("buffalo_l/arcface-w600k-r50")),
                },
                // Written before `face_model` existed at all.
                lrg_store::StoreRecord {
                    id: "unmarked".into(),
                    vector: None,
                    metadata: image_row(None),
                },
            ],
        )
        .await
        .unwrap();
    // Stored faces are the other way `faces_checked` is satisfied, so each
    // photo gets one — otherwise this would pass for the wrong reason.
    store
        .upsert(
            lrg_store::FACE_TABLE,
            &[
                face_row("current_0", Some(lrg_ml::faces::MODEL_ID)),
                face_row("old_model_0", Some("buffalo_l/arcface-w600k-r50")),
                face_row("unmarked_0", None),
            ],
        )
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::post("/v1/index/unprocessed")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "photo_ids": ["current", "old_model", "unmarked"],
                        "tasks": "faces",
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
    assert_eq!(
        json["photo_ids"],
        serde_json::json!(["old_model", "unmarked"]),
        "photos whose faces came from another model must be re-detected, \
         and one already on the current model must not be"
    );
}

/// Clustering must not mix embedding spaces. Rows from a superseded model are
/// set aside as unassigned rather than clustered against current ones — the
/// distances between the two spaces are numbers, not measurements.
#[tokio::test]
async fn cluster_leaves_faces_from_a_superseded_model_unassigned() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    let store = state.store().unwrap();

    let face = |id: &str, model: Option<&str>, seed: f32| {
        let mut m = serde_json::Map::new();
        if let Some(model) = model {
            m.insert("face_model".into(), serde_json::json!(model));
        }
        let mut v = vec![0.0f32; 512];
        v[0] = seed;
        v[1] = 1.0 - seed;
        lrg_store::StoreRecord {
            id: id.into(),
            vector: Some(v),
            metadata: m,
        }
    };

    store
        .upsert(
            lrg_store::FACE_TABLE,
            &[
                face("p1_0", Some(lrg_ml::faces::MODEL_ID), 1.0),
                face("p1_1", Some(lrg_ml::faces::MODEL_ID), 1.0),
                face("p2_0", Some("buffalo_l/arcface-w600k-r50"), 1.0),
                face("p3_0", None, 1.0),
            ],
        )
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::post("/v1/faces/cluster")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "min_faces_per_person": null,
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
    // The two stale rows are identical to the current ones, so anything that
    // clustered them would fold all four into one person.
    assert_eq!(json["unassigned"], 2, "got {json}");
    assert_eq!(json["face_count"], 4, "got {json}");

    let rows = store.scan_all(lrg_store::FACE_TABLE).await.unwrap();
    for row in rows {
        let person = row.metadata["person_id"].as_str().unwrap();
        let stale = row.metadata.get("face_model").and_then(|v| v.as_str())
            != Some(lrg_ml::faces::MODEL_ID);
        assert_eq!(
            stale,
            person == "person_unassigned",
            "{}: person_id {person} does not match its face_model",
            row.id
        );
    }
}

// ---------------------------------------------------------------------------
// /v1/edit/style
//
// The plugin's edit workflow routes here whenever "apply my saved edit style" is
// on, which is the default, so this is the first endpoint a new user's edit run
// touches. It had no contract coverage, and the plugin branches on the exact
// shapes below.
// ---------------------------------------------------------------------------

/// Builds a `multipart/form-data` body. `/v1/edit/style` and `/v1/edit/recipe` are the only
/// multipart endpoints the plugin calls, and both are driven from
/// `_requestMultipart` in APISearchIndex.lua.
fn multipart_body(fields: &[(&str, &str)], image: Option<(&str, &[u8])>) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "----lrgtestboundary";
    let mut body: Vec<u8> = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    if let Some((filename, bytes)) = image {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"image\"; filename=\"{filename}\"\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(b"Content-Type: image/jpeg\r\n\r\n");
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={BOUNDARY}"), body)
}

async fn style_edit_request(
    app: axum::Router,
    fields: &[(&str, &str)],
    image: Option<(&str, &[u8])>,
) -> (StatusCode, serde_json::Value) {
    let (content_type, body) = multipart_body(fields, image);
    let response = app
        .oneshot(
            Request::post("/v1/edit/style")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

#[tokio::test]
async fn style_edit_rejects_a_request_without_an_image() {
    let (app, _) = fresh_app();
    let (_, json) = style_edit_request(app.clone(), &[("photo_id", "photo1")], None).await;

    // The plugin checks `response.status` first; this branch carries only `error`,
    // so styleEdit reports "Unexpected response" rather than the actual reason.
    assert!(
        json.get("error").is_some(),
        "expected an error field, got {json}"
    );
}

#[tokio::test]
async fn style_edit_reports_an_unbound_database_rather_than_panicking() {
    let (app, _) = fresh_app();
    let (status, json) = style_edit_request(
        app.clone(),
        &[("photo_id", "photo1")],
        Some(("photo1.jpg", b"not-a-real-jpeg")),
    )
    .await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("db_path"),
        "expected the unbound-database message, got {json}"
    );
}

#[tokio::test]
async fn style_edit_without_training_data_is_an_error_the_plugin_must_handle() {
    // With no training examples and no LLM fallback the style engine cannot
    // answer, and this is what a first run hits. Pinned because the edit workflow
    // is moving to an LLM-free path: this 422 is exactly the response the
    // deterministic cold-start path has to replace, and a silent change of shape
    // here would strand the plugin.
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();

    let (status, json) = style_edit_request(
        app.clone(),
        &[
            ("photo_id", "photo1"),
            ("db_path", db_path_str.as_str()),
            ("use_llm_fallback", "false"),
        ],
        Some(("photo1.jpg", b"not-a-real-jpeg")),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json["status"], "error");
    assert_eq!(json["engine"], "none");
    assert_eq!(json["matched_examples"], 0);
    assert!(json["error"].is_string());
}

// ---------------------------------------------------------------------------
// Path layout: the `/v1` prefix and the unversioned bootstrap contract
// ---------------------------------------------------------------------------

/// The five paths a *new* plug-in must be able to reach on an *old* backend to
/// pull the install forward. `build_router` merges them at the root instead of
/// nesting them, and versioning any of them would mean a half-updated machine
/// could not repair itself. `/update/apply` is covered by its own tests; the
/// four lifecycle paths are asserted here.
#[tokio::test]
async fn bootstrap_contract_is_served_unversioned() {
    for (method, path) in [
        ("GET", "/ping"),
        ("GET", "/version"),
        ("POST", "/version/check"),
        ("POST", "/update/apply"),
    ] {
        let (app, _) = fresh_app();
        let request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let status = app.oneshot(request).await.unwrap().status();
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "{method} {path} must stay at the root — a plug-in that updated \
             ahead of its backend reaches the old binary through these"
        );
    }
}

/// ...and the corollary: they are *only* at the root. A `/v1` alias would
/// invite new code to depend on it, quietly re-coupling the two halves.
#[tokio::test]
async fn bootstrap_contract_is_not_also_versioned() {
    for path in [
        "/v1/ping",
        "/v1/version",
        "/v1/shutdown",
        "/v1/update/apply",
    ] {
        let (app, _) = fresh_app();
        let response = app
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{path} should not exist; the bootstrap contract lives at the root only"
        );
    }
}

/// Everything else moved under `/v1`, with no aliases left behind. This is a
/// hard cutover, so the old spellings must be gone rather than quietly working
/// and letting a caller keep using them.
#[tokio::test]
async fn pre_v1_paths_are_gone() {
    for (method, path) in [
        ("POST", "/index"),
        ("POST", "/index_by_reference"),
        ("POST", "/get"),
        ("GET", "/get/ids"),
        ("POST", "/remove"),
        ("POST", "/find_similar"),
        ("POST", "/group_similar"),
        ("POST", "/cull"),
        ("POST", "/edit"),
        ("POST", "/edit_base64"),
        ("POST", "/style_edit"),
        ("POST", "/initialize"),
        ("GET", "/health"),
        ("GET", "/logs"),
        ("GET", "/models"),
        ("GET", "/clip/status"),
        ("GET", "/assets/status"),
        ("GET", "/llm/catalog"),
        ("GET", "/db/stats"),
        ("POST", "/training/add"),
        ("POST", "/keywords/cluster"),
    ] {
        let (app, _) = fresh_app();
        let request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let status = app.oneshot(request).await.unwrap().status();
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{method} {path} is a pre-/v1 path and must no longer be served"
        );
    }
}

/// The db_path auto-bind middleware matches on the *full* request path, so the
/// bypass list had to learn the `/v1` prefix when `/initialize` became
/// `/v1/db/bind`. A stale entry there would silently re-enable binding on the
/// one route that handles db_path itself.
#[tokio::test]
async fn db_bind_is_reachable_under_v1() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db");
    let (app, state) = fresh_app();

    let body = serde_json::json!({ "db_path": db_path.to_str().unwrap() });
    let response = app
        .oneshot(
            Request::post("/v1/db/bind")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.db_path(), Some(db_path));
}

/// `photo_id` on the training routes is a file path, so it arrives
/// percent-encoded from `SearchIndexAPI.url` and has to survive the wildcard
/// capture intact. The plug-in used to concatenate it raw, which broke on any
/// id containing `?`, `#` or a space; encoding it only helps if axum decodes
/// the segment back, so assert the round-trip rather than assume it.
#[tokio::test]
async fn training_delete_decodes_a_path_shaped_photo_id() {
    let raw = "/Users/bm/My Photos/a b?c#d.jpg";
    let encoded = raw
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'~' | b'-' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect::<String>();

    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::delete(format!("/v1/edit/training/{encoded}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Unbound store: the handler 404s and echoes the id it actually received.
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_string(response).await;
    assert!(
        body.contains(raw),
        "wildcard capture should decode back to {raw:?}, got {body}"
    );
}

/// Job polling is generic and lives at one path, rather than each async
/// operation growing its own status route. The keyword-clustering enqueue must
/// therefore hand back where to poll, not just an opaque id.
#[tokio::test]
async fn unknown_job_is_a_404_with_a_reason() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::get("/v1/jobs/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert!(json["status"].is_null());
    // A collected-or-expired job is indistinguishable from a bad id here, so
    // the message has to say that rather than assert the id was wrong.
    let error = json["error"].as_str().unwrap();
    assert!(
        error.contains("collected") || error.contains("expired"),
        "404 should explain the one-shot/TTL semantics, got {error:?}"
    );
}

/// The old keyword-shaped poll route is gone; nothing should still answer on it.
#[tokio::test]
async fn keyword_specific_job_poll_route_is_gone() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::get("/v1/keywords/clusters/jobs/abc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// The People page has to be reachable through the *assembled* router, not
/// just through `routes::ui::router()` in isolation.
///
/// This is a regression test with a scar behind it: merging the `/v1` grouping
/// into the branch that added `/ui/` dropped the `.merge(routes::ui::router())`
/// line, and every unit test still passed because they mount that router
/// directly. The People menu item opened a 404 and nothing said so.
#[tokio::test]
async fn people_page_is_reachable_through_the_assembled_router() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(Request::get("/v1/ui/people").body(Body::empty()).unwrap())
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
    let page = body_string(response).await;
    // The page's own fetches must name the nested paths, or it renders and
    // then fails every request it makes.
    assert!(
        page.contains("/v1/faces/persons") && page.contains("/v1/ui/actions"),
        "the People page still points at unprefixed API paths"
    );
}

/// The browser→Lightroom queue, end to end through the assembled router: what
/// the page posts is what the plugin's poll hands back, exactly once.
#[tokio::test]
async fn ui_action_survives_the_round_trip_through_the_assembled_router() {
    let (app, _) = fresh_app();
    let queued = app
        .clone()
        .oneshot(
            Request::post("/v1/ui/actions")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "action": "show_in_library",
                        "person_ids": ["person_0"],
                        "match_mode": "intersection"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(queued.status(), StatusCode::OK);

    let drained = body_json(
        app.oneshot(Request::get("/v1/ui/actions").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(drained["actions"][0]["person_ids"][0], "person_0");
    assert_eq!(drained["actions"][0]["match_mode"], "intersection");
}

// ---------------------------------------------------------------------------
// The People page's editing surface: looking inside a person, moving faces
// between people, and merging two people into one.
//
// These go through the real store rather than a mock, because the thing that
// matters about all three is what `FACE_TABLE` looks like afterwards — and in
// particular that a decision the user made survives the next clustering run.

/// A face row: an embedding pointing `angle` radians into the first two
/// dimensions, plus quality metrics good enough to pass the clustering gate.
fn people_face(id: &str, photo: &str, person: &str, angle: f64) -> lrg_store::StoreRecord {
    let mut m = serde_json::Map::new();
    m.insert("photo_id".into(), serde_json::json!(photo));
    m.insert("person_id".into(), serde_json::json!(person));
    m.insert("thumbnail".into(), serde_json::json!("AAAA"));
    m.insert(
        "face_model".into(),
        serde_json::json!(lrg_ml::faces::MODEL_ID),
    );
    m.insert("face_det_score".into(), serde_json::json!(0.95));
    m.insert("face_area_ratio".into(), serde_json::json!(0.05));
    m.insert("face_sharpness".into(), serde_json::json!(0.6));
    m.insert("face_occlusion".into(), serde_json::json!(0.1));
    let mut v = vec![0.0f32; 512];
    v[0] = angle.cos() as f32;
    v[1] = angle.sin() as f32;
    lrg_store::StoreRecord {
        id: id.into(),
        vector: Some(v),
        metadata: m,
    }
}

async fn people_fixture() -> (axum::Router, Arc<AppState>, tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path).await.unwrap();
    state
        .store()
        .unwrap()
        .upsert(
            lrg_store::FACE_TABLE,
            &[
                // Two people, far enough apart that no sane threshold merges them.
                people_face("a1_0", "a1", "person_0", 0.0),
                people_face("a2_0", "a2", "person_0", 0.05),
                people_face("a3_0", "a3", "person_0", 0.08),
                people_face("b1_0", "b1", "person_1", 1.5),
                people_face("b2_0", "b2", "person_1", 1.55),
                people_face("c1_0", "c1", "", 3.0),
            ],
        )
        .await
        .unwrap();
    (app, state, dir, db_path)
}

async fn get_json(app: &axum::Router, uri: &str) -> serde_json::Value {
    let response = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "GET {uri}");
    body_json(response).await
}

async fn post_json(
    app: &axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

#[tokio::test]
async fn person_faces_returns_thumbnails_and_a_distance_per_face() {
    let (app, _state, _dir, db_path) = people_fixture().await;
    let json = get_json(
        &app,
        &format!("/v1/faces/persons/person_0/faces?db_path={db_path}"),
    )
    .await;

    assert_eq!(json["total"], 3);
    let faces = json["faces"].as_array().unwrap();
    assert_eq!(faces.len(), 3);
    for face in faces {
        assert!(face["face_id"].is_string(), "got {face}");
        assert_eq!(face["thumbnail"], "AAAA");
        assert!(
            face["distance"].as_f64().is_some(),
            "the page shows how far each face sits from its person: {face}"
        );
        assert_eq!(face["manual"], false);
    }
    // `typical` is the default: the face closest to the person's average first.
    let distances: Vec<f64> = faces
        .iter()
        .map(|f| f["distance"].as_f64().unwrap())
        .collect();
    assert!(
        distances.windows(2).all(|w| w[0] <= w[1] + 1e-9),
        "default sort is closest-first, got {distances:?}"
    );
}

/// The unassigned bucket is reachable under a name a URL path can carry, and
/// answers about faces with no person however their `person_id` is spelled.
#[tokio::test]
async fn the_unassigned_bucket_can_be_opened_like_a_person() {
    let (app, _state, _dir, db_path) = people_fixture().await;
    let json = get_json(
        &app,
        &format!("/v1/faces/persons/_unassigned/faces?db_path={db_path}"),
    )
    .await;
    assert_eq!(json["total"], 1);
    assert_eq!(json["faces"][0]["face_id"], "c1_0");
    assert_eq!(
        json["faces"][0]["distance"],
        serde_json::Value::Null,
        "unassigned faces are not a person, so there is no average to measure against"
    );
}

#[tokio::test]
async fn assigning_a_face_moves_it_and_marks_it_as_the_user_s_decision() {
    let (app, state, _dir, db_path) = people_fixture().await;
    let (status, json) = post_json(
        &app,
        "/v1/faces/assign",
        serde_json::json!({
            "face_ids": ["c1_0"],
            "person_id": "person_0",
            "db_path": db_path,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {json}");
    assert_eq!(json["updated"], 1);
    assert_eq!(json["person_id"], "person_0");

    let rows = state
        .store()
        .unwrap()
        .get(lrg_store::FACE_TABLE, &["c1_0".to_string()])
        .await
        .unwrap();
    assert_eq!(rows[0].metadata["person_id"], "person_0");
    assert_eq!(
        rows[0].metadata["person_manual"], true,
        "without this flag the next clustering run would undo the move"
    );
}

#[tokio::test]
async fn detaching_a_face_takes_it_off_the_person_without_forgetting_the_decision() {
    let (app, state, _dir, db_path) = people_fixture().await;
    let (status, json) = post_json(
        &app,
        "/v1/faces/assign",
        serde_json::json!({"face_ids": ["a1_0"], "person_id": "", "db_path": db_path}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {json}");

    let rows = state
        .store()
        .unwrap()
        .get(lrg_store::FACE_TABLE, &["a1_0".to_string()])
        .await
        .unwrap();
    assert_eq!(rows[0].metadata["person_id"], "person_unassigned");
    assert_eq!(rows[0].metadata["person_manual"], true);
}

#[tokio::test]
async fn moving_faces_to_a_new_person_picks_an_unused_id() {
    let (app, _state, _dir, db_path) = people_fixture().await;
    let (status, json) = post_json(
        &app,
        "/v1/faces/assign",
        serde_json::json!({
            "face_ids": ["a1_0", "a2_0"],
            "new_person": true,
            "name": "Split off",
            "db_path": db_path,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {json}");
    assert_eq!(json["updated"], 2);
    let new_id = json["person_id"].as_str().unwrap();
    assert_ne!(new_id, "person_0");
    assert_ne!(new_id, "person_1");
    assert_eq!(json["name"], "Split off");

    let persons = get_json(&app, &format!("/v1/faces/persons?db_path={db_path}")).await;
    let entry = persons["persons"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["person_id"] == new_id)
        .unwrap_or_else(|| panic!("the new person is missing from the list: {persons}"));
    assert_eq!(entry["face_count"], 2);
    assert_eq!(entry["manual_count"], 2);
}

#[tokio::test]
async fn assigning_reports_face_ids_that_no_longer_exist() {
    let (app, _state, _dir, db_path) = people_fixture().await;
    let (status, json) = post_json(
        &app,
        "/v1/faces/assign",
        serde_json::json!({
            "face_ids": ["a1_0", "gone_0"],
            "person_id": "person_1",
            "db_path": db_path,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {json}");
    assert_eq!(json["updated"], 1);
    let warnings = json["warnings"].as_array().unwrap();
    assert_eq!(
        warnings.len(),
        1,
        "moving fewer faces than asked for is not a silent success: {json}"
    );
}

#[tokio::test]
async fn merging_two_people_keeps_the_chosen_name() {
    let (app, state, _dir, db_path) = people_fixture().await;
    let (status, json) = post_json(
        &app,
        "/v1/faces/persons/merge",
        serde_json::json!({
            "person_ids": ["person_0", "person_1"],
            "into": "person_1",
            "name": "Anna",
            "db_path": db_path,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {json}");
    assert_eq!(json["person_id"], "person_1");
    assert_eq!(json["name"], "Anna");
    assert_eq!(json["moved"], 3, "person_0's three faces move: {json}");

    let rows = state
        .store()
        .unwrap()
        .scan_meta(lrg_store::FACE_TABLE)
        .await
        .unwrap();
    for (id, meta) in rows {
        if id == "c1_0" {
            continue;
        }
        assert_eq!(meta["person_id"], "person_1", "{id} did not move");
        assert_eq!(
            meta["person_manual"], true,
            "{id}: the whole merged person is pinned, or the next run splits it again"
        );
    }
}

#[tokio::test]
async fn merging_needs_two_real_people() {
    let (app, _state, _dir, db_path) = people_fixture().await;
    for body in [
        serde_json::json!({"person_ids": ["person_0"], "db_path": db_path}),
        serde_json::json!({"person_ids": ["person_0", ""], "db_path": db_path}),
        serde_json::json!({"person_ids": ["person_0", "person_0"], "db_path": db_path}),
    ] {
        let (status, json) = post_json(&app, "/v1/faces/persons/merge", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "got {json}");
    }
}

/// The reason a merge marks its faces manual. Two people the user merged are
/// deliberately far apart in the embedding space, so a plain re-cluster would
/// take them straight back apart — which would make merging pointless.
#[tokio::test]
async fn a_merge_survives_the_next_clustering_run() {
    let (app, state, _dir, db_path) = people_fixture().await;
    let (status, _) = post_json(
        &app,
        "/v1/faces/persons/merge",
        serde_json::json!({
            "person_ids": ["person_0", "person_1"],
            "into": "person_0",
            "name": "Anna",
            "db_path": db_path,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, json) = post_json(
        &app,
        "/v1/faces/cluster",
        // A threshold far below the distance between the two merged halves:
        // nothing but the pin can hold them together.
        serde_json::json!({"distance_threshold": 0.1, "db_path": db_path}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {json}");
    assert_eq!(json["diagnostics"]["pinned_faces"], 5, "got {json}");

    let rows = state
        .store()
        .unwrap()
        .scan_meta(lrg_store::FACE_TABLE)
        .await
        .unwrap();
    for (id, meta) in rows {
        if id == "c1_0" {
            continue;
        }
        assert_eq!(
            meta["person_id"], "person_0",
            "{id} was pulled back out of the merged person"
        );
    }
}

/// The other half of the same rule: a face the user took off a person must not
/// be quietly put back by the next run, even though it sits right next to that
/// person in the embedding space.
#[tokio::test]
async fn a_detached_face_is_not_re_attached_by_clustering() {
    let (app, state, _dir, db_path) = people_fixture().await;
    let (status, _) = post_json(
        &app,
        "/v1/faces/assign",
        serde_json::json!({"face_ids": ["a3_0"], "person_id": "", "db_path": db_path}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, json) = post_json(
        &app,
        "/v1/faces/cluster",
        serde_json::json!({"distance_threshold": 0.5, "db_path": db_path}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {json}");

    let rows = state
        .store()
        .unwrap()
        .get(lrg_store::FACE_TABLE, &["a3_0".to_string()])
        .await
        .unwrap();
    assert_eq!(rows[0].metadata["person_id"], "person_unassigned");
}

/// The clustering complaint this all started from: a chain of borderline faces
/// used to walk from one person to another. Complete linkage caps the distance
/// across the *whole* group, so it cannot.
#[tokio::test]
async fn clustering_does_not_chain_two_people_through_a_borderline_face() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path).await.unwrap();
    // Evenly spaced: each neighbour is inside the threshold, the two ends are
    // not. Anything density-based folds all five into one person.
    let step = 0.45_f64.acos();
    state
        .store()
        .unwrap()
        .upsert(
            lrg_store::FACE_TABLE,
            &[
                people_face("a1_0", "a1", "", 0.0),
                people_face("a2_0", "a2", "", 0.02),
                people_face("mid_0", "mid", "", step),
                people_face("b1_0", "b1", "", step * 2.0),
                people_face("b2_0", "b2", "", step * 2.0 + 0.02),
            ],
        )
        .await
        .unwrap();

    let (status, json) = post_json(
        &app,
        "/v1/faces/cluster",
        serde_json::json!({
            "distance_threshold": 0.56,
            "min_faces_per_person": 1,
            "db_path": db_path,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {json}");

    let rows = state
        .store()
        .unwrap()
        .scan_meta(lrg_store::FACE_TABLE)
        .await
        .unwrap();
    let person = |id: &str| {
        rows.iter()
            .find(|(row_id, _)| row_id == id)
            .map(|(_, m)| m["person_id"].as_str().unwrap().to_string())
            .unwrap()
    };
    assert_ne!(
        person("a1_0"),
        person("b1_0"),
        "the two ends are 1.6 apart and must not share a person: {json}"
    );
}

async fn put_json(
    app: &axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::put(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

/// Naming a cluster is the user saying "this is Anna", which is a claim about
/// the faces in it and not only about the label. Without the pin the next
/// **Cluster faces** run would happily regroup a person the user has already
/// vouched for, and the name would then sit on a different set of faces.
#[tokio::test]
async fn naming_a_person_confirms_every_face_it_has() {
    let (app, state, _dir, db_path) = people_fixture().await;
    let (status, json) = put_json(
        &app,
        &format!("/v1/faces/persons/person_0?db_path={db_path}"),
        serde_json::json!({"name": "Anna"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {json}");
    assert_eq!(json["name"], "Anna");
    assert_eq!(json["confirmed"], 3, "person_0 has three faces: {json}");

    let rows = state
        .store()
        .unwrap()
        .scan_meta(lrg_store::FACE_TABLE)
        .await
        .unwrap();
    for (id, meta) in &rows {
        let manual = meta
            .get("person_manual")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let is_annas = meta["person_id"] == "person_0";
        assert_eq!(
            manual, is_annas,
            "naming person_0 must pin its faces and nobody else's, but {id} came back {manual}"
        );
    }

    // And the listing the page renders has to agree, or the card would show a
    // name with no "confirmed" badge next to it.
    let listing = get_json(&app, &format!("/v1/faces/persons?db_path={db_path}")).await;
    let anna = listing["persons"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["person_id"] == "person_0")
        .expect("person_0 is in the listing");
    assert_eq!(anna["name"], "Anna");
    assert_eq!(anna["manual_count"], anna["face_count"]);
}

/// Clearing the name must not unpin anything. The same flag also records
/// hand-assignments and merges, and nothing in a row says which of those wrote
/// it — so undoing the naming would silently throw away decisions it never made.
#[tokio::test]
async fn clearing_a_name_leaves_the_faces_confirmed() {
    let (app, state, _dir, db_path) = people_fixture().await;
    let uri = format!("/v1/faces/persons/person_0?db_path={db_path}");
    put_json(&app, &uri, serde_json::json!({"name": "Anna"})).await;

    let (status, json) = put_json(&app, &uri, serde_json::json!({"name": "   "})).await;
    assert_eq!(status, StatusCode::OK, "got {json}");
    assert_eq!(json["name"], "");
    assert_eq!(json["confirmed"], 0, "there is nothing to confirm: {json}");

    let rows = state
        .store()
        .unwrap()
        .get(
            lrg_store::FACE_TABLE,
            &["a1_0".to_string(), "a2_0".to_string(), "a3_0".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    for record in &rows {
        assert_eq!(
            record.metadata["person_manual"], true,
            "{} lost its pin when the name went away",
            record.id
        );
    }
}

/// The unassigned bucket is not a person, so naming it pins nothing — the
/// faces in it are precisely the ones no decision has been made about.
#[tokio::test]
async fn naming_the_unassigned_bucket_confirms_nothing() {
    let (app, state, _dir, db_path) = people_fixture().await;
    let (status, json) = put_json(
        &app,
        &format!("/v1/faces/persons/_unassigned?db_path={db_path}"),
        serde_json::json!({"name": "Nobody"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {json}");
    assert_eq!(json["confirmed"], 0);

    let rows = state
        .store()
        .unwrap()
        .get(lrg_store::FACE_TABLE, &["c1_0".to_string()])
        .await
        .unwrap();
    assert!(
        !rows[0]
            .metadata
            .get("person_manual")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        "an unassigned face must stay open for the next clustering run"
    );
}

/// Entering a name another person already has is how the People page merges
/// two clusters: the page asks, then sends this. Both sides come out pinned,
/// so the merge survives the next re-cluster.
#[tokio::test]
async fn merging_by_name_keeps_the_name_and_confirms_both_sides() {
    let (app, state, _dir, db_path) = people_fixture().await;
    put_json(
        &app,
        &format!("/v1/faces/persons/person_0?db_path={db_path}"),
        serde_json::json!({"name": "Anna"}),
    )
    .await;

    let (status, json) = post_json(
        &app,
        "/v1/faces/persons/merge",
        serde_json::json!({
            "person_ids": ["person_1", "person_0"],
            "into": "person_0",
            "name": "Anna",
            "db_path": db_path,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got {json}");
    assert_eq!(json["person_id"], "person_0");
    assert_eq!(json["name"], "Anna");
    assert_eq!(json["moved"], 2, "person_1's two faces move: {json}");

    let listing = get_json(&app, &format!("/v1/faces/persons?db_path={db_path}")).await;
    let persons = listing["persons"].as_array().unwrap();
    assert!(
        !persons.iter().any(|p| p["person_id"] == "person_1"),
        "person_1 is gone after the merge: {listing}"
    );
    let anna = persons
        .iter()
        .find(|p| p["person_id"] == "person_0")
        .expect("person_0 survived");
    assert_eq!(anna["face_count"], 5);
    assert_eq!(
        anna["manual_count"], 5,
        "every face of a merged person is pinned, or the next run splits it again"
    );

    let rows = state
        .store()
        .unwrap()
        .scan_meta(lrg_store::FACE_TABLE)
        .await
        .unwrap();
    assert!(
        !rows.iter().any(|(_, m)| m["person_id"] == "person_1"),
        "no face may still point at the merged-away id"
    );
}

/// `/v1/db/backups` is POST-only on purpose (taking a backup also retains a
/// copy on disk), and the plugin's `downloadDatabaseBackup` has to match: it
/// once sent `GET` here and every backup download died with a bare `405`.
#[tokio::test]
async fn db_backup_is_post_only_and_streams_a_zip() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db");
    let db_path_str = db_path.to_str().unwrap();

    let (app, _) = fresh_app();
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/db/bind")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "db_path": db_path_str }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // What the plugin sends: POST with the db_path body the auto-bind
    // middleware injects.
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/db/backups")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "db_path": db_path_str }).to_string(),
                ))
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
        Some("application/zip"),
    );
    let zip = response.into_body().collect().await.unwrap().to_bytes();
    assert!(
        zip.starts_with(b"PK"),
        "expected a zip archive, got {} bytes starting {:?}",
        zip.len(),
        &zip[..zip.len().min(8)]
    );

    // And the method the plugin must not send: a GET is a 405, not a backup.
    let response = app
        .oneshot(Request::get("/v1/db/backups").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
