use crate::error::Result;
use crate::features::{FeatureExtractor, HashOnlyExtractor, PhotoFeatures};
use crate::group::{cluster_stage_a, Group, StageAParams};
use crate::ingest::{decode_thumbnail, FsScanner, PhotoId, PhotoRef, Scanner, ThumbnailSpec};
use crate::output::{materialize, write_json_report};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkMode {
    Copy,
    Hardlink,
    Symlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage {
    Scan,
    Features,
    Cluster,
    Write,
}

pub trait ProgressSink: Send + Sync {
    fn on_stage(&self, stage: Stage, total: u64);
    fn on_tick(&self, stage: Stage, done: u64);
    fn on_finish(&self, stage: Stage);
}

pub struct NoopProgress;
impl ProgressSink for NoopProgress {
    fn on_stage(&self, _: Stage, _: u64) {}
    fn on_tick(&self, _: Stage, _: u64) {}
    fn on_finish(&self, _: Stage) {}
}

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub root: PathBuf,
    pub output: PathBuf,
    pub report_path: Option<PathBuf>,
    pub stage_a: StageAParams,
    /// M1: not used; reserved for M2 technical-score top-K selection.
    pub k1: usize,
    /// M3+: not used yet.
    pub k2: usize,
    pub link_mode: LinkMode,
    pub thumbnail: ThumbnailSpec,
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
pub struct PipelineReport {
    pub photo_count: usize,
    pub group_count: usize,
    pub picked_count: usize,
    pub elapsed: Duration,
}

pub struct Pipeline {
    cfg: PipelineConfig,
}

impl Pipeline {
    pub fn new(cfg: PipelineConfig) -> Self {
        Self { cfg }
    }

    pub fn run(&self, progress: &dyn ProgressSink) -> Result<PipelineReport> {
        let start = Instant::now();

        // 1. Scan
        progress.on_stage(Stage::Scan, 0);
        let scanner = FsScanner::default();
        let photos = scanner.scan(&self.cfg.root)?;
        progress.on_finish(Stage::Scan);
        tracing::info!(count = photos.len(), "scan complete");

        // 2. Feature extraction in parallel.
        let extractor = HashOnlyExtractor::new();
        let total = photos.len() as u64;
        progress.on_stage(Stage::Features, total);
        let counter = Mutex::new(0u64);

        let features_vec: Vec<PhotoFeatures> = photos
            .par_iter()
            .filter_map(|p| {
                let thumb = match decode_thumbnail(&p.path, self.cfg.thumbnail) {
                    Ok(t) => t,
                    Err(err) => {
                        tracing::warn!(path = %p.path.display(), %err, "skipping (decode failed)");
                        return None;
                    }
                };
                let feat = match extractor.extract(p, &thumb) {
                    Ok(f) => f,
                    Err(err) => {
                        tracing::warn!(path = %p.path.display(), %err, "skipping (feature failed)");
                        return None;
                    }
                };
                let mut c = counter.lock().unwrap();
                *c += 1;
                progress.on_tick(Stage::Features, *c);
                Some(feat)
            })
            .collect();
        progress.on_finish(Stage::Features);

        let features: HashMap<PhotoId, PhotoFeatures> = features_vec
            .into_iter()
            .map(|f| (f.photo_id, f))
            .collect();

        // 3. Stage A clustering
        progress.on_stage(Stage::Cluster, 0);
        let groups: Vec<Group> = cluster_stage_a(&photos, &features, &self.cfg.stage_a);
        progress.on_finish(Stage::Cluster);
        tracing::info!(group_count = groups.len(), "stage A complete");

        // 4. Output
        let photos_by_id: HashMap<PhotoId, PhotoRef> =
            photos.iter().cloned().map(|p| (p.id, p)).collect();

        let picked_count = if self.cfg.dry_run {
            groups.iter().map(|g| g.photo_ids.len()).sum()
        } else {
            progress.on_stage(Stage::Write, groups.iter().map(|g| g.photo_ids.len() as u64).sum());
            let placed = materialize(&self.cfg.output, &photos_by_id, &groups, self.cfg.link_mode)?;
            progress.on_finish(Stage::Write);
            placed.len()
        };

        if let Some(report_path) = &self.cfg.report_path {
            write_json_report(report_path, &self.cfg.root, start.elapsed(), &photos_by_id, &groups)?;
        }

        Ok(PipelineReport {
            photo_count: photos.len(),
            group_count: groups.len(),
            picked_count,
            elapsed: start.elapsed(),
        })
    }
}
