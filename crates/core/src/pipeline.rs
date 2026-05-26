use crate::cache::CacheStore;
use crate::error::Result;
use crate::features::{FeatureExtractor, FullExtractor, PhotoFeatures};
use crate::group::{cluster_stage_a, cluster_stage_b, CompositionGroup, Group, StageAParams, StageBParams};
use crate::ingest::{
    decode_thumbnail_for, scan_files, FsScanner, PhotoId, PhotoRef, PhotoSource, Scanner,
    ThumbnailSpec,
};
use crate::models::ExecutionProvider;
#[cfg(feature = "onnx")]
use crate::models::ClipEncoder;
#[cfg(feature = "onnx")]
use crate::scoring::YunetFaceDetector;
use crate::output::{
    materialize, plan_output, write_html_report, write_json_report, ThumbDiskCache,
    DEFAULT_THUMB_LONG_EDGE, DEFAULT_THUMB_QUALITY,
};
use crate::scoring::{
    select_top_k_per_composition, select_top_k_per_group, CompositionPick, SelectedGroup,
    TechWeights,
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
    pub source: PhotoSource,
    pub output: PathBuf,
    pub report_path: Option<PathBuf>,
    pub html_report_path: Option<PathBuf>,
    /// SQLite cache path. `None` disables caching entirely.
    pub cache_path: Option<PathBuf>,
    pub stage_a: StageAParams,
    pub stage_b: StageBParams,
    pub k1: usize,
    pub k2: usize,
    pub tech_weights: TechWeights,
    pub link_mode: LinkMode,
    pub thumbnail: ThumbnailSpec,
    pub dry_run: bool,
    pub enable_clip: bool,
    pub enable_face: bool,
    /// When false, skip copying/linking picks+rejected into `output`. The
    /// directory is still used for cache + reports. Use this for the "review
    /// in UI, apply destructively to source" workflow.
    pub materialize_picks: bool,
    pub execution_provider: ExecutionProvider,
    /// When true, look at the fraction of photos containing a meaningful
    /// face and shift Stage A / Stage B CLIP thresholds:
    /// portrait-heavy shoots tighten (avoid merging different people),
    /// landscape-only shoots loosen (allow more burst / composition
    /// consolidation). Magnitude capped at ±0.025.
    pub adaptive_thresholds: bool,
    /// Directory where the pipeline persists JPEG thumbnails (one file per
    /// photo, keyed by sha256_short) during feature extraction. Both the
    /// HTML report and the server's thumbnail endpoints read from this dir
    /// to avoid re-decoding originals (RAW byte-scan is the dominant cost).
    /// `None` disables the cache. Default in callers: `<output>/.thumbs`.
    pub thumb_cache_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PipelineReport {
    pub photo_count: usize,
    pub cached_count: usize,
    pub extracted_count: usize,
    pub stage_a_group_count: usize,
    pub stage_b_group_count: usize,
    pub picked_count: usize,
    pub rejected_count: usize,
    pub elapsed: Duration,
}

/// Full pipeline output. The CLI cares only about `report`; the server uses
/// `composition_picks` + `photos` to drive the VLM "explain group" feature.
pub struct PipelineOutput {
    pub report: PipelineReport,
    pub stage_a_picks: Vec<SelectedGroup>,
    pub composition_picks: Vec<CompositionPick>,
    pub photos: HashMap<PhotoId, PhotoRef>,
}

pub struct Pipeline {
    cfg: PipelineConfig,
}

impl Pipeline {
    pub fn new(cfg: PipelineConfig) -> Self {
        Self { cfg }
    }

    pub fn run(&self, progress: &dyn ProgressSink) -> Result<PipelineOutput> {
        let start = Instant::now();

        // 1. Scan
        progress.on_stage(Stage::Scan, 0);
        let photos = match &self.cfg.source {
            PhotoSource::Directory(root) => {
                let scanner = FsScanner::default();
                scanner.scan(root)?
            }
            PhotoSource::Files(files) => scan_files(files)?,
        };
        progress.on_finish(Stage::Scan);
        tracing::info!(count = photos.len(), "scan complete");

        // 2a. Open the cache, look up features by content hash. Photos we
        //     already know about get attached features now; the rest go to the
        //     parallel extraction phase below.
        let cache = match &self.cfg.cache_path {
            Some(p) => match CacheStore::open(p) {
                Ok(c) => {
                    tracing::info!(path = %p.display(), "cache opened");
                    Some(c)
                }
                Err(err) => {
                    tracing::warn!(%err, "cache disabled (open failed)");
                    None
                }
            },
            None => None,
        };

        let mut features: HashMap<PhotoId, PhotoFeatures> = HashMap::new();
        let mut to_extract: Vec<&PhotoRef> = Vec::with_capacity(photos.len());
        let want_clip = self.cfg.enable_clip;
        let want_face = self.cfg.enable_face;
        if let Some(c) = &cache {
            for p in &photos {
                match c.get(&p.sha256_short, p.id) {
                    Ok(Some(feat)) => {
                        // Treat as a miss if the user asked for a feature the
                        // cached row doesn't have — otherwise toggling CLIP /
                        // face on after a no-model run would silently leave
                        // the pipeline without the data it needs.
                        let missing_clip = want_clip && feat.clip_embed.is_none();
                        let missing_face = want_face && feat.face.is_none();
                        if missing_clip || missing_face {
                            to_extract.push(p);
                        } else {
                            features.insert(p.id, feat);
                        }
                    }
                    Ok(None) => to_extract.push(p),
                    Err(err) => {
                        tracing::warn!(path = %p.path.display(), %err, "cache lookup failed; will re-extract");
                        to_extract.push(p);
                    }
                }
            }
        } else {
            to_extract.extend(photos.iter());
        }
        let cached_count = features.len();
        let extract_count = to_extract.len();
        tracing::info!(cached = cached_count, to_extract = extract_count, "cache lookup complete");

        // 2b. Parallel feature extraction for cache misses.
        let clip_enabled;
        let extracted_pairs: Vec<(PhotoId, [u8; 16], PhotoFeatures)> = if to_extract.is_empty() {
            clip_enabled = cache.is_some()
                && features.values().any(|f| f.clip_embed.is_some());
            vec![]
        } else {
            #[allow(unused_mut)]
            let mut extractor = FullExtractor::new();

            #[cfg(feature = "onnx")]
            {
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
                clip_enabled = clip.is_some();
                extractor = extractor.with_clip(clip);

                if self.cfg.enable_face {
                    match YunetFaceDetector::load(self.cfg.execution_provider) {
                        Ok(d) => {
                            tracing::info!("YuNet face detector loaded");
                            extractor = extractor.with_face_detector(Box::new(d));
                        }
                        Err(err) => {
                            tracing::warn!(%err, "YuNet load failed; continuing without face detection");
                        }
                    }
                }
            }
            #[cfg(not(feature = "onnx"))]
            {
                clip_enabled = false;
            }

            progress.on_stage(Stage::Features, extract_count as u64);
            let counter = Mutex::new(0u64);

            // Init the disk thumbnail cache once if requested. Persist runs
            // in the rayon loop below so we never re-decode the source for
            // the HTML report or for /thumb requests after this scan.
            let thumb_cache: Option<ThumbDiskCache> = self.cfg.thumb_cache_dir.as_ref().map(|d| {
                ThumbDiskCache::new(d.clone(), DEFAULT_THUMB_LONG_EDGE, DEFAULT_THUMB_QUALITY)
            });

            let pairs: Vec<(PhotoId, [u8; 16], PhotoFeatures)> = to_extract
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
                    if let Some(c) = &thumb_cache {
                        c.persist(&p.sha256_short, &thumb);
                    }
                    let mut c = counter.lock().unwrap();
                    *c += 1;
                    progress.on_tick(Stage::Features, *c);
                    Some((p.id, p.sha256_short, feat))
                })
                .collect();
            progress.on_finish(Stage::Features);
            pairs
        };

        // 2c. Persist newly extracted features back into the cache (one txn).
        if let Some(c) = &cache {
            let items: Vec<(&[u8; 16], &PhotoFeatures)> = extracted_pairs
                .iter()
                .map(|(_, sha, feat)| (sha, feat))
                .collect();
            if let Err(err) = c.put_many(&items) {
                tracing::warn!(%err, "cache write failed");
            }
        }
        for (id, _, feat) in extracted_pairs {
            features.insert(id, feat);
        }

        // 2d. Optional adaptive-threshold bias: shifts CLIP thresholds based
        //     on the fraction of photos with a non-trivial face. Portrait
        //     shoots (high share) tighten thresholds (avoid merging different
        //     people); landscape shoots loosen (allow more aggressive
        //     consolidation). Bias clamped to ±0.025.
        let (stage_a_params, stage_b_params) = if self.cfg.adaptive_thresholds && !features.is_empty() {
            let total = features.len() as f32;
            let portrait_count = features
                .values()
                .filter(|f| {
                    f.face
                        .as_ref()
                        .map(|fi| fi.faces.iter().any(|fb| fb.area_ratio() >= 0.05))
                        .unwrap_or(false)
                })
                .count() as f32;
            let portrait_share = portrait_count / total;
            let bias = ((portrait_share - 0.5) * 0.05).clamp(-0.025, 0.025);
            tracing::info!(
                portrait_share = portrait_share,
                threshold_bias = bias,
                "adaptive threshold bias applied"
            );
            let sa = StageAParams {
                clip_threshold: (self.cfg.stage_a.clip_threshold + bias).clamp(0.7, 0.99),
                ..self.cfg.stage_a.clone()
            };
            let sb = StageBParams {
                similarity_threshold: (self.cfg.stage_b.similarity_threshold + bias)
                    .clamp(0.7, 0.99),
                chain_margin: self.cfg.stage_b.chain_margin,
            };
            (sa, sb)
        } else {
            (self.cfg.stage_a.clone(), self.cfg.stage_b.clone())
        };

        // 3. Stage A clustering
        progress.on_stage(Stage::Cluster, 0);
        let groups: Vec<Group> = cluster_stage_a(&photos, &features, &stage_a_params);
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
            let bg = cluster_stage_b(&kept_with_embeds, &stage_b_params);
            progress.on_finish(Stage::StageB);
            tracing::info!(group_count = bg.len(), "stage B complete");
            bg
        } else {
            vec![]
        };

        // 6. Final K2 selection per composition group (final scene-aware score).
        // Sharpness is re-normalized within each composition group here.
        let composition_picks: Vec<CompositionPick> = if !stage_b_groups.is_empty() {
            progress.on_stage(Stage::FinalSelect, 0);
            let cp = select_top_k_per_composition(
                &stage_b_groups,
                &features,
                self.cfg.k2,
                &self.cfg.tech_weights,
            );
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

        let (picked_count, rejected_count) = if self.cfg.dry_run || !self.cfg.materialize_picks {
            (plan.picked.len(), plan.rejected.len())
        } else {
            progress.on_stage(Stage::Write, (plan.picked.len() + plan.rejected.len()) as u64);
            let counts = materialize(&self.cfg.output, &photos_by_id, &plan, self.cfg.link_mode)?;
            progress.on_finish(Stage::Write);
            counts
        };

        let root_hint = self.cfg.source.root_hint();
        if let Some(report_path) = &self.cfg.report_path {
            write_json_report(
                report_path,
                &root_hint,
                start.elapsed(),
                &photos_by_id,
                &stage_a_picks,
                &stage_b_groups,
                &composition_picks,
                &final_picked_ids,
            )?;
        }
        if let Some(html_path) = &self.cfg.html_report_path {
            let report_thumb_cache = self.cfg.thumb_cache_dir.as_ref().map(|d| {
                ThumbDiskCache::new(d.clone(), DEFAULT_THUMB_LONG_EDGE, DEFAULT_THUMB_QUALITY)
            });
            write_html_report(
                html_path,
                &root_hint,
                start.elapsed(),
                &photos_by_id,
                &stage_a_picks,
                &composition_picks,
                report_thumb_cache.as_ref(),
            )?;
        }

        let report = PipelineReport {
            photo_count: photos.len(),
            cached_count,
            extracted_count: extract_count,
            stage_a_group_count: groups.len(),
            stage_b_group_count: stage_b_groups.len(),
            picked_count,
            rejected_count,
            elapsed: start.elapsed(),
        };
        Ok(PipelineOutput {
            report,
            stage_a_picks,
            composition_picks,
            photos: photos_by_id,
        })
    }
}
