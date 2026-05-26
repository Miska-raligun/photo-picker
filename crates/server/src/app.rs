use crate::assets;
use crate::handlers;
use crate::state::AppState;
use axum::http::{request::Parts, HeaderValue};
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::trace::TraceLayer;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(assets::index))
        .route("/assets/*rest", get(assets::asset))
        .route("/api/browse", get(handlers::browse))
        .route("/api/scan", post(handlers::scan))
        .route("/api/runs", get(handlers::list_runs))
        .route("/api/runs/:id", get(handlers::get_run))
        .route("/api/runs/:id/events", get(handlers::run_events))
        .route("/api/runs/:id/html", get(handlers::get_run_html))
        .route("/api/runs/:id/thumb/:photo_id", get(handlers::get_thumb))
        .route("/api/runs/:id/preview/:photo_id", get(handlers::get_preview))
        .route("/api/runs/:id/explain", post(handlers::explain))
        .route("/api/runs/:id/apply", post(handlers::apply))
        .layer(cors_layer())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// CORS policy. The UI is served same-origin by this binary, so cross-origin
/// callers are typically other machines on the LAN. Restrict to localhost
/// origins by default; set `PHOTO_PICK_CORS_ANY=1` to allow any origin
/// (headless setups, reverse proxies, dev against a separate Vite server).
fn cors_layer() -> CorsLayer {
    if std::env::var_os("PHOTO_PICK_CORS_ANY").is_some() {
        return CorsLayer::permissive();
    }
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(
            |origin: &HeaderValue, _parts: &Parts| is_localhost_origin(origin),
        ))
        .allow_methods(Any)
        .allow_headers(Any)
}

fn is_localhost_origin(origin: &HeaderValue) -> bool {
    let Ok(s) = origin.to_str() else {
        return false;
    };
    let Some((_scheme, rest)) = s.split_once("://") else {
        return false;
    };
    // Extract the host, dropping an optional port and IPv6 brackets.
    let host = if let Some(inner) = rest.strip_prefix('[') {
        match inner.split_once(']') {
            Some((h, _)) => h, // e.g. "[::1]:7777" -> "::1"
            None => return false,
        }
    } else {
        rest.split(':').next().unwrap_or("") // "127.0.0.1:5173" -> "127.0.0.1"
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}
