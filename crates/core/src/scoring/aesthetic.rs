//! Aesthetic / image-quality scoring.
//!
//! **Primary: learned.** [`learned_aesthetic`] runs the LAION aesthetic
//! predictor (a linear head trained on ~176k human ratings from the
//! Simulacra Aesthetic Captions / LAION datasets) over the CLIP ViT-B/32 image
//! embedding the pipeline already computes for Stage B. It's a 512-wide dot
//! product, so it costs nothing next to the CLIP forward pass, and the 2 KB
//! of weights are compiled in (no extra model download). The pipeline uses it
//! whenever a photo has a CLIP embedding.
//!
//! **Fallback: heuristic.** [`HeuristicAestheticScorer`] is used only when CLIP
//! is disabled or failed to load. It's an honest non-constant signal based on
//! three classic visual-interest proxies:
//!
//! - **Hue diversity**: how spread across the colour wheel the image is
//!   (entropy of a 36-bin hue histogram, weighted by saturation so pale
//!   regions don't dominate).
//! - **Saturation richness**: mean of the top half of saturation distribution,
//!   rewards vivid colour without penalising intentional muted palettes.
//! - **Luminance dynamic range**: width of the p5 → p95 luma band; penalises
//!   flat/dull or fully washed-out frames.
//!
//! These are **not** a learned preference model — they correlate with "snappy"
//! photos but won't match individual taste.

use super::aesthetic_laion_weights::{LAION_B32_BIAS, LAION_B32_WEIGHTS};
use image::DynamicImage;

/// LAION predictor output (roughly a 1–10 rating) that maps to 0.5.
///
/// Calibrated against OpenAI CLIP ViT-B/32 on reference photos and degraded
/// copies of them: clean, well-made photos land around 5–7, the same frames
/// blurred / underexposed / badly cropped drop to ~2.5–4.5, and flat
/// documents or textures sit below 2.5. Centring the sigmoid at 4.5 with unit
/// scale spreads that range over [0,1] (6.8 → 0.91, 5.2 → 0.67, 3.8 → 0.33,
/// 2.5 → 0.12) instead of piling everything near one end.
const LAION_MIDPOINT: f32 = 4.5;
const LAION_SCALE: f32 = 1.0;

/// Raw LAION aesthetic prediction (≈1–10 scale) for an L2-normalized CLIP
/// ViT-B/32 image embedding. `None` if the embedding has the wrong width
/// (e.g. a different CLIP variant), so callers fall back instead of scoring
/// garbage.
pub fn laion_aesthetic_raw(embed: &[f32]) -> Option<f32> {
    if embed.len() != LAION_B32_WEIGHTS.len() {
        return None;
    }
    let dot: f32 = embed.iter().zip(LAION_B32_WEIGHTS.iter()).map(|(e, w)| e * w).sum();
    let raw = dot + LAION_B32_BIAS;
    raw.is_finite().then_some(raw)
}

/// Learned aesthetic score in `[0, 1]` from a CLIP ViT-B/32 embedding.
pub fn learned_aesthetic(embed: &[f32]) -> Option<f32> {
    let raw = laion_aesthetic_raw(embed)?;
    Some(1.0 / (1.0 + (-(raw - LAION_MIDPOINT) / LAION_SCALE).exp()))
}

pub trait AestheticScorer: Send + Sync {
    fn score(&self, thumb: &DynamicImage) -> f32;
}

pub struct NeutralAestheticStub;

impl AestheticScorer for NeutralAestheticStub {
    fn score(&self, _thumb: &DynamicImage) -> f32 {
        0.5
    }
}

pub struct HeuristicAestheticScorer;

impl AestheticScorer for HeuristicAestheticScorer {
    fn score(&self, thumb: &DynamicImage) -> f32 {
        let stats = compute(thumb);
        // Blend: range is the strongest signal (well-exposed dynamic photos
        // tend to look better), hue diversity adds variety credit, saturation
        // rewards vivid colour but capped so monochrome work doesn't crater.
        let s = 0.45 * stats.luma_range_score
            + 0.30 * stats.hue_diversity
            + 0.25 * stats.saturation;
        s.clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Copy)]
struct VisualStats {
    luma_range_score: f32,
    hue_diversity: f32,
    saturation: f32,
}

fn compute(img: &DynamicImage) -> VisualStats {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    if w == 0 || h == 0 {
        return VisualStats { luma_range_score: 0.0, hue_diversity: 0.0, saturation: 0.0 };
    }

    // Subsample to ~64k pixels max for speed.
    let stride = (((w * h) / 65_536).max(1) as f32).sqrt().ceil() as usize;
    let stride = stride.max(1);

    let mut luma: Vec<u8> = Vec::with_capacity(w * h / (stride * stride));
    let mut hue_hist = [0.0_f32; 36];
    let mut hue_total_weight = 0.0_f32;
    let mut sat_samples: Vec<f32> = Vec::with_capacity(w * h / (stride * stride));

    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let p = rgb.get_pixel(x as u32, y as u32);
            let (r, g, b) = (p.0[0] as f32 / 255.0, p.0[1] as f32 / 255.0, p.0[2] as f32 / 255.0);
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let s = if max > 1e-4 { (max - min) / max } else { 0.0 };
            let lum_byte = (0.299 * r + 0.587 * g + 0.114 * b) * 255.0;
            luma.push(lum_byte as u8);
            sat_samples.push(s);

            if s > 0.10 {
                // Hue in degrees
                let h_deg = hue(r, g, b, max, min);
                let bin = ((h_deg / 10.0).floor() as usize) % 36;
                let weight = s; // weight by saturation so washed pixels count less
                hue_hist[bin] += weight;
                hue_total_weight += weight;
            }

            x += stride;
        }
        y += stride;
    }

    // Luma dynamic-range score: width of p5-p95 band, normalised to [0,1].
    let mut sorted = luma.clone();
    sorted.sort_unstable();
    let n = sorted.len();
    let luma_range_score = if n >= 20 {
        let p5 = sorted[n * 5 / 100] as f32;
        let p95 = sorted[n * 95 / 100] as f32;
        ((p95 - p5) / 200.0).clamp(0.0, 1.0)
    } else {
        0.5
    };

    // Hue diversity: normalised entropy of the saturation-weighted hue histogram.
    let hue_diversity = if hue_total_weight > 1.0 {
        let mut entropy = 0.0_f32;
        for &w in &hue_hist {
            if w > 0.0 {
                let p = w / hue_total_weight;
                entropy -= p * p.ln();
            }
        }
        // Max entropy = ln(36) ≈ 3.58
        (entropy / 3.58).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // Saturation: mean of the top-half (filters out the always-low-sat
    // background pixels in most photos so vivid subjects get credit).
    sat_samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let half = sat_samples.len() / 2;
    let saturation = if half > 0 {
        sat_samples[half..].iter().sum::<f32>() / (sat_samples.len() - half) as f32
    } else {
        0.0
    };

    VisualStats { luma_range_score, hue_diversity, saturation: saturation.clamp(0.0, 1.0) }
}

fn hue(r: f32, g: f32, b: f32, max: f32, min: f32) -> f32 {
    let delta = max - min;
    if delta < 1e-4 {
        return 0.0;
    }
    let h = if max == r {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    if h < 0.0 {
        h + 360.0
    } else {
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgb, RgbImage};

    fn solid(r: u8, g: u8, b: u8) -> DynamicImage {
        let mut img = RgbImage::new(64, 64);
        for p in img.pixels_mut() {
            *p = Rgb([r, g, b]);
        }
        DynamicImage::ImageRgb8(img)
    }

    fn rainbow() -> DynamicImage {
        let mut img = RgbImage::new(128, 64);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            let h = (x as f32 / 128.0) * 360.0;
            let (r, g, b) = hsv_to_rgb(h, 0.9, 0.9);
            *p = Rgb([
                (r * 255.0) as u8,
                (g * 255.0) as u8,
                (b * 255.0) as u8,
            ]);
        }
        DynamicImage::ImageRgb8(img)
    }

    fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
        let c = v * s;
        let h2 = h / 60.0;
        let x = c * (1.0 - (h2.rem_euclid(2.0) - 1.0).abs());
        let (r, g, b) = match h2 as i32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = v - c;
        (r + m, g + m, b + m)
    }

    /// Reference values computed in Python from the source `.pth` tensors.
    #[test]
    fn laion_head_matches_reference_weights() {
        // Unit vector e0 → weight[0] + bias.
        let mut e0 = vec![0.0_f32; 512];
        e0[0] = 1.0;
        let raw = laion_aesthetic_raw(&e0).unwrap();
        assert!((raw - (1.208_079_6 + 3.421_470_9)).abs() < 1e-4, "raw={raw}");

        // Uniform unit vector → sum(weights)/sqrt(512) + bias.
        let u = vec![1.0 / (512.0_f32).sqrt(); 512];
        let raw = laion_aesthetic_raw(&u).unwrap();
        assert!((raw - 3.908_14).abs() < 1e-3, "raw={raw}");
    }

    #[test]
    fn learned_aesthetic_rejects_wrong_width_and_is_monotone() {
        assert!(learned_aesthetic(&[0.0; 768]).is_none());
        assert!(learned_aesthetic(&[]).is_none());
        // Scaling an embedding along the weight direction raises the score.
        let norm = LAION_B32_WEIGHTS.iter().map(|w| w * w).sum::<f32>().sqrt();
        let dir: Vec<f32> = LAION_B32_WEIGHTS.iter().map(|w| w / norm).collect();
        let anti: Vec<f32> = dir.iter().map(|x| -x).collect();
        let hi = learned_aesthetic(&dir).unwrap();
        let lo = learned_aesthetic(&anti).unwrap();
        assert!(hi > 0.99 && lo < 0.01, "hi={hi} lo={lo}");
    }

    #[test]
    fn flat_gray_scores_low() {
        let s = HeuristicAestheticScorer.score(&solid(128, 128, 128));
        assert!(s < 0.3, "flat gray should score low (got {s})");
    }

    #[test]
    fn vivid_rainbow_scores_higher_than_gray() {
        let gray = HeuristicAestheticScorer.score(&solid(128, 128, 128));
        let rb = HeuristicAestheticScorer.score(&rainbow());
        assert!(rb > gray + 0.2, "rainbow={rb} gray={gray}");
    }
}
