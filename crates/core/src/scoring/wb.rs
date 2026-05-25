use image::DynamicImage;

/// Plan B.2: gray-world assumption, with a colorfulness exemption.
///
/// Base score `wb_raw = exp(-d / 0.15)` where `d = max(|R̄-Ḡ|, |B̄-Ḡ|) / Ḡ`.
/// A global cast lowers `wb_raw` — but an intentionally colorful scene (sunset,
/// autumn foliage, a color-graded frame) also reads as a cast under gray-world.
/// We separate the two by **hue diversity**: a true cast concentrates in one
/// hue, while a colorful scene spreads across the wheel. High-diversity images
/// are blended back toward neutral so correct, vivid frames aren't penalized.
/// Face-aware skin-tone fusion is deferred to M3 (depends on face detection).
pub fn score(img: &DynamicImage) -> f32 {
    let rgb = img.to_rgb8();
    let n = (rgb.width() as f64) * (rgb.height() as f64);
    if n < 1.0 {
        return 0.0;
    }

    let (mut sr, mut sg, mut sb) = (0.0f64, 0.0f64, 0.0f64);
    // Saturation-weighted hue histogram (36 × 10° bins); pale pixels barely count.
    let mut hue_hist = [0.0f32; 36];
    let mut hue_weight = 0.0f32;
    for p in rgb.pixels() {
        let (r, g, b) = (p.0[0], p.0[1], p.0[2]);
        sr += r as f64;
        sg += g as f64;
        sb += b as f64;

        let (rf, gf, bf) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        let max = rf.max(gf).max(bf);
        let min = rf.min(gf).min(bf);
        let sat = if max > 1e-4 { (max - min) / max } else { 0.0 };
        if sat > 0.10 {
            let bin = ((hue(rf, gf, bf, max, min) / 10.0).floor() as usize) % 36;
            hue_hist[bin] += sat;
            hue_weight += sat;
        }
    }

    let (mr, mg, mb) = (sr / n, sg / n, sb / n);
    if mg < 1.0 {
        // Almost black image — no useful WB signal; neutral score.
        return 0.5;
    }
    let d = ((mr - mg).abs().max((mb - mg).abs())) / mg;
    let wb_raw = (-d / 0.15).exp().clamp(0.0, 1.0) as f32;

    // Hue diversity in [0,1]: normalized entropy of the histogram. One dominant
    // hue (a real cast) → ~0; many hues (a colorful scene) → ~1.
    let diversity = if hue_weight > 1.0 {
        let mut entropy = 0.0f32;
        for &w in &hue_hist {
            if w > 0.0 {
                let pp = w / hue_weight;
                entropy -= pp * pp.ln();
            }
        }
        (entropy / 36.0f32.ln()).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // Relax the gray-world penalty in proportion to colorfulness.
    (wb_raw + (1.0 - wb_raw) * diversity).clamp(0.0, 1.0)
}

/// HSV hue in degrees `[0, 360)` from RGB in `[0,1]` and precomputed max/min.
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

    fn solid_rgb(r: u8, g: u8, b: u8) -> DynamicImage {
        let mut img = RgbImage::new(32, 32);
        for p in img.pixels_mut() {
            *p = Rgb([r, g, b]);
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn neutral_gray_scores_perfect() {
        let s = score(&solid_rgb(128, 128, 128));
        assert!(s > 0.99);
    }

    #[test]
    fn heavy_blue_cast_scores_lower() {
        let s = score(&solid_rgb(80, 100, 200));
        assert!(s < 0.5, "got {}", s);
    }

    /// Six saturated hues pushed slightly warm — a colorful scene that also
    /// carries a mild gray-world cast.
    fn multi_hue() -> DynamicImage {
        let palette = [
            [220u8, 60, 60],
            [220, 200, 60],
            [120, 200, 80],
            [80, 200, 200],
            [90, 110, 220],
            [200, 90, 200],
        ];
        let mut img = RgbImage::new(60, 10);
        for (i, p) in img.pixels_mut().enumerate() {
            *p = Rgb(palette[i % palette.len()]);
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn diverse_hues_relax_cast_penalty() {
        let diverse = multi_hue();
        // Solid image at the exact mean colour of `diverse`: identical gray-world
        // cast, but zero hue diversity. The colorful frame must score higher
        // because the diversity exemption lifts it.
        let rgb = diverse.to_rgb8();
        let n = rgb.pixels().len() as f64;
        let (mut sr, mut sg, mut sb) = (0.0f64, 0.0, 0.0);
        for p in rgb.pixels() {
            sr += p.0[0] as f64;
            sg += p.0[1] as f64;
            sb += p.0[2] as f64;
        }
        let solid = solid_rgb(
            (sr / n).round() as u8,
            (sg / n).round() as u8,
            (sb / n).round() as u8,
        );
        let (sd, ss) = (score(&diverse), score(&solid));
        assert!(sd > ss + 0.1, "diverse={sd} solid={ss}");
    }
}
