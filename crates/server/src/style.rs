//! Style extraction endpoints — a standalone tool, independent of scan runs.
//!
//! `POST /api/style/preset` learns a look from *reference* photos, optionally
//! fitting it against a set of *target* photos (the ones the user wants to
//! restyle), and returns a Lightroom preset and/or 3D LUT. Targets are
//! optional — without them only the content-independent (unpaired) fit is
//! possible, and there are no previews. `POST /api/style/preview`
//! renders a target photo before / after (approximately) applying a preset.
//!
//! References that are edited versions of target photos are paired with their
//! originals automatically. Tone curves and colour grades keep the brightness
//! *ordering* that dHash encodes and the low-frequency structure pHash
//! encodes, so on real photos an edit sits 0–3 bits from its original on
//! both, while unrelated shots are 22–31 bits away. Requiring both guards
//! against low-detail frames (plain sky, a wall) whose dHashes collide.
//! Paired fitting is far more accurate; see `photo_pick_core::style`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use photo_pick_core::features::{hash::hamming, FeatureExtractor, HashOnlyExtractor};
use photo_pick_core::ingest::{
    classify_extension, decode_thumbnail_for, encode_jpeg, PhotoId, PhotoRef, ThumbnailSpec,
};
use photo_pick_core::output::DEFAULT_THUMB_LONG_EDGE;
use photo_pick_core::style::{PairedPixels, StyleFile, StyleFormat, StylePreset, StyleStats};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// An edit and its original differ by a few bits on both hashes; unrelated
/// frames by 22+. At or under this on *both* counts as the same shot.
const PAIR_MAX_HAMMING: u32 = 10;
/// Cap on target photos decoded per request. Beyond it, targets whose file
/// name resembles a reference's are kept (likely originals) and the rest are
/// sampled evenly.
const MAX_TARGETS: usize = 600;
/// Cap on target photos used as the unpaired baseline (evenly sampled).
const MAX_BASELINE: usize = 300;
/// Reference photos accepted per request.
const MAX_REFERENCES: usize = 50;
/// Target photos suggested for before/after previews.
const PREVIEW_COUNT: usize = 4;

#[derive(Debug, Deserialize)]
pub struct StylePresetRequest {
    /// Photos whose look to learn.
    pub reference_paths: Vec<PathBuf>,
    /// Photos to restyle (optional), as individual files…
    #[serde(default)]
    pub target_paths: Vec<PathBuf>,
    /// …and/or a folder of them (walked recursively, hidden entries skipped).
    #[serde(default)]
    pub target_dir: Option<PathBuf>,
    #[serde(default)]
    pub name: Option<String>,
    /// 0–1; scales every adjustment. Defaults to 1.
    #[serde(default)]
    pub strength: Option<f32>,
    /// Files to produce. Defaults to a Lightroom preset only.
    #[serde(default = "default_formats")]
    pub formats: Vec<StyleFormat>,
}

fn default_formats() -> Vec<StyleFormat> {
    vec![StyleFormat::LrPreset]
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
    /// One rendered file per requested format, in request order.
    pub files: Vec<StyleFile>,
    pub mode: FitMode,
    /// References that contributed to the fit.
    pub references_used: usize,
    /// Paired references whose framing matches their original (not cropped
    /// or rotated), so per-colour changes (HSL, colour grading) could be
    /// learned from them. 0 in unpaired mode.
    pub references_aligned: usize,
    /// In paired mode: readable references with no original among the
    /// targets (left out, since mixing modes would dilute the paired fit).
    pub references_unpaired: Vec<PathBuf>,
    /// References that couldn't be read at all.
    pub references_failed: Vec<ReferenceIssue>,
    /// Target photos found, and how many of them were actually read (both 0
    /// when none were given).
    pub targets_total: usize,
    pub targets_read: usize,
    /// Photos the fit compared against (originals, or the baseline sample).
    pub baseline_count: usize,
    /// Target photos suggested for before/after previews.
    pub preview_paths: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub struct StylePreviewRequest {
    pub path: PathBuf,
    /// Preset to apply; omit for the untouched "before" image.
    #[serde(default)]
    pub preset: Option<StylePreset>,
}

fn bad_request(msg: impl Into<String>) -> Response {
    (StatusCode::BAD_REQUEST, msg.into()).into_response()
}

/// Same allowlist as /browse and /scan — the style tool must not be a way
/// around it. Paths that don't resolve pass here and fail later as unreadable.
/// Returns a ready 403 response for the first path outside the roots.
fn check_in_roots<'a>(paths: impl IntoIterator<Item = &'a PathBuf>) -> Option<Response> {
    let roots = crate::handlers::allowed_roots();
    paths.into_iter().find_map(|p| {
        let c = std::fs::canonicalize(p).map(crate::handlers::strip_verbatim_prefix).ok()?;
        (!crate::handlers::path_under(&c, &roots)).then(|| {
            (
                StatusCode::FORBIDDEN,
                format!(
                    "{} is outside the configured browse roots; set PHOTO_PICK_BROWSE_ROOTS to widen",
                    c.display()
                ),
            )
                .into_response()
        })
    })
}

pub async fn style_preset(Json(req): Json<StylePresetRequest>) -> Response {
    if req.reference_paths.is_empty() {
        return bad_request("pick at least one reference photo");
    }
    if req.reference_paths.len() > MAX_REFERENCES {
        return bad_request(format!("at most {MAX_REFERENCES} reference photos"));
    }
    let mut seen = std::collections::HashSet::new();
    let formats: Vec<StyleFormat> = req.formats.iter().copied().filter(|f| seen.insert(*f)).collect();
    if formats.is_empty() {
        return bad_request("pick at least one output format");
    }
    if let Some(forbidden) =
        check_in_roots(req.reference_paths.iter().chain(&req.target_paths).chain(req.target_dir.iter()))
    {
        return forbidden;
    }

    let name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("photo-pick style")
        .to_string();
    let strength = req.strength.unwrap_or(1.0).clamp(0.0, 1.0);

    let result = tokio::task::spawn_blocking(move || {
        let given = !req.target_paths.is_empty() || req.target_dir.is_some();
        let mut targets: Vec<PathBuf> = req.target_paths.clone();
        if let Some(dir) = &req.target_dir {
            if !dir.is_dir() {
                return Err(format!("{} is not a folder", dir.display()));
            }
            list_photos(dir, &mut targets);
        }
        fit_style(&name, strength, &formats, &req.reference_paths, given.then_some(targets))
    })
    .await;
    match result {
        Ok(Ok(r)) => Json(r).into_response(),
        Ok(Err(e)) => bad_request(e),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("style task: {e}")).into_response(),
    }
}

/// Recursively collect supported photo files under `dir` (hidden entries
/// skipped, symlinks not followed). No content hashing — unlike a scan, the
/// style tool only needs thumbnails, so a big RAW folder lists instantly.
fn list_photos(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.')) {
            continue;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() => list_photos(&path, out),
            Ok(t) if t.is_file() && classify_extension(&path).is_some() => out.push(path),
            _ => {}
        }
    }
}

/// A minimal `PhotoRef` for decoding a file: format from the extension;
/// orientation is read from the file by the decoder itself.
fn photo_ref(path: &Path) -> Option<PhotoRef> {
    Some(PhotoRef {
        id: PhotoId::new(),
        path: path.to_path_buf(),
        format: classify_extension(path)?,
        captured_at: None,
        file_size: 0,
        sha256_short: [0; 16],
        burst_id: None,
        drive_mode: None,
        iso: None,
        exposure_bias_ev: None,
    })
}

fn thumb_spec() -> ThumbnailSpec {
    ThumbnailSpec { long_edge: DEFAULT_THUMB_LONG_EDGE }
}

/// A photo's thumbnail plus its perceptual hashes.
struct Loaded {
    path: PathBuf,
    img: image::DynamicImage,
    dhash: u64,
    phash: u64,
}

impl Loaded {
    fn distance(&self, other: &Loaded) -> (u32, u32) {
        (hamming(self.dhash, other.dhash), hamming(self.phash, other.phash))
    }
}

fn load(path: &Path, hasher: &HashOnlyExtractor) -> Result<Loaded, String> {
    let photo = photo_ref(path).ok_or_else(|| "unsupported file type".to_string())?;
    let img = decode_thumbnail_for(&photo, thumb_spec()).map_err(|e| e.to_string())?;
    let f = hasher.extract(&photo, &img).map_err(|e| e.to_string())?;
    Ok(Loaded { path: path.to_path_buf(), img, dhash: f.dhash, phash: f.phash })
}

/// Lower-cased file stem with common edit suffixes' separators normalised,
/// e.g. `IMG_1234-Edit-2.jpg` → `img_1234-edit-2`.
fn stem(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase()
}

/// Keep at most `cap` targets: every one whose stem a reference's stem starts
/// with (or vice versa) — edited exports usually keep the original's name —
/// then an even sample of the rest.
fn choose_targets(mut targets: Vec<PathBuf>, refs: &[PathBuf], cap: usize) -> Vec<PathBuf> {
    targets.sort();
    targets.dedup();
    if targets.len() <= cap {
        return targets;
    }
    let ref_stems: Vec<String> = refs.iter().map(|r| stem(r)).filter(|s| s.len() >= 3).collect();
    let (mut keep, rest): (Vec<PathBuf>, Vec<PathBuf>) = targets.into_iter().partition(|t| {
        let s = stem(t);
        s.len() >= 3 && ref_stems.iter().any(|r| r.starts_with(&s) || s.starts_with(r.as_str()))
    });
    keep.truncate(cap);
    let room = cap - keep.len();
    if room > 0 && !rest.is_empty() {
        let step = rest.len() as f64 / room as f64;
        keep.extend((0..room.min(rest.len())).map(|i| rest[(i as f64 * step) as usize].clone()));
    }
    keep
}

fn fit_style(
    name: &str,
    strength: f32,
    formats: &[StyleFormat],
    reference_paths: &[PathBuf],
    targets: Option<Vec<PathBuf>>,
) -> Result<StylePresetResponse, String> {
    let hasher = HashOnlyExtractor::new();
    let mut targets_total = 0usize;

    let mut failed = Vec::new();
    let mut refs: Vec<Loaded> = Vec::new();
    for path in reference_paths {
        match load(path, &hasher) {
            Ok(l) => refs.push(l),
            Err(reason) => failed.push(ReferenceIssue { path: path.clone(), reason }),
        }
    }
    if refs.is_empty() {
        return Err("none of the reference photos could be read".into());
    }

    // Targets, minus the references themselves (they may sit in the same
    // folder). None given → just the style file, from the references alone.
    let run: Vec<Loaded> = match targets {
        None => Vec::new(),
        Some(targets) => {
            let targets: Vec<PathBuf> = targets.into_iter().filter(|t| !reference_paths.contains(t)).collect();
            if targets.is_empty() {
                return Err("no photos to restyle were found (besides the references)".into());
            }
            let total = targets.len();
            let run = load_parallel(&choose_targets(targets, reference_paths, MAX_TARGETS), &hasher);
            if run.is_empty() {
                return Err("none of the photos to restyle could be read".into());
            }
            targets_total = total;
            run
        }
    };

    // Pair each reference with its nearest target.
    let mut edited = StyleStats::new();
    let mut originals = StyleStats::new();
    let mut pixels = PairedPixels::new();
    let mut paired_refs = 0usize;
    let mut unpaired: Vec<&Loaded> = Vec::new();
    let mut paired_paths: Vec<&Path> = Vec::new();
    for r in &refs {
        let best = run.iter().map(|l| (l, l.distance(r))).min_by_key(|(_, (d, p))| d + p);
        match best {
            Some((o, (d, p))) if d <= PAIR_MAX_HAMMING && p <= PAIR_MAX_HAMMING => {
                edited.add_image(&r.img);
                originals.add_image(&o.img);
                pixels.add_pair(&r.img, &o.img);
                paired_refs += 1;
                paired_paths.push(&o.path);
            }
            _ => unpaired.push(r),
        }
    }

    let (preset, mode, used, baseline_count, references_unpaired) = if paired_refs > 0 {
        let p = StylePreset::fit_paired(name, &edited, &originals, &pixels, strength)
            .map_err(|e| e.to_string())?;
        let left_out = unpaired.iter().map(|r| r.path.clone()).collect();
        (p, FitMode::Paired, paired_refs, paired_refs, left_out)
    } else {
        let mut reference = StyleStats::new();
        refs.iter().for_each(|r| reference.add_image(&r.img));
        let mut baseline = StyleStats::new();
        let step = (run.len() / MAX_BASELINE).max(1);
        run.iter().step_by(step).for_each(|l| baseline.add_image(&l.img));
        let baseline = (baseline.image_count() > 0).then_some(&baseline);
        let p = StylePreset::fit_unpaired(name, &reference, baseline, strength).map_err(|e| e.to_string())?;
        (p, FitMode::Unpaired, refs.len(), baseline.map_or(0, |b| b.image_count()), Vec::new())
    };

    // Previews: photos the preset will actually be used on — skip the ones
    // that already have an edit (paired originals), spread across the set.
    let pool: Vec<&Loaded> = run.iter().filter(|l| !paired_paths.contains(&l.path.as_path())).collect();
    let pool: Vec<&Loaded> = if pool.is_empty() { run.iter().collect() } else { pool };
    let step = (pool.len() / PREVIEW_COUNT).max(1);
    let preview_paths = pool.iter().step_by(step).take(PREVIEW_COUNT).map(|l| l.path.clone()).collect();

    Ok(StylePresetResponse {
        files: formats.iter().map(|f| preset.render(*f)).collect(),
        preset,
        mode,
        references_used: used,
        references_aligned: pixels.pair_count(),
        references_unpaired,
        references_failed: failed,
        targets_total,
        targets_read: run.len(),
        baseline_count,
        preview_paths,
    })
}

/// Decode thumbnails and hash them on a CPU-sized worker pool, keeping input
/// order. Unreadable photos are skipped.
fn load_parallel(paths: &[PathBuf], hasher: &HashOnlyExtractor) -> Vec<Loaded> {
    let n_workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(paths.len()).max(1);
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<(usize, Loaded)>> = Mutex::new(Vec::with_capacity(paths.len()));
    std::thread::scope(|scope| {
        for _ in 0..n_workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = paths.get(i) else { break };
                match load(path, hasher) {
                    Ok(l) => out.lock().unwrap().push((i, l)),
                    Err(e) => tracing::warn!(path = %path.display(), error = %e, "style: photo unreadable; skipped"),
                }
            });
        }
    });
    let mut v = out.into_inner().unwrap();
    v.sort_by_key(|(i, _)| *i);
    v.into_iter().map(|(_, l)| l).collect()
}

pub async fn style_preview(Json(req): Json<StylePreviewRequest>) -> Response {
    if let Some(forbidden) = check_in_roots(std::iter::once(&req.path)) {
        return forbidden;
    }
    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let photo = photo_ref(&req.path).ok_or_else(|| "unsupported file type".to_string())?;
        let img = decode_thumbnail_for(&photo, thumb_spec()).map_err(|e| e.to_string())?;
        let out = match &req.preset {
            Some(p) => image::DynamicImage::ImageRgb8(p.apply_preview(&img)),
            None => img,
        };
        encode_jpeg(&out, 88).map_err(|e| e.to_string())
    })
    .await;
    match result {
        Ok(Ok(bytes)) => ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], bytes).into_response(),
        Ok(Err(e)) => bad_request(e),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("preview task: {e}")).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_targets_keeps_likely_originals_and_samples_the_rest() {
        let mut targets: Vec<PathBuf> = (0..1000).map(|i| PathBuf::from(format!("/t/DSC{i:05}.ARW"))).collect();
        targets.push(PathBuf::from("/t/IMG_0042.CR2"));
        let refs = vec![PathBuf::from("/e/IMG_0042-Edit.jpg")];
        let chosen = choose_targets(targets, &refs, 100);
        assert_eq!(chosen.len(), 100);
        assert!(chosen.contains(&PathBuf::from("/t/IMG_0042.CR2")), "name-matched original kept");
        let firsts: Vec<_> = chosen.iter().filter(|p| p.to_string_lossy().contains("DSC0000")).collect();
        assert!(firsts.len() <= 2, "the rest is sampled across the set, not the first N");
    }

    #[test]
    fn choose_targets_is_a_no_op_under_the_cap() {
        let t = vec![PathBuf::from("/b.jpg"), PathBuf::from("/a.jpg"), PathBuf::from("/a.jpg")];
        assert_eq!(choose_targets(t, &[], 10), vec![PathBuf::from("/a.jpg"), PathBuf::from("/b.jpg")]);
    }
}
