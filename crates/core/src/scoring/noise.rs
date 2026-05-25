use image::GrayImage;

/// Plan B.4: estimate noise from the flattest 5 % of 8×8 patches, then compare
/// against ISO-derived expectation.
///
/// `expected_σ = 1.0 · √(ISO/100)` (luma units, calibrated empirically — a
/// modern FF sensor at ISO 100 should sit near σ≈1, ISO 6400 near σ≈8).
/// `noise = clamp(expected / max(actual, ε), 0, 1)`.
pub fn score(gray: &GrayImage, iso: Option<u32>) -> f32 {
    let actual = estimate_noise_sigma(gray);
    let iso = iso.unwrap_or(100).max(50) as f32;
    let expected = (iso / 100.0).sqrt().max(0.5);
    let actual = actual.max(0.5);
    (expected / actual).clamp(0.0, 1.0)
}

/// Returns an estimate of per-pixel noise sigma in luma units (0..255).
fn estimate_noise_sigma(gray: &GrayImage) -> f32 {
    let (w, h) = (gray.width(), gray.height());
    let patch: u32 = 8;
    if w < patch || h < patch {
        return 1.0;
    }

    let raw = gray.as_raw();
    let row = w as usize;
    let mut variances: Vec<f32> = Vec::with_capacity(((w / patch) * (h / patch)) as usize);
    let mut y = 0;
    while y + patch <= h {
        let mut x = 0;
        while x + patch <= w {
            variances.push(patch_variance(raw, row, x as usize, y as usize, patch as usize));
            x += patch;
        }
        y += patch;
    }

    if variances.is_empty() {
        return 1.0;
    }
    variances.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let flat_count = (variances.len() / 20).max(1);
    let mean_var: f32 = variances[..flat_count].iter().sum::<f32>() / flat_count as f32;
    mean_var.sqrt()
}

/// Population variance of a `patch×patch` block at `(x0, y0)`, read directly
/// from the luma buffer (no per-patch crop/allocation).
fn patch_variance(raw: &[u8], row: usize, x0: usize, y0: usize, patch: usize) -> f32 {
    let n = (patch * patch) as f32;
    let mut sum = 0.0_f32;
    let mut sum_sq = 0.0_f32;
    for dy in 0..patch {
        let base = (y0 + dy) * row + x0;
        for dx in 0..patch {
            let v = raw[base + dx] as f32;
            sum += v;
            sum_sq += v * v;
        }
    }
    (sum_sq / n - (sum / n).powi(2)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    fn noisy(size: u32, sigma: u8, seed: u64) -> GrayImage {
        // Deterministic LCG-style pseudo-noise around 128.
        let mut img = GrayImage::new(size, size);
        let mut s = seed;
        for p in img.pixels_mut() {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let delta = ((s >> 32) as i32 % (2 * sigma as i32 + 1)) - sigma as i32;
            let v = (128 + delta).clamp(0, 255) as u8;
            *p = Luma([v]);
        }
        img
    }

    fn solid(size: u32, v: u8) -> GrayImage {
        let mut img = GrayImage::new(size, size);
        for p in img.pixels_mut() {
            *p = Luma([v]);
        }
        img
    }

    #[test]
    fn clean_image_scores_high_at_iso_100() {
        let s = score(&solid(128, 128), Some(100));
        assert!(s > 0.9, "clean ISO 100 should score high, got {}", s);
    }

    #[test]
    fn very_noisy_image_scores_low_at_iso_100() {
        let s = score(&noisy(128, 40, 42), Some(100));
        assert!(s < 0.3, "very noisy ISO 100 should score low, got {}", s);
    }

    #[test]
    fn high_iso_forgives_more_noise() {
        let img = noisy(128, 12, 42);
        let s_low = score(&img, Some(100));
        let s_high = score(&img, Some(6400));
        assert!(s_high > s_low, "high ISO should forgive: low={} high={}", s_low, s_high);
    }
}
