use image::DynamicImage;

/// Plan B.1:
///   exposure = 0.4·(H/8) + 0.3·(1-over_pen) + 0.2·(1-under_pen) + 0.1·p_mid
/// EV bias outside ±1.5 EV trims an extra 0.1 (high-key/low-key intent flagged).
pub fn score(img: &DynamicImage, exposure_bias_ev: Option<f32>) -> f32 {
    let gray = img.to_luma8();
    let total = (gray.width() as f32) * (gray.height() as f32);
    if total < 1.0 {
        return 0.0;
    }

    let mut hist = [0u32; 256];
    for p in gray.pixels() {
        hist[p.0[0] as usize] += 1;
    }
    let h: Vec<f32> = hist.iter().map(|c| *c as f32 / total).collect();

    let p_over: f32 = h[250..].iter().sum();
    let p_under: f32 = h[..=5].iter().sum();
    let p_mid: f32 = h[64..=192].iter().sum();

    let mut entropy = 0.0f32;
    for &p in &h {
        if p > 0.0 {
            entropy -= p * p.log2();
        }
    }
    let h_norm = (entropy / 8.0).clamp(0.0, 1.0);

    let over_pen = (p_over / 0.01).clamp(0.0, 1.0);
    let under_pen = (p_under / 0.05).clamp(0.0, 1.0);

    let mut s = 0.4 * h_norm
        + 0.3 * (1.0 - over_pen)
        + 0.2 * (1.0 - under_pen)
        + 0.1 * p_mid;

    if let Some(ev) = exposure_bias_ev {
        if ev.abs() > 1.5 {
            s -= 0.1;
        }
    }
    s.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgb, RgbImage};

    fn solid(color: u8) -> DynamicImage {
        let mut img = RgbImage::new(64, 64);
        for p in img.pixels_mut() {
            *p = Rgb([color, color, color]);
        }
        DynamicImage::ImageRgb8(img)
    }

    /// Ramp covering 5..250 (avoids clipping at either end, like a well-exposed scene).
    fn ramp() -> DynamicImage {
        let mut img = RgbImage::new(256, 64);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            let v = (5 + (x as u32 * 245 / 255)) as u8;
            *p = Rgb([v, v, v]);
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn clipped_white_outranked_by_ramp() {
        let s_clipped = score(&solid(255), None);
        let s_ramp = score(&ramp(), None);
        assert!(s_clipped < 0.3);
        assert!(s_ramp > s_clipped + 0.4, "ramp={} clipped={}", s_ramp, s_clipped);
    }

    #[test]
    fn pitch_black_outranked_by_ramp() {
        let s_black = score(&solid(0), None);
        let s_ramp = score(&ramp(), None);
        // Black has no entropy and is fully under-exposed — caps near 0.3.
        assert!(s_black <= 0.31);
        assert!(s_ramp > s_black + 0.4);
    }

    #[test]
    fn well_exposed_ramp_scores_high() {
        let s = score(&ramp(), None);
        assert!(s > 0.75, "well-exposed ramp should score high (got {})", s);
    }

    #[test]
    fn extreme_ev_bias_penalized() {
        let img = ramp();
        let s_neutral = score(&img, Some(0.0));
        let s_extreme = score(&img, Some(2.0));
        assert!(s_extreme < s_neutral);
    }
}
