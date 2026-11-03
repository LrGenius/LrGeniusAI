//! A stand-in for GitHub's release downloads on a local port, for the
//! download tests in `routes::clip` and `routes::assets`.
//!
//! Kept out of `routes/` on purpose: `scripts/check-docs.py` reads every
//! `.route(` there as an endpoint the API reference must document.

use axum::extract::Path;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::routes::clip::UrlAsset;

/// What `/ok/{name}` serves for `name`: different for every name, so a
/// test can tell the files apart on disk.
pub(crate) fn content(name: &str) -> Vec<u8> {
    format!("{name}\n").repeat(4096).into_bytes()
}

/// The summed size of what `/ok/` serves for `names`.
pub(crate) fn size_of(names: &[&str]) -> u64 {
    names.iter().map(|name| content(name).len() as u64).sum()
}

async fn ok(Path(name): Path<String>) -> Vec<u8> {
    content(&name)
}

/// Answers the HEAD with the file's size and fails the GET: a transfer
/// that breaks after the sizes were resolved.
async fn flaky(method: Method, Path(name): Path<String>) -> Response {
    if method == Method::HEAD {
        content(&name).into_response()
    } else {
        StatusCode::BAD_GATEWAY.into_response()
    }
}

/// Serves `/ok/{name}`, `/gone/{name}` (404, like a release that was never
/// published) and `/flaky/{name}`, and returns the base URL. axum answers
/// a HEAD on a GET route with the GET's headers, `Content-Length`
/// included, as GitHub's CDN does.
pub(crate) async fn serve() -> String {
    let app = Router::new()
        .route("/ok/{name}", get(ok))
        .route("/gone/{name}", get(|| async { StatusCode::NOT_FOUND }))
        .route("/flaky/{name}", get(flaky));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// `name` as served under `route` (`ok`, `gone` or `flaky`), going to `dest`.
pub(crate) fn asset<'a>(
    server: &str,
    route: &str,
    name: &'a str,
    dest: &'a std::path::Path,
) -> UrlAsset<'a> {
    (format!("{server}/{route}/{name}"), name, dest)
}
