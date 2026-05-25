//! Embedded React build output (the photo-pick web UI).
//!
//! `pnpm build` in `/web` produces `/web/dist`; rust-embed bakes that into the
//! server binary so a single executable ships the whole app.

use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebAsset;

/// Serve the SPA shell. No-store so a server rebuild always wins over the
/// browser's cache of the previous HTML.
pub async fn index() -> impl IntoResponse {
    serve_path("index.html", true).await
}

/// Serve any static asset under `/assets/*` from the embedded build output.
/// Falls through to index.html for unknown routes so client-side routing
/// would also work in the future.
pub async fn asset(Path(rest): Path<String>) -> Response {
    let path = format!("assets/{rest}");
    serve_path(&path, false).await.into_response()
}

async fn serve_path(p: &str, no_store: bool) -> Response {
    match WebAsset::get(p) {
        Some(file) => {
            let mime = mime_guess::from_path(p).first_or_octet_stream();
            let mut resp = Response::builder()
                .status(StatusCode::OK)
                .header(
                    header::CONTENT_TYPE,
                    HeaderValue::from_str(mime.as_ref()).unwrap(),
                );
            if no_store {
                resp = resp.header(header::CACHE_CONTROL, "no-store, must-revalidate");
            } else {
                // Hashed asset filenames — safe to cache forever.
                resp = resp.header(header::CACHE_CONTROL, "public, max-age=31536000, immutable");
            }
            resp.body(Body::from(file.data.into_owned())).unwrap()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
