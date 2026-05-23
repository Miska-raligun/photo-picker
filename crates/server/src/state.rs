use photo_pick_core::pipeline::PipelineReport;
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
