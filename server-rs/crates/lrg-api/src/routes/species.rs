//! Species link lookup — turns the names BioCLIP writes into the catalog into
//! links on the free taxonomy databases.
//!
//! The plugin calls this once per distinct taxon (it caches per session, and
//! [`crate::species_links`] caches across restarts) and writes the chosen
//! provider's URL into the photo's read-only `speciesLookupUrl` field, which
//! Lightroom renders as a clickable link in the Metadata panel.
//!
//! Deliberately *not* folded into the indexing path: resolution needs the
//! network, indexing must not, and a catalog's taxa repeat so heavily that
//! doing it on demand costs a handful of calls for a whole library.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::{routing::get, routing::post, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/species/links", get(species_links))
        .route("/species/links/batch", post(species_links_batch))
}

#[derive(Deserialize)]
struct LinkQuery {
    name: String,
    /// `species`, `genus`, … — the plugin's `speciesRank`. Optional, but it
    /// stops a genus query from matching a same-named species.
    rank: Option<String>,
    /// Wikipedia edition and iNaturalist common-name locale.
    lang: Option<String>,
}

async fn species_links(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LinkQuery>,
) -> Json<Value> {
    let lang = q.lang.as_deref().unwrap_or("en");
    let links = state
        .species_links
        .resolve(&q.name, q.rank.as_deref(), lang)
        .await;
    Json(json!({ "status": "success", "links": links }))
}

#[derive(Deserialize)]
struct BatchRequest {
    /// `[{ "name": "Panthera leo", "rank": "species" }, …]`
    names: Vec<BatchItem>,
    lang: Option<String>,
}

#[derive(Deserialize)]
struct BatchItem {
    name: String,
    rank: Option<String>,
}

/// Resolve several taxa in one request.
///
/// Sequential on purpose: the resolver's rate gate would serialize concurrent
/// calls anyway, and fanning out would only build a queue of requests the
/// upstream APIs asked us not to send.
async fn species_links_batch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BatchRequest>,
) -> Json<Value> {
    let lang = req.lang.as_deref().unwrap_or("en");
    let mut out = Vec::with_capacity(req.names.len());
    for item in &req.names {
        out.push(
            state
                .species_links
                .resolve(&item.name, item.rank.as_deref(), lang)
                .await,
        );
    }
    Json(json!({ "status": "success", "links": out }))
}
