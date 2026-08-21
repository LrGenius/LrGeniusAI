//! The version-independent bootstrap contract.
//!
//! These four paths, plus `POST /update/apply` in [`super::update`], are the
//! only ones served off the root instead of under `/v1`, and they must never
//! move again. The plug-in's ensure-backend loop (`Util.waitForServerDialog`)
//! drives `ping` -> `ensureVersionCompatibility` (`/version`) -> `/shutdown`
//! against **whatever backend is already installed**, and
//! `SearchIndexAPI.applyUpdate` posts to `/update/apply` on that same old
//! binary to pull the machine forward.
//!
//! So these paths are what a *new* plug-in uses to talk to an *old* server. If
//! they were versioned, a plug-in that updated ahead of its backend would call
//! paths the running binary does not serve, and the mechanism needed to repair
//! the install would be the very thing that broke. Everything else is free to
//! move between API versions precisely because this set does not.
//!
//! The handlers themselves live in [`super::server`]; only the registration is
//! here, so that `scripts/check-docs.py` can tell versioned routes from
//! unversioned ones by file name alone.

use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

use super::server::{get_version, ping, shutdown, version_check};
use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ping", get(ping))
        .route("/shutdown", post(shutdown))
        .route("/version", get(get_version))
        .route("/version/check", post(version_check))
}
