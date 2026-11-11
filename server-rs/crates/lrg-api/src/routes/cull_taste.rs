//! `/v1/cull/taste`: learning the photographer's culling style from their own
//! flags and stars (`lrg_analysis::taste`), keeping it with the database, and
//! forgetting it.
//!
//! The plugin sends the photos and what the photographer decided about each;
//! the backend ranks them as a cull would, learns from the decisions inside
//! each group, and says how often the result agrees with the photographer
//! against how often the default ranking does. `/v1/cull/grade` uses it with
//! `use_taste` — only when it agreed more often.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use lrg_analysis::taste::{learn, Label, LearnError, TasteModel, MIN_PAIRS};
use serde::Deserialize;
use serde_json::{json, Value};

use super::group_similar::{compute_groups, parse_grouping_params};
use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route(
        "/cull/taste",
        axum::routing::post(learn_taste)
            .get(taste_status)
            .delete(forget_taste),
    )
}

/// Where the learned style lives: next to the database it was learned from,
/// like People's names.
fn model_path(state: &AppState) -> Option<std::path::PathBuf> {
    state.lance_root().map(|r| r.join("cull_taste.json"))
}

/// The learned style, if one was learned with this build's features.
pub(crate) fn load(state: &AppState) -> Option<TasteModel> {
    let text = std::fs::read_to_string(model_path(state)?).ok()?;
    serde_json::from_str::<TasteModel>(&text)
        .ok()
        .filter(|m| m.version == lrg_analysis::taste::MODEL_VERSION)
}

fn summary(model: &TasteModel) -> Value {
    json!({
        "pairs": model.pairs,
        "groups": model.groups,
        "agreement": (model.agreement * 1000.0).round() / 1000.0,
        "default_agreement": (model.default_agreement * 1000.0).round() / 1000.0,
        "used": model.useful(),
    })
}

#[derive(Deserialize)]
struct LabelIn {
    photo_id: String,
    #[serde(default)]
    flag: i8,
    #[serde(default)]
    rating: u8,
    #[serde(default)]
    own: bool,
}

async fn learn_taste(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let Some(Json(data)) = body else {
        return bad("Request must be JSON".into());
    };
    let labels: Vec<LabelIn> = match data
        .get("labels")
        .cloned()
        .map(serde_json::from_value::<Vec<LabelIn>>)
    {
        Some(Ok(l)) if !l.is_empty() => l,
        Some(Err(e)) => return bad(format!("Invalid labels: {e}")),
        _ => return bad("Missing 'labels': what you decided about each photo".into()),
    };
    // Ranked as a cull would rank them, with the preset asked for (`auto` by
    // default), so the model learns a correction to what culling does.
    let mut grade = json!({
        "photo_ids": labels.iter().map(|l| l.photo_id.clone()).collect::<Vec<_>>(),
        "culling_preset": data.get("culling_preset").cloned().unwrap_or(json!("auto")),
    });
    if let Some(db) = data.get("db_path") {
        grade["db_path"] = db.clone();
    }
    let params = match parse_grouping_params(&grade) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let outcome = match compute_groups(&state, &params).await {
        Ok(o) => o,
        Err(e) => {
            log::error!("Learning a culling style failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response();
        }
    };
    let labels: HashMap<String, Label> = labels
        .into_iter()
        .map(|l| {
            (
                l.photo_id,
                Label {
                    flag: l.flag.clamp(-1, 1),
                    rating: l.rating.min(5),
                    own: l.own,
                },
            )
        })
        .collect();
    match learn(&outcome.groups, &labels) {
        Ok(model) => {
            if let Some(path) = model_path(&state) {
                if let Err(e) = std::fs::write(&path, serde_json::to_string_pretty(&model).unwrap())
                {
                    log::error!("Saving the culling style failed: {e}");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(
                            json!({"error": format!("The learned style could not be saved: {e}")}),
                        ),
                    )
                        .into_response();
                }
            }
            Json(json!({"model": summary(&model)})).into_response()
        }
        Err(LearnError::TooFewDecisions(n)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": format!(
                    "Only {n} decision(s) to learn from; at least {MIN_PAIRS} are needed. A \
                     decision is two frames of one burst you marked differently — a flag, a \
                     reject or more stars. Add photos from more shoots you culled yourself."
                ),
                "error_code": "too_few_decisions",
                "pairs": n,
                "needed": MIN_PAIRS,
            })),
        )
            .into_response(),
    }
}

async fn taste_status(State(state): State<Arc<AppState>>) -> Response {
    Json(json!({"model": load(&state).as_ref().map(summary)})).into_response()
}

async fn forget_taste(State(state): State<Arc<AppState>>) -> Response {
    let deleted = model_path(&state).is_some_and(|p| std::fs::remove_file(p).is_ok());
    Json(json!({"deleted": deleted})).into_response()
}
