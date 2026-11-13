//! `/v1/edit/sessions` — AI Edit's retoucher loop: open a session for one
//! photo, hand back each render, end it. The loop itself is
//! [`crate::edit_loop`].

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Value};

use lrg_providers::chat::ChatModel;
use lrg_providers::cloud_chat::build_chat_model;
use lrg_providers::edit_loop::LoopSetup;
use lrg_providers::local_chat::LocalChatModel;
use lrg_providers::provider::ProviderSelection;

use crate::edit_loop::{self, StartRequest, Step};
use crate::routes::edit::{parse_edit_options_form, persist_edit_recipe};
use crate::routes::llm::{engine_for_request, engine_overrides_from_fields};
use crate::routes::route_util::{parse_multipart, SinglePhotoForm};
use crate::state::AppState;

/// Edits a session allows when the plugin does not say.
const DEFAULT_MAX_EDITS: u32 = 3;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route("/edit/sessions", axum::routing::post(start_session))
        .route(
            "/edit/sessions/{session_id}/render",
            axum::routing::post(render_session),
        )
        .route(
            "/edit/sessions/{session_id}",
            axum::routing::delete(end_session),
        )
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(json!({"status": "error", "error": message.into()})),
    )
        .into_response()
}

fn text(fields: &HashMap<String, String>, key: &str) -> Option<String> {
    fields
        .get(key)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The model the session runs on, from the same fields the other edit and
/// indexing requests send.
async fn chat_model_for(
    state: &AppState,
    fields: &HashMap<String, String>,
) -> Result<Arc<dyn ChatModel>, String> {
    let missing = "No AI model is chosen for AI Edit. Pick one in the AI Edit dialog.";
    let provider = match text(fields, "provider")
        .ok_or(missing)?
        .to_lowercase()
        .as_str()
    {
        "openai" => "chatgpt".to_string(),
        other => other.to_string(),
    };
    let model = text(fields, "model").ok_or(missing)?;
    if matches!(provider.as_str(), "llamacpp" | "mlx") {
        let engine = engine_for_request(
            state,
            &provider,
            &model,
            engine_overrides_from_fields(fields),
        )
        .await?
        .ok_or_else(|| format!("\"{provider}\" is not a local AI model provider."))?;
        return Ok(Arc::new(LocalChatModel::new(engine)));
    }
    let selection = ProviderSelection {
        name: provider,
        api_key: text(fields, "api_key"),
        ollama_base_url: text(fields, "ollama_base_url"),
        lmstudio_base_url: text(fields, "lmstudio_base_url"),
        server_url: text(fields, "server_url"),
        local_engine: None,
    };
    build_chat_model(&selection, &model)
}

fn json_field(fields: &HashMap<String, String>, key: &str) -> Result<Option<Value>, String> {
    match fields.get(key).map(|s| s.trim()).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(raw) => serde_json::from_str(raw)
            .map(Some)
            .map_err(|e| format!("{key} is not valid JSON: {e}")),
    }
}

fn setup_from(fields: &HashMap<String, String>) -> LoopSetup {
    LoopSetup {
        // Decided by `edit_loop::start` from the photo's own settings.
        is_raw: None,
        masks: fields
            .get("masks")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("true")),
        language: text(fields, "language").unwrap_or_else(|| "English".to_string()),
        look: text(fields, "look").unwrap_or_else(|| "natural".to_string()),
        instructions: text(fields, "instructions"),
        max_edits: fields
            .get("max_edits")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(DEFAULT_MAX_EDITS),
        from_style: false,
    }
}

/// `POST /v1/edit/sessions` — opens a session and answers with its first
/// step.
async fn start_session(State(state): State<Arc<AppState>>, mut multipart: Multipart) -> Response {
    let form = match parse_multipart(&mut multipart).await {
        Ok(form) => form,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let SinglePhotoForm {
        photo_ids,
        image_bytes,
        filename,
        fields,
    } = form.into_single_photo();
    if let Some(db_path) = fields.get("db_path") {
        if let Err(e) = state.ensure_db_path(db_path).await {
            log::error!("Auto-bind to db_path {db_path} failed: {e}");
        }
    }
    let (Some(image), [photo_id]) = (image_bytes, photo_ids.as_slice()) else {
        return error(
            StatusCode::BAD_REQUEST,
            "An edit session needs one photo_id and one image.",
        );
    };
    let develop_settings = match json_field(&fields, "develop_settings") {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let style_recipe = match json_field(&fields, "style_recipe") {
        Ok(v) => v,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let model = match chat_model_for(&state, &fields).await {
        Ok(model) => model,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    log::info!(
        "Edit session for {photo_id} on {} (look {:?}, from style: {})",
        model.name(),
        fields.get("look"),
        style_recipe.is_some()
    );

    let request = StartRequest {
        photo_id: photo_id.clone(),
        filename,
        image,
        develop_settings,
        is_raw: fields.get("is_raw").map(|v| v.eq_ignore_ascii_case("true")),
        setup: setup_from(&fields),
        style_recipe,
        model,
        fields,
    };
    match edit_loop::start(&state, request).await {
        Ok((id, step)) => {
            let Some(session) = state.edit_sessions.get(&id) else {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "the edit session vanished as it was opened",
                );
            };
            let session = session.lock().await;
            Json(step.to_json(&id, &session)).into_response()
        }
        Err(e) => {
            log::warn!("Edit session for {photo_id} could not start: {}", e.message);
            error(e.status, e.message)
        }
    }
}

/// `POST /v1/edit/sessions/{session_id}/render` — the render of the last
/// step's edit; answers with the next step.
async fn render_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    mut multipart: Multipart,
) -> Response {
    let form = match parse_multipart(&mut multipart).await {
        Ok(form) => form,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let lightroom_notes: Vec<String> = match json_field(&form.fields, "lightroom_notes") {
        Ok(Some(Value::Array(items))) => items
            .into_iter()
            .filter_map(|n| n.as_str().map(str::to_string))
            .collect(),
        Ok(_) => Vec::new(),
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let Some(image) = form.images.into_iter().next().map(|i| i.bytes) else {
        return error(StatusCode::BAD_REQUEST, "No render was uploaded.");
    };
    let Some(session) = state.edit_sessions.get(&session_id) else {
        return error(
            StatusCode::NOT_FOUND,
            "This edit session has ended or expired. Run AI Edit on the photo again.",
        );
    };
    let mut session = session.lock().await;
    let step = match edit_loop::render(&state, &mut session, &image, lightroom_notes).await {
        Ok(step) => step,
        Err(e) => {
            log::warn!(
                "Edit session {session_id} ({}): {}",
                session.photo_id,
                e.message
            );
            return error(e.status, e.message);
        }
    };
    let mut step = step;
    if step.done {
        if let Some(warning) = persist(&state, &session, &step).await {
            step.warnings.push(warning);
        }
    }
    let body = step.to_json(&session_id, &session);
    if step.done {
        drop(session);
        state.edit_sessions.remove(&session_id);
    }
    Json(body).into_response()
}

/// Stores the finished recipe on the photo's index record, as
/// `/v1/edit/style` stores the recipe it generates: what the AI made of the
/// photo, whether or not the user then keeps it in Lightroom (the plugin's
/// review comes after this answer). Returns what to tell the user when it
/// could not be stored; the edit itself is in Lightroom already.
async fn persist(
    state: &AppState,
    session: &edit_loop::EditSession,
    step: &Step,
) -> Option<String> {
    // No catalog bound means nothing of this photo is indexed either, so
    // there is no record to keep the recipe on; nothing the user lost.
    let store = state.store()?;
    let (recipe, fields) = step
        .keep_round
        .and_then(|round| session.result(round, step.verdict.as_deref()))?;
    let options = parse_edit_options_form(&fields);
    match persist_edit_recipe(
        &store,
        &session.photo_id,
        session.filename.as_deref(),
        &recipe,
        &options,
    )
    .await
    {
        Ok(()) => None,
        Err(e) => {
            log::error!(
                "Failed to persist the AI Edit recipe for {}: {e}",
                session.photo_id
            );
            Some(
                "The edit is on the photo, but the backend could not store a copy of it with the \
                 photo's record in the index. Nothing on the photo is affected; the backend log \
                 has the cause."
                    .to_string(),
            )
        }
    }
}

/// `DELETE /v1/edit/sessions/{session_id}` — ends a session early. Ending one
/// that has already ended is not an error.
async fn end_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Response {
    let ended = state.edit_sessions.remove(&session_id);
    Json(json!({"status": "success", "ended": ended})).into_response()
}
