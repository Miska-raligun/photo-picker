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
use crate::models::{ClipEncoder, SessionPool};
#[cfg(feature = "onnx")]
use crate::scoring::YunetFaceDetector;
use crate::output::{
    materialize, plan_output, write_html_report, write_json_report, ThumbDiskCache,
    DEFAULT_THUMB_LONG_EDGE, DEFAULT_THUMB_QUALITY,
};
use crate::scoring::{
    select_top_k_per_composition, select_top_k_per_group, CompositionPick, K2Policy,
    SelectedGroup,
    TechWeights,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// `None` = auto: per-group keep count driven by score-gap heuristics
    /// (always ≥1; clusters of near-tied photos keep more; clear winners
    /// keep just one; capped at 5 per group).
    pub k2: Option<usize>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl PipelineConfig {
    /// Shared defaults for everything that isn't source/output-specific.
    /// Both entry points (CLI flags, server ScanRequest) start from this and
    /// override only the knobs they actually expose, so a new config field
    /// gets one default here instead of two hand-copied ones that drift.
    pub fn with_defaults(source: PhotoSource, output: PathBuf) -> Self {
        let thumb_cache_dir = Some(output.join(".thumbs"));
        Self {
            source,
            output,
            report_path: None,
            html_report_path: None,
            cache_path: None,
            stage_a: StageAParams::default(),
            stage_b: StageBParams::default(),
            k1: 3,
            k2: None,
            tech_weights: TechWeights::default(),
            link_mode: LinkMode::Hardlink,
            thumbnail: ThumbnailSpec::default(),
            dry_run: false,
            enable_clip: true,
            enable_face: true,
            materialize_picks: true,
            execution_provider: ExecutionProvider::Cpu,
            adaptive_thresholds: true,
            thumb_cache_dir,
        }
    }
}

/// Normalize a user-facing K2 value: `0` is CLI/API shorthand for "auto"
/// (same as omitting it). Shared by the CLI flag and the server request so
/// the two entry points can't disagree about what 0 means.
pub fn normalize_k2(k2: Option<usize>) -> Option<usize> {
    match k2 {
        Some(0) | None => None,
        Some(k) => Some(k),
    }
}

pub struct Pipeline {
    cfg: PipelineConfig,
}

impl Pipeline {
    pub fn new(cfg: PipelineConfig) -> Self {
        Self { cfg }
    }

    /// Run to completion with no external cancellation.
    pub fn run(&self, progress: &dyn ProgressSink) -> Result<PipelineOutput> {
        self.run_with_cancel(progress, &AtomicBool::new(false))
    }

    /// Run the pipeline, checking `cancel` at each stage boundary and per
    /// photo inside the (dominant-cost) feature-extraction loop. When the
    /// flag flips to `true` the pipeline returns [`Error::Cancelled`] at the
    /// next checkpoint — partial cache writes up to that point are kept (they
    /// are valid, content-keyed data that speeds up the next attempt).
    pub fn run_with_cancel(
        &self,
        progress: &dyn ProgressSink,
        cancel: &AtomicBool,
    ) -> Result<PipelineOutput> {
        let check = || -> Result<()> {
            if cancel.load(Ordering::Relaxed) {
                Err(crate::error::Error::Cancelled)
            } else {
                Ok(())
            }
        };
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
        check()?;

        // 2a. Open the cache, look up features by content hash. Photos we
        //     already know about get attached features now; the rest go to the
        //     parallel extraction phase below.
        let cache = match &self.cfg.cache_path {
            Some(p) => match CacheStore::open(p) {
                Ok(c) => {
                    tracing::info!(path = %p.display(), "cache opened");
                    // Opt-in LRU trim: long-running installs can pin the
                    // cache size via PHOTO_PICK_CACHE_MAX_ROWS so it
                    // doesn't grow unbounded. Best effort — failures
                    // here are logged but don't abort the scan.
                    if let Ok(s) = std::env::var("PHOTO_PICK_CACHE_MAX_ROWS") {
                        if let Ok(max) = s.parse::<u64>() {
                            if let Ok(dropped) = c.trim_to(max) {
                                if dropped > 0 {
                                    tracing::info!(dropped, max, "cache LRU trim");
                                }
                            }
                        }
                    }
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
                let pool_size = crate::models::default_pool_size();
                let clip_pool = if self.cfg.enable_clip {
                    match load_clip_pool(self.cfg.execution_provider, pool_size) {
                        Ok(p) => {
                            tracing::info!(sessions = p.len(), "CLIP encoder pool loaded");
                            Some(p)
                        }
                        Err(err) => {
                            tracing::warn!(%err, "CLIP load failed; continuing without Stage B");
                            None
                        }
                    }
                } else {
                    None
                };
                clip_enabled = clip_pool.is_some();
                extractor = extractor.with_clip_pool(clip_pool);

                if self.cfg.enable_face {
                    match YunetFaceDetector::load_pool(self.cfg.execution_provider, pool_size) {
                        Ok(d) => {
                            tracing::info!(
                                "YuNet face detector loaded (pool size {})",
                                pool_size
                            );
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
            // Throttle the tick stream: each rayon worker bumps the atomic
            // unconditionally (relaxed is fine — we're not synchronising
            // anything on top of the counter), but only emits an SSE event
            // every ~1% of the work or on the final image. A 5000-photo
            // run goes from 5000 broadcasts (one per worker per image) to
            // ~100, well under the channel's bound.
            let counter = AtomicU64::new(0);
            let tick_step = ((extract_count as u64) / 100).max(1);

            // Init the disk thumbnail cache once if requested. Persist runs
            // in the rayon loop below so we never re-decode the source for
            // the HTML report or for /thumb requests after this scan.
            let thumb_cache: Option<ThumbDiskCache> = self.cfg.thumb_cache_dir.as_ref().map(|d| {
                ThumbDiskCache::new(d.clone(), DEFAULT_THUMB_LONG_EDGE, DEFAULT_THUMB_QUALITY)
            });

            let pairs: Vec<(PhotoId, [u8; 16], PhotoFeatures)> = to_extract
                .par_iter()
                .filter_map(|p| {
                    // Cancellation check per photo: this loop is where a large
                    // scan spends minutes, so a flag flip should stop new
                    // decode/inference work within one item's latency. Photos
                    // already extracted stay in `pairs` and get persisted to
                    // the cache below before the Cancelled error surfaces.
                    if cancel.load(Ordering::Relaxed) {
                        return None;
                    }
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
                    let done = counter.fetch_add(1, Ordering::Relaxed) + 1;
                    if done % tick_step == 0 || done == extract_count as u64 {
                        progress.on_tick(Stage::Features, done);
                    }
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
        // Checked AFTER the cache write above so a cancelled run still keeps
        // every feature it paid for — the next scan of the same folder
        // resumes from the cache instead of starting over.
        check()?;

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

        // 3. Stage A clustering. Pass the sink through so the inner loop can
        // emit a real total + throttled ticks (cluster_stage_a fires its own
        // `on_stage` once it knows the timed-photo count); we keep the
        // surrounding `on_stage`/`on_finish` brackets so the UI still sees a
        // Cluster phase even when the run has no timed photos.
        progress.on_stage(Stage::Cluster, 0);
        let groups: Vec<Group> = cluster_stage_a(&photos, &features, &stage_a_params, progress);
        progress.on_finish(Stage::Cluster);
        check()?;
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
            let policy = match self.cfg.k2 {
                Some(k) => K2Policy::Fixed(k),
                None => K2Policy::Auto,
            };
            let cp = select_top_k_per_composition(
                &stage_b_groups,
                &features,
                policy,
                &self.cfg.tech_weights,
            );
            let kept_total: usize = cp.iter().map(|p| p.kept.len()).sum();
            progress.on_finish(Stage::FinalSelect);
            tracing::info!(kept_total, "K2 selection complete");
            cp
        } else {
            vec![]
        };

        check()?;

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

        // Opt-in disk cap for the thumbnail cache, mirroring the
        // PHOTO_PICK_CACHE_MAX_ROWS pattern for the feature DB. Runs after
        // the reports are written so the current scan's (freshest-mtime)
        // thumbs are preferentially kept and older scans' files are evicted
        // first. Unset = unbounded (existing behaviour).
        if let Some(dir) = &self.cfg.thumb_cache_dir {
            if let Some(max_mb) = std::env::var("PHOTO_PICK_THUMB_DISK_MAX_MB")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
            {
                let cache =
                    ThumbDiskCache::new(dir.clone(), DEFAULT_THUMB_LONG_EDGE, DEFAULT_THUMB_QUALITY);
                let removed = cache.trim_to_bytes(max_mb * 1024 * 1024);
                if removed > 0 {
                    tracing::info!(removed, max_mb, "thumb disk cache trimmed");
                }
            }
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

#[cfg(feature = "onnx")]
fn load_clip_pool(
    ep: ExecutionProvider,
    n: usize,
) -> Result<SessionPool<ClipEncoder>> {
    let n = n.max(1);
    let mut encoders = Vec::with_capacity(n);
    for _ in 0..n {
        encoders.push(ClipEncoder::load(ep)?);
    }
    Ok(SessionPool::new(encoders))
}
