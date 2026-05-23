use crate::error::Result;
use crate::features::{FeatureExtractor, FullExtractor, PhotoFeatures};
use crate::group::{cluster_stage_a, cluster_stage_b, CompositionGroup, Group, StageAParams, StageBParams};
use crate::ingest::{decode_thumbnail_for, FsScanner, PhotoId, PhotoRef, Scanner, ThumbnailSpec};
use crate::models::{ClipEncoder, ExecutionProvider};
use crate::output::{materialize, plan_output, write_html_report, write_json_report};
use crate::scoring::{
    select_top_k_per_composition, select_top_k_per_group, CompositionPick, SelectedGroup,
    TechScore, TechWeights,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
    FinalSelect,
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
    pub html_report_path: Option<PathBuf>,
    pub stage_a: StageAParams,
    pub stage_b: StageBParams,
    pub k1: usize,
    pub k2: usize,
    pub tech_weights: TechWeights,
    pub link_mode: LinkMode,
    pub thumbnail: ThumbnailSpec,
    pub dry_run: bool,
    pub enable_clip: bool,
    pub execution_provider: ExecutionProvider,
}

#[derive(Debug, Serialize)]
pub struct PipelineReport {
    pub photo_count: usize,
    pub stage_a_group_count: usize,
    pub stage_b_group_count: usize,
    pub picked_count: usize,
    pub rejected_count: usize,
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

        // 2. Feature extraction
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

        // 4. Per-group top-K1 tech-score selection
        progress.on_stage(Stage::Score, 0);
        let stage_a_picks: Vec<SelectedGroup> =
            select_top_k_per_group(&groups, &features, self.cfg.k1, &self.cfg.tech_weights);
        let k1_kept: usize = stage_a_picks.iter().map(|s| s.kept.len()).sum();
        let k1_rejected: usize = stage_a_picks.iter().map(|s| s.rejected.len()).sum();
        progress.on_finish(Stage::Score);
        tracing::info!(kept = k1_kept, rejected = k1_rejected, "K1 selection complete");

        // 5. Stage B clustering on K1-kept photos (if CLIP available)
        let stage_b_groups: Vec<CompositionGroup> = if clip_enabled {
            progress.on_stage(Stage::StageB, 0);
            let kept_with_embeds: Vec<(PhotoId, Vec<f32>)> = stage_a_picks
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
            tracing::info!(group_count = bg.len(), "stage B complete");
            bg
        } else {
            vec![]
        };

        // 6. Final K2 selection per composition group (final scene-aware score)
        let composition_picks: Vec<CompositionPick> = if !stage_b_groups.is_empty() {
            progress.on_stage(Stage::FinalSelect, 0);
            // Collect tech scores from Stage A picks for lookup.
            let tech_scores: HashMap<PhotoId, TechScore> = stage_a_picks
                .iter()
                .flat_map(|s| s.kept.iter().chain(s.rejected.iter()).copied())
                .collect();
            let cp =
                select_top_k_per_composition(&stage_b_groups, &features, &tech_scores, self.cfg.k2);
            let kept_total: usize = cp.iter().map(|p| p.kept.len()).sum();
            progress.on_finish(Stage::FinalSelect);
            tracing::info!(kept_total, "K2 selection complete");
            cp
        } else {
            vec![]
        };

        // 7. Build the output plan, materialize, report
        let photos_by_id: HashMap<PhotoId, PhotoRef> =
            photos.iter().cloned().map(|p| (p.id, p)).collect();

        let plan = plan_output(&photos_by_id, &stage_a_picks, &composition_picks);
        let final_picked_ids: HashSet<PhotoId> = plan.picked.iter().map(|(p, _)| *p).collect();

        let (picked_count, rejected_count) = if self.cfg.dry_run {
            (plan.picked.len(), plan.rejected.len())
        } else {
            progress.on_stage(Stage::Write, (plan.picked.len() + plan.rejected.len()) as u64);
            let counts = materialize(&self.cfg.output, &photos_by_id, &plan, self.cfg.link_mode)?;
            progress.on_finish(Stage::Write);
            counts
        };

        if let Some(report_path) = &self.cfg.report_path {
            write_json_report(
                report_path,
                &self.cfg.root,
                start.elapsed(),
                &photos_by_id,
                &stage_a_picks,
                &stage_b_groups,
                &composition_picks,
                &final_picked_ids,
            )?;
        }
        if let Some(html_path) = &self.cfg.html_report_path {
            write_html_report(
                html_path,
                &self.cfg.root,
                start.elapsed(),
                &photos_by_id,
                &stage_a_picks,
                &composition_picks,
            )?;
        }

        Ok(PipelineReport {
            photo_count: photos.len(),
            stage_a_group_count: groups.len(),
            stage_b_group_count: stage_b_groups.len(),
            picked_count,
            rejected_count,
            elapsed: start.elapsed(),
        })
    }
}
