use crate::assets;
use crate::handlers;
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::http::{request::Parts, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::trace::TraceLayer;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(assets::index))
        .route("/assets/*rest", get(assets::asset))
        .route("/api/browse", get(handlers::browse))
        .route("/api/reveal", get(handlers::reveal))
        .route("/api/scan", post(handlers::scan))
        .route("/api/info", get(handlers::info))
        .route("/api/health", get(handlers::health))
        .route("/api/runs", get(handlers::list_runs))
        .route("/api/providers", get(handlers::list_providers))
        .route("/api/runs/:id", get(handlers::get_run))
        .route("/api/runs/:id/events", get(handlers::run_events))
        .route("/api/runs/:id/cancel", post(handlers::cancel_run))
        .route("/api/runs/:id/diff/:other", get(handlers::diff_runs))
        .route("/api/runs/:id/duplicates", get(handlers::exact_duplicates))
        .route("/api/runs/:id/similar/:photo_id", get(handlers::similar_photos))
        .route("/api/runs/:id/html", get(handlers::get_run_html))
        .route("/api/runs/:id/report.json", get(handlers::get_run_report_json))
        .route("/api/runs/:id/thumb/:photo_id", get(handlers::get_thumb))
        .route("/api/runs/:id/preview/:photo_id", get(handlers::get_preview))
        .route("/api/runs/:id/explain", post(handlers::explain))
        .route("/api/runs/:id/apply", post(handlers::apply))
        .route("/api/runs/:id/export", post(handlers::export))
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .layer(cors_layer())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Bearer-token gate for the API surface, active only when PHOTO_PICK_TOKEN
/// is set. Static assets stay open (the UI shell must load so the user can
/// enter the token) and /api/health stays open for probes; everything else
/// under /api requires the token via one of three channels:
///   - `Authorization: Bearer <token>` (fetch calls)
///   - `photo_pick_token=<token>` cookie (needed for <img> thumbs and
///     EventSource, which can't set headers)
///   - `?token=<token>` query param (curl convenience)
async fn require_token(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected) = state.api_token.as_deref() else {
        return next.run(req).await;
    };
    let path = req.uri().path();
    if !path.starts_with("/api") || path == "/api/health" {
        return next.run(req).await;
    }
    if token_matches(req.headers(), req.uri().query(), expected) {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, "missing or invalid PHOTO_PICK_TOKEN").into_response()
    }
}

fn token_matches(headers: &HeaderMap, query: Option<&str>, expected: &str) -> bool {
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(tok) = auth.strip_prefix("Bearer ") {
            if ct_eq(tok.trim(), expected) {
                return true;
            }
        }
    }
    if let Some(cookies) = headers.get("cookie").and_then(|v| v.to_str().ok()) {
        for pair in cookies.split(';') {
            if let Some(tok) = pair.trim().strip_prefix("photo_pick_token=") {
                if ct_eq(tok, expected) {
                    return true;
                }
            }
        }
    }
    if let Some(q) = query {
        for pair in q.split('&') {
            if let Some(tok) = pair.strip_prefix("token=") {
                if ct_eq(tok, expected) {
                    return true;
                }
            }
        }
    }
    false
}

/// Constant-time-ish comparison — cheap fold so a timing probe can't walk
/// the token byte-by-byte. Length still leaks; acceptable for this threat
/// model (LAN curl, not a crypto oracle).
fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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
