use photo_pick_core::group::CompositionGroup;
use photo_pick_core::ingest::{ImageFormat, PhotoId, PhotoRef, RawKind};
use photo_pick_core::pipeline::{PipelineReport, ProgressSink, Stage};
use photo_pick_core::scoring::{CompositionPick, FinalScore};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as SyncMutex};
use tokio::sync::{broadcast, Mutex, Semaphore};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Completed,
    Failed { error: String },
    /// Stopped early by an explicit user request (POST /api/runs/:id/cancel)
    /// or server shutdown. Distinct from `Failed` so the UI can render it as
    /// a neutral outcome instead of an error banner.
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunRecord {
    pub id: String,
    pub root: PathBuf,
    pub output: PathBuf,
    /// In-place mode: no `picked/`/`rejected/` materialized; user will apply
    /// selections destructively to the source via `/api/runs/:id/apply`.
    pub in_place: bool,
    pub status: RunStatus,
    pub report: Option<PipelineReport>,
    /// Path to the on-disk HTML report (when the run completed and one was
    /// requested).
    pub html_report: Option<PathBuf>,
    /// Composition picks used by the VLM `explain` endpoint. Skipped from the
    /// JSON list view to keep responses small.
    #[serde(skip)]
    pub composition_picks: Vec<CompositionPick>,
    #[serde(skip)]
    pub photos: HashMap<PhotoId, PhotoRef>,
    /// Cached VLM explanations keyed by composition group index.
    pub explanations: HashMap<usize, ExplanationRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExplanationRecord {
    pub provider: String,
    pub model: String,
    pub text: String,
}

/// Bounded in-memory LRU for rendered JPEG previews / thumbnails. Keyed on
/// `(photo_id, long_edge, quality)` so different size requests don't collide.
/// Prevents the `/thumb` and `/preview` endpoints from re-decoding the source
/// (which for NEFs scans up to 128MB of bytes) on every request.
pub struct ThumbCache {
    inner: SyncMutex<ThumbCacheInner>,
    max_bytes: usize,
}

struct ThumbCacheInner {
    map: HashMap<ThumbKey, Vec<u8>>,
    order: VecDeque<ThumbKey>,
    bytes: usize,
}

#[derive(Clone, Hash, Eq, PartialEq)]
pub struct ThumbKey {
    pub photo_id: PhotoId,
    pub long_edge: u32,
    pub quality: u8,
}

impl ThumbCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            inner: SyncMutex::new(ThumbCacheInner {
                map: HashMap::new(),
                order: VecDeque::new(),
                bytes: 0,
            }),
            max_bytes,
        }
    }

    pub fn get(&self, key: &ThumbKey) -> Option<Vec<u8>> {
        let mut g = self.inner.lock().ok()?;
        if let Some(bytes) = g.map.get(key).cloned() {
            // Touch — move to back (most recently used).
            if let Some(pos) = g.order.iter().position(|k| k == key) {
                g.order.remove(pos);
                g.order.push_back(key.clone());
            }
            return Some(bytes);
        }
        None
    }

    pub fn put(&self, key: ThumbKey, bytes: Vec<u8>) {
        let Ok(mut g) = self.inner.lock() else { return };
        let sz = bytes.len();
        if let Some(old) = g.map.insert(key.clone(), bytes) {
            g.bytes = g.bytes.saturating_sub(old.len());
            if let Some(pos) = g.order.iter().position(|k| k == &key) {
                g.order.remove(pos);
            }
        }
        g.bytes += sz;
        g.order.push_back(key);
        // Evict LRU while over budget.
        while g.bytes > self.max_bytes {
            let Some(victim) = g.order.pop_front() else { break };
            if let Some(v) = g.map.remove(&victim) {
                g.bytes = g.bytes.saturating_sub(v.len());
            }
        }
    }
}

/// One progress event from the pipeline's `ProgressSink`. Serialized to SSE
/// `data:` payloads.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProgressEvent {
    Stage {
        stage: String,
        total: u64,
    },
    Tick {
        stage: String,
        done: u64,
    },
    Finish {
        stage: String,
    },
    /// Terminal — the run completed or failed; sink drops its sender right
    /// after sending this so subscribers see the stream close.
    Done {
        ok: bool,
    },
}

fn stage_name(s: Stage) -> &'static str {
    match s {
        Stage::Scan => "scan",
        Stage::Features => "features",
        Stage::Cluster => "cluster",
        Stage::Score => "score",
        Stage::StageB => "stage_b",
        Stage::FinalSelect => "final_select",
        Stage::Write => "write",
    }
}

/// Broadcast channel + replay log for a single run. New SSE subscribers
/// receive the full history first (so they don't miss events fired between
/// the `POST /api/scan` returning and the client's `EventSource` connecting),
/// then live updates. For cache-hit scans that finish in <500ms this is the
/// difference between "saw 0 events" and "saw the whole timeline".
#[derive(Clone)]
pub struct ProgressStream {
    pub tx: broadcast::Sender<ProgressEvent>,
    /// Bounded history. `Tick` events are coalesced (last tick per stage)
    /// so a long Features stage with 1000 photos doesn't blow this up.
    pub history: Arc<SyncMutex<ProgressHistory>>,
}

#[derive(Default)]
pub struct ProgressHistory {
    pub stages: Vec<ProgressEvent>,
    /// Most-recent Tick per stage, keyed by stage name. Replayed after the
    /// stage's `Stage` event so a late-joiner sees current progress without
    /// every intermediate tick.
    pub last_tick: std::collections::HashMap<String, ProgressEvent>,
    pub done: Option<ProgressEvent>,
}

impl ProgressStream {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            tx,
            history: Arc::new(SyncMutex::new(ProgressHistory::default())),
        }
    }

    pub fn record(&self, ev: ProgressEvent) {
        if let Ok(mut h) = self.history.lock() {
            match &ev {
                ProgressEvent::Stage { .. } | ProgressEvent::Finish { .. } => {
                    h.stages.push(ev.clone());
                }
                ProgressEvent::Tick { stage, .. } => {
                    h.last_tick.insert(stage.clone(), ev.clone());
                }
                ProgressEvent::Done { .. } => {
                    h.done = Some(ev.clone());
                }
            }
        }
        let _ = self.tx.send(ev);
    }

    /// Build a snapshot vec for a late subscriber: every Stage/Finish in
    /// order, plus each stage's most recent Tick, plus terminal Done if any.
    pub fn snapshot(&self) -> Vec<ProgressEvent> {
        let Ok(h) = self.history.lock() else { return vec![] };
        let mut out: Vec<ProgressEvent> = Vec::with_capacity(h.stages.len() + h.last_tick.len() + 1);
        for ev in &h.stages {
            out.push(ev.clone());
            if let ProgressEvent::Stage { stage, .. } = ev {
                if let Some(t) = h.last_tick.get(stage) {
                    out.push(t.clone());
                }
            }
        }
        if let Some(d) = &h.done {
            out.push(d.clone());
        }
        out
    }
}

/// `ProgressSink` implementation that records events into a `ProgressStream`
/// (history + broadcast). Sync (called from the pipeline's rayon workers).
pub struct ChannelProgressSink {
    pub stream: ProgressStream,
}

impl ProgressSink for ChannelProgressSink {
    fn on_stage(&self, stage: Stage, total: u64) {
        self.stream.record(ProgressEvent::Stage {
            stage: stage_name(stage).into(),
            total,
        });
    }
    fn on_tick(&self, stage: Stage, done: u64) {
        self.stream.record(ProgressEvent::Tick {
            stage: stage_name(stage).into(),
            done,
        });
    }
    fn on_finish(&self, stage: Stage) {
        self.stream.record(ProgressEvent::Finish {
            stage: stage_name(stage).into(),
        });
    }
}

#[derive(Clone)]
pub struct AppState {
    pub runs: Arc<Mutex<HashMap<String, RunRecord>>>,
    /// One stream per run. Carries both the live broadcast channel and a
    /// replay buffer so late SSE subscribers see the timeline from t=0.
    pub progress_streams: Arc<Mutex<HashMap<String, ProgressStream>>>,
    /// Insertion order — first element is the oldest run. Used to evict the
    /// least-recently-inserted completed run when `runs` grows past
    /// `max_runs`. Insert order is "good enough" given runs are append-only
    /// (no in-place mutation triggers a reorder) and the photographer's
    /// mental model is "old runs go away first".
    pub run_order: Arc<Mutex<VecDeque<String>>>,
    /// Soft cap on retained runs in memory. Tunable via PHOTO_PICK_MAX_RUNS
    /// (default 50). Currently-running runs are never evicted.
    pub max_runs: usize,
    /// Bounds concurrent scan pipelines so N parallel /api/scan POSTs don't
    /// oversubscribe the blocking pool and starve thumbnail / detail
    /// requests. Configurable via PHOTO_PICK_SCAN_CONCURRENCY (default 2).
    pub scan_semaphore: Arc<Semaphore>,
    /// Bounds concurrent image decode work for `/thumb` + `/preview`. Without
    /// this, fast-scrolling a 1000-item grid can spawn ~1000 simultaneous
    /// `spawn_blocking` decodes — tokio's default blocking pool is 512, so
    /// the run-completion writes (also `spawn_blocking`) get starved. The
    /// permit is held across the decode; cache hits skip it. Configurable
    /// via `PHOTO_PICK_IMAGE_DECODE_CONCURRENCY` (default `num_cpus`).
    pub image_decode_semaphore: Arc<Semaphore>,
    /// Shared rendered-JPEG cache for /thumb and /preview.
    pub thumb_cache: Arc<ThumbCache>,
    /// Where the on-disk run index lives (`runs.json`). `None` if no platform
    /// data dir is available (rare — usually a misconfigured CI env).
    pub runs_index_path: Option<Arc<PathBuf>>,
    /// Per-run mutex guarding `ensure_rehydrated`. Without this a fresh
    /// detail page firing N parallel `/thumb` requests on a stub run all
    /// race to read + parse the same (potentially multi-MB) `report.json`.
    /// The inner Mutex is held only for the rehydrate work; outer Mutex is
    /// just for the lookup/insert into the per-id map.
    pub rehydrate_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    /// Per-run cancellation flags. Inserted when a scan starts, flipped by
    /// POST /api/runs/:id/cancel (or server shutdown), observed by
    /// `Pipeline::run_with_cancel` at stage boundaries and per photo in the
    /// feature-extraction loop. Removed when the run reaches a terminal state.
    pub cancel_flags: Arc<Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>>,
    /// Bearer token required on every /api route when set (PHOTO_PICK_TOKEN).
    /// `None` = open access — fine on localhost, dangerous on 0.0.0.0
    /// (main.rs prints a loud warning for that combination). Run ids are
    /// listable via /api/runs, so without this gate any LAN client could
    /// chain list→detail→apply and delete the user's photos.
    pub api_token: Option<Arc<str>>,
}

impl AppState {
    pub fn new() -> Self {
        let scan_concurrency = std::env::var("PHOTO_PICK_SCAN_CONCURRENCY")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(2);
        let thumb_cache_mb = std::env::var("PHOTO_PICK_THUMB_CACHE_MB")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(256);
        let max_runs = std::env::var("PHOTO_PICK_MAX_RUNS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(50);
        let image_decode_concurrency = std::env::var("PHOTO_PICK_IMAGE_DECODE_CONCURRENCY")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(8)
            });
        let runs_index_path = dirs::data_dir()
            .map(|d| Arc::new(d.join("photo-pick").join("runs.json")));
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            progress_streams: Arc::new(Mutex::new(HashMap::new())),
            run_order: Arc::new(Mutex::new(VecDeque::new())),
            max_runs,
            scan_semaphore: Arc::new(Semaphore::new(scan_concurrency)),
            image_decode_semaphore: Arc::new(Semaphore::new(image_decode_concurrency)),
            thumb_cache: Arc::new(ThumbCache::new(thumb_cache_mb * 1024 * 1024)),
            runs_index_path,
            rehydrate_locks: Arc::new(Mutex::new(HashMap::new())),
            cancel_flags: Arc::new(Mutex::new(HashMap::new())),
            api_token: std::env::var("PHOTO_PICK_TOKEN")
                .ok()
                .filter(|t| !t.is_empty())
                .map(|t| Arc::from(t.as_str())),
        }
    }

    /// Flip every live cancellation flag. Called on graceful shutdown so
    /// in-flight pipelines stop at their next checkpoint instead of burning
    /// CPU on a scan whose server is going away.
    pub async fn cancel_all_runs(&self) {
        let flags = self.cancel_flags.lock().await;
        for (run_id, flag) in flags.iter() {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
            tracing::info!(run_id, "cancelling run for shutdown");
        }
    }

    /// Read the on-disk run index (if any) and seed `runs` with stubs for
    /// every completed/failed run so the UI shows past work after a server
    /// restart. Stubs carry no `composition_picks`/`photos`/`report` — those
    /// are lazily restored on detail access via [`ensure_rehydrated`].
    pub async fn load_from_disk(&self) {
        let Some(path) = self.runs_index_path.as_deref() else { return };
        let bytes = match tokio::fs::read(path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::warn!("runs index unreadable at {}: {e}", path.display());
                return;
            }
        };
        let parsed: PersistedRunsFile = match serde_json::from_slice(&bytes) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("runs index parse failed ({e}); ignoring");
                return;
            }
        };
        let mut runs = self.runs.lock().await;
        let mut order = self.run_order.lock().await;
        for entry in parsed.runs {
            let rec = RunRecord {
                id: entry.id.clone(),
                root: entry.root,
                output: entry.output,
                in_place: entry.in_place,
                status: entry.status,
                report: entry.report,
                html_report: entry.html_report,
                composition_picks: vec![],
                photos: HashMap::new(),
                explanations: HashMap::new(),
            };
            runs.insert(entry.id.clone(), rec);
            order.push_back(entry.id);
        }
        tracing::info!("restored {} runs from {}", runs.len(), path.display());
    }

    /// Atomically rewrite the runs index from current in-memory state. Skips
    /// `Running` records (they can't survive a restart). Fire-and-forget —
    /// errors are logged but never bubble up to the request that triggered
    /// the persist.
    pub async fn persist_runs(&self) {
        let Some(path) = self.runs_index_path.clone() else { return };
        let runs = self.runs.lock().await;
        let order = self.run_order.lock().await;
        let mut entries: Vec<PersistedRun> = Vec::with_capacity(runs.len());
        for id in order.iter() {
            let Some(rec) = runs.get(id) else { continue };
            if matches!(rec.status, RunStatus::Running) {
                continue;
            }
            entries.push(PersistedRun {
                id: rec.id.clone(),
                root: rec.root.clone(),
                output: rec.output.clone(),
                in_place: rec.in_place,
                status: rec.status.clone(),
                report: rec.report.clone(),
                html_report: rec.html_report.clone(),
            });
        }
        drop(order);
        drop(runs);
        let file = PersistedRunsFile { version: 1, runs: entries };
        if let Err(e) = write_runs_index(&path, &file).await {
            tracing::warn!("persist runs index to {}: {e}", path.display());
        }
    }

    /// Ensure the in-memory `RunRecord` has its `composition_picks`/`photos`
    /// populated, lazily reading `report.json` from disk if needed. Safe to
    /// call repeatedly — no-ops once rehydrated. Only meaningful for
    /// Completed runs.
    ///
    /// Serialized per-run: N concurrent callers on the same stub run wait on
    /// a single shared mutex so they don't all parse the same multi-MB JSON
    /// in parallel. After the first one wins, the rest see populated picks
    /// inside the lock and bail.
    pub async fn ensure_rehydrated(&self, run_id: &str) {
        // First quick check — already rehydrated or nothing to do — without
        // taking the per-run lock (avoids overhead on the hot path where
        // the run has been touched before).
        {
            let runs = self.runs.lock().await;
            let Some(rec) = runs.get(run_id) else { return };
            if !rec.composition_picks.is_empty() {
                return;
            }
            if !matches!(rec.status, RunStatus::Completed) {
                return;
            }
        }
        let lock = {
            let mut locks = self.rehydrate_locks.lock().await;
            locks
                .entry(run_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        // Double-check inside the per-run lock: a peer may have just
        // finished rehydrating while we waited.
        let output = {
            let runs = self.runs.lock().await;
            let Some(rec) = runs.get(run_id) else { return };
            if !rec.composition_picks.is_empty() {
                return;
            }
            rec.output.clone()
        };
        let report_path = output.join("report.json");
        let bytes = match tokio::fs::read(&report_path).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("rehydrate {run_id}: report.json read failed: {e}");
                return;
            }
        };
        let parsed: PersistedReport = match serde_json::from_slice(&bytes) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("rehydrate {run_id}: report.json parse failed: {e}");
                return;
            }
        };
        let (picks, photos) = match parsed.into_runtime() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("rehydrate {run_id}: convert failed: {e}");
                return;
            }
        };
        let mut runs = self.runs.lock().await;
        if let Some(rec) = runs.get_mut(run_id) {
            if rec.composition_picks.is_empty() {
                rec.composition_picks = picks;
                rec.photos = photos;
            }
        }
    }
}

/// On-disk run index: small metadata only. The heavy `composition_picks`
/// and `photos` map live in each run's own `report.json` and are lazily
/// re-read on demand.
#[derive(Serialize, Deserialize)]
struct PersistedRunsFile {
    version: u32,
    runs: Vec<PersistedRun>,
}

#[derive(Serialize, Deserialize)]
struct PersistedRun {
    id: String,
    root: PathBuf,
    output: PathBuf,
    in_place: bool,
    status: RunStatus,
    report: Option<PipelineReport>,
    html_report: Option<PathBuf>,
}

async fn write_runs_index(path: &Path, file: &PersistedRunsFile) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let bytes = serde_json::to_vec_pretty(file)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, &bytes).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

/// Owned mirror of [`photo_pick_core::output::report::JsonReport`] for
/// deserialization — the core type borrows `&Path` and so can't be parsed
/// back. We only pluck the fields needed to rebuild composition_picks +
/// a minimal photo map (path + format from filename).
#[derive(Deserialize)]
struct PersistedReport {
    composition_groups: Vec<PersistedCompositionGroup>,
}

#[derive(Deserialize)]
struct PersistedCompositionGroup {
    id: String,
    member_ids: Vec<String>,
    picks: Vec<PersistedPick>,
}

#[derive(Deserialize)]
struct PersistedPick {
    id: String,
    path: PathBuf,
    verdict: PersistedVerdict,
    final_score: FinalScore,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum PersistedVerdict { Kept, Rejected, Unscored }

impl PersistedReport {
    /// Rebuild runtime types from the on-disk report. The `photos` map is
    /// the minimal subset needed by the apply/export/thumb paths — `path`
    /// is correct; sha256/exif/burst metadata is reset because they're not
    /// in the JSON (thumbs degrade to decode-from-source, which is fine).
    fn into_runtime(self) -> anyhow::Result<(Vec<CompositionPick>, HashMap<PhotoId, PhotoRef>)> {
        use std::str::FromStr;
        use uuid::Uuid;
        let mut picks = Vec::with_capacity(self.composition_groups.len());
        let mut photos: HashMap<PhotoId, PhotoRef> = HashMap::new();
        for g in self.composition_groups {
            let group_uuid = Uuid::from_str(&g.id)
                .map_err(|e| anyhow::anyhow!("group id {} not a uuid: {e}", g.id))?;
            let mut photo_ids = Vec::with_capacity(g.member_ids.len());
            for m in &g.member_ids {
                let pid = PhotoId(Uuid::from_str(m)
                    .map_err(|e| anyhow::anyhow!("member id {m} not a uuid: {e}"))?);
                photo_ids.push(pid);
            }
            let mut kept = Vec::new();
            let mut rejected = Vec::new();
            for p in g.picks {
                let pid = PhotoId(Uuid::from_str(&p.id)
                    .map_err(|e| anyhow::anyhow!("pick id {} not a uuid: {e}", p.id))?);
                let format = guess_format_from_path(&p.path);
                photos.entry(pid).or_insert(PhotoRef {
                    id: pid,
                    path: p.path.clone(),
                    format,
                    captured_at: None,
                    file_size: 0,
                    sha256_short: [0u8; 16],
                    burst_id: None,
                    drive_mode: None,
                    iso: None,
                    exposure_bias_ev: None,
                });
                match p.verdict {
                    PersistedVerdict::Kept => kept.push((pid, p.final_score)),
                    PersistedVerdict::Rejected => rejected.push((pid, p.final_score)),
                    PersistedVerdict::Unscored => {}
                }
            }
            picks.push(CompositionPick {
                group: CompositionGroup {
                    id: photo_pick_core::group::GroupId(group_uuid),
                    photo_ids,
                },
                kept,
                rejected,
            });
        }
        Ok((picks, photos))
    }
}

fn guess_format_from_path(path: &Path) -> ImageFormat {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("nef") => ImageFormat::Raw(RawKind::Nef),
        Some("cr2") => ImageFormat::Raw(RawKind::Cr2),
        Some("cr3") => ImageFormat::Raw(RawKind::Cr3),
        Some("arw") => ImageFormat::Raw(RawKind::Arw),
        Some("dng") => ImageFormat::Raw(RawKind::Dng),
        Some("pef") => ImageFormat::Raw(RawKind::Pef),
        Some("orf") => ImageFormat::Raw(RawKind::Orf),
        Some("raf") => ImageFormat::Raw(RawKind::Raf),
        // Anything else (jpeg/jpg/png/heic/...) — the thumb decoder picks
        // by content sniffing anyway, so Jpeg is a fine default.
        _ => ImageFormat::Jpeg,
    }
}

impl AppState {
    /// Register a new run + enforce the LRU cap. Evicts non-running records
    /// from the oldest end until we're at or below `max_runs`. Also drops the
    /// run's `progress_streams` entry — otherwise the channel + replay buffer
    /// outlive the `RunRecord` until the 10s SSE tail task fires, which under
    /// churn leaks memory proportional to scans-per-10s.
    pub async fn insert_run(&self, record: RunRecord) {
        let id = record.id.clone();
        let mut runs = self.runs.lock().await;
        let mut order = self.run_order.lock().await;
        let mut streams = self.progress_streams.lock().await;
        runs.insert(id.clone(), record);
        order.push_back(id);
        while runs.len() > self.max_runs {
            let Some(victim) = order.pop_front() else { break };
            if runs
                .get(&victim)
                .map(|r| matches!(r.status, RunStatus::Running))
                .unwrap_or(false)
            {
                // Don't evict a running scan; push it back to the tail.
                order.push_back(victim);
                if order.len() == runs.len() {
                    // Whole map is currently running — give up evicting.
                    break;
                }
                continue;
            }
            runs.remove(&victim);
            streams.remove(&victim);
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
