use crate::error::Result;
use crate::features::{FeatureExtractor, FullExtractor, PhotoFeatures};
use crate::group::{cluster_stage_a, cluster_stage_b, CompositionGroup, Group, StageAParams, StageBParams};
use crate::ingest::{decode_thumbnail_for, FsScanner, PhotoId, PhotoRef, Scanner, ThumbnailSpec};
use crate::models::{ClipEncoder, ExecutionProvider};
use crate::output::{materialize, write_json_report};
use crate::scoring::{select_top_k_per_group, SelectedGroup, TechWeights};
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
    Score,
    StageB,
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
    pub stage_b: StageBParams,
    pub k1: usize,
    pub k2: usize, // M3.7+: used once final score is in
    pub tech_weights: TechWeights,
    pub link_mode: LinkMode,
    pub thumbnail: ThumbnailSpec,
    pub dry_run: bool,
    /// When true, load CLIP and run Stage B. When false (or model load fails),
    /// behaves like M2 — only Stage A + tech score.
    pub enable_clip: bool,
    pub execution_provider: ExecutionProvider,
}

#[derive(Debug, Serialize)]
pub struct PipelineReport {
    pub photo_count: usize,
    pub group_count: usize,
    pub picked_count: usize,
    pub rejected_count: usize,
    pub stage_b_group_count: usize,
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

        // 2. Feature extraction (hashes + raw tech scores + optional CLIP)
        let clip = if self.cfg.enable_clip {
            match ClipEncoder::load(self.cfg.execution_provider) {
                Ok(e) => {
                    tracing::info!("CLIP encoder loaded");
                    Some(e)
                }
                Err(err) => {
                    tracing::warn!(%err, "CLIP load failed; continuing without Stage B");
                    None
                }
            }
        } else {
            None
        };
        let clip_enabled = clip.is_some();

        let extractor = FullExtractor::new(clip);
        let total = photos.len() as u64;
        progress.on_stage(Stage::Features, total);
        let counter = Mutex::new(0u64);

        let features_vec: Vec<PhotoFeatures> = photos
            .par_iter()
            .filter_map(|p| {
                let thumb = match decode_thumbnail_for(p, self.cfg.thumbnail) {
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

        // 4. Per-group top-K1 selection
        progress.on_stage(Stage::Score, 0);
        let selections: Vec<SelectedGroup> =
            select_top_k_per_group(&groups, &features, self.cfg.k1, &self.cfg.tech_weights);
        let picked_total: usize = selections.iter().map(|s| s.kept.len()).sum();
        let rejected_total: usize = selections.iter().map(|s| s.rejected.len()).sum();
        progress.on_finish(Stage::Score);
        tracing::info!(picked = picked_total, rejected = rejected_total, "selection complete");

        // 5. Stage B — composition clustering on the K1-kept photos
        let stage_b_groups: Vec<CompositionGroup> = if clip_enabled {
            progress.on_stage(Stage::StageB, 0);
            let kept_with_embeds: Vec<(PhotoId, Vec<f32>)> = selections
                .iter()
                .flat_map(|s| s.kept.iter().map(|(pid, _)| *pid))
                .filter_map(|pid| {
                    features
                        .get(&pid)
                        .and_then(|f| f.clip_embed.clone().map(|e| (pid, e)))
                })
                .collect();
            let bg = cluster_stage_b(&kept_with_embeds, &self.cfg.stage_b);
            progress.on_finish(Stage::StageB);
            tracing::info!(group_count = bg.len(), photos = kept_with_embeds.len(), "stage B complete");
            bg
        } else {
            vec![]
        };

        // 6. Output
        let photos_by_id: HashMap<PhotoId, PhotoRef> =
            photos.iter().cloned().map(|p| (p.id, p)).collect();

        let (picked_count, rejected_count) = if self.cfg.dry_run {
            (picked_total, rejected_total)
        } else {
            let write_total = (picked_total + rejected_total) as u64;
            progress.on_stage(Stage::Write, write_total);
            let (picked, rejected) =
                materialize(&self.cfg.output, &photos_by_id, &selections, self.cfg.link_mode)?;
            progress.on_finish(Stage::Write);
            (picked.len(), rejected.len())
        };

        if let Some(report_path) = &self.cfg.report_path {
            write_json_report(
                report_path,
                &self.cfg.root,
                start.elapsed(),
                &photos_by_id,
                &selections,
                &stage_b_groups,
            )?;
        }

        Ok(PipelineReport {
            photo_count: photos.len(),
            group_count: groups.len(),
            picked_count,
            rejected_count,
            stage_b_group_count: stage_b_groups.len(),
            elapsed: start.elapsed(),
        })
    }
}
