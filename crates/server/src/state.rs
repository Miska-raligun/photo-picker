use photo_pick_core::ingest::{PhotoId, PhotoRef};
use photo_pick_core::pipeline::PipelineReport;
use photo_pick_core::scoring::CompositionPick;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as SyncMutex};
use tokio::sync::{Mutex, Semaphore};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Completed,
    Failed { error: String },
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

#[derive(Clone)]
pub struct AppState {
    pub runs: Arc<Mutex<HashMap<String, RunRecord>>>,
    /// Bounds concurrent scan pipelines so N parallel /api/scan POSTs don't
    /// oversubscribe the blocking pool and starve thumbnail / detail
    /// requests. Configurable via PHOTO_PICK_SCAN_CONCURRENCY (default 2).
    pub scan_semaphore: Arc<Semaphore>,
    /// Shared rendered-JPEG cache for /thumb and /preview.
    pub thumb_cache: Arc<ThumbCache>,
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
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            scan_semaphore: Arc::new(Semaphore::new(scan_concurrency)),
            thumb_cache: Arc::new(ThumbCache::new(thumb_cache_mb * 1024 * 1024)),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
