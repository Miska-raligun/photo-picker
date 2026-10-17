use image::GrayImage;

/// Exposure quality in `[0, 1]`:
///   exposure = 0.35·(H/8) + 0.20·(1-over_pen) + 0.15·(1-under_pen)
///            + 0.10·p_mid + 0.20·key
/// where `H` is luma entropy, `p_mid` the mid-tone share and `key` how close
/// mean brightness sits to a normal exposure. EV bias outside ±1.5 EV trims
/// an extra 0.1 (high-key/low-key intent flagged).
///
/// Clipping penalties are graded, not a cliff. The old rule maxed the
/// highlight penalty at 1 % clipped pixels — which almost every frame with
/// sky, a white shirt or a specular highlight exceeds — so darkening such a
/// frame by a stop *raised* its score and underexposed burst frames won. Now
/// the first 2 % (highlights) / 5 % (shadows) are free and the penalty ramps
/// to full over the next 10 % / 20 %, and the `key` term stops a dark frame
/// from beating a correctly exposed one.
pub fn score(gray: &GrayImage, exposure_bias_ev: Option<f32>) -> f32 {
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

    let over_pen = ((p_over - 0.02) / 0.10).clamp(0.0, 1.0);
    let under_pen = ((p_under - 0.05) / 0.20).clamp(0.0, 1.0);

    // Mean brightness in [0,1]; Gaussian preference around a normal exposure.
    let mean = h.iter().enumerate().map(|(i, p)| i as f32 * p).sum::<f32>() / 255.0;
    let key = (-((mean - 0.45) / 0.25).powi(2)).exp();

    let mut s = 0.35 * h_norm
        + 0.20 * (1.0 - over_pen)
        + 0.15 * (1.0 - under_pen)
        + 0.10 * p_mid
        + 0.20 * key;

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
    use image::{GrayImage, Luma};

    fn solid(color: u8) -> GrayImage {
        let mut img = GrayImage::new(64, 64);
        for p in img.pixels_mut() {
            *p = Luma([color]);
        }
        img
    }

    /// Ramp covering 5..250 (avoids clipping at either end, like a well-exposed scene).
    fn ramp() -> GrayImage {
        let mut img = GrayImage::new(256, 64);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            let v = (5 + (x as u32 * 245 / 255)) as u8;
            *p = Luma([v]);
        }
        img
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

    /// Regression: a frame with a few percent of blown highlights (sky, white
    /// clothing) must not lose to the same frame darkened by a stop.
    #[test]
    fn small_highlight_clipping_does_not_reward_underexposure() {
        let mut img = ramp();
        // Clip ~4 % of pixels to pure white.
        for (x, _y, p) in img.enumerate_pixels_mut() {
            if x >= 246 {
                *p = Luma([255]);
            }
        }
        let dark = GrayImage::from_fn(img.width(), img.height(), |x, y| {
            Luma([(img.get_pixel(x, y).0[0] as f32 * 0.5) as u8])
        });
        let (s_img, s_dark) = (score(&img, None), score(&dark, None));
        assert!(s_img > s_dark, "normal={s_img} dark={s_dark}");
    }

    #[test]
    fn extreme_ev_bias_penalized() {
        let img = ramp();
        let s_neutral = score(&img, Some(0.0));
        let s_extreme = score(&img, Some(2.0));
        assert!(s_extreme < s_neutral);
    }
}
