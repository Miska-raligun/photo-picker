use image::GrayImage;

/// Plan B.3: raw multi-ROI sharpness signal.
/// `sharpness_raw = max over ROIs of (0.6·laplacian_var + 0.4·tenengrad)`.
///
/// M2 uses a 3×3 grid of patches (no face/AF data yet). The pipeline normalizes
/// across photos *within a group* via z-score → sigmoid in [`finalize_group`].
///
/// [`finalize_group`]: super::finalize_group
pub fn raw(gray: &GrayImage) -> f32 {
    let (w, h) = (gray.width(), gray.height());
    let roi_size = (w.min(h) / 3).max(64);

    let centers: [(u32, u32); 9] = [
        (w / 6, h / 6),
        (w / 2, h / 6),
        (5 * w / 6, h / 6),
        (w / 6, h / 2),
        (w / 2, h / 2),
        (5 * w / 6, h / 2),
        (w / 6, 5 * h / 6),
        (w / 2, 5 * h / 6),
        (5 * w / 6, 5 * h / 6),
    ];

    let mut best = 0.0f32;
    for &(cx, cy) in &centers {
        let half = roi_size / 2;
        let x = cx
            .saturating_sub(half)
            .min(w.saturating_sub(roi_size).max(0));
        let y = cy
            .saturating_sub(half)
            .min(h.saturating_sub(roi_size).max(0));
        let roi = image::imageops::crop_imm(gray, x, y, roi_size, roi_size).to_image();
        let lv = laplacian_variance(&roi);
        let tg = tenengrad(&roi);
        let s = 0.6 * lv + 0.4 * tg;
        if s > best {
            best = s;
        }
    }
    best
}

fn laplacian_variance(roi: &GrayImage) -> f32 {
    let (w, h) = (roi.width() as i32, roi.height() as i32);
    if w < 3 || h < 3 {
        return 0.0;
    }
    let cap = ((w - 2) * (h - 2)) as usize;
    let mut vals: Vec<f32> = Vec::with_capacity(cap);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let c = roi.get_pixel(x as u32, y as u32).0[0] as i32;
            let u = roi.get_pixel(x as u32, (y - 1) as u32).0[0] as i32;
            let d = roi.get_pixel(x as u32, (y + 1) as u32).0[0] as i32;
            let l = roi.get_pixel((x - 1) as u32, y as u32).0[0] as i32;
            let r = roi.get_pixel((x + 1) as u32, y as u32).0[0] as i32;
            vals.push((4 * c - u - d - l - r) as f32);
        }
    }
    let n = vals.len() as f32;
    let mean = vals.iter().sum::<f32>() / n;
    vals.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n
}

fn tenengrad(roi: &GrayImage) -> f32 {
    let (w, h) = (roi.width() as i32, roi.height() as i32);
    if w < 3 || h < 3 {
        return 0.0;
    }
    let mut sum = 0.0f32;
    let mut count = 0u32;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let p = |dx: i32, dy: i32| {
                roi.get_pixel((x + dx) as u32, (y + dy) as u32).0[0] as f32
            };
            let sx = -p(-1, -1) - 2.0 * p(-1, 0) - p(-1, 1)
                + p(1, -1)
                + 2.0 * p(1, 0)
                + p(1, 1);
            let sy = -p(-1, -1) - 2.0 * p(0, -1) - p(1, -1)
                + p(-1, 1)
                + 2.0 * p(0, 1)
                + p(1, 1);
            sum += (sx * sx + sy * sy).sqrt();
            count += 1;
        }
    }
    if count == 0 { 0.0 } else { sum / count as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    fn checkerboard(size: u32, cell: u32) -> GrayImage {
        let mut img = GrayImage::new(size, size);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let v = if ((x / cell) + (y / cell)) % 2 == 0 { 0 } else { 255 };
            *p = Luma([v]);
        }
        img
    }

    fn solid_gray(size: u32, v: u8) -> GrayImage {
        let mut img = GrayImage::new(size, size);
        for p in img.pixels_mut() {
            *p = Luma([v]);
        }
        img
    }

    #[test]
    fn flat_image_has_zero_sharpness() {
        assert_eq!(raw(&solid_gray(128, 128)), 0.0);
    }

    #[test]
    fn checkerboard_outranks_flat() {
        let sharp = raw(&checkerboard(256, 4));
        let flat = raw(&solid_gray(256, 128));
        assert!(sharp > flat * 100.0, "sharp={} flat={}", sharp, flat);
    }

    #[test]
    fn fine_pattern_outranks_coarse() {
        let fine = raw(&checkerboard(256, 2));
        let coarse = raw(&checkerboard(256, 32));
        assert!(fine > coarse, "fine={} coarse={}", fine, coarse);
    }
}
