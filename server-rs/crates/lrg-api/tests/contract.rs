//! API contract tests for the M1 surface, asserting the exact response
//! shapes the Lightroom plugin (APISearchIndex.lua) depends on.

use std::sync::{Arc, LazyLock};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

use lrg_api::state::{AppState, ModelAssetPaths};

fn app(state: Arc<AppState>) -> axum::Router {
    lrg_api::build_router(state)
}

/// An empty directory, kept for the lifetime of the test process, that every
/// `AppState` here resolves its model files out of.
///
/// These are *contract* tests: they assert the response shapes
/// `APISearchIndex.lua` parses, and every one of them was written against a
/// server with no models on disk —
/// `cull_warning_tracks_stored_embeddings_not_model_residency` says so in as
/// many words ("SigLIP definitely not resident in this test process").
/// `AppState::new` resolves to the shared `~/.cache/lrgenius/models`
/// directory, so that comment only held where the assets happened to be
/// absent, and the suite quietly became a different suite where they were
/// present: four cull tests seed non-zero embeddings, `/v1/cull/grade` scores
/// them through CLIP-IQA, and — because each test builds its own `AppState`,
/// where the server has exactly one — each loads its *own* private copy of
/// SigLIP2's 818 MB image and 1.4 GB text towers. Four run at once under the
/// default harness parallelism, for 13.7 GB of resident model. That is what
/// OOM-killed the nightly `golden-tests-full` runner three nights running,
/// and it would do the same to any developer who had run the plugin's model
/// download before `cargo test`.
///
/// Pointing every family at an empty directory makes the suite
/// hermetic: same assertions, same cost, whether or not the assets are on the
/// machine. Numerical coverage of the real weights belongs to the golden
/// tests in `lrg-ml`, which is where `LRG_REQUIRE_GOLDENS` enforces it.
static NO_MODELS: LazyLock<tempfile::TempDir> =
    LazyLock::new(|| tempfile::tempdir().expect("temp dir for the empty model directory"));

fn models_absent() -> ModelAssetPaths {
    let dir = NO_MODELS.path();
    ModelAssetPaths {
        siglip: lrg_ml::siglip::ModelPaths {
            image_onnx: dir.join("siglip2_naflex_image.onnx"),
            text_onnx: dir.join("siglip2_naflex_text.onnx"),
            tokenizer_json: dir.join("siglip2_naflex_tokenizer.json"),
        },
        aesthetic: lrg_ml::aesthetic::AestheticPaths {
            onnx: dir.join("aesthetic_siglip.onnx"),
        },
        face: lrg_ml::faces::FaceModelPaths {
            det_onnx: dir.join("yunet_face_detection.onnx"),
            rec_onnx: dir.join("facenet_vggface2.onnx"),
            auraface_onnx: dir.join("auraface_glintr100.onnx"),
        },
        face_state: lrg_ml::face_state::FaceStatePaths {
            mesh_onnx: dir.join("face_mesh_v2.onnx"),
            blendshapes_onnx: dir.join("face_blendshapes_v2.onnx"),
        },
        subject: lrg_ml::subject::SubjectPaths {
            onnx: dir.join("subject_rfdetr_seg_nano.onnx"),
        },
        bioclip: lrg_ml::bioclip::BioclipModelPaths {
            image_onnx: dir.join("bioclip2_image.onnx"),
            taxa_bin: dir.join("bioclip2_taxa.bin"),
            taxa_json: dir.join("bioclip2_taxa.json"),
        },
    }
}

fn fresh_app() -> (axum::Router, Arc<AppState>) {
    let state = Arc::new(AppState::with_model_paths(None, false, models_absent()));
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

/// A remote backend (a container) is started with `--db-path`, and the plugin
/// sends it no `db_path` at all: it must open that store without being bound.
#[tokio::test]
async fn a_backend_started_with_a_db_path_opens_it_without_a_bind() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db");
    let state = Arc::new(AppState::with_model_paths(
        Some(db_path.clone()),
        false,
        models_absent(),
    ));
    assert!(
        state.store().is_none(),
        "nothing opens the store eagerly here"
    );

    // What the middleware does for a request without db_path.
    let response = app(state.clone())
        .oneshot(
            Request::get("/v1/server/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(state.store().is_some());
    assert_eq!(state.db_path(), Some(db_path));

    // What startup does; a second call is a no-op once the store is open.
    assert!(!state.ensure_initial_db_path().await.unwrap());
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

/// `GET /v1/models/assets`: the Plug-in Manager's indicators, the setup
/// wizard's "about N GB" line and the run preflight all read it. With no model
/// on disk every family is missing, and `missing_approx_bytes` is their sum.
#[tokio::test]
async fn model_assets_report_every_family_the_plugin_reads() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::get("/v1/models/assets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;

    assert_eq!(json["ready"], serde_json::json!(false));
    let families = json["families"].as_array().expect("families array");
    let ids: Vec<&str> = families
        .iter()
        .map(|family| family["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "clip",
            "aesthetic",
            "bioclip",
            "face",
            "face_state",
            "subject"
        ]
    );
    for family in families {
        assert_eq!(family["ready"], serde_json::json!(false), "{family}");
        assert_eq!(family["downloadable"], serde_json::json!(true), "{family}");
        assert!(
            family["name"].as_str().is_some_and(|name| !name.is_empty()),
            "{family}"
        );
        assert!(
            family["approx_bytes"]
                .as_u64()
                .is_some_and(|bytes| bytes > 0),
            "{family}"
        );
    }
    let sum: u64 = families
        .iter()
        .map(|family| family["approx_bytes"].as_u64().unwrap())
        .sum();
    assert_eq!(json["missing_approx_bytes"].as_u64(), Some(sum));
}

/// `error_messages` is only there when a combined download skipped a family,
/// so a status without failures (and every single-family route's) keeps the
/// shape it always had.
#[tokio::test]
async fn model_download_status_has_no_error_list_without_failures() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::get("/v1/models/assets/downloads")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["status"], "not_started");
    assert!(json["error"].is_null());
    assert!(json.get("error_messages").is_none(), "{json}");
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

/// An app whose aesthetic model *file* exists (empty: nothing here loads it), so
/// routes that only ask whether the model is on disk behave as with the real one.
fn app_with_aesthetic_file() -> (axum::Router, Arc<AppState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let onnx = dir.path().join("aesthetic_siglip.onnx");
    std::fs::write(&onnx, b"").unwrap();
    let mut models = models_absent();
    models.aesthetic = lrg_ml::aesthetic::AestheticPaths { onnx };
    let state = Arc::new(AppState::with_model_paths(None, false, models));
    (app(state.clone()), state, dir)
}

async fn seed_embedded_photos(
    state: &Arc<AppState>,
    rows: &[(&str, Option<(f64, &str)>)],
) -> serde_json::Value {
    let records: Vec<lrg_store::StoreRecord> = rows
        .iter()
        .map(|(id, aesthetic)| {
            let mut m = serde_json::Map::new();
            m.insert("has_embedding".into(), serde_json::json!(true));
            m.insert(
                "embedding_model".into(),
                serde_json::json!(lrg_ml::siglip::MODEL_ID),
            );
            m.insert("cull_phash".into(), serde_json::json!("abcd0123abcd0123"));
            if let Some((score, model)) = aesthetic {
                m.insert("aesthetic_score".into(), serde_json::json!(score));
                m.insert("aesthetic_model".into(), serde_json::json!(model));
            }
            lrg_store::StoreRecord {
                id: (*id).into(),
                vector: Some(vec![0.1; 1152]),
                metadata: m,
            }
        })
        .collect();
    state
        .store()
        .unwrap()
        .upsert(lrg_store::IMAGE_TABLE, &records)
        .await
        .unwrap();
    serde_json::json!(rows.iter().map(|(id, _)| *id).collect::<Vec<_>>())
}

/// A photo is only done for the embedding tasks once it carries a score from
/// the current export, and only when the model that makes one is on disk —
/// otherwise the same run would be offered for ever.
#[tokio::test]
async fn check_unprocessed_re_queues_photos_without_a_current_aesthetic_score() {
    let (app, state, _models) = app_with_aesthetic_file();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    state.ensure_db_path(&db_path).await.unwrap();
    let ids = seed_embedded_photos(
        &state,
        &[
            ("scored", Some((7.1, lrg_ml::aesthetic::MODEL_ID))),
            ("unscored", None),
            ("stale", Some((6.0, "aesthetic-from-another-export"))),
        ],
    )
    .await;

    // `cull` is left out: it also wants `cull_faces_present`, which none of
    // these rows have, so it would re-queue all of them for that reason.
    for tasks in ["embeddings", "aesthetic"] {
        let (status, json) = post_json(
            &app,
            "/v1/index/unprocessed",
            serde_json::json!({
                "photo_ids": ids, "tasks": tasks,
                "regenerate_metadata": "false", "db_path": db_path,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let mut queued: Vec<&str> = json["photo_ids"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        queued.sort();
        assert_eq!(queued, ["stale", "unscored"], "tasks={tasks}");
    }
}

/// Without the model on disk nothing can be brought up to date, so the score
/// does not keep the photos in the queue.
#[tokio::test]
async fn check_unprocessed_ignores_the_aesthetic_score_without_the_model() {
    let (app, state) = fresh_app();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    state.ensure_db_path(&db_path).await.unwrap();
    let ids = seed_embedded_photos(&state, &[("unscored", None)]).await;
    let (_, json) = post_json(
        &app,
        "/v1/index/unprocessed",
        serde_json::json!({
            "photo_ids": ids, "tasks": "embeddings",
            "regenerate_metadata": "false", "db_path": db_path,
        }),
    )
    .await;
    assert_eq!(json["photo_ids"], serde_json::json!([]));
}

/// The plug-in reads the score and genre back from the lookup, in a block of
/// their own that does not leak into the LLM metadata the review dialog edits.
#[tokio::test]
async fn photo_lookup_reports_the_aesthetic_score_and_genre() {
    let (app, state) = fresh_app();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    state.ensure_db_path(&db_path).await.unwrap();
    seed_embedded_photos(
        &state,
        &[
            ("scored", Some((7.1, lrg_ml::aesthetic::MODEL_ID))),
            ("stale", Some((6.0, "aesthetic-from-another-export"))),
            ("bare", None),
        ],
    )
    .await;
    // Genre lives on the same row; add it to one photo.
    let store = state.store().unwrap();
    let mut row = store
        .get(lrg_store::IMAGE_TABLE, &["scored".to_string()])
        .await
        .unwrap()
        .pop()
        .unwrap();
    row.metadata
        .insert("genre".into(), serde_json::json!("macro"));
    row.metadata
        .insert("genre_confidence".into(), serde_json::json!(0.74));
    store.upsert(lrg_store::IMAGE_TABLE, &[row]).await.unwrap();

    let lookup = |id: &'static str| {
        let app = app.clone();
        let db_path = db_path.clone();
        async move {
            post_json(
                &app,
                "/v1/photos/lookup",
                serde_json::json!({"photo_id": id, "db_path": db_path}),
            )
            .await
            .1
        }
    };

    let scored = lookup("scored").await;
    assert_eq!(scored["aesthetic"]["score"], 7.1);
    assert_eq!(scored["aesthetic"]["outdated"], false);
    assert_eq!(scored["aesthetic"]["genre"], "macro");
    assert_eq!(scored["aesthetic"]["genre_label"], "Macro");
    assert_eq!(scored["aesthetic"]["genre_confidence"], 0.74);
    assert!(
        scored["metadata"].get("aesthetic_score").is_none(),
        "the score must not reach the review dialog's metadata"
    );

    let stale = lookup("stale").await;
    assert_eq!(stale["aesthetic"]["score"], 6.0);
    assert_eq!(stale["aesthetic"]["outdated"], true);
    assert!(stale["aesthetic"]["genre"].is_null());

    assert!(lookup("bare").await["aesthetic"].is_null());
}

/// `(id, score, model, genre, title)` of a photo for the search tests; a `None`
/// score is a photo never scored.
type SearchableRow<'a> = (&'a str, Option<f64>, &'a str, Option<&'a str>, &'a str);

/// Photos with a stored aesthetic score and genre, for the search tests.
async fn seed_searchable(state: &Arc<AppState>, rows: &[SearchableRow<'_>]) {
    let records: Vec<lrg_store::StoreRecord> = rows
        .iter()
        .map(|(id, score, model, genre, title)| {
            let mut m = serde_json::Map::new();
            m.insert("has_embedding".into(), serde_json::json!(false));
            m.insert("title".into(), serde_json::json!(title));
            if let Some(score) = score {
                m.insert("aesthetic_score".into(), serde_json::json!(score));
                m.insert("aesthetic_model".into(), serde_json::json!(model));
            }
            if let Some(genre) = genre {
                m.insert("genre".into(), serde_json::json!(genre));
            }
            lrg_store::StoreRecord {
                id: (*id).into(),
                vector: None,
                metadata: m,
            }
        })
        .collect();
    state
        .store()
        .unwrap()
        .upsert(lrg_store::IMAGE_TABLE, &records)
        .await
        .unwrap();
}

fn result_ids(json: &serde_json::Value) -> Vec<&str> {
    json["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["photo_id"].as_str().unwrap())
        .collect()
}

/// No term: "the best N", by stored score, best first. A photo with no score
/// from the current export is not on the scale and is left out — and said so.
#[tokio::test]
async fn search_without_a_term_returns_the_best_photos_by_aesthetic_score() {
    let (app, state) = fresh_app();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    state.ensure_db_path(&db).await.unwrap();
    let current = lrg_ml::aesthetic::MODEL_ID;
    seed_searchable(
        &state,
        &[
            ("mid", Some(6.0), current, Some("street"), "a"),
            ("best", Some(7.5), current, Some("macro"), "b"),
            ("worst", Some(4.0), current, None, "c"),
            ("never", None, current, None, "d"),
            ("stale", Some(9.0), "another-export", None, "e"),
        ],
    )
    .await;

    let (status, json) = post_json(
        &app,
        "/v1/search",
        serde_json::json!({"max_results": 10, "db_path": db}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(result_ids(&json), ["best", "mid", "worst"]);
    let first = &json["results"][0];
    assert_eq!(first["aesthetic_score"], 7.5);
    assert_eq!(first["genre"], "macro");
    assert_eq!(first["genre_label"], "Macro");
    assert!(first["distance"].is_null());
    // The two without a current score are counted, once.
    let warnings = json["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{json}");
    assert!(warnings[0]
        .as_str()
        .unwrap()
        .starts_with("2 photo(s) in scope have no aesthetic score"));
    assert_eq!(json["warning"], warnings[0]);

    // The count is a cap on what comes back, not on what is considered.
    let (_, json) = post_json(
        &app,
        "/v1/search",
        serde_json::json!({"max_results": 10, "min_aesthetic": 6.0, "db_path": db}),
    )
    .await;
    assert_eq!(result_ids(&json), ["best", "mid"]);
}

#[tokio::test]
async fn search_asks_of_the_faces_on_the_photo() {
    let (app, state) = fresh_app();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    state.ensure_db_path(&db).await.unwrap();
    let current = lrg_ml::aesthetic::MODEL_ID;
    let names = ["all_good", "blink", "frown", "away", "landscape", "old"];
    let seed: Vec<_> = names
        .iter()
        .map(|id| (*id, Some(7.0), current, Some("portrait"), "flower"))
        .collect();
    seed_searchable(&state, &seed).await;
    let store = state.store().unwrap();
    let ids: Vec<String> = names.iter().map(|id| id.to_string()).collect();
    let mut rows = store.get(lrg_store::IMAGE_TABLE, &ids).await.unwrap();
    for row in rows.iter_mut() {
        let read = |key: &str, value: serde_json::Value, m: &mut serde_json::Map<_, _>| {
            m.insert(key.to_string(), value);
        };
        let m = &mut row.metadata;
        match row.id.as_str() {
            "landscape" => read("cull_face_count", serde_json::json!(0), m),
            "old" => read("cull_face_count", serde_json::json!(1), m),
            id => {
                read("cull_face_count", serde_json::json!(2), m);
                read(
                    "cull_face_state_version",
                    serde_json::json!(lrg_analysis::grouping::FACE_STATE_VERSION),
                    m,
                );
                let eyes = if id == "blink" { "closed" } else { "open" };
                read("cull_eye_state", serde_json::json!(eyes), m);
                let smile = if id == "frown" { 0.1 } else { 0.8 };
                read("cull_smile", serde_json::json!(smile), m);
                let gaze = if id == "away" { 40.0 } else { 6.0 };
                read("cull_gaze_off_lens", serde_json::json!(gaze), m);
            }
        }
    }
    store.upsert(lrg_store::IMAGE_TABLE, &rows).await.unwrap();

    let ask = |filter: serde_json::Value| {
        let mut body = serde_json::json!({"max_results": 50, "db_path": db});
        body.as_object_mut()
            .unwrap()
            .extend(filter.as_object().unwrap().clone());
        body
    };
    let found = |json: &serde_json::Value| {
        let mut ids: Vec<String> = result_ids(json).into_iter().map(str::to_string).collect();
        ids.sort();
        ids
    };

    let (_, json) = post_json(
        &app,
        "/v1/search",
        ask(serde_json::json!({"smiling": true})),
    )
    .await;
    assert_eq!(found(&json), ["all_good", "away", "blink"]);
    // The one that was never analysed is counted once; the landscape was
    // measured, has no faces, and is simply not a match.
    let warnings = json["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{json}");
    assert!(warnings[0]
        .as_str()
        .unwrap()
        .starts_with("1 matching photo(s) could not be judged"));

    let (_, json) = post_json(
        &app,
        "/v1/search",
        ask(serde_json::json!({"eyes_open": true, "smiling": true, "looking_at_camera": true})),
    )
    .await;
    assert_eq!(found(&json), ["all_good"]);
}

#[tokio::test]
async fn search_restricts_by_genre_and_minimum_score() {
    let (app, state) = fresh_app();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    state.ensure_db_path(&db).await.unwrap();
    let current = lrg_ml::aesthetic::MODEL_ID;
    seed_searchable(
        &state,
        &[
            ("macro_hi", Some(7.0), current, Some("macro"), "flower"),
            ("macro_lo", Some(5.0), current, Some("macro"), "flower"),
            ("street_hi", Some(8.0), current, Some("street"), "flower"),
            ("macro_noscore", None, current, Some("macro"), "flower"),
        ],
    )
    .await;

    // A term that matches by title only (no model in this test app), restricted.
    let (status, json) = post_json(
        &app,
        "/v1/search",
        serde_json::json!({
            "term": "flower", "genres": ["macro"], "min_aesthetic": 6.0,
            "sort": "aesthetic", "search_sources": {"semantic_siglip": false},
            "db_path": db,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(result_ids(&json), ["macro_hi"]);
    // The photo that could not be judged against the minimum is named, once;
    // the ones judged and found short or of another genre are just not matches.
    let warnings = json["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{json}");
    assert!(warnings[0]
        .as_str()
        .unwrap()
        .starts_with("1 matching photo(s) have no aesthetic score"));
}

/// With a term and `sort=aesthetic` the metadata matches are ordered by score,
/// unscored ones last.
#[tokio::test]
async fn search_orders_matches_by_aesthetic_score_when_asked() {
    let (app, state) = fresh_app();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    state.ensure_db_path(&db).await.unwrap();
    let current = lrg_ml::aesthetic::MODEL_ID;
    seed_searchable(
        &state,
        &[
            ("plain", Some(5.0), current, None, "cat on a sofa"),
            ("none", None, current, None, "cat in a box"),
            ("lovely", Some(7.0), current, None, "cat in the sun"),
        ],
    )
    .await;
    let search = |sort: &'static str| {
        let app = app.clone();
        let db = db.clone();
        async move {
            post_json(
                &app,
                "/v1/search",
                serde_json::json!({
                    "term": "cat", "sort": sort,
                    "search_sources": {"semantic_siglip": false}, "db_path": db,
                }),
            )
            .await
            .1
        }
    };
    let by_score = search("aesthetic").await;
    assert_eq!(result_ids(&by_score), ["lovely", "plain", "none"]);
    assert!(by_score["warning"]
        .as_str()
        .unwrap()
        .contains("1 matching photo(s) have no aesthetic score"));
    // Relevance leaves the search's own order, and says nothing about scores.
    let by_relevance = search("relevance").await;
    assert_eq!(by_relevance["results"].as_array().unwrap().len(), 3);
    assert!(by_relevance["warning"].is_null(), "{by_relevance}");
}

/// Relevance is how close a photo is to the term; without one there is nothing
/// to be relevant to, which is said rather than quietly reinterpreted.
#[tokio::test]
async fn search_rejects_relevance_ordering_without_a_term_and_unknown_sorts() {
    let (app, state) = fresh_app();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    state.ensure_db_path(&db).await.unwrap();
    for sort in ["relevance", "combined"] {
        let (status, json) = post_json(
            &app,
            "/v1/search",
            serde_json::json!({"sort": sort, "db_path": db}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
        assert!(json["error"]
            .as_str()
            .unwrap()
            .contains("need a search term"));
    }
    let (status, json) = post_json(
        &app,
        "/v1/search",
        serde_json::json!({"term": "x", "sort": "best", "db_path": db}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert!(json["error"].as_str().unwrap().contains("Unknown sort"));
}

/// The dialog's genre list comes from the classifier's own, `other` included.
#[tokio::test]
async fn search_genres_lists_what_the_classifier_can_name() {
    let (app, _state) = fresh_app();
    let json = get_json(&app, "/v1/search/genres").await;
    let genres = json["genres"].as_array().unwrap();
    assert_eq!(genres.len(), lrg_ml::genre::GENRES.len() + 1);
    assert!(genres
        .iter()
        .any(|g| g["id"] == "macro" && g["label"] == "Macro"));
    assert_eq!(genres.last().unwrap()["id"], lrg_ml::genre::OTHER_ID);
}

/// A genre made under another version of the genre list, or never made, is
/// re-queued from the stored embedding — but only when the search model that makes
/// it is on disk.
#[tokio::test]
async fn check_unprocessed_re_queues_photos_with_a_stale_genre() {
    let dir = tempfile::tempdir().unwrap();
    // This test's own directory, never the shared empty one the other tests rely
    // on to have no model files.
    let mut models = models_absent();
    models.siglip = lrg_ml::siglip::ModelPaths {
        image_onnx: dir.path().join("siglip2_naflex_image.onnx"),
        text_onnx: dir.path().join("siglip2_naflex_text.onnx"),
        tokenizer_json: dir.path().join("siglip2_naflex_tokenizer.json"),
    };
    for file in [
        &models.siglip.image_onnx,
        &models.siglip.text_onnx,
        &models.siglip.tokenizer_json,
    ] {
        std::fs::write(file, b"").unwrap();
    }
    let present = Arc::new(AppState::with_model_paths(None, false, {
        let onnx = dir.path().join("aesthetic_siglip.onnx");
        std::fs::write(&onnx, b"").unwrap();
        models.aesthetic = lrg_ml::aesthetic::AestheticPaths { onnx };
        models
    }));
    let app = app(present.clone());
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    present.ensure_db_path(&db).await.unwrap();
    let current = lrg_ml::aesthetic::MODEL_ID;
    let ids = seed_embedded_photos(
        &present,
        &[
            ("current", Some((7.0, current))),
            ("old_version", Some((7.0, current))),
            ("no_genre", Some((7.0, current))),
        ],
    )
    .await;
    let store = present.store().unwrap();
    let mut rows = store
        .get(
            lrg_store::IMAGE_TABLE,
            &["current".to_string(), "old_version".to_string()],
        )
        .await
        .unwrap();
    for row in &mut rows {
        row.metadata
            .insert("genre".into(), serde_json::json!("landscape"));
        let version = if row.id == "current" {
            lrg_ml::genre::VERSION
        } else {
            "an-older-version"
        };
        row.metadata
            .insert("genre_version".into(), serde_json::json!(version));
    }
    store.upsert(lrg_store::IMAGE_TABLE, &rows).await.unwrap();

    let (_, json) = post_json(
        &app,
        "/v1/index/unprocessed",
        serde_json::json!({
            "photo_ids": ids, "tasks": "embeddings",
            "regenerate_metadata": "false", "db_path": db,
        }),
    )
    .await;
    let mut queued: Vec<&str> = json["photo_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    queued.sort();
    assert_eq!(queued, ["no_genre", "old_version"]);

    // Without the search model there is nothing to make a genre with.
    let (app, state) = fresh_app();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    state.ensure_db_path(&db).await.unwrap();
    let ids = seed_embedded_photos(&state, &[("no_genre", Some((7.0, current)))]).await;
    let (_, json) = post_json(
        &app,
        "/v1/index/unprocessed",
        serde_json::json!({
            "photo_ids": ids, "tasks": "embeddings",
            "regenerate_metadata": "false", "db_path": db,
        }),
    )
    .await;
    assert_eq!(json["photo_ids"], serde_json::json!([]));
}

/// A stored embedding written by a different function is re-queued; one whose
/// identity is simply not recorded is not.
///
/// The asymmetry is the point. A row that predates `embedding_model` cannot be
/// checked either way, and treating absence as stale would re-embed an entire
/// catalog on the first run after an upgrade — so absence is taken as current.
/// A *different* identity is the case the field exists for.
#[tokio::test]
async fn check_unprocessed_re_queues_only_a_different_embedding_model() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db");
    let db_path_str = db_path.to_str().unwrap().to_string();

    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    let store = state.store().unwrap();

    let embedded = |model: Option<&str>| {
        let mut m = serde_json::Map::new();
        m.insert("has_embedding".into(), serde_json::json!(true));
        // Required for the row to count as done at all: `any_task` is true
        // whenever any task is asked for, so a missing `cull_phash` re-queues
        // the photo regardless of its embedding.
        m.insert("cull_phash".into(), serde_json::json!("abcd0123abcd0123"));
        if let Some(model) = model {
            m.insert("embedding_model".into(), serde_json::json!(model));
        }
        m
    };
    store
        .upsert(
            lrg_store::IMAGE_TABLE,
            &[
                // Another function wrote this vector — an older model, a
                // re-export, a preprocessing change. Indistinguishable from a
                // current one except by the identity.
                lrg_store::StoreRecord {
                    id: "other".into(),
                    vector: Some(vec![0.1; 1152]),
                    metadata: embedded(Some("some-previous-model")),
                },
                // Predates the field.
                lrg_store::StoreRecord {
                    id: "unlabelled".into(),
                    vector: Some(vec![0.2; 1152]),
                    metadata: embedded(None),
                },
                // This build's own work.
                lrg_store::StoreRecord {
                    id: "current".into(),
                    vector: Some(vec![0.3; 1152]),
                    metadata: embedded(Some(lrg_ml::siglip::MODEL_ID)),
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
                        "photo_ids": ["other", "unlabelled", "current"],
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
    assert_eq!(json["photo_ids"], serde_json::json!(["other"]));
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
    seed_burst_measured(state, count, with_embedding, true).await;
}

/// [`seed_burst`], with or without the subject pass behind it.
async fn seed_burst_measured(
    state: &Arc<AppState>,
    count: usize,
    with_embedding: bool,
    subject_measured: bool,
) {
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
            // A current score, so a cull over these photos has nothing to say
            // about missing ones; descending like the sharpness, so the winner
            // is still photo 0.
            meta.insert(
                "aesthetic_score".into(),
                serde_json::json!(7.0 - 0.1 * i as f64),
            );
            meta.insert(
                "aesthetic_model".into(),
                serde_json::json!(lrg_ml::aesthetic::MODEL_ID),
            );
            // Through the subject pass, which found nothing: "no subject" is
            // an answer too, so a cull has nothing to say about it either.
            if subject_measured {
                meta.insert(
                    lrg_analysis::grouping::SUBJECT_VERSION_KEY.into(),
                    serde_json::json!(lrg_analysis::grouping::SUBJECT_VERSION),
                );
                meta.insert(
                    "cull_subject_model".into(),
                    serde_json::json!(lrg_ml::subject::MODEL_ID),
                );
            }
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

/// A search can only find embeddings made by the model it embeds the query
/// with, so stats say how many stored ones were made by another.
#[tokio::test]
async fn stats_count_embeddings_from_another_search_model() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path).await.unwrap();
    let row = |id: &str, model: Option<&str>| {
        let mut m = serde_json::Map::new();
        m.insert("has_embedding".into(), serde_json::json!(true));
        if let Some(model) = model {
            m.insert("embedding_model".into(), serde_json::json!(model));
        }
        lrg_store::StoreRecord {
            id: id.into(),
            vector: Some(vec![0.1; 1152]),
            metadata: m,
        }
    };
    state
        .store()
        .unwrap()
        .upsert(
            lrg_store::IMAGE_TABLE,
            &[
                row("current", Some(lrg_ml::siglip::MODEL_ID)),
                row("legacy", Some(lrg_ml::siglip::LEGACY_MODEL_ID)),
                row("unstamped", None),
            ],
        )
        .await
        .unwrap();

    let stats = get_json(&app, "/v1/db/stats").await;
    assert_eq!(stats["photos"]["with_embedding"], 3);
    // The unstamped row counts as current, as in /v1/index/unprocessed.
    assert_eq!(stats["photos"]["with_outdated_embedding"], 1);
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

    // TaskCullPhotos.lua reads the list; `warning` stays for older plugins.
    assert!(json["warnings"].is_array(), "{json}");

    // Per-photo fields the plugin writes into catalog metadata.
    let winner = &group["photos"][0];
    assert_eq!(winner["winner"], true);
    assert_eq!(winner["rank"], 1);
    assert!(winner["cull_score"].is_number());
    assert!(winner["reason_codes"].is_array());
    assert!(winner["explanation"].is_string());
    assert!(winner["metrics"].is_object());
}

/// `reject_margin_scale` is the plugin's "how hard to cull" control. The seeded
/// burst's scores sit ~0.047 apart per frame, under the default 0.18 margin,
/// so nothing is rejected until the margin is scaled down — and then the
/// frames fall one by one, furthest from the pick first.
#[tokio::test]
async fn cull_reject_margin_scale_moves_the_score_margin_only() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    seed_burst(&state, 3, true).await;

    let rejects = |scale: Option<f64>| {
        let app = app.clone();
        let db = db_path_str.clone();
        async move {
            let mut body = serde_json::json!({
                "photo_ids": ["burst0", "burst1", "burst2"],
                "db_path": db,
            });
            if let Some(scale) = scale {
                body["reject_margin_scale"] = serde_json::json!(scale);
            }
            let json = cull_request(app, body).await;
            assert_eq!(json["groups"][0]["winner_photo_id"], "burst0");
            json["summary"]["reject_candidate_count"].as_u64().unwrap()
        }
    };

    assert_eq!(rejects(None).await, 0, "preset margin: a gentle cull");
    assert_eq!(
        rejects(Some(1.0)).await,
        0,
        "1.0 is the preset's own margin"
    );
    assert_eq!(
        rejects(Some(0.5)).await,
        1,
        "half the margin takes the weakest frame"
    );
    assert_eq!(
        rejects(Some(0.25)).await,
        2,
        "a quarter leaves only the pick"
    );
    // Out-of-range values are clamped, not refused: 0 would mean 0.25.
    assert_eq!(rejects(Some(0.0)).await, 2);

    let response = app
        .oneshot(
            Request::post("/v1/cull/grade")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "photo_ids": ["burst0"],
                        "db_path": db_path_str,
                        "reject_margin_scale": "wide",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
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

/// Writes every fixed prompt's embedding, the way an earlier run that had the
/// text tower would have left them.
async fn seed_prompt_embeddings(state: &Arc<AppState>) {
    let rows: Vec<lrg_store::TextEmbedding> = lrg_api::prompt_embeddings::fixed_prompts()
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let mut vector = vec![0.0f32; 1152];
            vector[i] = 1.0;
            lrg_store::TextEmbedding {
                model_id: lrg_ml::siglip::MODEL_ID.to_string(),
                text: text.to_string(),
                vector,
            }
        })
        .collect();
    state
        .store()
        .unwrap()
        .put_text_embeddings(&rows)
        .await
        .unwrap();
}

#[tokio::test]
async fn cull_warning_tracks_stored_embeddings_not_model_residency() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    // Embeddings present, SigLIP definitely not resident in this test process
    // — not even on disk. With the prompt vectors an earlier run stored,
    // culling needs no model at all.
    seed_burst(&state, 3, true).await;
    seed_prompt_embeddings(&state).await;

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

/// A single that is clearly blurred is a reject candidate with no pick, and
/// `winner_photo_id` is `null` rather than naming it. The plugin decodes JSON
/// null to nil, which its `if winnerPhotoId` already handles.
#[tokio::test]
async fn a_blurred_single_has_no_winner() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    let store = state.store().unwrap();
    let mut meta = serde_json::Map::new();
    meta.insert("filename".into(), serde_json::json!("soft.jpg"));
    meta.insert("capture_time".into(), serde_json::json!(1_700_000_000));
    meta.insert("cull_phash".into(), serde_json::json!("0f0f0f0f0f0f0f0f"));
    meta.insert("cull_sharpness".into(), serde_json::json!(0.04));
    meta.insert("cull_exposure".into(), serde_json::json!(0.7));
    meta.insert("cull_noise".into(), serde_json::json!(0.1));
    store
        .upsert(
            lrg_store::IMAGE_TABLE,
            &[lrg_store::StoreRecord {
                id: "soft".into(),
                vector: None,
                metadata: meta,
            }],
        )
        .await
        .unwrap();

    let json = cull_request(
        app,
        serde_json::json!({"photo_ids": ["soft"], "db_path": db_path_str}),
    )
    .await;

    let group = &json["groups"][0];
    assert_eq!(group["group_type"], "single");
    assert!(group["winner_photo_id"].is_null(), "{group}");
    assert_eq!(
        group["reject_candidate_photo_ids"],
        serde_json::json!(["soft"])
    );
    assert_eq!(json["summary"]["pick_count"], 0);
    assert_eq!(json["summary"]["reject_candidate_count"], 1);
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

/// Photos with embeddings, but no stored prompt vectors and no model to make
/// them: the moment cannot be judged, and the ranking that skipped it has to
/// say so — and say how to fix it.
#[tokio::test]
async fn cull_warns_when_the_moment_has_no_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();
    seed_burst(&state, 3, true).await;

    // `sports` ranks with the action set and the sport emotion set, so two sets
    // miss for one cause.
    let json = cull_request(
        app,
        serde_json::json!({
            "photo_ids": ["burst0", "burst1", "burst2"],
            "culling_preset": "sports",
            "db_path": db_path_str,
        }),
    )
    .await;

    // The run still succeeds, ranked on sharpness, exposure and faces.
    assert_eq!(json["status"], "success");
    assert_eq!(json["summary"]["group_count"], 1);
    assert_eq!(json["groups"][0]["winner_photo_id"], "burst0");
    // Said once, not once per set — and not a second time as photos whose
    // moment went unjudged, which is the same cause.
    let warnings = json["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{json}");
    let warning = warnings[0].as_str().unwrap();
    assert!(
        warning.contains("moment") && warning.contains("Download AI models"),
        "expected one caveat naming every lost signal and the fix, got {warning:?}"
    );
    // The string the plug-in shows carries it too.
    assert_eq!(json["warning"], warnings[0], "{json}");
}

/// A second catalog's database gets the prompt vectors from the first one's
/// run, out of memory — no text tower, and nothing for the user to see.
#[tokio::test]
async fn a_second_database_is_filled_with_prompts_from_memory() {
    let (app, state) = fresh_app();

    // `sports` is a preset that asks the prompts questions: the default one
    // ranks on stored numbers alone and reads none.
    let cull = |db: &str| {
        serde_json::json!({
            "photo_ids": ["burst0", "burst1", "burst2"],
            "culling_preset": "sports",
            "db_path": db,
        })
    };

    let first = tempfile::tempdir().unwrap();
    let first_db = first
        .path()
        .join("lrgenius.db")
        .to_str()
        .unwrap()
        .to_string();
    state.ensure_db_path(&first_db).await.unwrap();
    seed_burst(&state, 3, true).await;
    seed_prompt_embeddings(&state).await;
    let json = cull_request(app.clone(), cull(&first_db)).await;
    assert!(json["warning"].is_null(), "{json}");

    let second = tempfile::tempdir().unwrap();
    let second_db = second
        .path()
        .join("lrgenius.db")
        .to_str()
        .unwrap()
        .to_string();
    state.ensure_db_path(&second_db).await.unwrap();
    seed_burst(&state, 3, true).await;
    let json = cull_request(app, cull(&second_db)).await;
    assert!(json["warning"].is_null(), "{json}");

    // What this cull read: the sets the preset ranks with, nothing else copied.
    let ranking = lrg_analysis::culling_config::get_culling_config("sports").ranking;
    let mut wanted: Vec<String> = [ranking.semantic_prompt_set, ranking.emotion_prompt_set]
        .into_iter()
        .flatten()
        .filter_map(lrg_ml::clip_iqa::PromptSet::from_name)
        .flat_map(|set| set.texts().map(str::to_string).collect::<Vec<_>>())
        .collect();
    if ranking.eyes_closed_intent_threshold > 0.0 {
        wanted.extend(
            lrg_ml::clip_iqa::PromptSet::EyesClosedIntent
                .texts()
                .map(str::to_string),
        );
    }
    wanted.sort();
    wanted.dedup();
    assert!(!wanted.is_empty());
    let store = state.store().unwrap();
    let copied = store
        .get_text_embeddings(lrg_ml::siglip::MODEL_ID, &wanted)
        .await
        .unwrap();
    assert_eq!(copied.len(), wanted.len());
    assert_eq!(
        store.count(lrg_store::TEXT_EMBEDDING_TABLE).await.unwrap(),
        wanted.len()
    );
}

/// Photos prepared before the subject pass are ranked on frame-wide sharpness,
/// and the response says so once, with the fix for a model that is not on
/// disk.
#[tokio::test]
async fn cull_says_once_when_the_subject_was_not_measured() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db).await.unwrap();
    seed_burst_measured(&state, 3, true, false).await;
    let json = cull_request(
        app,
        serde_json::json!({"photo_ids": ["burst0", "burst1", "burst2"], "db_path": db}),
    )
    .await;
    let warnings = json["warnings"].as_array().expect("warnings");
    let subject: Vec<&str> = warnings
        .iter()
        .filter_map(serde_json::Value::as_str)
        .filter(|w| w.contains("subject"))
        .collect();
    assert_eq!(subject.len(), 1, "{json}");
    assert!(subject[0].contains("3 photo(s)"), "{}", subject[0]);
    assert!(subject[0].contains("Download AI models"), "{}", subject[0]);
}

/// `culling_preset: "auto"`: each burst is ranked with the preset its photos'
/// genre calls for, the response says which, and a photo without a genre is
/// counted in a warning with the fix.
#[tokio::test]
async fn auto_ranks_each_burst_with_the_preset_its_genre_calls_for() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db).await.unwrap();
    // Two bursts an hour apart: a match, and a coast where one frame has no
    // genre yet.
    let photo = |id: &str, t: i64, hash: &str, sharpness: f64, genre: Option<&str>| {
        let mut meta = serde_json::Map::new();
        meta.insert("filename".into(), serde_json::json!(format!("{id}.jpg")));
        meta.insert("capture_time".into(), serde_json::json!(t));
        meta.insert("cull_phash".into(), serde_json::json!(hash));
        meta.insert("cull_sharpness".into(), serde_json::json!(sharpness));
        meta.insert("cull_exposure".into(), serde_json::json!(0.8));
        meta.insert("cull_noise".into(), serde_json::json!(0.1));
        meta.insert(
            lrg_analysis::grouping::SUBJECT_VERSION_KEY.into(),
            serde_json::json!(lrg_analysis::grouping::SUBJECT_VERSION),
        );
        if let Some(g) = genre {
            meta.insert(lrg_store::meta::GENRE_FIELD.into(), serde_json::json!(g));
        }
        lrg_store::StoreRecord {
            id: id.into(),
            vector: None,
            metadata: meta,
        }
    };
    let t0 = 1_700_000_000;
    state
        .store()
        .unwrap()
        .upsert(
            lrg_store::IMAGE_TABLE,
            &[
                photo("match0", t0, "ffffffffffffffff", 0.9, Some("sport")),
                photo("match1", t0 + 1, "ffffffffffffffff", 0.8, Some("sport")),
                photo("match2", t0 + 2, "ffffffffffffffff", 0.7, Some("portrait")),
                photo(
                    "coast0",
                    t0 + 3600,
                    "0000000000000000",
                    0.9,
                    Some("landscape"),
                ),
                photo("coast1", t0 + 3601, "0000000000000000", 0.8, None),
            ],
        )
        .await
        .unwrap();
    let json = cull_request(
        app,
        serde_json::json!({
            "photo_ids": ["match0", "match1", "match2", "coast0", "coast1"],
            "culling_preset": "auto",
            "use_iqa": false,
            "db_path": db,
        }),
    )
    .await;
    let groups = json["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 2, "{json}");
    let preset_of = |first: &str| {
        groups
            .iter()
            .find(|g| g["photo_ids"][0] == first)
            .map(|g| g["preset"].clone())
            .unwrap()
    };
    assert_eq!(preset_of("match0"), "sports");
    assert_eq!(preset_of("coast0"), "default");
    assert_eq!(json["summary"]["culling_preset"], "auto");
    assert_eq!(json["summary"]["presets"]["sports"], 1);
    assert_eq!(json["summary"]["presets"]["default"], 1);
    let genre: Vec<&str> = json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .filter(|w| w.contains("genre"))
        .collect();
    assert_eq!(genre.len(), 1, "{json}");
    assert!(genre[0].starts_with("1 photo(s)"), "{}", genre[0]);
    assert!(genre[0].contains("Prepare now"), "{}", genre[0]);
}

/// `key_people`: the people named in People and those in many of the photos
/// are the faces a frame is judged by. A guest stands larger in front; the bride
/// behind her blinks in the sharpest frame. Indexing summed the faces up by
/// size, so only the key people see the blink — and the explanation names her.
#[tokio::test]
async fn key_people_judge_the_frame_and_are_named() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db).await.unwrap();
    let store = state.store().unwrap();

    let t0 = 1_700_000_000;
    let photo = |i: i64, sharpness: f64| {
        let mut m = serde_json::Map::new();
        m.insert("filename".into(), serde_json::json!(format!("w{i}.jpg")));
        m.insert("capture_time".into(), serde_json::json!(t0 + i));
        m.insert("cull_phash".into(), serde_json::json!("ffffffffffffffff"));
        m.insert("cull_sharpness".into(), serde_json::json!(sharpness));
        m.insert("cull_exposure".into(), serde_json::json!(0.8));
        m.insert("cull_noise".into(), serde_json::json!(0.1));
        // What indexing stored: the guest's open eyes speak for every frame.
        m.insert("cull_faces_detected".into(), serde_json::json!(2));
        m.insert("cull_face_count".into(), serde_json::json!(1));
        m.insert("cull_faces_present".into(), serde_json::json!(true));
        m.insert("cull_face_score".into(), serde_json::json!(0.6));
        m.insert("cull_eye_openness".into(), serde_json::json!(0.9));
        m.insert("cull_blink_penalty".into(), serde_json::json!(0.1));
        m.insert("cull_eye_closed".into(), serde_json::json!(0.05));
        m.insert("cull_eye_state".into(), serde_json::json!("open"));
        m.insert(
            lrg_analysis::grouping::FACE_STATE_VERSION_KEY.into(),
            serde_json::json!(lrg_analysis::grouping::FACE_STATE_VERSION),
        );
        m.insert(
            lrg_analysis::grouping::FOCUS_VERSION_KEY.into(),
            serde_json::json!(lrg_analysis::grouping::FOCUS_VERSION),
        );
        m.insert(
            lrg_analysis::grouping::SUBJECT_VERSION_KEY.into(),
            serde_json::json!(lrg_analysis::grouping::SUBJECT_VERSION),
        );
        lrg_store::StoreRecord {
            id: format!("w{i}"),
            vector: None,
            metadata: m,
        }
    };
    store
        .upsert(
            lrg_store::IMAGE_TABLE,
            &[photo(0, 0.9), photo(1, 0.8), photo(2, 0.7)],
        )
        .await
        .unwrap();
    let face = |photo: i64, n: usize, person: &str, area: f64, closed: f64| {
        let state = if closed >= 0.6 { "closed" } else { "open" };
        lrg_store::StoreRecord {
            id: format!("w{photo}_{n}"),
            vector: Some(vec![0.0; 512]),
            metadata: serde_json::json!({
                "photo_id": format!("w{photo}"),
                "person_id": person,
                "face_area_ratio": area,
                "face_sharpness": 0.6,
                "face_det_score": 0.95,
                "face_center_proximity": 0.8,
                "face_eye_openness": 1.0 - closed,
                "face_blink_penalty": closed,
                "face_occlusion": 0.1,
                "face_eye_state": state,
                "face_eye_closed": closed,
                "face_smile": 0.3,
                "face_eye_squint": 0.1,
                "face_gaze_off_lens": 5.0,
                "face_mouth_open": 0.1,
            })
            .as_object()
            .unwrap()
            .clone(),
        }
    };
    let mut faces = Vec::new();
    for i in 0..3 {
        faces.push(face(i, 0, "person_2", 0.06, 0.05));
        faces.push(face(
            i,
            1,
            "person_1",
            0.01,
            if i == 0 { 0.9 } else { 0.05 },
        ));
    }
    store.upsert(lrg_store::FACE_TABLE, &faces).await.unwrap();
    let (code, _) = put_json(
        &app,
        &format!("/v1/faces/persons/person_1?db_path={db}"),
        serde_json::json!({"name": "Anna"}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);

    let ask = |key_people: bool| {
        serde_json::json!({
            "photo_ids": ["w0", "w1", "w2"],
            "culling_preset": "event",
            "use_iqa": false,
            "key_people": key_people,
            "db_path": db,
        })
    };
    // By size, the sharpest frame wins: nobody seems to blink.
    let by_size = cull_request(app.clone(), ask(false)).await;
    assert_eq!(by_size["groups"][0]["winner_photo_id"], "w0", "{by_size}");

    let json = cull_request(app, ask(true)).await;
    let group = &json["groups"][0];
    assert_ne!(group["winner_photo_id"], "w0", "{json}");
    let w0 = group["photos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["photo_id"] == "w0")
        .unwrap();
    assert!(
        w0["explanation"]
            .as_str()
            .unwrap()
            .contains("Anna: eyes closed"),
        "{w0}"
    );
    // Both are key people: Anna by name, the guest by being in every frame.
    assert_eq!(
        json["summary"]["key_people"]["named"],
        serde_json::json!(["Anna"])
    );
    assert_eq!(json["summary"]["key_people"]["unnamed"], 1);
    assert_eq!(json["summary"]["key_people"]["groups"], 1);
}

/// The default preset asks the prompts nothing, so a cull on it neither needs
/// nor writes any — and has no text tower to miss.
#[tokio::test]
async fn the_default_cull_reads_no_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db).await.unwrap();
    seed_burst(&state, 3, true).await;
    let json = cull_request(
        app,
        serde_json::json!({"photo_ids": ["burst0", "burst1", "burst2"], "db_path": db}),
    )
    .await;
    assert!(json["warning"].is_null(), "{json}");
    assert_eq!(
        state
            .store()
            .unwrap()
            .count(lrg_store::TEXT_EMBEDDING_TABLE)
            .await
            .unwrap(),
        0
    );
}

/// Seeds two frames that differ in nothing but their aesthetic score (and the
/// heuristic, which prefers the other one), and returns the id the cull picks.
async fn winner_among_equal_frames(scores: [(f64, &str); 2], heuristic: [f64; 2]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db).await.unwrap();
    let records: Vec<lrg_store::StoreRecord> = (0..2)
        .map(|i| {
            let mut m = serde_json::Map::new();
            m.insert("filename".into(), serde_json::json!(format!("f{i}.jpg")));
            m.insert("capture_time".into(), serde_json::json!(1_700_000_000 + i));
            m.insert("cull_phash".into(), serde_json::json!("ffffffffffffffff"));
            m.insert("cull_sharpness".into(), serde_json::json!(0.7));
            m.insert("cull_exposure".into(), serde_json::json!(0.8));
            m.insert("cull_noise".into(), serde_json::json!(0.1));
            m.insert("cull_aesthetic".into(), serde_json::json!(heuristic[i]));
            m.insert("aesthetic_score".into(), serde_json::json!(scores[i].0));
            m.insert("aesthetic_model".into(), serde_json::json!(scores[i].1));
            lrg_store::StoreRecord {
                id: format!("frame{i}"),
                vector: None,
                metadata: m,
            }
        })
        .collect();
    state
        .store()
        .unwrap()
        .upsert(lrg_store::IMAGE_TABLE, &records)
        .await
        .unwrap();
    let json = cull_request(
        app,
        serde_json::json!({"photo_ids": ["frame0", "frame1"], "db_path": db}),
    )
    .await;
    json["groups"][0]["winner_photo_id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The stored model score moves the pick, against the heuristic: frame 0 looks
/// punchier to the heuristic, the model scores frame 1 higher.
#[tokio::test]
async fn cull_prefers_the_frame_the_aesthetic_model_scores_higher() {
    let current = lrg_ml::aesthetic::MODEL_ID;
    let winner = winner_among_equal_frames([(5.0, current), (7.5, current)], [0.9, 0.2]).await;
    assert_eq!(winner, "frame1");
}

/// A score from another export is not comparable with this one's, so it must
/// not decide anything: the heuristic does, and the cull says the scores are
/// missing.
#[tokio::test]
async fn cull_ignores_a_score_from_another_export() {
    let winner = winner_among_equal_frames(
        [(5.0, "another-export"), (7.5, "another-export")],
        [0.9, 0.2],
    )
    .await;
    assert_eq!(winner, "frame0");
}

/// Photos indexed before scoring existed rank on the heuristic and say so, once.
#[tokio::test]
async fn cull_warns_once_about_photos_without_an_aesthetic_score() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db).await.unwrap();
    seed_burst(&state, 3, true).await;
    // Strip the scores the helper adds: these are photos from before scoring.
    let store = state.store().unwrap();
    let mut rows = store
        .get(
            lrg_store::IMAGE_TABLE,
            &[
                "burst0".to_string(),
                "burst1".to_string(),
                "burst2".to_string(),
            ],
        )
        .await
        .unwrap();
    for row in &mut rows {
        row.metadata.remove("aesthetic_score");
        row.metadata.remove("aesthetic_model");
    }
    store.upsert(lrg_store::IMAGE_TABLE, &rows).await.unwrap();

    let json = cull_request(
        app,
        serde_json::json!({"photo_ids": ["burst0", "burst1", "burst2"], "db_path": db}),
    )
    .await;
    assert_eq!(json["status"], "success");
    let warnings = json["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{json}");
    let warning = warnings[0].as_str().unwrap();
    assert!(
        warning.starts_with("3 photo(s) have no aesthetic score"),
        "{warning:?}"
    );
    // The test app has no scorer on disk, so the fix named is the download.
    assert!(warning.contains("Download AI models"), "{warning:?}");
}

/// Binding a database stamps the embedding identity on rows written before
/// the field existed, once, and leaves a row naming another model alone.
#[tokio::test]
async fn binding_stamps_the_embedding_model_on_rows_that_predate_it() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db");
    {
        // What an older build left: a store with no upgrade marker.
        let lance_root = lrg_store::migrate::lance_root_for_db(&db_path);
        let store = lrg_store::Store::open(&lance_root).await.unwrap();
        let row = |id: &str, model: Option<&str>| {
            let mut m = serde_json::Map::new();
            m.insert("has_embedding".into(), serde_json::json!(true));
            if let Some(model) = model {
                m.insert("embedding_model".into(), serde_json::json!(model));
            }
            lrg_store::StoreRecord {
                id: id.into(),
                vector: Some(vec![0.1; 1152]),
                metadata: m,
            }
        };
        store
            .upsert(
                lrg_store::IMAGE_TABLE,
                &[row("old", None), row("other", Some("some-previous-model"))],
            )
            .await
            .unwrap();
    }

    let (_, state) = fresh_app();
    state
        .ensure_db_path(db_path.to_str().unwrap())
        .await
        .unwrap();

    let rows = state
        .store()
        .unwrap()
        .get(
            lrg_store::IMAGE_TABLE,
            &["old".to_string(), "other".to_string()],
        )
        .await
        .unwrap();
    let model_of = |id: &str| {
        rows.iter()
            .find(|r| r.id == id)
            .and_then(|r| r.metadata.get("embedding_model"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    // The earlier model, not this build's: those vectors were made by it, and
    // labelling them as current would keep them from ever being re-embedded.
    assert_eq!(
        model_of("old").as_deref(),
        Some(lrg_ml::siglip::LEGACY_MODEL_ID)
    );
    assert_ne!(model_of("old").as_deref(), Some(lrg_ml::siglip::MODEL_ID));
    assert_eq!(model_of("other").as_deref(), Some("some-previous-model"));
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

    // A prepared row carries a capture time: grouping runs on it, and a row
    // without one is reported again (see `undated` below). It also carries the
    // metrics version it was measured with (see `measured_before_fine_noise`).
    let row = |phash: Option<&str>, faces_present: Option<bool>| {
        let mut m = serde_json::Map::new();
        if let Some(p) = phash {
            m.insert("cull_phash".into(), serde_json::json!(p));
            m.insert(
                "cull_metrics_version".into(),
                serde_json::json!(lrg_imaging::metrics::METRICS_VERSION),
            );
        }
        if let Some(f) = faces_present {
            m.insert("cull_faces_present".into(), serde_json::json!(f));
        }
        m.insert("capture_time".into(), serde_json::json!(1_700_000_000.0));
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
                // Culled by a version whose fast path sent no capture time:
                // the signals are there, the date is not, and it groups
                // wrongly until prepared again.
                lrg_store::StoreRecord {
                    id: "undated".into(),
                    vector: None,
                    metadata: {
                        let mut m = row(Some("abcd0123abcd0126"), Some(true));
                        m.remove("capture_time");
                        m
                    },
                },
                // ...unless neither the catalog nor the file had one, which
                // is stamped so the photo is not asked for on every run.
                lrg_store::StoreRecord {
                    id: "undated_for_good".into(),
                    vector: None,
                    metadata: {
                        let mut m = row(Some("abcd0123abcd0127"), Some(true));
                        m.remove("capture_time");
                        m.insert("capture_time_unknown".into(), serde_json::json!(true));
                        m
                    },
                },
                // Prepared before grain was measured at full working size:
                // measured once more, then never again.
                lrg_store::StoreRecord {
                    id: "measured_before_fine_noise".into(),
                    vector: None,
                    metadata: {
                        let mut m = row(Some("abcd0123abcd0128"), Some(false));
                        m.remove("cull_metrics_version");
                        m
                    },
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
                            "undated", "undated_for_good", "measured_before_fine_noise",
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
        serde_json::json!([
            "needs_face_pass",
            "needs_everything",
            "unknown",
            "undated",
            "measured_before_fine_noise"
        ]),
        "a faceless photo that has been culled must not be reported forever, and \
         neither is one whose capture time is known to be unavailable"
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
// /v1/edit/score
//
// The plugin's aesthetic refinement loop scores each candidate render through
// this route. Its two failure shapes decide what the user is told, so both are
// pinned: a request with nothing to score, and a backend without the model.
// ---------------------------------------------------------------------------

async fn score_request(
    app: axum::Router,
    image: Option<(&str, &[u8])>,
) -> (StatusCode, serde_json::Value) {
    let (content_type, body) = multipart_body(&[], image);
    let response = app
        .oneshot(
            Request::post("/v1/edit/score")
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
async fn edit_score_rejects_a_request_without_an_image() {
    let (app, _) = fresh_app();
    let (status, json) = score_request(app, None).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["status"], "error");
    assert!(json["error"].is_string(), "{json}");
}

#[tokio::test]
async fn edit_score_names_the_missing_model_and_what_to_do_about_it() {
    let (app, _) = fresh_app();
    let (status, json) = score_request(app, Some(("render.jpg", b"not-a-real-jpeg"))).await;

    // Not a 500: nothing is broken, the model just is not on disk. The plugin
    // shows this string as it stands, so it has to be the instruction.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    assert_eq!(json["status"], "error");
    let error = json["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("aesthetic model") && error.contains("Download AI models"),
        "{json}"
    );
}

// ---------------------------------------------------------------------------
// Analysis depth (`reasoning_effort`): a value that is not a level is refused
// by name rather than quietly run — and billed — at another level.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn index_by_path_refuses_an_unknown_analysis_depth() {
    let (app, _) = fresh_app();
    let body = serde_json::json!({
        "images": [{"path": "/nonexistent/photo1.jpg", "photo_id": "photo1"}],
        "tasks": ["metadata"],
        "reasoning_effort": "max",
    });
    let response = app
        .oneshot(
            Request::post("/v1/index/photos/by-path")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    let error = json["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("reasoning_effort") && error.contains("max"),
        "{json}"
    );
}

#[tokio::test]
async fn edit_routes_refuse_an_unknown_analysis_depth() {
    let (app, _) = fresh_app();
    let fields = [("photo_id", "photo1"), ("reasoning_effort", "extreme")];
    let image = Some(("photo1.jpg", b"not-a-real-jpeg".as_slice()));

    let (status, json) = style_edit_request(app.clone(), &fields, image).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");

    let (content_type, body) = multipart_body(&fields, image);
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/edit/recipe")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = serde_json::json!({
        "image": "AAAA",
        "photo_id": "photo1",
        "filename": "photo1.jpg",
        "reasoning_effort": "extreme",
    });
    let response = app
        .oneshot(
            Request::post("/v1/edit/recipe/base64")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
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
/// One person's row with readings on it, for the questions asked of a person.
fn read_face(
    id: &str,
    photo: &str,
    eyes: &str,
    smile: f64,
    gaze: f64,
    yaw: f64,
) -> lrg_store::StoreRecord {
    let mut row = people_face(id, photo, "person_0", 0.0);
    let m = &mut row.metadata;
    m.insert("face_eye_state".into(), serde_json::json!(eyes));
    m.insert("face_smile".into(), serde_json::json!(smile));
    m.insert("face_gaze_off_lens".into(), serde_json::json!(gaze));
    m.insert("face_yaw".into(), serde_json::json!(yaw));
    row
}

async fn read_people_fixture() -> (axum::Router, tempfile::TempDir, String) {
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
                read_face("laugh_0", "laugh", "open", 0.9, 6.0, 3.0),
                read_face("neutral_0", "neutral", "open", 0.1, 8.0, 5.0),
                read_face("away_0", "away", "open", 0.8, 38.0, 35.0),
                read_face("blink_0", "blink", "closed", 0.7, 6.0, 2.0),
                // Indexed before the model: no reading to ask.
                people_face("old_0", "old", "person_0", 0.0),
                // Somebody else's, whatever it reads.
                {
                    let mut other = read_face("b1_0", "b1", "open", 0.9, 5.0, 1.0);
                    other
                        .metadata
                        .insert("person_id".into(), serde_json::json!("person_1"));
                    other
                },
            ],
        )
        .await
        .unwrap();
    (app, dir, db_path)
}

#[tokio::test]
async fn a_person_s_photos_answer_what_is_asked_of_their_face() {
    let (app, _dir, db) = read_people_fixture().await;
    let ask = |query: &str| format!("/v1/faces/persons/person_0/photos?db_path={db}{query}");
    let ids = |json: &serde_json::Value| {
        json["photo_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };

    // Nothing asked: every photo, and no complaint about the old one.
    let json = get_json(&app, &ask("")).await;
    assert_eq!(ids(&json), ["away", "blink", "laugh", "neutral", "old"]);
    assert_eq!(json["not_judged"], 0);
    assert_eq!(json["warnings"], serde_json::json!([]));

    let json = get_json(&app, &ask("&eyes_open=true")).await;
    assert_eq!(ids(&json), ["away", "laugh", "neutral"]);
    // The one that could not be judged is counted and named as fixable.
    assert_eq!(json["not_judged"], 1);
    let warning = json["warnings"][0].as_str().unwrap();
    assert!(warning.contains("Download AI models"), "{warning}");

    let json = get_json(&app, &ask("&smiling=true&eyes_open=true")).await;
    assert_eq!(ids(&json), ["away", "laugh"]);

    // Shut eyes look nowhere, whatever `eyeLook*` read.
    let json = get_json(&app, &ask("&looking_at_camera=true&smiling=true")).await;
    assert_eq!(ids(&json), ["laugh"]);
}

#[tokio::test]
async fn a_person_s_best_photos_are_the_ones_with_eyes_open_facing_the_camera() {
    let (app, _dir, db) = read_people_fixture().await;
    let json = get_json(
        &app,
        &format!("/v1/faces/persons/person_0/photos?db_path={db}&sort=best&limit=2"),
    )
    .await;
    assert_eq!(json["photo_ids"], serde_json::json!(["laugh", "neutral"]));
}

/// The picture on a person's card, and the first face in their sheet when
/// sorted for it, is not the sharpest blink.
#[tokio::test]
async fn a_person_s_card_does_not_show_them_blinking() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db).await.unwrap();
    let mut blink = read_face("blink_0", "blink", "closed", 0.1, 5.0, 1.0);
    blink
        .metadata
        .insert("face_sharpness".into(), serde_json::json!(0.95));
    blink
        .metadata
        .insert("thumbnail".into(), serde_json::json!("BLINK"));
    let mut open = read_face("open_0", "open", "open", 0.3, 6.0, 4.0);
    open.metadata
        .insert("face_sharpness".into(), serde_json::json!(0.5));
    open.metadata
        .insert("thumbnail".into(), serde_json::json!("OPEN"));
    state
        .store()
        .unwrap()
        .upsert(lrg_store::FACE_TABLE, &[blink, open])
        .await
        .unwrap();

    let persons = get_json(&app, &format!("/v1/faces/persons?db_path={db}")).await;
    assert_eq!(persons["persons"][0]["thumbnail"], "OPEN", "{persons}");
    let thumb = get_json(
        &app,
        &format!("/v1/faces/persons/person_0/thumbnail?db_path={db}"),
    )
    .await;
    assert_eq!(thumb["thumbnail"], "OPEN");
    let faces = get_json(
        &app,
        &format!("/v1/faces/persons/person_0/faces?db_path={db}&sort=portrait"),
    )
    .await;
    assert_eq!(faces["faces"][0]["face_id"], "open_0");
}

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

/// Addresses nothing listens on, so the probes answer the same on every
/// machine.
fn models_body(extra: serde_json::Value) -> Body {
    let mut body = serde_json::json!({
        "ollama_base_url": "http://127.0.0.1:1",
        "lmstudio_base_url": "127.0.0.1:1",
    });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    Body::from(body.to_string())
}

#[tokio::test]
async fn provider_models_list_every_provider_and_no_warnings_by_default() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::post("/v1/llm/providers/models")
                .header("content-type", "application/json")
                .body(models_body(serde_json::json!({})))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    for provider in [
        "llamacpp",
        "mlx",
        "ollama",
        "lmstudio",
        "openai_compatible",
        "chatgpt",
        "gemini",
        "anthropic",
    ] {
        assert!(
            json["models"][provider].is_array(),
            "{provider} missing: {json}"
        );
    }
    assert_eq!(json["warnings"], serde_json::json!([]));
    assert_eq!(json["servers"], serde_json::json!({}));
    assert!(json["aliases"]["mlx"].is_object(), "{json}");
}

/// Only probed providers stay quiet when they are not there. The user's own
/// server is one they entered, so failing to reach it has to be reported.
#[tokio::test]
async fn an_unreachable_server_the_user_entered_is_a_warning() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::post("/v1/llm/providers/models")
                .header("content-type", "application/json")
                .body(models_body(
                    serde_json::json!({"server_url": "127.0.0.1:1", "server_apikey": "sk-x"}),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["models"]["openai_compatible"], serde_json::json!([]));
    assert_eq!(json["servers"]["openai_compatible"]["label"], "127.0.0.1:1");
    let warnings = json["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{json}");
    let warning = warnings[0].as_str().unwrap();
    assert!(
        warning.contains("Other AI server") && warning.contains("Could not reach"),
        "{warning}"
    );
}

#[tokio::test]
async fn a_malformed_server_address_is_a_warning_not_a_failure() {
    let (app, _) = fresh_app();
    let response = app
        .oneshot(
            Request::post("/v1/llm/providers/models")
                .header("content-type", "application/json")
                .body(models_body(
                    serde_json::json!({"server_url": "ftp://example.com"}),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let warning = json["warnings"][0].as_str().unwrap_or_default();
    assert!(warning.contains("http:// or https://"), "{json}");
}

/// A fresh app per request, for the download routes below.
async fn post_fresh(path: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let (app, _) = fresh_app();
    post_json(&app, path, body).await
}

/// A typo is reported before anything else — even on a machine that could not
/// run the model — and never reaches Hugging Face.
#[tokio::test]
async fn a_model_name_that_is_not_one_is_refused_up_front() {
    for path in ["/v1/llm/downloads/check", "/v1/llm/downloads"] {
        let (status, json) =
            post_fresh(path, serde_json::json!({"repo": "gemma3", "engine": "mlx"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {json}");
        assert!(
            json["error"]
                .as_str()
                .unwrap_or_default()
                .contains("not a Hugging Face model name"),
            "{path}: {json}"
        );
    }
}

#[tokio::test]
async fn a_check_needs_a_repo_and_a_known_engine() {
    let (status, json) = post_fresh(
        "/v1/llm/downloads/check",
        serde_json::json!({"engine": "mlx"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"], "repo is required.");

    let (status, json) = post_fresh(
        "/v1/llm/downloads/check",
        serde_json::json!({"repo": "org/model", "engine": "onnx"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(json["error"].as_str().unwrap().contains("engine must be"));
}

// ---------------------------------------------------------------------------
// /v1/photos/catalogs/cleanup
//
// The plugin runs this unattended, so the two things it must never do are
// remove a photo another catalog still has and act on an inventory that lost
// most of the catalog.
// ---------------------------------------------------------------------------

async fn seed_catalog_rows(state: &Arc<AppState>, rows: &[(&str, &[&str])]) {
    let records: Vec<lrg_store::StoreRecord> = rows
        .iter()
        .map(|(id, catalogs)| {
            let set = catalogs.iter().map(|c| c.to_string()).collect();
            let mut m = serde_json::Map::new();
            m.insert(
                "catalog_ids".into(),
                serde_json::json!(lrg_store::meta::serialize_catalog_ids(&set)),
            );
            lrg_store::StoreRecord {
                id: (*id).into(),
                vector: Some(vec![0.1; 1152]),
                metadata: m,
            }
        })
        .collect();
    state
        .store()
        .unwrap()
        .upsert(lrg_store::IMAGE_TABLE, &records)
        .await
        .unwrap();
}

async fn catalogs_of(state: &Arc<AppState>, id: &str) -> Vec<String> {
    let rows = state
        .store()
        .unwrap()
        .get_meta(lrg_store::IMAGE_TABLE, &[id.to_string()])
        .await
        .unwrap();
    lrg_store::meta::parse_catalog_ids(&rows[0].1)
        .into_iter()
        .collect()
}

#[tokio::test]
async fn cleanup_releases_deleted_photos_and_reports_what_waits_for_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path).await.unwrap();
    seed_catalog_rows(
        &state,
        &[
            ("kept", &["cat"]),
            ("deleted", &["cat"]),
            ("shared", &["cat", "other"]),
            ("came_back", &[]),
        ],
    )
    .await;

    let (status, json) = post_json(
        &app,
        "/v1/photos/catalogs/cleanup",
        serde_json::json!({
            "catalog_id": "cat",
            "photo_ids": ["kept", "came_back"],
            "db_path": db_path,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["checked"], 3, "{json}");
    assert_eq!(json["disassociated"], 2, "{json}");
    assert_eq!(json["orphaned"], 1, "{json}");
    assert_eq!(json["readopted"], 1, "{json}");
    assert_eq!(json["purged"], 0, "{json}");
    assert_eq!(json["waiting"], 1, "{json}");
    assert_eq!(json["grace_days"], 30, "{json}");
    assert!(json["next_purge_at"].as_i64().is_some(), "{json}");
    assert_eq!(json["warnings"], serde_json::json!([]), "{json}");

    assert!(catalogs_of(&state, "deleted").await.is_empty());
    assert_eq!(catalogs_of(&state, "shared").await, vec!["other"]);
    assert_eq!(catalogs_of(&state, "came_back").await, vec!["cat"]);
    assert_eq!(catalogs_of(&state, "kept").await, vec!["cat"]);
}

#[tokio::test]
async fn cleanup_holds_back_a_removal_of_most_of_the_catalog_until_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path).await.unwrap();
    let ids: Vec<String> = (0..60).map(|i| format!("p{i}")).collect();
    let rows: Vec<(&str, &[&str])> = ids.iter().map(|id| (id.as_str(), &["cat"][..])).collect();
    seed_catalog_rows(&state, &rows).await;

    // What an inventory resolved under another identity looks like: none of
    // the catalog's ids.
    let request = |confirm: bool| {
        serde_json::json!({
            "catalog_id": "cat",
            "photo_ids": ["unknown_1", "unknown_2"],
            "confirm_large_removal": confirm,
            "db_path": db_path,
        })
    };
    let (status, json) = post_json(&app, "/v1/photos/catalogs/cleanup", request(false)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["needs_confirmation"], true, "{json}");
    assert_eq!(json["would_disassociate"], 60, "{json}");
    assert_eq!(json["checked"], 60, "{json}");
    assert!(json["error"]
        .as_str()
        .unwrap()
        .contains("Nothing was removed"));
    assert_eq!(catalogs_of(&state, "p0").await, vec!["cat"]);

    let (status, json) = post_json(&app, "/v1/photos/catalogs/cleanup", request(true)).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["disassociated"], 60, "{json}");
    assert!(catalogs_of(&state, "p0").await.is_empty());
}

/// Indexing stores a small preview of the photo for the browser search page;
/// `/v1/photos/thumbnail` serves it, the search marks which results have one,
/// and removing the photo removes the preview with it.
#[tokio::test]
async fn indexing_stores_a_preview_that_search_reports_and_remove_deletes() {
    let (app, state) = fresh_app();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    state.ensure_db_path(&db_path).await.unwrap();

    let jpeg = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/astronaut.jpg"
    ))
    .unwrap();
    let (content_type, body) = multipart_body(
        &[
            ("photo_id", "astro"),
            ("tasks", "cull"),
            ("catalog_id", "cat-1"),
            ("db_path", db_path.as_str()),
        ],
        Some(("astronaut.jpg", &jpeg)),
    );
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/index/photos")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let indexed = body_json(response).await;
    assert_eq!(indexed["success_count"], 1, "{indexed}");
    // The preview is not one of the run's warnings: it was made.
    assert!(
        !indexed["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("preview")),
        "{indexed}"
    );

    let thumb = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/v1/photos/thumbnail?photo_id=astro&db_path={db_path}"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(thumb.status(), StatusCode::OK);
    assert_eq!(
        thumb
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "image/jpeg"
    );
    let bytes = thumb.into_body().collect().await.unwrap().to_bytes();
    let preview = image::load_from_memory(&bytes).unwrap();
    assert_eq!(
        preview.width().max(preview.height()),
        lrg_imaging::convert::THUMBNAIL_LONG_EDGE
    );

    // A photo with no preview is a 404 the page can recognise.
    let none = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/v1/photos/thumbnail?photo_id=nope&db_path={db_path}"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(none.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(none).await["error_code"], "no_thumbnail");

    // "The best N" ranks by aesthetic score, which this test has none of, so
    // give the photo one and a second photo without a preview.
    let store = state.store().unwrap();
    let mut row = store
        .get(lrg_store::IMAGE_TABLE, &["astro".to_string()])
        .await
        .unwrap()
        .pop()
        .unwrap();
    row.metadata
        .insert("aesthetic_score".into(), serde_json::json!(6.0));
    row.metadata.insert(
        "aesthetic_model".into(),
        serde_json::json!(lrg_ml::aesthetic::MODEL_ID),
    );
    store.upsert(lrg_store::IMAGE_TABLE, &[row]).await.unwrap();
    seed_searchable(
        &state,
        &[(
            "old",
            Some(7.0),
            lrg_ml::aesthetic::MODEL_ID,
            None,
            "indexed before previews",
        )],
    )
    .await;
    let (_, found) = post_json(
        &app,
        "/v1/search",
        serde_json::json!({"sort": "aesthetic", "db_path": db_path}),
    )
    .await;
    let has = |id: &str| {
        found["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["photo_id"] == id)
            .map(|r| r["has_thumbnail"].clone())
    };
    assert_eq!(has("astro"), Some(serde_json::json!(true)), "{found}");
    assert_eq!(has("old"), Some(serde_json::json!(false)), "{found}");

    let (code, _) = post_json(
        &app,
        "/v1/photos/remove",
        serde_json::json!({"photo_id": "astro", "db_path": db_path}),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert!(!lrg_api::thumbnails::exists(
        &state.lance_root().unwrap(),
        "astro"
    ));
}

/// Photos indexed before previews were stored get one without being indexed
/// again: `/v1/photos/thumbnails/missing` lists them (catalog-scoped, and only
/// photos that are indexed), `POST /v1/photos/thumbnail` stores a preview
/// scaled to the indexing size, and a photo with no index row is refused.
#[tokio::test]
async fn missing_previews_are_listed_and_filled_without_reindexing() {
    let (app, state) = fresh_app();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    state.ensure_db_path(&db_path).await.unwrap();
    seed_searchable(
        &state,
        &[
            ("old", None, "", None, "indexed before previews"),
            ("other", None, "", None, "another catalog's photo"),
            ("done", None, "", None, "already has one"),
        ],
    )
    .await;
    let store = state.store().unwrap();
    let mut rows = store
        .get(
            lrg_store::IMAGE_TABLE,
            &["old".to_string(), "other".to_string(), "done".to_string()],
        )
        .await
        .unwrap();
    for row in &mut rows {
        let catalog = if row.id == "other" { "cat-2" } else { "cat-1" };
        row.metadata
            .insert("catalog_ids".into(), serde_json::json!([catalog]));
    }
    store.upsert(lrg_store::IMAGE_TABLE, &rows).await.unwrap();
    let root = state.lance_root().unwrap();
    lrg_api::thumbnails::write(&root, "done", b"jpeg").unwrap();

    let (status, listed) = post_json(
        &app,
        "/v1/photos/thumbnails/missing",
        serde_json::json!({"catalog_id": "cat-1", "db_path": db_path}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["photo_ids"], serde_json::json!(["old"]), "{listed}");
    assert_eq!(listed["indexed"], 2, "{listed}");

    // Narrowed to a selection: a photo that is not indexed is not listed.
    let (_, selection) = post_json(
        &app,
        "/v1/photos/thumbnails/missing",
        serde_json::json!({"photo_ids": ["other", "never-indexed"], "db_path": db_path}),
    )
    .await;
    assert_eq!(
        selection["photo_ids"],
        serde_json::json!(["other"]),
        "{selection}"
    );

    let jpeg = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/astronaut.jpg"
    ))
    .unwrap();
    let upload = |id: &'static str| {
        let (content_type, body) = multipart_body(
            &[("photo_id", id), ("db_path", db_path.as_str())],
            Some(("preview.jpg", &jpeg)),
        );
        Request::post("/v1/photos/thumbnail")
            .header("content-type", content_type)
            .body(Body::from(body))
            .unwrap()
    };
    let stored = app.clone().oneshot(upload("old")).await.unwrap();
    assert_eq!(stored.status(), StatusCode::OK);
    let preview =
        image::load_from_memory(&lrg_api::thumbnails::read(&root, "old").unwrap()).unwrap();
    assert_eq!(
        preview.width().max(preview.height()),
        lrg_imaging::convert::THUMBNAIL_LONG_EDGE
    );

    let refused = app.clone().oneshot(upload("never-indexed")).await.unwrap();
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(refused).await["error_code"], "not_indexed");
    assert!(!lrg_api::thumbnails::exists(&root, "never-indexed"));

    let (_, after) = post_json(
        &app,
        "/v1/photos/thumbnails/missing",
        serde_json::json!({"catalog_id": "cat-1", "db_path": db_path}),
    )
    .await;
    assert_eq!(after["photo_ids"], serde_json::json!([]), "{after}");
}

/// The face recognizer is FaceNet until the user picks AuraFace, and AuraFace
/// cannot be picked before its model is on disk: a switch that fails on the
/// next photo would leave every run reporting a missing model.
#[tokio::test]
async fn auraface_cannot_be_chosen_before_it_is_downloaded() {
    let (app, state) = fresh_app();
    let status = get_json(&app, "/v1/faces/recognizer").await;
    assert_eq!(status["active"], "facenet", "{status}");
    let ids: Vec<&str> = status["recognizers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["facenet", "auraface"]);
    assert_eq!(status["recognizers"][1]["ready"], false, "{status}");
    assert_eq!(status["recognizers"][1]["downloadable"], true, "{status}");

    let (code, refused) = post_json(
        &app,
        "/v1/faces/recognizer",
        serde_json::json!({"recognizer": "auraface"}),
    )
    .await;
    assert_eq!(code, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error_code"], "not_downloaded");
    assert_eq!(state.face.recognizer(), lrg_ml::faces::Recognizer::FaceNet);
    assert_eq!(state.face.model_id(), lrg_ml::faces::MODEL_ID);

    let (bad, _) = post_json(
        &app,
        "/v1/faces/recognizer",
        serde_json::json!({"recognizer": "arcface"}),
    )
    .await;
    assert_eq!(bad, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Eye and face focus
// ---------------------------------------------------------------------------

/// A photo with faces prepared before eye and face focus were measured is
/// prepared once more; one that went through the pass is not, even when its
/// faces were too small to measure, and a faceless photo never is.
#[tokio::test]
async fn check_unprocessed_remeasures_faces_prepared_before_eye_focus() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();

    let row = |id: &str, faces: i64, version: bool, eye_focus: Option<f64>| {
        let mut m = serde_json::Map::new();
        m.insert("cull_phash".into(), serde_json::json!("abcd0123abcd0123"));
        m.insert("cull_faces_present".into(), serde_json::json!(faces > 0));
        m.insert("cull_face_count".into(), serde_json::json!(faces));
        m.insert("capture_time".into(), serde_json::json!(1_700_000_000.0));
        m.insert(
            "cull_metrics_version".into(),
            serde_json::json!(lrg_imaging::metrics::METRICS_VERSION),
        );
        if version {
            m.insert("cull_focus_version".into(), serde_json::json!(1));
        }
        if let Some(v) = eye_focus {
            m.insert("cull_eye_focus".into(), serde_json::json!(v));
        }
        lrg_store::StoreRecord {
            id: id.into(),
            vector: None,
            metadata: m,
        }
    };
    state
        .store()
        .unwrap()
        .upsert(
            lrg_store::IMAGE_TABLE,
            &[
                row("faces_before_focus", 2, false, None),
                row("faces_measured", 1, true, Some(0.66)),
                row("faces_too_small", 3, true, None),
                row("faceless_before_focus", 0, false, None),
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
                            "faces_before_focus", "faces_measured",
                            "faces_too_small", "faceless_before_focus",
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
    assert_eq!(json["photo_ids"], serde_json::json!(["faces_before_focus"]));
}

/// Two portrait frames, focus on the eyes and focus missed: the response
/// carries the readings, the reason in words and the 0–10 the plugin writes
/// into the explanation. A third photo prepared before focus existed is named
/// in a warning, with the fix.
#[tokio::test]
async fn cull_ranks_on_eye_focus_and_names_photos_without_it() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();

    let frame = |id: &str, t: i64, eye: Option<f64>, measured: bool| {
        let mut m = serde_json::Map::new();
        m.insert("filename".into(), serde_json::json!(format!("{id}.jpg")));
        m.insert("capture_time".into(), serde_json::json!(t));
        m.insert("cull_phash".into(), serde_json::json!("ffffffffffffffff"));
        m.insert("cull_sharpness".into(), serde_json::json!(0.6));
        m.insert("cull_sharpness_peak".into(), serde_json::json!(0.6));
        m.insert("cull_exposure".into(), serde_json::json!(0.7));
        m.insert("cull_noise".into(), serde_json::json!(0.1));
        m.insert("cull_face_count".into(), serde_json::json!(1));
        m.insert("cull_face_score".into(), serde_json::json!(0.6));
        m.insert("cull_eye_openness".into(), serde_json::json!(0.8));
        m.insert("cull_blink_penalty".into(), serde_json::json!(0.2));
        m.insert("cull_occlusion".into(), serde_json::json!(0.1));
        m.insert("aesthetic_score".into(), serde_json::json!(6.0));
        m.insert(
            "aesthetic_model".into(),
            serde_json::json!(lrg_ml::aesthetic::MODEL_ID),
        );
        if measured {
            m.insert("cull_focus_version".into(), serde_json::json!(1));
        }
        if let Some(v) = eye {
            m.insert("cull_eye_focus".into(), serde_json::json!(v));
            m.insert("cull_face_focus".into(), serde_json::json!(0.5));
        }
        lrg_store::StoreRecord {
            id: id.into(),
            vector: Some(vec![0.5f32; 1152]),
            metadata: m,
        }
    };
    state
        .store()
        .unwrap()
        .upsert(
            lrg_store::IMAGE_TABLE,
            &[
                frame("missed", 1_700_000_000, Some(0.40), true),
                frame("sharp", 1_700_000_001, Some(0.69), true),
                // Far outside the burst window: its own group.
                frame("older", 1_700_009_000, None, false),
            ],
        )
        .await
        .unwrap();

    let json = cull_request(
        app,
        serde_json::json!({
            "photo_ids": ["missed", "sharp", "older"],
            "culling_preset": "portrait",
            "db_path": db_path_str,
        }),
    )
    .await;

    let groups = json["groups"].as_array().unwrap();
    let burst = groups
        .iter()
        .find(|g| g["group_size"] == 2)
        .expect("the two frames form one group");
    assert_eq!(burst["winner_photo_id"], "sharp");
    let photo = |id: &str| {
        burst["photos"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["photo_id"] == id)
            .unwrap()
            .clone()
    };
    let missed = photo("missed");
    assert_eq!(missed["metrics"]["eye_focus"], 0.4);
    assert_eq!(missed["reject_candidate"], true);
    assert!(missed["reason_codes"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("eyes_out_of_focus")));
    let explanation = photo("sharp")["explanation"].as_str().unwrap().to_string();
    assert!(
        explanation.contains("eye focus 10/10 within the group"),
        "{explanation}"
    );

    let warnings = json["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| w
            .as_str()
            .unwrap()
            .starts_with("1 photo(s) with faces were prepared before eye and face focus")),
        "{warnings:?}"
    );
}

// ---------------------------------------------------------------------------
// Face state (eye state, smile, gaze)
// ---------------------------------------------------------------------------

/// A family of its own in the asset status, so the Plug-in Manager offers it
/// and face detection never reports "not ready" because of it.
#[tokio::test]
async fn the_face_state_model_is_its_own_downloadable_family() {
    let (app, _state) = fresh_app();
    let status = get_json(&app, "/v1/models/assets").await;
    let family = status["families"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "face_state")
        .expect("face_state family listed");
    assert_eq!(family["ready"], false);
    assert_eq!(family["downloadable"], true);
    assert!(family["approx_bytes"].as_u64().unwrap() > 0);
    assert_eq!(status["ready"], false);
}

/// Photos with faces prepared before the face state model was on disk are
/// prepared again once it is, and not before: re-preparing without the model
/// would change nothing.
#[tokio::test]
async fn check_unprocessed_asks_for_face_state_only_with_the_model() {
    let row = |id: &str, faces: i64, state: Option<i64>| {
        let mut m = serde_json::Map::new();
        m.insert("cull_phash".into(), serde_json::json!("abcd0123abcd0123"));
        m.insert("cull_faces_present".into(), serde_json::json!(faces > 0));
        m.insert("cull_face_count".into(), serde_json::json!(faces));
        m.insert("capture_time".into(), serde_json::json!(1_700_000_000.0));
        m.insert(
            "cull_metrics_version".into(),
            serde_json::json!(lrg_imaging::metrics::METRICS_VERSION),
        );
        m.insert("cull_focus_version".into(), serde_json::json!(1));
        if let Some(version) = state {
            m.insert("cull_face_state_version".into(), serde_json::json!(version));
        }
        lrg_store::StoreRecord {
            id: id.into(),
            vector: None,
            metadata: m,
        }
    };
    let rows = [
        row("faces_without_state", 2, None),
        row(
            "faces_with_state",
            1,
            Some(lrg_analysis::grouping::FACE_STATE_VERSION),
        ),
        row(
            "faces_with_older_state",
            1,
            Some(lrg_analysis::grouping::FACE_STATE_VERSION - 1),
        ),
        row("faceless", 0, None),
    ];
    let ask = |app: axum::Router, db: String| async move {
        let response = app
            .oneshot(
                Request::post("/v1/index/unprocessed")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "photo_ids": [
                                "faces_without_state",
                                "faces_with_state",
                                "faces_with_older_state",
                                "faceless",
                            ],
                            "tasks": "cull",
                            "regenerate_metadata": "false",
                            "db_path": db,
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response).await["photo_ids"].clone()
    };

    // Without the model: nothing to do.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (without_model, state) = fresh_app();
    state.ensure_db_path(&db).await.unwrap();
    state
        .store()
        .unwrap()
        .upsert(lrg_store::IMAGE_TABLE, &rows)
        .await
        .unwrap();
    assert_eq!(ask(without_model, db).await, serde_json::json!([]));

    // With the model on disk (only its presence is checked here).
    let models = tempfile::tempdir().unwrap();
    std::fs::write(models.path().join("face_mesh_v2.onnx"), b"x").unwrap();
    std::fs::write(models.path().join("face_blendshapes_v2.onnx"), b"x").unwrap();
    let mut paths = models_absent();
    paths.face_state = lrg_ml::face_state::FaceStatePaths {
        mesh_onnx: models.path().join("face_mesh_v2.onnx"),
        blendshapes_onnx: models.path().join("face_blendshapes_v2.onnx"),
    };
    let state = Arc::new(AppState::with_model_paths(None, false, paths));
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    state.ensure_db_path(&db).await.unwrap();
    state
        .store()
        .unwrap()
        .upsert(lrg_store::IMAGE_TABLE, &rows)
        .await
        .unwrap();
    assert_eq!(
        ask(app(state.clone()), db).await,
        serde_json::json!(["faces_without_state", "faces_with_older_state"])
    );
}

/// A photo goes back through the full face pass when its readings are out of date
/// or when a cull pass could not bring its face rows along. Its own readings being
/// current is no help then, and People would otherwise never be able to read it.
#[tokio::test]
async fn check_unprocessed_asks_the_face_pass_for_rows_left_behind() {
    let models = tempfile::tempdir().unwrap();
    std::fs::write(models.path().join("face_mesh_v2.onnx"), b"x").unwrap();
    std::fs::write(models.path().join("face_blendshapes_v2.onnx"), b"x").unwrap();
    let mut paths = models_absent();
    paths.face_state = lrg_ml::face_state::FaceStatePaths {
        mesh_onnx: models.path().join("face_mesh_v2.onnx"),
        blendshapes_onnx: models.path().join("face_blendshapes_v2.onnx"),
    };
    let state = Arc::new(AppState::with_model_paths(None, false, paths));
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    state.ensure_db_path(&db).await.unwrap();
    let row = |id: &str, version: i64, behind: bool| {
        let mut m = serde_json::Map::new();
        // Everything else a face run asks about is done, so only the face state
        // can bring a photo back.
        m.insert("cull_phash".into(), serde_json::json!("abcd0123abcd0123"));
        m.insert("cull_face_count".into(), serde_json::json!(1));
        m.insert("cull_face_state_version".into(), serde_json::json!(version));
        m.insert("faces_checked".into(), serde_json::json!(true));
        m.insert(
            "face_model".into(),
            serde_json::json!(lrg_ml::faces::MODEL_ID),
        );
        if behind {
            m.insert("face_rows_behind".into(), serde_json::json!(true));
        }
        lrg_store::StoreRecord {
            id: id.into(),
            vector: None,
            metadata: m,
        }
    };
    let current = lrg_analysis::grouping::FACE_STATE_VERSION;
    state
        .store()
        .unwrap()
        .upsert(
            lrg_store::IMAGE_TABLE,
            &[
                row("up_to_date", current, false),
                row("older", current - 1, false),
                row("behind", current, true),
            ],
        )
        .await
        .unwrap();
    let (status, json) = post_json(
        &app(state.clone()),
        "/v1/index/unprocessed",
        serde_json::json!({
            "photo_ids": ["up_to_date", "older", "behind"],
            "tasks": "faces",
            "regenerate_metadata": "false",
            "db_path": db,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["photo_ids"], serde_json::json!(["older", "behind"]));
}

/// The model's reading replaces the proxy: the blinking frame is a reject with
/// the plain-language reason, its reading is in the metrics, and the frame the
/// proxy also called "closed" is not.
#[tokio::test]
async fn cull_names_a_real_blink_from_the_face_state() {
    let dir = tempfile::tempdir().unwrap();
    let db_path_str = dir.path().join("lrgenius.db").to_str().unwrap().to_string();
    let (app, state) = fresh_app();
    state.ensure_db_path(&db_path_str).await.unwrap();

    let frame = |id: &str, t: i64, closed: f64| {
        let mut m = serde_json::Map::new();
        m.insert("filename".into(), serde_json::json!(format!("{id}.jpg")));
        m.insert("capture_time".into(), serde_json::json!(t));
        m.insert("cull_phash".into(), serde_json::json!("ffffffffffffffff"));
        m.insert("cull_sharpness".into(), serde_json::json!(0.6));
        m.insert("cull_sharpness_peak".into(), serde_json::json!(0.6));
        m.insert("cull_exposure".into(), serde_json::json!(0.7));
        m.insert("cull_noise".into(), serde_json::json!(0.1));
        m.insert("cull_face_count".into(), serde_json::json!(1));
        m.insert("cull_face_score".into(), serde_json::json!(0.6));
        // What the proxy said on the real shoot: closed, everywhere.
        m.insert("cull_eye_openness".into(), serde_json::json!(0.1));
        m.insert("cull_blink_penalty".into(), serde_json::json!(0.9));
        m.insert("cull_occlusion".into(), serde_json::json!(0.1));
        m.insert("cull_focus_version".into(), serde_json::json!(1));
        m.insert(
            "cull_face_state_version".into(),
            serde_json::json!(lrg_analysis::grouping::FACE_STATE_VERSION),
        );
        m.insert("cull_eye_closed".into(), serde_json::json!(closed));
        m.insert("cull_smile".into(), serde_json::json!(0.4));
        m.insert("aesthetic_score".into(), serde_json::json!(6.0));
        m.insert(
            "aesthetic_model".into(),
            serde_json::json!(lrg_ml::aesthetic::MODEL_ID),
        );
        lrg_store::StoreRecord {
            id: id.into(),
            vector: Some(vec![0.5f32; 1152]),
            metadata: m,
        }
    };
    state
        .store()
        .unwrap()
        .upsert(
            lrg_store::IMAGE_TABLE,
            &[
                frame("open", 1_700_000_000, 0.04),
                frame("blink", 1_700_000_001, 0.81),
            ],
        )
        .await
        .unwrap();

    let json = cull_request(
        app,
        serde_json::json!({
            "photo_ids": ["open", "blink"],
            "culling_preset": "event",
            "db_path": db_path_str,
        }),
    )
    .await;
    let group = &json["groups"][0];
    assert_eq!(group["winner_photo_id"], "open");
    let blink = group["photos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["photo_id"] == "blink")
        .unwrap();
    assert_eq!(blink["reject_candidate"], true);
    assert_eq!(blink["metrics"]["eye_closed"], 0.81);
    assert!(blink["explanation"]
        .as_str()
        .unwrap()
        .contains("eyes closed"));
    let open = group["photos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["photo_id"] == "open")
        .unwrap();
    assert!(!open["reason_codes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "eyes_closed" || c == "possible_blink"));
}
