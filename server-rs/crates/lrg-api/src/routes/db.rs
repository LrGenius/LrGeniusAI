//! DB blueprint (M2 subset) — port of `routes/db.py`: /db/stats.
//! /db/backup and /db/migrate-photo-ids follow later in M2/M6.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};

use lrg_store::{meta, FACE_TABLE, IMAGE_TABLE, VERTEX_TABLE};

use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/db/stats", get(database_stats))
}

#[derive(serde::Deserialize)]
struct StatsQuery {
    catalog_id: Option<String>,
    #[allow(dead_code)]
    db_path: Option<String>,
}

async fn database_stats(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StatsQuery>,
) -> Response {
    let empty = json!({
        "photos": {"total": 0, "with_embedding": 0, "with_title": 0,
                    "with_caption": 0, "with_keywords": 0, "with_vertexai": 0},
        "faces": {"total": 0},
        "persons": {"total": 0},
    });
    let Some(store) = state.store() else {
        return Json(empty).into_response();
    };

    let result: Result<Value, String> = async {
        let vertex_ids: BTreeSet<String> = store
            .scan_meta(VERTEX_TABLE)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let catalog_id = query
            .catalog_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let non_empty = |m: &serde_json::Map<String, Value>, key: &str| {
            m.get(key)
                .and_then(Value::as_str)
                .is_some_and(|s| !s.trim().is_empty())
        };

        let (mut total, mut with_embedding, mut with_title) = (0u64, 0u64, 0u64);
        let (mut with_caption, mut with_keywords, mut with_vertexai) = (0u64, 0u64, 0u64);
        for (id, m) in store
            .scan_meta(IMAGE_TABLE)
            .await
            .map_err(|e| e.to_string())?
        {
            if let Some(cat) = catalog_id {
                if !meta::parse_catalog_ids(&m).contains(cat) {
                    continue;
                }
            }
            total += 1;
            if meta::has_embedding(&m) {
                with_embedding += 1;
            }
            if non_empty(&m, "title") {
                with_title += 1;
            }
            if non_empty(&m, "caption") {
                with_caption += 1;
            }
            if non_empty(&m, "keywords") || non_empty(&m, "flattened_keywords") {
                with_keywords += 1;
            }
            if vertex_ids.contains(&id) {
                with_vertexai += 1;
            }
        }

        let face_rows = store
            .scan_meta(FACE_TABLE)
            .await
            .map_err(|e| e.to_string())?;
        let faces_total = face_rows.len();
        let persons: BTreeSet<String> = face_rows
            .iter()
            .filter_map(|(_, m)| m.get("person_id").and_then(Value::as_str))
            .filter(|p| !p.is_empty() && *p != "person_unassigned")
            .map(str::to_string)
            .collect();

        Ok(json!({
            "photos": {"total": total, "with_embedding": with_embedding,
                        "with_title": with_title, "with_caption": with_caption,
                        "with_keywords": with_keywords, "with_vertexai": with_vertexai},
            "faces": {"total": faces_total},
            "persons": {"total": persons.len()},
        }))
    }
    .await;

    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            log::error!("Error computing database stats: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response()
        }
    }
}
