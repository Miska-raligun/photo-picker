//! photo-pick HTTP server (M4.2).
//!
//! Exposes a small axum-backed API for triggering scans and polling their
//! status, plus a single-page HTML UI. Pipeline work runs in a blocking task
//! pool so the async runtime stays responsive.

pub mod app;
pub mod assets;
pub mod handlers;
pub mod state;

pub use app::router;
pub use state::{AppState, RunRecord, RunStatus};
