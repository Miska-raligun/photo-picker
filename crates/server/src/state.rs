use photo_pick_core::ingest::{PhotoId, PhotoRef};
use photo_pick_core::pipeline::PipelineReport;
use photo_pick_core::scoring::CompositionPick;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

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

#[derive(Clone)]
pub struct AppState {
    pub runs: Arc<Mutex<HashMap<String, RunRecord>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
