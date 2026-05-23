use crate::state::{AppState, ExplanationRecord, RunRecord, RunStatus};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json};
use photo_pick_core::group::{StageAParams, StageBParams};
use photo_pick_core::ingest::{decode_thumbnail_for, encode_jpeg, ThumbnailSpec};
use photo_pick_core::models::ExecutionProvider;
use photo_pick_core::pipeline::{LinkMode, NoopProgress, Pipeline, PipelineConfig};
use photo_pick_core::scoring::TechWeights;
use photo_pick_core::vlm::{
    explain_group_prompt, AnthropicProvider, OpenAiProvider, VlmImage, VlmProvider, VlmRequest,
};
use serde::Deserialize;
use std::collections::HashMap;
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
        composition_picks: vec![],
        photos: HashMap::new(),
        explanations: HashMap::new(),
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
                Ok(output) => {
                    rec.status = RunStatus::Completed;
                    rec.report = Some(output.report);
                    rec.html_report = Some(html_report_path);
                    rec.composition_picks = output.composition_picks;
                    rec.photos = output.photos;
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
    let Some(rec) = guard.get(&id) else {
        return (StatusCode::NOT_FOUND, "run not found").into_response();
    };

    let mut resp = serde_json::to_value(rec).unwrap();

    let picks_view: Vec<serde_json::Value> = rec
        .composition_picks
        .iter()
        .enumerate()
        .map(|(idx, cp)| {
            let scene = cp
                .kept
                .first()
                .or_else(|| cp.rejected.first())
                .map(|(_, fs)| match fs.scene {
                    photo_pick_core::scoring::Scene::Portrait => "portrait",
                    photo_pick_core::scoring::Scene::Landscape => "landscape",
                    photo_pick_core::scoring::Scene::Mixed => "mixed",
                })
                .unwrap_or("unknown");
            serde_json::json!({
                "index": idx,
                "id": cp.group.id.0.to_string(),
                "scene": scene,
                "kept": cp.kept.iter().map(|(pid, fs)| photo_view(rec, pid, Some(*fs))).collect::<Vec<_>>(),
                "rejected": cp.rejected.iter().map(|(pid, fs)| photo_view(rec, pid, Some(*fs))).collect::<Vec<_>>(),
            })
        })
        .collect();
    resp["composition_picks"] = serde_json::Value::Array(picks_view);

    Json(resp).into_response()
}

fn photo_view(
    rec: &RunRecord,
    pid: &photo_pick_core::ingest::PhotoId,
    final_score: Option<photo_pick_core::scoring::FinalScore>,
) -> serde_json::Value {
    let p = rec.photos.get(pid);
    serde_json::json!({
        "photo_id": pid.0.to_string(),
        "filename": p.and_then(|p| p.path.file_name().map(|n| n.to_string_lossy().to_string())),
        "captured_at": p.and_then(|p| p.captured_at),
        "iso": p.and_then(|p| p.iso),
        "final_score": final_score,
    })
}

/// Stream a small JPEG thumbnail of the requested photo (256px long edge).
pub async fn get_thumb(
    State(state): State<AppState>,
    Path((run_id, photo_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let parsed = match Uuid::parse_str(&photo_id) {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad photo id").into_response(),
    };
    let pid = photo_pick_core::ingest::PhotoId(parsed);

    let photo_ref = {
        let guard = state.runs.lock().await;
        let Some(rec) = guard.get(&run_id) else {
            return (StatusCode::NOT_FOUND, "run not found").into_response();
        };
        match rec.photos.get(&pid) {
            Some(p) => p.clone(),
            None => return (StatusCode::NOT_FOUND, "photo not in run").into_response(),
        }
    };

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let img = decode_thumbnail_for(&photo_ref, ThumbnailSpec { long_edge: 256 })
            .map_err(|e| e.to_string())?;
        encode_jpeg(&img, 75).map_err(|e| e.to_string())
    })
    .await;

    match result {
        Ok(Ok(bytes)) => (
            [
                ("content-type", "image/jpeg".to_string()),
                ("cache-control", "max-age=3600".to_string()),
            ],
            bytes,
        )
            .into_response(),
        Ok(Err(msg)) => (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("task: {e}")).into_response(),
    }
}

pub async fn list_runs(State(state): State<AppState>) -> Json<serde_json::Value> {
    let guard = state.runs.lock().await;
    let runs: Vec<&RunRecord> = guard.values().collect();
    Json(serde_json::to_value(runs).unwrap())
}

#[derive(Debug, Deserialize)]
pub struct ExplainRequest {
    /// Index into the run's composition_picks vector.
    pub composition_index: usize,
    /// "openai" or "anthropic" (default: "openai").
    #[serde(default = "default_provider")]
    pub provider: String,
}

fn default_provider() -> String { "openai".into() }

/// Send a composition group's photos to a VLM and return its explanation.
/// Results are cached in the run record — re-asking the same composition with
/// the same provider returns instantly.
pub async fn explain(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ExplainRequest>,
) -> impl IntoResponse {
    // Snapshot what we need without holding the lock across the blocking call.
    let snapshot = {
        let guard = state.runs.lock().await;
        let Some(rec) = guard.get(&id) else {
            return (StatusCode::NOT_FOUND, "run not found").into_response();
        };
        if let Some(cached) = rec.explanations.get(&req.composition_index) {
            if cached.provider == req.provider {
                return Json(serde_json::to_value(cached).unwrap()).into_response();
            }
        }
        let Some(pick) = rec.composition_picks.get(req.composition_index) else {
            return (StatusCode::NOT_FOUND, "composition_index out of range").into_response();
        };
        let scene = pick
            .kept
            .first()
            .or_else(|| pick.rejected.first())
            .map(|(_, fs)| match fs.scene {
                photo_pick_core::scoring::Scene::Portrait => "portrait",
                photo_pick_core::scoring::Scene::Landscape => "landscape",
                photo_pick_core::scoring::Scene::Mixed => "mixed",
            })
            .unwrap_or("unknown")
            .to_string();
        let kept_count = pick.kept.len();
        let total = pick.kept.len() + pick.rejected.len();
        let entries: Vec<(String, std::path::PathBuf)> = pick
            .kept
            .iter()
            .chain(pick.rejected.iter())
            .filter_map(|(pid, _)| {
                let p = rec.photos.get(pid)?;
                let label = p.path.file_name()?.to_string_lossy().to_string();
                Some((label, p.path.clone()))
            })
            .collect();
        (scene, kept_count, total, entries)
    };

    let (scene, kept_count, total, entries) = snapshot;
    let provider_name = req.provider.clone();
    let index = req.composition_index;
    let runs = state.runs.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<ExplanationRecord, String> {
        let provider: Box<dyn VlmProvider> = match provider_name.as_str() {
            "openai" => Box::new(OpenAiProvider::from_env().map_err(|e| e.to_string())?),
            "anthropic" => Box::new(AnthropicProvider::from_env().map_err(|e| e.to_string())?),
            other => return Err(format!("unknown provider: {other}")),
        };

        let images: Vec<VlmImage> = entries
            .into_iter()
            .filter_map(|(label, path)| {
                let p = photo_pick_core::ingest::PhotoRef {
                    id: photo_pick_core::ingest::PhotoId::new(),
                    path,
                    format: photo_pick_core::ingest::ImageFormat::Jpeg,
                    captured_at: None,
                    file_size: 0,
                    sha256_short: [0; 16],
                    burst_id: None,
                    drive_mode: None,
                    iso: None,
                    exposure_bias_ev: None,
                };
                let img = decode_thumbnail_for(&p, ThumbnailSpec { long_edge: 512 }).ok()?;
                let jpeg_bytes = encode_jpeg(&img, 80).ok()?;
                Some(VlmImage { jpeg_bytes, label })
            })
            .collect();

        let prompt = explain_group_prompt(&scene, kept_count, total);
        let req = VlmRequest::new(prompt, images);
        let text = provider.complete(&req).map_err(|e| e.to_string())?;
        Ok(ExplanationRecord {
            provider: provider.name().to_string(),
            model: provider.model().to_string(),
            text,
        })
    })
    .await;

    match result {
        Ok(Ok(rec)) => {
            let mut guard = runs.lock().await;
            if let Some(run) = guard.get_mut(&id) {
                run.explanations.insert(index, rec.clone());
            }
            Json(serde_json::to_value(rec).unwrap()).into_response()
        }
        Ok(Err(msg)) => (StatusCode::BAD_GATEWAY, msg).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("task: {e}")).into_response(),
    }
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
