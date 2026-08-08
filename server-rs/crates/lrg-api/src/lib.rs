//! HTTP layer: axum routers mirroring the Flask blueprints, the shared
//! application state, and the `db_path` auto-bind middleware.

pub mod llm_engine;
pub mod llm_models;
pub mod middleware;
pub mod routes;
pub mod state;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::Router;

use state::AppState;

/// Build the full application router (M1: server blueprint only; the
/// remaining blueprints are added milestone by milestone).
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(routes::server::router())
        .merge(routes::index::router())
        .merge(routes::db::router())
        .merge(routes::clip::router())
        .merge(routes::faces::router())
        .merge(routes::index_upload::router())
        .merge(routes::index_by_reference::router())
        .merge(routes::search::router())
        .merge(routes::find_similar::router())
        .merge(routes::group_similar::router())
        .merge(routes::edit::router())
        .merge(routes::training::router())
        .merge(routes::keywords::router())
        .merge(routes::llm::router())
        .merge(routes::style_edit::router())
        .merge(routes::import_::router())
        .merge(routes::update::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auto_bind_db_path,
        ))
        // Flask never set MAX_CONTENT_LENGTH, so the Python backend had no
        // request-body cap; axum's own default (2 MiB) is far below a real
        // Lightroom-exported JPEG preview (or a multi-photo /index batch),
        // which silently failed `Multipart` field reads on the `image`
        // field once exceeded. Disable it to restore parity.
        .layer(DefaultBodyLimit::disable())
        .with_state(state)
}
