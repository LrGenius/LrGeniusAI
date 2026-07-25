//! HTTP layer: axum routers mirroring the Flask blueprints, the shared
//! application state, and the `db_path` auto-bind middleware.

pub mod middleware;
pub mod routes;
pub mod state;

use std::sync::Arc;

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
        .merge(routes::search::router())
        .merge(routes::find_similar::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auto_bind_db_path,
        ))
        .with_state(state)
}
