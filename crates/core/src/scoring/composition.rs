//! Composition score (plan B.5).
//!
//! Heuristic implementation that doesn't need a saliency segmentation model.
//! The idea: the in-focus subject usually has the highest local sharpness
//! (Laplacian magnitude), so we threshold the Laplacian map at a high quantile
//! and treat the resulting hot region as the subject proxy. From its centroid
//! and area we derive:
//!
//! - **Rule of thirds**: distance from the centroid to the nearest 1/3 power point
//! - **Subject size**: Gaussian preferring ~25% area coverage
//! - **Edge clipping**: fraction of the four frame edges the hot region touches
//!
//! Trade-off vs. a real saliency model: this misses correctly-framed
//! out-of-focus subjects (silhouettes against bright backgrounds, mid-shutter
//! pans), but for typical sharp-focused photography it's a reasonable proxy
//! and zero extra dependencies.

use image::DynamicImage;

pub trait CompositionScorer: Send + Sync {
    fn score(&self, thumb: &DynamicImage) -> f32;
}

/// Original neutral stub kept for tests / fallback.
pub struct NeutralCompositionStub;

impl CompositionScorer for NeutralCompositionStub {
    fn score(&self, _thumb: &DynamicImage) -> f32 {
        0.5
    }
}

/// Laplacian-saliency composition scorer (production default).
pub struct HeuristicCompositionScorer;

impl CompositionScorer for HeuristicCompositionScorer {
    fn score(&self, thumb: &DynamicImage) -> f32 {
        // Primary: Laplacian-saliency. Misses soft/backlit/silhouette subjects.
        // Fallback: color-saliency (high-saturation region). Handles intentional
        // soft focus + dramatic color subjects where edges are weak.
        let (center, area_ratio, edge_clip) = match estimate_subject(thumb)
            .or_else(|| estimate_subject_by_saturation(thumb))
        {
            Some(s) => s,
            None => return 0.5,
        };

        let thirds = thirds_score(center);
        // Gaussian, peak at 25% area, σ=15%.
        let size = (-((area_ratio - 0.25_f32).powi(2) / (2.0 * 0.15_f32.powi(2)))).exp();
        let edge = 1.0 - edge_clip;

        (0.5 * thirds + 0.3 * size + 0.2 * edge).clamp(0.0, 1.0)
    }
}

/// Saturation-based subject estimator. Used when the edge/Laplacian estimator
/// fails (uniform luma, soft focus, silhouettes against bright skies). Returns
/// the same `(centroid, area_ratio, edge_clip)` triple as `estimate_subject`.
///
/// Strategy: subsample to ≤256px, compute per-pixel HSV saturation as
/// `(max−min) / max`, threshold at 60% of the image's peak saturation, then
/// take the bbox + centroid of the accepted pixels.
fn estimate_subject_by_saturation(img: &DynamicImage) -> Option<((f32, f32), f32, f32)> {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    if w < 8 || h < 8 {
        return None;
    }
    let raw = rgb.as_raw();
    let stride = ((w.max(h) / 256).max(1)).max(1);
    let sw = w.div_ceil(stride);
    let sh = h.div_ceil(stride);
    if sw < 4 || sh < 4 {
        return None;
    }

    let mut sat = vec![0u8; sw * sh];
    let mut max_sat = 0u8;
    for sy in 0..sh {
        for sx in 0..sw {
            let ix = (sx * stride).min(w - 1);
            let iy = (sy * stride).min(h - 1);
            let off = (iy * w + ix) * 3;
            let r = raw[off];
            let g = raw[off + 1];
            let b = raw[off + 2];
            let mx = r.max(g).max(b);
            let mn = r.min(g).min(b);
            // HSV saturation × 255; mx==0 → fully black → no color → 0.
            let s = if mx == 0 {
                0
            } else {
                (((mx - mn) as u32 * 255) / mx as u32) as u8
            };
            sat[sy * sw + sx] = s;
            if s > max_sat {
                max_sat = s;
            }
        }
    }
    // Need a minimally colorful scene; otherwise this is no better than the
    // Laplacian fallback returning 0.5.
    if max_sat < 40 {
        return None;
    }
    let threshold = ((max_sat as u32 * 60) / 100).max(30) as u8;

    let mut sum_x = 0.0f64;
    let mut sum_y = 0.0f64;
    let mut count = 0.0f64;
    let mut min_x = sw;
    let mut min_y = sh;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    for y in 0..sh {
        for x in 0..sw {
            if sat[y * sw + x] >= threshold {
                sum_x += x as f64;
                sum_y += y as f64;
                count += 1.0;
                if x < min_x { min_x = x; }
                if y < min_y { min_y = y; }
                if x > max_x { max_x = x; }
                if y > max_y { max_y = y; }
            }
        }
    }
    if count < 4.0 {
        return None;
    }
    let cx = sum_x / count / sw as f64;
    let cy = sum_y / count / sh as f64;
    let bbox_w = (max_x - min_x + 1) as f64;
    let bbox_h = (max_y - min_y + 1) as f64;
    let area = bbox_w * bbox_h / (sw as f64 * sh as f64);

    let margin_x = sw / 30 + 1;
    let margin_y = sh / 30 + 1;
    let mut touches = 0u8;
    if min_y < margin_y { touches += 1; }
    if max_y + margin_y >= sh { touches += 1; }
    if min_x < margin_x { touches += 1; }
    if max_x + margin_x >= sw { touches += 1; }
    let edge_clip = touches as f32 / 4.0;

    Some(((cx as f32, cy as f32), area as f32, edge_clip))
}

/// Returns `(centroid in [0,1]², area_ratio, edge_clip_fraction)` for the
/// estimated subject. `None` when the image is too uniform to localise one.
///
/// Threshold strategy: pixels are accepted at ≥ 30% of the image's peak
/// Laplacian magnitude. Adapts to image-specific dynamics — high-contrast
/// dots pick out their edges, low-contrast in-focus subjects also pass.
/// Area is estimated from the bounding box of accepted pixels (not their
/// count) so edge-based detection still gives a sensible "subject region"
/// for the size penalty.
fn estimate_subject(img: &DynamicImage) -> Option<((f32, f32), f32, f32)> {
    let gray = img.to_luma8();
    let (w, h) = (gray.width() as i32, gray.height() as i32);
    if w < 8 || h < 8 {
        return None;
    }
    let raw = gray.as_raw();
    let row_len = w as usize;

    // Subsample to ~256px max for speed.
    let stride = ((w.max(h) / 256).max(1)) as usize;
    let sw = (w as usize).div_ceil(stride);
    let sh = (h as usize).div_ceil(stride);
    if sw < 4 || sh < 4 {
        return None;
    }

    let mut lap: Vec<u16> = Vec::with_capacity(sw * sh);
    for y in 0..sh {
        for x in 0..sw {
            let ix = (x * stride).min(w as usize - 1);
            let iy = (y * stride).min(h as usize - 1);
            let c = raw[iy * row_len + ix] as i32;
            let l = raw[iy * row_len + ix.saturating_sub(stride.min(ix))] as i32;
            let r = raw[iy * row_len + (ix + stride).min(w as usize - 1)] as i32;
            let u = raw[iy.saturating_sub(stride.min(iy)) * row_len + ix] as i32;
            let d = raw[(iy + stride).min(h as usize - 1) * row_len + ix] as i32;
            let v = (4 * c - l - r - u - d).unsigned_abs() as u16;
            lap.push(v);
        }
    }

    let max_lap = lap.iter().copied().max().unwrap_or(0);
    if max_lap < 8 {
        return None;
    }
    let threshold = ((max_lap as u32) / 3).max(4) as u16;

    let mut sum_x = 0.0_f64;
    let mut sum_y = 0.0_f64;
    let mut count = 0.0_f64;
    let mut min_x = sw;
    let mut min_y = sh;
    let mut max_x = 0_usize;
    let mut max_y = 0_usize;

    for y in 0..sh {
        for x in 0..sw {
            if lap[y * sw + x] >= threshold {
                sum_x += x as f64;
                sum_y += y as f64;
                count += 1.0;
                if x < min_x { min_x = x; }
                if y < min_y { min_y = y; }
                if x > max_x { max_x = x; }
                if y > max_y { max_y = y; }
            }
        }
    }

    if count < 1.0 {
        return None;
    }
    let cx = sum_x / count / sw as f64;
    let cy = sum_y / count / sh as f64;

    // Area from bounding box of the high-saliency region.
    let bbox_w = (max_x - min_x + 1) as f64;
    let bbox_h = (max_y - min_y + 1) as f64;
    let area = bbox_w * bbox_h / (sw as f64 * sh as f64);

    let margin_x = sw / 30 + 1;
    let margin_y = sh / 30 + 1;
    let mut touches = 0u8;
    if min_y < margin_y { touches += 1; }
    if max_y + margin_y >= sh { touches += 1; }
    if min_x < margin_x { touches += 1; }
    if max_x + margin_x >= sw { touches += 1; }
    let edge_clip = touches as f32 / 4.0;

    Some(((cx as f32, cy as f32), area as f32, edge_clip))
}

fn thirds_score(center: (f32, f32)) -> f32 {
    let (cx, cy) = center;
    const THIRDS: [(f32, f32); 4] = [
        (1.0 / 3.0, 1.0 / 3.0),
        (2.0 / 3.0, 1.0 / 3.0),
        (1.0 / 3.0, 2.0 / 3.0),
        (2.0 / 3.0, 2.0 / 3.0),
    ];
    let min_dist = THIRDS
        .iter()
        .map(|(tx, ty)| {
            let dx = cx - tx;
            let dy = cy - ty;
            (dx * dx + dy * dy).sqrt()
        })
        .fold(f32::INFINITY, f32::min);
    (-min_dist / 0.15).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, GrayImage, Luma};

    fn solid(size: u32, v: u8) -> DynamicImage {
        let mut img = GrayImage::new(size, size);
        for p in img.pixels_mut() {
            *p = Luma([v]);
        }
        DynamicImage::ImageLuma8(img)
    }

    fn dot_at(size: u32, cx: u32, cy: u32, dot: u32) -> DynamicImage {
        let mut img = GrayImage::new(size, size);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let inside = ((x as i32 - cx as i32).pow(2) + (y as i32 - cy as i32).pow(2))
                < (dot * dot) as i32;
            *p = if inside { Luma([255]) } else { Luma([20]) };
        }
        DynamicImage::ImageLuma8(img)
    }

    #[test]
    fn uniform_image_returns_neutral() {
        let s = HeuristicCompositionScorer.score(&solid(128, 128));
        assert!((s - 0.5).abs() < 1e-3);
    }

    #[test]
    fn subject_at_thirds_scores_higher_than_dead_center() {
        let thirds = HeuristicCompositionScorer.score(&dot_at(300, 100, 100, 20));
        let center = HeuristicCompositionScorer.score(&dot_at(300, 150, 150, 20));
        assert!(thirds > center, "thirds={thirds} center={center}");
    }

    #[test]
    fn huge_subject_penalized_for_size() {
        let medium = HeuristicCompositionScorer.score(&dot_at(300, 100, 100, 40));
        let huge = HeuristicCompositionScorer.score(&dot_at(300, 100, 100, 140));
        assert!(medium > huge, "medium={medium} huge={huge}");
    }

    /// A soft-edge bright red blob on a neutral gray background — Laplacian
    /// saliency may miss it (low edge energy), but the color fallback should
    /// localize it, yielding a non-neutral score.
    #[test]
    fn color_fallback_localizes_soft_subject() {
        let mut img = image::RgbImage::new(300, 300);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let dx = x as f32 - 100.0;
            let dy = y as f32 - 100.0;
            let d2 = dx * dx + dy * dy;
            // Soft red blob with a wide gaussian falloff into mid gray.
            let fall = (-d2 / 3000.0).exp();
            let r = (128.0 + 127.0 * fall) as u8;
            let g = (128.0 - 60.0 * fall) as u8;
            let b = (128.0 - 60.0 * fall) as u8;
            *p = image::Rgb([r, g, b]);
        }
        let s = HeuristicCompositionScorer.score(&DynamicImage::ImageRgb8(img));
        assert!(
            (s - 0.5).abs() > 0.05,
            "expected non-neutral score from color fallback, got {s}"
        );
    }
}
