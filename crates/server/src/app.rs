use crate::assets;
use crate::handlers;
use crate::state::AppState;
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(assets::index))
        .route("/assets/*rest", get(assets::asset))
        .route("/api/browse", get(handlers::browse))
        .route("/api/scan", post(handlers::scan))
        .route("/api/runs", get(handlers::list_runs))
        .route("/api/runs/:id", get(handlers::get_run))
        .route("/api/runs/:id/html", get(handlers::get_run_html))
        .route("/api/runs/:id/thumb/:photo_id", get(handlers::get_thumb))
        .route("/api/runs/:id/preview/:photo_id", get(handlers::get_preview))
        .route("/api/runs/:id/explain", post(handlers::explain))
        .route("/api/runs/:id/apply", post(handlers::apply))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
