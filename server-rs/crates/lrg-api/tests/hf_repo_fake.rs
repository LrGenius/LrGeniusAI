//! `hf_repo::check_repo` against a fake Hugging Face server: the requests it
//! makes, the revision it pins, and the errors it reports.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use lrg_api::hf_repo::{check_repo, CheckError, Engine};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

async fn info(
    Path((org, name, _rev)): Path<(String, String, String)>,
) -> (StatusCode, Json<Value>) {
    let repo = format!("{org}/{name}");
    match repo.as_str() {
        "mlx-community/gemma-3-12b-it-qat-4bit" => (
            StatusCode::OK,
            Json(json!({"id": repo, "sha": SHA, "library_name": "mlx", "gated": false})),
        ),
        "ggml-org/gemma-3-12b-it-GGUF" => (
            StatusCode::OK,
            Json(json!({"id": repo, "sha": SHA, "gated": false})),
        ),
        "google/gemma-3-12b-it" => (
            StatusCode::OK,
            Json(json!({"id": repo, "sha": SHA, "gated": "manual"})),
        ),
        _ => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Repository not found"})),
        ),
    }
}

async fn tree(Path((org, name, sha)): Path<(String, String, String)>) -> (StatusCode, Json<Value>) {
    assert_eq!(sha, SHA, "the listing must be pinned to the checked commit");
    let files: Vec<(&str, u64)> = match format!("{org}/{name}").as_str() {
        "mlx-community/gemma-3-12b-it-qat-4bit" => vec![
            ("config.json", 5_000),
            ("model.safetensors", 8_000_000_000),
            ("tokenizer.json", 30_000_000),
            ("tokenizer_config.json", 1_000_000),
            ("preprocessor_config.json", 500),
            ("README.md", 2_000),
        ],
        "ggml-org/gemma-3-12b-it-GGUF" => vec![
            ("gemma-3-12b-it-Q4_K_M.gguf", 7_300_000_000),
            ("gemma-3-12b-it-Q8_0.gguf", 12_500_000_000),
            ("mmproj-model-f16.gguf", 850_000_000),
        ],
        _ => vec![],
    };
    let entries: Vec<Value> = files
        .into_iter()
        .map(|(path, size)| json!({"type": "file", "path": path, "size": size}))
        .collect();
    (StatusCode::OK, Json(Value::Array(entries)))
}

async fn resolve(
    Path((_org, _name, sha, file)): Path<(String, String, String, String)>,
) -> (StatusCode, Json<Value>) {
    assert_eq!(sha, SHA, "small files must come from the checked commit");
    match file.as_str() {
        "config.json" => (
            StatusCode::OK,
            Json(json!({"model_type": "gemma3", "quantization": {"bits": 4}})),
        ),
        "preprocessor_config.json" => (
            StatusCode::OK,
            Json(json!({"processor_class": "Gemma3Processor"})),
        ),
        "tokenizer_config.json" => (
            StatusCode::OK,
            Json(json!({"chat_template": "{{ messages }}"})),
        ),
        _ => (StatusCode::NOT_FOUND, Json(json!({}))),
    }
}

async fn fake_hub() -> String {
    let app = Router::new()
        .route("/api/models/{org}/{name}/revision/{rev}", get(info))
        .route("/api/models/{org}/{name}/tree/{sha}", get(tree))
        .route("/{org}/{name}/resolve/{sha}/{*file}", get(resolve));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn the_mlx_model_from_issue_341_passes_and_is_pinned() {
    let hub = fake_hub().await;
    let client = reqwest::Client::new();
    let check = check_repo(
        &client,
        &hub,
        "https://huggingface.co/mlx-community/gemma-3-12b-it-qat-4bit",
        Engine::Mlx,
        None,
    )
    .await
    .expect("should pass");
    assert_eq!(check.repo, "mlx-community/gemma-3-12b-it-qat-4bit");
    assert_eq!(check.revision, SHA);
    assert_eq!(check.model_type.as_deref(), Some("gemma3"));
    assert_eq!(check.installed_name, "gemma-3-12b-it-qat-4bit");
    assert!(
        check.files.iter().all(|f| f.path != "README.md"),
        "only what the engine loads is fetched"
    );
}

#[tokio::test]
async fn a_gguf_repo_gets_its_quantization_and_projector() {
    let hub = fake_hub().await;
    let client = reqwest::Client::new();
    let check = check_repo(
        &client,
        &hub,
        "ggml-org/gemma-3-12b-it-GGUF:Q8_0",
        Engine::Llamacpp,
        None,
    )
    .await
    .expect("should pass");
    let paths: Vec<&str> = check.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["gemma-3-12b-it-Q8_0.gguf", "mmproj-model-f16.gguf"]
    );
    assert_eq!(check.installed_name, "gemma-3-12b-it-Q8_0.gguf");
    assert_eq!(check.dir_name, "gemma-3-12b-it-GGUF");
}

#[tokio::test]
async fn missing_and_gated_repos_are_refused_with_a_reason() {
    let hub = fake_hub().await;
    let client = reqwest::Client::new();

    let missing = check_repo(&client, &hub, "nobody/nothing", Engine::Mlx, None)
        .await
        .unwrap_err();
    assert!(
        matches!(&missing, CheckError::Unusable(m) if m.contains("no public model named nobody/nothing")),
        "{missing:?}"
    );

    let gated = check_repo(&client, &hub, "google/gemma-3-12b-it", Engine::Mlx, None)
        .await
        .unwrap_err();
    assert!(
        matches!(&gated, CheckError::Unusable(m) if m.contains("gated")),
        "{gated:?}"
    );
}

#[tokio::test]
async fn a_quantization_suffix_is_refused_for_mlx() {
    let hub = fake_hub().await;
    let client = reqwest::Client::new();
    let err = check_repo(
        &client,
        &hub,
        "mlx-community/gemma-3-12b-it-qat-4bit:Q4_K_M",
        Engine::Mlx,
        None,
    )
    .await
    .unwrap_err();
    assert!(err.message().contains("only applies to GGUF"), "{err:?}");
}

#[tokio::test]
async fn an_unreachable_hub_is_told_apart_from_a_bad_repo() {
    let client = reqwest::Client::new();
    let err = check_repo(
        &client,
        "http://127.0.0.1:1",
        "mlx-community/gemma-3-12b-it-qat-4bit",
        Engine::Mlx,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, CheckError::Unreachable(_)), "{err:?}");
}
