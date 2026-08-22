//! HTTP layer: axum routers mirroring the Flask blueprints, the shared
//! application state, and the `db_path` auto-bind middleware.

pub mod edit_budget;
pub mod llm_engine;
pub mod llm_models;
pub mod middleware;
pub mod mlx_engine;
pub mod mlx_models;
pub mod routes;
pub mod species_links;
pub mod state;
pub mod ui_bridge;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::Router;

use state::AppState;

/// The API version prefix every route is served under, bar the frozen
/// bootstrap contract. Declared once here rather than repeated in each
/// module's path literals, so it cannot drift.
pub const API_PREFIX: &str = "/v1";

/// Build the full application router.
///
/// Two tiers: the version-independent bootstrap contract at the root (see
/// [`routes::bootstrap`]) and everything else under [`API_PREFIX`]. Route
/// modules therefore spell only their domain path — `"/faces/detect"`, not
/// `"/v1/faces/detect"` — and nesting supplies the prefix.
pub fn build_router(state: Arc<AppState>) -> Router {
    let v1 = Router::new()
        .merge(routes::server::router())
        .merge(routes::index::router())
        .merge(routes::db::router())
        .merge(routes::clip::router())
        .merge(routes::bioclip::router())
        .merge(routes::assets::router())
        .merge(routes::faces::router())
        .merge(routes::index_upload::router())
        .merge(routes::index_by_reference::router())
        .merge(routes::jobs::router())
        .merge(routes::search::router())
        .merge(routes::find_similar::router())
        .merge(routes::group_similar::router())
        .merge(routes::edit::router())
        .merge(routes::training::router())
        .merge(routes::keywords::router())
        .merge(routes::llm::router())
        .merge(routes::style_edit::router())
        .merge(routes::import_::router())
        .merge(routes::species::router())
        .merge(routes::ui::router());

    Router::new()
        // Never versioned — a new plug-in must be able to reach these on an
        // old binary to pull the install forward.
        .merge(routes::bootstrap::router())
        .merge(routes::update::router())
        .nest(API_PREFIX, v1)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auto_bind_db_path,
        ))
        // Flask never set MAX_CONTENT_LENGTH, so the Python backend had no
        // request-body cap; axum's own default (2 MiB) is far below a real
        // Lightroom-exported JPEG preview (or a multi-photo /v1/index/photos batch),
        // which silently failed `Multipart` field reads on the `image`
        // field once exceeded. Disable it to restore parity.
        .layer(DefaultBodyLimit::disable())
        .with_state(state)
}
