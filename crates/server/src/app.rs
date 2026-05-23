use crate::handlers;
use crate::state::AppState;
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::index))
        .route("/api/scan", post(handlers::scan))
        .route("/api/runs", get(handlers::list_runs))
        .route("/api/runs/:id", get(handlers::get_run))
        .route("/api/runs/:id/html", get(handlers::get_run_html))
        .route("/api/runs/:id/explain", post(handlers::explain))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
