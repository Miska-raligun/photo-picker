use crate::state::{AppState, ExplanationRecord, RunRecord, RunStatus};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json};
use photo_pick_core::group::{StageAParams, StageBParams};
use photo_pick_core::ingest::{
    classify_extension, decode_thumbnail_for, encode_jpeg, PhotoSource, ThumbnailSpec,
};
use photo_pick_core::models::ExecutionProvider;
use photo_pick_core::pipeline::{LinkMode, Pipeline, PipelineConfig};
use photo_pick_core::scoring::TechWeights;
use photo_pick_core::vlm::{
    explain_group_prompt, AnthropicProvider, OpenAiProvider, VlmImage, VlmProvider, VlmRequest,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

// The `/` index handler now lives in `crate::assets::index` (serves the
// React build). The old single-file HTML is kept on disk as a reference but
// no longer compiled in.

#[derive(Debug, Deserialize)]
pub struct ScanRequest {
    /// Scan an entire directory. Mutually exclusive with `files`.
    pub root: Option<PathBuf>,
    /// Scan only this explicit list of file paths. Mutually exclusive with
    /// `root`.
    #[serde(default)]
    pub files: Vec<PathBuf>,
    /// Where the run's internal artifacts (report.html/json, feature cache,
    /// disk thumbnails) live. Optional: when absent the server picks a managed
    /// per-source directory under the OS data dir, keeping the user's photo
    /// folders clean. Runs never copy the photos themselves here — exporting
    /// is the deferred `/export` action.
    #[serde(default)]
    pub output: Option<PathBuf>,
    #[serde(default = "default_k1")]
    pub k1: usize,
    /// `None` (or `0`) = auto mode (per-group keep count driven by score gaps).
    /// Positive integer = fixed K2 for every composition group.
    #[serde(default)]
    pub k2: Option<usize>,
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
    #[serde(default = "default_stage_a_clip")]
    pub stage_a_clip_threshold: f32,
    #[serde(default = "default_clip")]
    pub enable_clip: bool,
    #[serde(default = "default_face")]
    pub enable_face: bool,
    /// "In-place" review workflow: don't copy picks/rejected into `output`;
    /// the user will apply selections destructively to the source via
    /// `/api/runs/:id/apply`.
    #[serde(default)]
    pub in_place: bool,
    /// "copy" | "hardlink" | "symlink". Ignored in in-place mode.
    #[serde(default = "default_link_mode")]
    pub link_mode: String,
    /// Long-edge px for the analysis thumbnail (also caps preview size). 512–4096.
    #[serde(default = "default_thumb_long_edge")]
    pub thumbnail_long_edge: u32,
    /// "cpu" | "cuda" | "coreml" | "directml". Falls back to cpu if the
    /// requested provider isn't compiled in.
    #[serde(default = "default_provider_str")]
    pub execution_provider: String,
    /// Apply portrait/landscape bias to Stage A/B thresholds.
    #[serde(default = "default_adaptive")]
    pub adaptive_thresholds: bool,
}

fn default_k1() -> usize { 3 }
fn default_time_k() -> f32 { 3.0 }
fn default_min_dt() -> f32 { 0.3 }
fn default_max_dt() -> f32 { 30.0 }
fn default_hash_dist() -> u32 { 6 }
fn default_threshold() -> f32 { 0.93 }
fn default_stage_a_clip() -> f32 { 0.95 }
fn default_clip() -> bool { true }
fn default_face() -> bool { true }
fn default_link_mode() -> String { "hardlink".into() }
fn default_thumb_long_edge() -> u32 { 1024 }
fn default_provider_str() -> String { "cpu".into() }
fn default_adaptive() -> bool { true }

fn parse_link_mode(s: &str) -> LinkMode {
    match s {
        "copy" => LinkMode::Copy,
        "symlink" => LinkMode::Symlink,
        _ => LinkMode::Hardlink,
    }
}
fn parse_provider(s: &str) -> ExecutionProvider {
    match s {
        "cuda" => ExecutionProvider::Cuda,
        "coreml" => ExecutionProvider::CoreMl,
        "directml" => ExecutionProvider::DirectMl,
        _ => ExecutionProvider::Cpu,
    }
}

/// Managed location for a run's internal artifacts (report.html/json, feature
/// cache, disk thumbnails) when the client doesn't specify `output`. Keyed by
/// a stable hash of the canonicalized source path so re-scanning the same
/// folder reuses its feature/thumbnail caches — and the user's photo folders
/// stay free of `.cache.db`/`report.html`.
fn artifacts_dir(source: &std::path::Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let canon = std::fs::canonicalize(source).unwrap_or_else(|e| {
        // Fall back to the raw path. Note: different spellings of the same dir
        // (symlink vs target, relative vs absolute) then hash to different
        // cache dirs and won't share the feature/thumbnail cache.
        tracing::debug!(path = %source.display(), %e, "canonicalize failed; using raw path for cache key");
        source.to_path_buf()
    });
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canon.hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("photo-pick")
        .join("cache")
        .join(key)
}

/// Kick off a scan in the blocking pool. Returns immediately with the run id;
/// poll `/api/runs/{id}` to follow status.
pub async fn scan(
    State(state): State<AppState>,
    Json(req): Json<ScanRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let source = match (&req.root, req.files.is_empty()) {
        (Some(r), true) => PhotoSource::Directory(r.clone()),
        (None, false) => PhotoSource::Files(req.files.clone()),
        (Some(_), false) => {
            return Err((StatusCode::BAD_REQUEST, "specify either `root` or `files`, not both".into()));
        }
        (None, true) => {
            return Err((StatusCode::BAD_REQUEST, "must specify `root` or `files`".into()));
        }
    };
    let display_root = source.root_hint();

    let run_id = Uuid::new_v4().to_string();

    // Internal artifacts live in the client-specified `output` if given, else a
    // managed per-source dir. Photos are never copied here — runs are always
    // analyze-only; exporting the keepers is the deferred `/export` action.
    let artifacts = req
        .output
        .clone()
        .unwrap_or_else(|| artifacts_dir(&display_root));

    let html_report_path = artifacts.join("report.html");
    let report_path = artifacts.join("report.json");
    let cache_path = artifacts.join(".cache.db");

    let record = RunRecord {
        id: run_id.clone(),
        root: display_root,
        output: artifacts.clone(),
        in_place: true,
        status: RunStatus::Running,
        report: None,
        html_report: None,
        composition_picks: vec![],
        photos: HashMap::new(),
        explanations: HashMap::new(),
    };
    state.insert_run(record).await;

    // Create a progress stream (broadcast + replay buffer). Late SSE
    // subscribers replay the buffer before receiving live updates so
    // cache-hit / fast scans aren't silent.
    let progress_stream = crate::state::ProgressStream::new();
    state
        .progress_streams
        .lock()
        .await
        .insert(run_id.clone(), progress_stream.clone());

    let runs = state.runs.clone();
    let semaphore = state.scan_semaphore.clone();
    let progress_streams = state.progress_streams.clone();
    let run_id_for_task = run_id.clone();
    let req_for_task = req;
    let source_for_task = source;
    let artifacts_for_task = artifacts;

    tokio::spawn(async move {
        // Wait for a scan-pipeline permit so N concurrent /api/scan POSTs
        // don't oversubscribe the blocking pool and starve thumbnail
        // requests. UI shows the run as `running` for the entire wait.
        let _permit = semaphore.acquire_owned().await.expect("semaphore closed");
        let runs_inner = runs.clone();
        let progress_stream_inner = progress_stream.clone();
        let run_id_for_blocking = run_id_for_task.clone();
        let _ = tokio::task::spawn_blocking(move || {
        let cfg = PipelineConfig {
            source: source_for_task,
            output: artifacts_for_task.clone(),
            report_path: Some(report_path),
            html_report_path: Some(html_report_path.clone()),
            cache_path: Some(cache_path),
            stage_a: StageAParams {
                k_time: req_for_task.time_k,
                min_dt: Duration::from_secs_f32(req_for_task.min_dt),
                max_dt: Duration::from_secs_f32(req_for_task.max_dt),
                max_hash_dist: req_for_task.hash_dist,
                clip_threshold: req_for_task.stage_a_clip_threshold,
            },
            stage_b: StageBParams {
                similarity_threshold: req_for_task.stage_b_threshold,
                chain_margin: StageBParams::default().chain_margin,
            },
            k1: req_for_task.k1,
            k2: match req_for_task.k2 {
                Some(0) | None => None,
                Some(k) => Some(k),
            },
            tech_weights: TechWeights::default(),
            link_mode: parse_link_mode(&req_for_task.link_mode),
            thumbnail: ThumbnailSpec {
                long_edge: req_for_task.thumbnail_long_edge.clamp(512, 4096),
            },
            dry_run: false,
            enable_clip: req_for_task.enable_clip,
            enable_face: req_for_task.enable_face,
            // Runs are always analyze-only now; the user exports keepers or
            // deletes rejects after reviewing, via the deferred endpoints.
            materialize_picks: false,
            execution_provider: parse_provider(&req_for_task.execution_provider),
            adaptive_thresholds: req_for_task.adaptive_thresholds,
            thumb_cache_dir: Some(artifacts_for_task.join(".thumbs")),
        };
        let pipeline = Pipeline::new(cfg);
        let sink = crate::state::ChannelProgressSink {
            stream: progress_stream_inner.clone(),
        };
        let result = pipeline.run(&sink);
        // Send a terminal Done so SSE clients know to stop subscribing.
        progress_stream_inner.record(crate::state::ProgressEvent::Done {
            ok: result.is_ok(),
        });
        let mut guard = runs_inner.blocking_lock();
        if let Some(rec) = guard.get_mut(&run_id_for_blocking) {
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
        }).await;
        // Keep the ProgressStream around for a short tail so late SSE
        // subscribers (UI tab focus, slow EventSource) can still replay the
        // terminal Done. Drop after a few seconds.
        let progress_streams2 = progress_streams.clone();
        let run_id_evict = run_id_for_task.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            progress_streams2.lock().await.remove(&run_id_evict);
        });
        drop(_permit);
    });

    Ok(Json(serde_json::json!({ "run_id": run_id })))
}

/// SSE stream of `ProgressEvent`s for a single run. New subscribers receive
/// the full history first (so cache-hit / fast scans that fired all their
/// events before the client subscribed aren't silent), then live updates.
pub async fn run_events(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> impl IntoResponse {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures_util::stream::StreamExt;
    use std::convert::Infallible;
    use tokio_stream::wrappers::BroadcastStream;

    // Subscribe BEFORE reading the snapshot — that way any event that fires
    // between snapshot read and broadcast subscription is delivered live
    // (with at most a harmless duplicate vs. the snapshot, which the UI
    // handles idempotently).
    let (snapshot, rx) = {
        let guard = state.progress_streams.lock().await;
        match guard.get(&run_id) {
            Some(stream) => (stream.snapshot(), Some(stream.tx.subscribe())),
            None => (vec![], None),
        }
    };

    let to_event = |ev: crate::state::ProgressEvent| {
        Ok::<_, Infallible>(
            Event::default()
                .event("progress")
                .json_data(ev)
                .unwrap_or_else(|_| Event::default().data("bad-event")),
        )
    };

    // History first (replay). If the run already finished and the stream
    // was evicted, fall back to a synthesized Done.
    let history_stream = if !snapshot.is_empty() {
        futures_util::stream::iter(snapshot.into_iter().map(to_event)).boxed()
    } else if rx.is_none() {
        futures_util::stream::iter(std::iter::once(to_event(
            crate::state::ProgressEvent::Done { ok: true },
        )))
        .boxed()
    } else {
        futures_util::stream::iter(std::iter::empty()).boxed()
    };

    let combined = if let Some(rx) = rx {
        let live = BroadcastStream::new(rx).filter_map(move |msg| async move {
            match msg {
                Ok(ev) => Some(to_event(ev)),
                Err(_) => None, // lagged / closed
            }
        });
        history_stream.chain(live).boxed()
    } else {
        history_stream
    };

    Sse::new(combined).keep_alive(KeepAlive::default()).into_response()
}

#[derive(Debug, Deserialize)]
pub struct BrowseQuery {
    /// Directory to list. If absent, defaults to `$HOME` (or `/` if no home).
    pub path: Option<PathBuf>,
}

#[derive(Debug, serde::Serialize)]
pub struct BrowseResponse {
    pub current: PathBuf,
    pub parent: Option<PathBuf>,
    pub dirs: Vec<BrowseEntry>,
    pub files: Vec<BrowseFile>,
}

#[derive(Debug, serde::Serialize)]
pub struct BrowseEntry {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, serde::Serialize)]
pub struct BrowseFile {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub format: String,
}

/// List photo-relevant entries in a server-side directory.
pub async fn browse(
    axum::extract::Query(q): axum::extract::Query<BrowseQuery>,
) -> Result<Json<BrowseResponse>, (StatusCode, String)> {
    // Default landing: /mnt on WSL (so users see /mnt/c, /mnt/d at a glance);
    // home dir elsewhere.
    let target = q.path.unwrap_or_else(|| {
        let mnt = PathBuf::from("/mnt");
        if cfg!(target_os = "linux") && mnt.is_dir() {
            // Heuristic: under WSL, /mnt is the Windows-drive root.
            mnt
        } else {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
        }
    });

    let canonical = match std::fs::canonicalize(&target) {
        Ok(p) => p,
        Err(e) => return Err((StatusCode::BAD_REQUEST, format!("{}: {e}", target.display()))),
    };

    let read = match std::fs::read_dir(&canonical) {
        Ok(r) => r,
        Err(e) => return Err((StatusCode::FORBIDDEN, format!("{}: {e}", canonical.display()))),
    };

    let mut dirs: Vec<BrowseEntry> = Vec::new();
    let mut files: Vec<BrowseFile> = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        let Some(name_os) = path.file_name() else { continue };
        let name = name_os.to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            dirs.push(BrowseEntry { name, path });
        } else if meta.is_file() {
            if let Some(format) = classify_extension(&path) {
                files.push(BrowseFile {
                    name,
                    path,
                    size: meta.len(),
                    format: format!("{:?}", format),
                });
            }
        }
    }

    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let parent = canonical.parent().map(|p| p.to_path_buf());
    Ok(Json(BrowseResponse {
        current: canonical,
        parent,
        dirs,
        files,
    }))
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
    serve_jpeg(state, run_id, photo_id, 256, 75).await
}

/// Stream a larger preview JPEG (default 1920px long edge, quality 88) for
/// the lightbox "view original" feature. Optional `?size=N` query param.
pub async fn get_preview(
    State(state): State<AppState>,
    Path((run_id, photo_id)): Path<(String, String)>,
    axum::extract::Query(q): axum::extract::Query<PreviewQuery>,
) -> impl IntoResponse {
    let size = q.size.unwrap_or(1920).clamp(512, 4096);
    serve_jpeg(state, run_id, photo_id, size, 88).await
}

#[derive(Debug, Deserialize)]
pub struct PreviewQuery {
    pub size: Option<u32>,
}

async fn serve_jpeg(
    state: AppState,
    run_id: String,
    photo_id: String,
    long_edge: u32,
    quality: u8,
) -> axum::response::Response {
    let parsed = match Uuid::parse_str(&photo_id) {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad photo id").into_response(),
    };
    let pid = photo_pick_core::ingest::PhotoId(parsed);

    // LRU lookup — for RAW this avoids 100s of ms of byte-scan + decode on
    // every grid render.
    let key = crate::state::ThumbKey { photo_id: pid, long_edge, quality };
    if let Some(bytes) = state.thumb_cache.get(&key) {
        return jpeg_response(bytes);
    }

    let (photo_ref, output_dir) = {
        let guard = state.runs.lock().await;
        let Some(rec) = guard.get(&run_id) else {
            return (StatusCode::NOT_FOUND, "run not found").into_response();
        };
        let Some(p) = rec.photos.get(&pid) else {
            return (StatusCode::NOT_FOUND, "photo not in run").into_response();
        };
        (p.clone(), rec.output.clone())
    };

    // Disk thumb cache (populated by the pipeline during scan). Hits skip the
    // RAW byte-scan entirely — only matters when `long_edge` matches the
    // cache's spec; otherwise fall through to decode-and-encode.
    let sha = photo_ref.sha256_short;
    let thumb_cache_disk = photo_pick_core::output::ThumbDiskCache::new(
        output_dir.join(".thumbs"),
        photo_pick_core::output::DEFAULT_THUMB_LONG_EDGE,
        photo_pick_core::output::DEFAULT_THUMB_QUALITY,
    );
    let cached_long_edge = thumb_cache_disk.spec().long_edge;
    let cached_quality = thumb_cache_disk.quality();
    if long_edge <= cached_long_edge && quality <= cached_quality {
        if let Some(bytes) = thumb_cache_disk.read(&sha) {
            state.thumb_cache.put(key.clone(), bytes.clone());
            return jpeg_response(bytes);
        }
    }

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let img = decode_thumbnail_for(&photo_ref, ThumbnailSpec { long_edge })
            .map_err(|e| e.to_string())?;
        encode_jpeg(&img, quality).map_err(|e| e.to_string())
    })
    .await;

    match result {
        Ok(Ok(bytes)) => {
            state.thumb_cache.put(key, bytes.clone());
            jpeg_response(bytes)
        }
        Ok(Err(msg)) => (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("task: {e}")).into_response(),
    }
}

fn jpeg_response(bytes: Vec<u8>) -> axum::response::Response {
    (
        [
            ("content-type", "image/jpeg".to_string()),
            ("cache-control", "max-age=3600".to_string()),
        ],
        bytes,
    )
        .into_response()
}

pub async fn list_runs(State(state): State<AppState>) -> Json<serde_json::Value> {
    let guard = state.runs.lock().await;
    let runs: Vec<&RunRecord> = guard.values().collect();
    Json(serde_json::to_value(runs).unwrap())
}

/// Capability endpoint: which ONNX execution providers are actually compiled
/// into this build. The UI uses this to filter the provider dropdown so a
/// user on a CPU-only build doesn't pick "CUDA" and silently fall back.
pub async fn list_providers() -> Json<serde_json::Value> {
    use photo_pick_core::models::{available_providers, ExecutionProvider};
    let to_str = |ep: ExecutionProvider| match ep {
        ExecutionProvider::Cpu => "cpu",
        ExecutionProvider::Cuda => "cuda",
        ExecutionProvider::CoreMl => "coreml",
        ExecutionProvider::DirectMl => "directml",
    };
    let providers: Vec<&'static str> = available_providers().into_iter().map(to_str).collect();
    Json(serde_json::json!({ "providers": providers }))
}

#[derive(Debug, Deserialize)]
pub struct ExplainRequest {
    /// Index into the run's composition_picks vector.
    pub composition_index: usize,
    /// "openai" or "anthropic" (default: "openai"). Used when `vlm` is None.
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Optional per-request VLM override. When present, instantiates a provider
    /// with these values directly instead of reading server environment vars.
    /// Lets the UI store its own keys / point at any OpenAI-compatible service.
    #[serde(default)]
    pub vlm: Option<VlmConfig>,
    /// UI language code ("en"/"zh") — the prompt asks the model to respond
    /// in this language. Defaults to English.
    #[serde(default = "default_lang")]
    pub language: String,
}

fn default_lang() -> String { "en".into() }

#[derive(Debug, Deserialize)]
pub struct VlmConfig {
    /// "openai" (OpenAI-compatible chat completions) or "anthropic" (Messages API).
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
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
    // The cache key incorporates the resolved provider name + optional model,
    // so flipping providers (openai → anthropic) or models doesn't return the
    // stale prior answer.
    let cache_provider_key = match &req.vlm {
        Some(v) => format!("{}:{}", v.provider, v.model),
        None => req.provider.clone(),
    };
    let snapshot = {
        let guard = state.runs.lock().await;
        let Some(rec) = guard.get(&id) else {
            return (StatusCode::NOT_FOUND, "run not found").into_response();
        };
        if let Some(cached) = rec.explanations.get(&req.composition_index) {
            if cached.provider == cache_provider_key {
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
    let vlm_override = req.vlm;
    let cache_key_for_task = cache_provider_key.clone();
    let language = req.language.clone();
    let index = req.composition_index;
    let runs = state.runs.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<ExplanationRecord, String> {
        let provider: Box<dyn VlmProvider> = match &vlm_override {
            Some(v) => match v.provider.as_str() {
                "openai" => Box::new(
                    OpenAiProvider::from_parts(v.api_key.clone(), v.model.clone(), v.base_url.clone())
                ),
                "anthropic" => Box::new(
                    AnthropicProvider::from_parts(v.api_key.clone(), v.model.clone(), v.base_url.clone())
                ),
                other => return Err(format!("unknown provider: {other}")),
            },
            None => match provider_name.as_str() {
                "openai" => Box::new(OpenAiProvider::from_env().map_err(|e| e.to_string())?),
                "anthropic" => Box::new(AnthropicProvider::from_env().map_err(|e| e.to_string())?),
                other => return Err(format!("unknown provider: {other}")),
            },
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

        let prompt = explain_group_prompt(&scene, kept_count, total, &language);
        let req = VlmRequest::new(prompt, images);
        let text = provider.complete(&req).map_err(|e| e.to_string())?;
        Ok(ExplanationRecord {
            // Provider field doubles as cache key; embed model so swaps invalidate.
            provider: cache_key_for_task,
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

#[derive(Debug, Deserialize)]
pub struct ApplyRequest {
    /// Photos the user wants to delete (typically the algorithm's "rejected"
    /// set minus any manually-overridden keeps). Each entry is a PhotoId UUID.
    pub delete_ids: Vec<String>,
    /// When true, items go to the OS trash (recoverable). When false, they
    /// are permanently deleted via `fs::remove_file`.
    #[serde(default = "default_use_trash")]
    pub use_trash: bool,
}

fn default_use_trash() -> bool { true }

#[derive(Debug, serde::Serialize)]
pub struct ApplyResult {
    pub requested: usize,
    pub deleted: usize,
    pub failed: Vec<ApplyFailure>,
    pub used_trash: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct ApplyFailure {
    pub photo_id: String,
    pub path: PathBuf,
    pub error: String,
}

/// Destructively apply the user's selections: for each photo id in
/// `delete_ids`, send the source file to the OS trash (or delete outright).
pub async fn apply(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(req): Json<ApplyRequest>,
) -> impl IntoResponse {
    // Resolve photo ids to paths under the lock, then release before doing I/O.
    let resolved: Vec<(String, PathBuf)> = {
        let guard = state.runs.lock().await;
        let Some(rec) = guard.get(&run_id) else {
            return (StatusCode::NOT_FOUND, "run not found").into_response();
        };
        let mut out = Vec::with_capacity(req.delete_ids.len());
        for id_str in &req.delete_ids {
            let Ok(uuid) = Uuid::parse_str(id_str) else {
                return (StatusCode::BAD_REQUEST, format!("bad photo id: {id_str}")).into_response();
            };
            let pid = photo_pick_core::ingest::PhotoId(uuid);
            match rec.photos.get(&pid) {
                Some(p) => out.push((id_str.clone(), p.path.clone())),
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        format!("photo {id_str} not in run"),
                    )
                        .into_response()
                }
            }
        }
        out
    };

    let use_trash = req.use_trash;
    let result = tokio::task::spawn_blocking(move || -> ApplyResult {
        let mut deleted = 0;
        let mut failed: Vec<ApplyFailure> = Vec::new();
        for (id, path) in &resolved {
            let outcome = if use_trash {
                trash::delete(path).map_err(|e| e.to_string())
            } else {
                std::fs::remove_file(path).map_err(|e| e.to_string())
            };
            match outcome {
                Ok(()) => deleted += 1,
                Err(e) => failed.push(ApplyFailure {
                    photo_id: id.clone(),
                    path: path.clone(),
                    error: e,
                }),
            }
        }
        ApplyResult {
            requested: resolved.len(),
            deleted,
            failed,
            used_trash: use_trash,
        }
    })
    .await;

    match result {
        Ok(r) => Json(serde_json::to_value(r).unwrap()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("apply task: {e}")).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ExportRequest {
    /// Photos to copy out — typically the final kept set after the user's
    /// keep/drop overrides. Each entry is a PhotoId UUID.
    pub photo_ids: Vec<String>,
    /// Destination directory; created if it doesn't exist.
    pub target_dir: PathBuf,
    /// "copy" | "hardlink" | "symlink". Defaults to copy — exports commonly
    /// cross filesystems where hardlink fails (place_file falls back anyway).
    #[serde(default)]
    pub link_mode: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ExportResult {
    pub requested: usize,
    pub exported: usize,
    pub failed: Vec<ApplyFailure>,
    pub target_dir: PathBuf,
}

/// Pick a non-colliding destination path inside `dir` for `name`. Flat layout
/// means two same-named originals from different bursts would clobber, so when
/// a name is already taken we append ` (2)`, ` (3)`… before the extension.
/// `used` is the authority (seed it with the target dir's existing entries to
/// avoid clobbering prior contents); membership check + reserve are combined
/// via `HashSet::insert`.
fn unique_dest(
    dir: &std::path::Path,
    name: &std::ffi::OsStr,
    used: &mut std::collections::HashSet<PathBuf>,
) -> PathBuf {
    let base = dir.join(name);
    if used.insert(base.clone()) {
        return base;
    }
    let p = std::path::Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = p.extension().and_then(|s| s.to_str());
    let mut n = 2u32;
    loop {
        let candidate = match ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let cand = dir.join(candidate);
        if used.insert(cand.clone()) {
            return cand;
        }
        n += 1;
    }
}

/// Copy the requested photos out of the run's source into `target_dir`, flat.
/// Non-destructive counterpart to `apply` — the originals are untouched.
pub async fn export(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(req): Json<ExportRequest>,
) -> impl IntoResponse {
    // Resolve photo ids to source paths under the lock, then release for I/O.
    let resolved: Vec<(String, PathBuf)> = {
        let guard = state.runs.lock().await;
        let Some(rec) = guard.get(&run_id) else {
            return (StatusCode::NOT_FOUND, "run not found").into_response();
        };
        let mut out = Vec::with_capacity(req.photo_ids.len());
        for id_str in &req.photo_ids {
            let Ok(uuid) = Uuid::parse_str(id_str) else {
                return (StatusCode::BAD_REQUEST, format!("bad photo id: {id_str}")).into_response();
            };
            let pid = photo_pick_core::ingest::PhotoId(uuid);
            match rec.photos.get(&pid) {
                Some(p) => out.push((id_str.clone(), p.path.clone())),
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        format!("photo {id_str} not in run"),
                    )
                        .into_response()
                }
            }
        }
        out
    };

    let mode = parse_link_mode(req.link_mode.as_deref().unwrap_or("copy"));
    let target = req.target_dir.clone();
    let result = tokio::task::spawn_blocking(move || -> std::result::Result<ExportResult, String> {
        std::fs::create_dir_all(&target).map_err(|e| format!("create target dir: {e}"))?;
        // Seed `used` with existing entries so we don't clobber prior contents.
        let mut used: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        if let Ok(rd) = std::fs::read_dir(&target) {
            for entry in rd.flatten() {
                used.insert(entry.path());
            }
        }
        let mut exported = 0usize;
        let mut failed: Vec<ApplyFailure> = Vec::new();
        for (id, src) in &resolved {
            let name = src
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("photo"));
            let dest = unique_dest(&target, name, &mut used);
            match photo_pick_core::output::place_file(src, &dest, mode) {
                Ok(()) => exported += 1,
                Err(e) => failed.push(ApplyFailure {
                    photo_id: id.clone(),
                    path: dest,
                    error: e.to_string(),
                }),
            }
        }
        Ok(ExportResult {
            requested: resolved.len(),
            exported,
            failed,
            target_dir: target,
        })
    })
    .await;

    match result {
        Ok(Ok(r)) => Json(serde_json::to_value(r).unwrap()).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("export task: {e}")).into_response(),
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

#[cfg(test)]
mod tests {
    use super::unique_dest;
    use std::collections::HashSet;
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    #[test]
    fn unique_dest_disambiguates_collisions() {
        let dir = Path::new("/out");
        let mut used: HashSet<PathBuf> = HashSet::new();
        assert_eq!(
            unique_dest(dir, OsStr::new("a.jpg"), &mut used),
            PathBuf::from("/out/a.jpg")
        );
        assert_eq!(
            unique_dest(dir, OsStr::new("a.jpg"), &mut used),
            PathBuf::from("/out/a (2).jpg")
        );
        assert_eq!(
            unique_dest(dir, OsStr::new("a.jpg"), &mut used),
            PathBuf::from("/out/a (3).jpg")
        );
        // Distinct names don't collide.
        assert_eq!(
            unique_dest(dir, OsStr::new("b.jpg"), &mut used),
            PathBuf::from("/out/b.jpg")
        );
        // Extension-less names disambiguate too.
        assert_eq!(
            unique_dest(dir, OsStr::new("README"), &mut used),
            PathBuf::from("/out/README")
        );
        assert_eq!(
            unique_dest(dir, OsStr::new("README"), &mut used),
            PathBuf::from("/out/README (2)")
        );
    }

    #[test]
    fn unique_dest_respects_preexisting_seed() {
        let dir = Path::new("/out");
        let mut used: HashSet<PathBuf> = HashSet::new();
        // Seed as if /out/a.jpg already exists on disk.
        used.insert(PathBuf::from("/out/a.jpg"));
        assert_eq!(
            unique_dest(dir, OsStr::new("a.jpg"), &mut used),
            PathBuf::from("/out/a (2).jpg")
        );
    }
}
