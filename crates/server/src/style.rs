//! Lightroom style-preset endpoints.
//!
//! `POST /api/runs/:id/style-preset` learns a look from reference photos and
//! returns it as a Lightroom preset (`.xmp`); `POST /api/runs/:id/style-preview`
//! renders an approximate before/after of a fitted preset on one run photo.
//!
//! References that are edited versions of photos in the run are paired with
//! their originals automatically. Tone curves and colour grades keep the
//! brightness *ordering* that dHash encodes and the low-frequency structure
//! pHash encodes, so on real photos an edit sits 0–3 bits from its original
//! on both, while unrelated shots are 22–31 bits away. Requiring both guards
//! against low-detail frames (plain sky, a wall) whose dHashes collide.
//! Paired fitting is far more accurate; see `photo_pick_core::style`.

use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use photo_pick_core::features::{hash::hamming, FeatureExtractor, HashOnlyExtractor};
use photo_pick_core::ingest::{decode_thumbnail_for, encode_jpeg, scan_files, PhotoId, PhotoRef, ThumbnailSpec};
use photo_pick_core::output::{ThumbDiskCache, DEFAULT_THUMB_LONG_EDGE, DEFAULT_THUMB_QUALITY};
use photo_pick_core::style::{StylePreset, StyleStats};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// An edit and its original differ by a few bits on both hashes; unrelated
/// frames by 22+. At or under this on *both* counts as the same shot.
const PAIR_MAX_HAMMING: u32 = 10;
/// Cap on baseline photos read for unpaired fitting (evenly sampled).
const MAX_BASELINE: usize = 300;
/// Reference photos accepted per request.
const MAX_REFERENCES: usize = 50;
/// Run photos offered to the UI for before/after previews.
const PREVIEW_COUNT: usize = 4;

#[derive(Debug, Deserialize)]
pub struct StylePresetRequest {
    /// Reference photos (anywhere on disk) whose look to learn.
    pub reference_paths: Vec<PathBuf>,
    #[serde(default)]
    pub name: Option<String>,
    /// 0–1; scales every adjustment. Defaults to 1.
    #[serde(default)]
    pub strength: Option<f32>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    Paired,
    Unpaired,
}

#[derive(Debug, Serialize)]
pub struct ReferenceIssue {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct StylePresetResponse {
    pub preset: StylePreset,
    pub xmp: String,
    pub file_name: String,
    pub mode: FitMode,
    /// References that contributed to the fit.
    pub references_used: usize,
    /// In paired mode: readable references with no original in this run
    /// (left out, since mixing modes would dilute the paired fit).
    pub references_unpaired: Vec<PathBuf>,
    /// References that couldn't be read at all.
    pub references_failed: Vec<ReferenceIssue>,
    /// Run photos the fit compared against (originals, or the baseline sample).
    pub baseline_count: usize,
    /// Run photo ids suggested for before/after previews.
    pub preview_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct StylePreviewRequest {
    pub preset: StylePreset,
    pub photo_id: String,
}

fn bad_request(msg: impl Into<String>) -> Response {
    (StatusCode::BAD_REQUEST, msg.into()).into_response()
}

pub async fn style_preset(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(req): Json<StylePresetRequest>,
) -> Response {
    if req.reference_paths.is_empty() {
        return bad_request("pick at least one reference photo");
    }
    if req.reference_paths.len() > MAX_REFERENCES {
        return bad_request(format!("at most {MAX_REFERENCES} reference photos"));
    }
    // Same allowlist as /browse and /scan: reading references must not be a
    // way around it. Paths that don't resolve fall through and are reported
    // per-file as unreadable.
    let roots = crate::handlers::allowed_roots();
    for p in &req.reference_paths {
        if let Ok(c) = std::fs::canonicalize(p).map(crate::handlers::strip_verbatim_prefix) {
            if !crate::handlers::path_under(&c, &roots) {
                return (
                    StatusCode::FORBIDDEN,
                    format!(
                        "{} is outside the configured browse roots; set PHOTO_PICK_BROWSE_ROOTS to widen",
                        c.display()
                    ),
                )
                    .into_response();
            }
        }
    }
    state.ensure_rehydrated(&run_id).await;
    let (photos, output_dir) = {
        let guard = state.runs.lock().await;
        let Some(rec) = guard.get(&run_id) else {
            return (StatusCode::NOT_FOUND, "run not found").into_response();
        };
        let mut photos: Vec<PhotoRef> = rec.photos.values().cloned().collect();
        photos.sort_by(|a, b| a.path.cmp(&b.path));
        (photos, rec.output.clone())
    };
    if photos.is_empty() {
        return bad_request("this run has no photos to compare against");
    }

    let name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("photo-pick style")
        .to_string();
    let strength = req.strength.unwrap_or(1.0).clamp(0.0, 1.0);
    let refs = req.reference_paths;

    let result = tokio::task::spawn_blocking(move || {
        fit_style(&name, strength, &refs, &photos, &output_dir)
    })
    .await;
    match result {
        Ok(Ok(r)) => Json(r).into_response(),
        Ok(Err(e)) => bad_request(e),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("style task: {e}")).into_response(),
    }
}

/// A photo's thumbnail plus its perceptual hashes.
struct Loaded {
    photo: PhotoRef,
    img: image::DynamicImage,
    dhash: u64,
    phash: u64,
}

impl Loaded {
    fn distance(&self, other: &Loaded) -> (u32, u32) {
        (hamming(self.dhash, other.dhash), hamming(self.phash, other.phash))
    }
}

fn fit_style(
    name: &str,
    strength: f32,
    reference_paths: &[PathBuf],
    photos: &[PhotoRef],
    output_dir: &std::path::Path,
) -> Result<StylePresetResponse, String> {
    let hasher = HashOnlyExtractor::new();
    let spec = ThumbnailSpec { long_edge: DEFAULT_THUMB_LONG_EDGE };

    // References: decode at the same size as run thumbnails so statistics
    // and hashes are comparable.
    let mut failed = Vec::new();
    let mut refs: Vec<Loaded> = Vec::new();
    for path in reference_paths {
        let loaded = scan_files(std::slice::from_ref(path))
            .map_err(|e| e.to_string())
            .and_then(|mut v| v.pop().ok_or_else(|| "unsupported file type".to_string()))
            .and_then(|photo| {
                let img = decode_thumbnail_for(&photo, spec).map_err(|e| e.to_string())?;
                let f = hasher.extract(&photo, &img).map_err(|e| e.to_string())?;
                Ok(Loaded { photo, img, dhash: f.dhash, phash: f.phash })
            });
        match loaded {
            Ok(l) => refs.push(l),
            Err(reason) => failed.push(ReferenceIssue { path: path.clone(), reason }),
        }
    }
    if refs.is_empty() {
        return Err("none of the reference photos could be read".into());
    }

    // Run photos (excluding the references themselves, which may live in
    // the scanned folder) from the scan's thumbnail cache.
    let ref_paths: Vec<&PathBuf> = refs.iter().map(|r| &r.photo.path).collect();
    let candidates: Vec<&PhotoRef> = photos.iter().filter(|p| !ref_paths.contains(&&p.path)).collect();
    if candidates.is_empty() {
        return Err("this run has no photos besides the references".into());
    }
    let thumbs = ThumbDiskCache::new(output_dir.join(".thumbs"), DEFAULT_THUMB_LONG_EDGE, DEFAULT_THUMB_QUALITY);
    let run = load_parallel(&candidates, &thumbs, &hasher);

    // Pair each reference with its nearest run photo.
    let mut edited = StyleStats::new();
    let mut originals = StyleStats::new();
    let mut paired_refs = 0usize;
    let mut unpaired: Vec<&Loaded> = Vec::new();
    let mut paired_ids: Vec<PhotoId> = Vec::new();
    for r in &refs {
        let best = run.iter().map(|l| (l, l.distance(r))).min_by_key(|(_, (d, p))| d + p);
        match best {
            Some((o, (d, p))) if d <= PAIR_MAX_HAMMING && p <= PAIR_MAX_HAMMING => {
                edited.add_image(&r.img);
                originals.add_image(&o.img);
                paired_refs += 1;
                paired_ids.push(o.photo.id);
            }
            _ => unpaired.push(r),
        }
    }

    let (preset, mode, used, baseline_count, references_unpaired) = if paired_refs > 0 {
        let p = StylePreset::fit_paired(name, &edited, &originals, strength).map_err(|e| e.to_string())?;
        let left_out = unpaired.iter().map(|r| r.photo.path.clone()).collect();
        (p, FitMode::Paired, paired_refs, paired_refs, left_out)
    } else {
        let mut reference = StyleStats::new();
        refs.iter().for_each(|r| reference.add_image(&r.img));
        let mut baseline = StyleStats::new();
        let step = (run.len() / MAX_BASELINE).max(1);
        run.iter().step_by(step).for_each(|l| baseline.add_image(&l.img));
        let p = StylePreset::fit_unpaired(name, &reference, &baseline, strength).map_err(|e| e.to_string())?;
        (p, FitMode::Unpaired, refs.len(), baseline.image_count(), Vec::new())
    };

    // Previews: photos the preset will actually be used on — prefer ones not
    // already edited (i.e. not a paired original), spread across the run.
    let pool: Vec<&Loaded> = run.iter().filter(|l| !paired_ids.contains(&l.photo.id)).collect();
    let pool = if pool.is_empty() { run.iter().collect() } else { pool };
    let step = (pool.len() / PREVIEW_COUNT).max(1);
    let preview_ids = pool.iter().step_by(step).take(PREVIEW_COUNT).map(|l| l.photo.id.to_string()).collect();

    Ok(StylePresetResponse {
        xmp: preset.to_lr_preset_xmp(),
        file_name: preset.file_name(),
        preset,
        mode,
        references_used: used,
        references_unpaired,
        references_failed: failed,
        baseline_count,
        preview_ids,
    })
}

/// Read thumbnails (disk cache, rendering misses) and hash them on a
/// CPU-sized worker pool. Unreadable photos are skipped.
fn load_parallel(photos: &[&PhotoRef], thumbs: &ThumbDiskCache, hasher: &HashOnlyExtractor) -> Vec<Loaded> {
    let n_workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(photos.len()).max(1);
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<(usize, Loaded)>> = Mutex::new(Vec::with_capacity(photos.len()));
    std::thread::scope(|scope| {
        for _ in 0..n_workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(photo) = photos.get(i) else { break };
                let loaded = thumbs
                    .read_or_render(photo)
                    .ok()
                    .and_then(|bytes| image::load_from_memory(&bytes).ok())
                    .and_then(|img| {
                        let f = hasher.extract(photo, &img).ok()?;
                        Some(Loaded { photo: (*photo).clone(), img, dhash: f.dhash, phash: f.phash })
                    });
                match loaded {
                    Some(l) => out.lock().unwrap().push((i, l)),
                    None => tracing::warn!(path = %photo.path.display(), "style preset: thumbnail unreadable; skipped"),
                }
            });
        }
    });
    let mut v = out.into_inner().unwrap();
    v.sort_by_key(|(i, _)| *i);
    v.into_iter().map(|(_, l)| l).collect()
}

pub async fn style_preview(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(req): Json<StylePreviewRequest>,
) -> Response {
    let Ok(uuid) = uuid::Uuid::parse_str(&req.photo_id) else {
        return bad_request(format!("bad photo id: {}", req.photo_id));
    };
    state.ensure_rehydrated(&run_id).await;
    let (photo, output_dir) = {
        let guard = state.runs.lock().await;
        let Some(rec) = guard.get(&run_id) else {
            return (StatusCode::NOT_FOUND, "run not found").into_response();
        };
        let Some(p) = rec.photos.get(&PhotoId(uuid)) else {
            return (StatusCode::NOT_FOUND, "photo not in run").into_response();
        };
        (p.clone(), rec.output.clone())
    };
    let preset = req.preset;
    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let thumbs =
            ThumbDiskCache::new(output_dir.join(".thumbs"), DEFAULT_THUMB_LONG_EDGE, DEFAULT_THUMB_QUALITY);
        let bytes = thumbs.read_or_render(&photo).map_err(|e| e.to_string())?;
        let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
        let styled = image::DynamicImage::ImageRgb8(preset.apply_preview(&img));
        encode_jpeg(&styled, 88).map_err(|e| e.to_string())
    })
    .await;
    match result {
        Ok(Ok(bytes)) => ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], bytes).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("preview task: {e}")).into_response(),
    }
}
