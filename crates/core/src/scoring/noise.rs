use image::{DynamicImage, GrayImage};

/// Plan B.4: estimate noise from the flattest 5 % of 8×8 patches, then compare
/// against ISO-derived expectation.
///
/// `expected_σ = 1.0 · √(ISO/100)` (luma units, calibrated empirically — a
/// modern FF sensor at ISO 100 should sit near σ≈1, ISO 6400 near σ≈8).
/// `noise = clamp(expected / max(actual, ε), 0, 1)`.
pub fn score(img: &DynamicImage, iso: Option<u32>) -> f32 {
    let gray = img.to_luma8();
    let actual = estimate_noise_sigma(&gray);
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

    let mut variances: Vec<f32> = Vec::with_capacity(((w / patch) * (h / patch)) as usize);
    let mut y = 0;
    while y + patch <= h {
        let mut x = 0;
        while x + patch <= w {
            let p = image::imageops::crop_imm(gray, x, y, patch, patch).to_image();
            variances.push(patch_variance(&p));
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

fn patch_variance(patch: &GrayImage) -> f32 {
    let pixels: Vec<f32> = patch.pixels().map(|p| p.0[0] as f32).collect();
    if pixels.is_empty() {
        return 0.0;
    }
    let n = pixels.len() as f32;
    let mean = pixels.iter().sum::<f32>() / n;
    pixels.iter().map(|p| (p - mean).powi(2)).sum::<f32>() / n
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, GrayImage, Luma};

    fn noisy(size: u32, sigma: u8, seed: u64) -> DynamicImage {
        // Deterministic LCG-style pseudo-noise around 128.
        let mut img = GrayImage::new(size, size);
        let mut s = seed;
        for p in img.pixels_mut() {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let delta = ((s >> 32) as i32 % (2 * sigma as i32 + 1)) - sigma as i32;
            let v = (128 + delta).clamp(0, 255) as u8;
            *p = Luma([v]);
        }
        DynamicImage::ImageLuma8(img)
    }

    fn solid(size: u32, v: u8) -> DynamicImage {
        let mut img = GrayImage::new(size, size);
        for p in img.pixels_mut() {
            *p = Luma([v]);
        }
        DynamicImage::ImageLuma8(img)
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
