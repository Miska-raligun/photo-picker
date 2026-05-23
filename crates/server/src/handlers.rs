use crate::state::{AppState, RunRecord, RunStatus};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json};
use photo_pick_core::group::{StageAParams, StageBParams};
use photo_pick_core::ingest::ThumbnailSpec;
use photo_pick_core::models::ExecutionProvider;
use photo_pick_core::pipeline::{LinkMode, NoopProgress, Pipeline, PipelineConfig};
use photo_pick_core::scoring::TechWeights;
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

const INDEX_HTML: &str = include_str!("../static/index.html");

pub async fn index() -> impl IntoResponse {
    Html(INDEX_HTML)
}

#[derive(Debug, Deserialize)]
pub struct ScanRequest {
    pub root: PathBuf,
    pub output: PathBuf,
    #[serde(default = "default_k1")]
    pub k1: usize,
    #[serde(default = "default_k2")]
    pub k2: usize,
    #[serde(default = "default_time_k")]
    pub time_k: f32,
    #[serde(default = "default_min_dt")]
    pub min_dt: f32,
    #[serde(default = "default_max_dt")]
    pub max_dt: f32,
    #[serde(default = "default_hash_dist")]
    pub hash_dist: u32,
    #[serde(default = "default_threshold")]
    pub stage_b_threshold: f32,
    #[serde(default = "default_clip")]
    pub enable_clip: bool,
}

fn default_k1() -> usize { 3 }
fn default_k2() -> usize { 1 }
fn default_time_k() -> f32 { 3.0 }
fn default_min_dt() -> f32 { 0.3 }
fn default_max_dt() -> f32 { 30.0 }
fn default_hash_dist() -> u32 { 6 }
fn default_threshold() -> f32 { 0.93 }
fn default_clip() -> bool { true }

/// Kick off a scan in the blocking pool. Returns immediately with the run id;
/// poll `/api/runs/{id}` to follow status.
pub async fn scan(
    State(state): State<AppState>,
    Json(req): Json<ScanRequest>,
) -> Json<serde_json::Value> {
    let run_id = Uuid::new_v4().to_string();

    let html_report_path = req.output.join("report.html");
    let report_path = req.output.join("report.json");
    let cache_path = req.output.join(".cache.db");

    let record = RunRecord {
        id: run_id.clone(),
        root: req.root.clone(),
        output: req.output.clone(),
        status: RunStatus::Running,
        report: None,
        html_report: None,
    };
    state.runs.lock().await.insert(run_id.clone(), record);

    let runs = state.runs.clone();
    let run_id_for_task = run_id.clone();
    let req_for_task = req;

    tokio::task::spawn_blocking(move || {
        let cfg = PipelineConfig {
            root: req_for_task.root.clone(),
            output: req_for_task.output.clone(),
            report_path: Some(report_path),
            html_report_path: Some(html_report_path.clone()),
            cache_path: Some(cache_path),
            stage_a: StageAParams {
                k_time: req_for_task.time_k,
                min_dt: Duration::from_secs_f32(req_for_task.min_dt),
                max_dt: Duration::from_secs_f32(req_for_task.max_dt),
                max_hash_dist: req_for_task.hash_dist,
            },
            stage_b: StageBParams {
                similarity_threshold: req_for_task.stage_b_threshold,
            },
            k1: req_for_task.k1,
            k2: req_for_task.k2,
            tech_weights: TechWeights::default(),
            link_mode: LinkMode::Hardlink,
            thumbnail: ThumbnailSpec::default(),
            dry_run: false,
            enable_clip: req_for_task.enable_clip,
            execution_provider: ExecutionProvider::Cpu,
        };
        let pipeline = Pipeline::new(cfg);
        let result = pipeline.run(&NoopProgress);
        let mut guard = runs.blocking_lock();
        if let Some(rec) = guard.get_mut(&run_id_for_task) {
            match result {
                Ok(report) => {
                    rec.status = RunStatus::Completed;
                    rec.report = Some(report);
                    rec.html_report = Some(html_report_path);
                }
                Err(e) => {
                    rec.status = RunStatus::Failed { error: e.to_string() };
                }
            }
        }
    });

    Json(serde_json::json!({ "run_id": run_id }))
}

pub async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let guard = state.runs.lock().await;
    match guard.get(&id) {
        Some(rec) => Json(serde_json::to_value(rec).unwrap()).into_response(),
        None => (StatusCode::NOT_FOUND, "run not found").into_response(),
    }
}

pub async fn list_runs(State(state): State<AppState>) -> Json<serde_json::Value> {
    let guard = state.runs.lock().await;
    let runs: Vec<&RunRecord> = guard.values().collect();
    Json(serde_json::to_value(runs).unwrap())
}

/// Serve the on-disk HTML report a completed run wrote. We avoid generic
/// `ServeDir` because reports live in user-chosen output directories — this
/// reads only the registered run's `html_report` path.
pub async fn get_run_html(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let path = {
        let guard = state.runs.lock().await;
        guard.get(&id).and_then(|r| r.html_report.clone())
    };
    match path {
        Some(p) if p.exists() => match tokio::fs::read_to_string(&p).await {
            Ok(body) => Html(body).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("read failed: {e}")).into_response(),
        },
        _ => (StatusCode::NOT_FOUND, "no html report for this run").into_response(),
    }
}
