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

    let buf = gray.as_raw();
    let stride = w as usize;

    let mut best = 0.0f32;
    for &(cx, cy) in &centers {
        let half = roi_size / 2;
        let x = cx
            .saturating_sub(half)
            .min(w.saturating_sub(roi_size).max(0));
        let y = cy
            .saturating_sub(half)
            .min(h.saturating_sub(roi_size).max(0));
        let lv = laplacian_variance_window(buf, stride, x as usize, y as usize, roi_size as usize);
        let tg = tenengrad_window(buf, stride, x as usize, y as usize, roi_size as usize);
        let s = 0.6 * lv + 0.4 * tg;
        if s > best {
            best = s;
        }
    }
    best
}

/// Window-based Laplacian variance — reads directly from the parent gray
/// buffer instead of allocating a `crop_imm(...).to_image()` per ROI.
/// `(x, y)` is the ROI's top-left in pixels, `size` is its edge length.
fn laplacian_variance_window(buf: &[u8], stride: usize, x: usize, y: usize, size: usize) -> f32 {
    if size < 3 {
        return 0.0;
    }
    let n = (size - 2) * (size - 2);
    if n == 0 {
        return 0.0;
    }
    let mut sum = 0.0f32;
    let mut sum_sq = 0.0f32;
    for ry in 1..size - 1 {
        let row = (y + ry) * stride + x;
        let row_up = (y + ry - 1) * stride + x;
        let row_dn = (y + ry + 1) * stride + x;
        for rx in 1..size - 1 {
            let c = buf[row + rx] as i32;
            let u = buf[row_up + rx] as i32;
            let d = buf[row_dn + rx] as i32;
            let l = buf[row + rx - 1] as i32;
            let r = buf[row + rx + 1] as i32;
            let v = (4 * c - u - d - l - r) as f32;
            sum += v;
            sum_sq += v * v;
        }
    }
    let nf = n as f32;
    let mean = sum / nf;
    (sum_sq / nf) - mean * mean
}

fn tenengrad_window(buf: &[u8], stride: usize, x: usize, y: usize, size: usize) -> f32 {
    if size < 3 {
        return 0.0;
    }
    let mut sum = 0.0f32;
    let mut count = 0u32;
    for ry in 1..size - 1 {
        let row = (y + ry) * stride + x;
        let row_up = (y + ry - 1) * stride + x;
        let row_dn = (y + ry + 1) * stride + x;
        for rx in 1..size - 1 {
            let ul = buf[row_up + rx - 1] as f32;
            let uu = buf[row_up + rx] as f32;
            let ur = buf[row_up + rx + 1] as f32;
            let ml = buf[row + rx - 1] as f32;
            let mr = buf[row + rx + 1] as f32;
            let dl = buf[row_dn + rx - 1] as f32;
            let dd = buf[row_dn + rx] as f32;
            let dr = buf[row_dn + rx + 1] as f32;
            let sx = -ul - 2.0 * ml - dl + ur + 2.0 * mr + dr;
            let sy = -ul - 2.0 * uu - ur + dl + 2.0 * dd + dr;
            sum += (sx * sx + sy * sy).sqrt();
            count += 1;
        }
    }
    if count == 0 { 0.0 } else { sum / count as f32 }
}

/// Variance of the 4-neighbor Laplacian over a grayscale region — a standard
/// focus/sharpness measure. Exposed crate-wide so the face detector can reuse it
/// on face crops (see `scoring::face_yunet`); only that ONNX-gated path uses it.
#[cfg(feature = "onnx")]
pub(crate) fn laplacian_variance(roi: &GrayImage) -> f32 {
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
