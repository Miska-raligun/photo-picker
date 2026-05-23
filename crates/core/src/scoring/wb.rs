use image::DynamicImage;

/// Plan B.2: gray-world assumption.
/// `wb = exp(-d / 0.15)` where `d = max(|R̄-Ḡ|, |B̄-Ḡ|) / Ḡ`.
/// Face-aware skin-tone fusion is deferred to M3 (depends on face detection).
pub fn score(img: &DynamicImage) -> f32 {
    let rgb = img.to_rgb8();
    let n = (rgb.width() as f64) * (rgb.height() as f64);
    if n < 1.0 {
        return 0.0;
    }

    let (mut sr, mut sg, mut sb) = (0.0f64, 0.0f64, 0.0f64);
    for p in rgb.pixels() {
        sr += p.0[0] as f64;
        sg += p.0[1] as f64;
        sb += p.0[2] as f64;
    }
    let (mr, mg, mb) = (sr / n, sg / n, sb / n);
    if mg < 1.0 {
        // Almost black image — no useful WB signal; neutral score.
        return 0.5;
    }
    let d = ((mr - mg).abs().max((mb - mg).abs())) / mg;
    (-d / 0.15).exp().clamp(0.0, 1.0) as f32
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
}
