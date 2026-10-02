//! Geometry for the learned eye-open classifier (OCEC).
//!
//! The model itself runs in `face_yunet` (behind the `onnx` feature); this
//! module holds the pure parts — where to crop each eye from YuNet's
//! keypoints, how to resample it to the classifier's input, and how to fold
//! two per-eye probabilities into one per-face value — so they can be unit
//! tested without linking onnxruntime.
//!
//! OCEC (<https://github.com/PINTO0309/OCEC>, MIT) was trained on tight eye
//! crops (median ~22×11 px open, ~14×7 px closed) resized to 24×40, RGB, /255.

use image::RgbImage;

/// Classifier input height × width.
pub const EYE_INPUT_H: usize = 24;
pub const EYE_INPUT_W: usize = 40;

/// Crop width as a multiple of the interocular distance. A human eye opening
/// is roughly half the pupil-to-pupil distance; 0.55 frames the eye plus a
/// thin margin, matching OCEC's tight training crops. Real open eyes scored
/// ≈1.0 for anything from 0.45 to 0.8, so this isn't a sensitive knob.
pub const EYE_CROP_WIDTH_RATIO: f32 = 0.55;

/// Below this interocular distance (thumbnail pixels) an eye is only a few
/// pixels tall — too little signal to call open vs closed. Those faces keep
/// the fallback heuristic.
pub const MIN_INTEROCULAR_PX: f32 = 16.0;

/// When the nose keypoint sits more than this fraction of the interocular
/// distance off the eyes' midpoint (along the eye line), the head is turned
/// far enough that the far eye is foreshortened or hidden behind the nose.
const PROFILE_OFFSET_RATIO: f32 = 0.35;

/// Eye-line geometry for one face, in thumbnail pixel coordinates.
#[derive(Debug, Clone, Copy)]
pub struct EyeGeometry {
    pub right_eye: (f32, f32),
    pub left_eye: (f32, f32),
    /// Pupil-to-pupil distance in pixels.
    pub interocular: f32,
    /// Roll of the eye line, radians (0 = level).
    pub angle: f32,
    /// Head turned far enough that only the nearer eye is trustworthy.
    pub profile: bool,
}

impl EyeGeometry {
    /// Build from YuNet's right-eye, left-eye and nose keypoints. Returns
    /// `None` when the eyes are too close together to classify reliably.
    pub fn from_keypoints(
        right_eye: (f32, f32),
        left_eye: (f32, f32),
        nose: (f32, f32),
    ) -> Option<Self> {
        let (dx, dy) = (left_eye.0 - right_eye.0, left_eye.1 - right_eye.1);
        let interocular = (dx * dx + dy * dy).sqrt();
        if !interocular.is_finite() || interocular < MIN_INTEROCULAR_PX {
            return None;
        }
        let mid = ((right_eye.0 + left_eye.0) / 2.0, (right_eye.1 + left_eye.1) / 2.0);
        // Signed offset of the nose from the midpoint, projected on the eye line.
        let along = ((nose.0 - mid.0) * dx + (nose.1 - mid.1) * dy) / (interocular * interocular);
        Some(Self {
            right_eye,
            left_eye,
            interocular,
            angle: dy.atan2(dx),
            profile: along.abs() > PROFILE_OFFSET_RATIO,
        })
    }
}

/// Fold per-eye open probabilities into one value for the face.
///
/// Frontal faces take the minimum — a photo where either eye is mid-blink is
/// the one to avoid. Turned heads take the maximum, because the far eye is
/// foreshortened or occluded and would otherwise read as "closed".
pub fn combine_eyes(right: f32, left: f32, profile: bool) -> f32 {
    let v = if profile { right.max(left) } else { right.min(left) };
    v.clamp(0.0, 1.0)
}

/// Resample an eye-aligned `EYE_INPUT_W × EYE_INPUT_H` patch centred on
/// `center` and append it to `out` as CHW planes of RGB in `[0, 1]`.
///
/// The patch is rotated by `angle` so a tilted head still yields a level eye,
/// and is `width` pixels wide in the source with the classifier's 24:40
/// aspect. Bilinear sampling with edge clamping, so eyes near the frame
/// border degrade gracefully instead of failing.
pub fn sample_eye_patch(
    rgb: &RgbImage,
    center: (f32, f32),
    width: f32,
    angle: f32,
    out: &mut Vec<f32>,
) {
    let height = width * EYE_INPUT_H as f32 / EYE_INPUT_W as f32;
    let (sin, cos) = angle.sin_cos();
    let plane = EYE_INPUT_H * EYE_INPUT_W;
    let base = out.len();
    out.resize(base + 3 * plane, 0.0);
    for j in 0..EYE_INPUT_H {
        let v = ((j as f32 + 0.5) / EYE_INPUT_H as f32 - 0.5) * height;
        for i in 0..EYE_INPUT_W {
            let u = ((i as f32 + 0.5) / EYE_INPUT_W as f32 - 0.5) * width;
            let x = center.0 + u * cos - v * sin;
            let y = center.1 + u * sin + v * cos;
            let px = bilinear(rgb, x, y);
            let idx = j * EYE_INPUT_W + i;
            for (c, value) in px.iter().enumerate() {
                out[base + c * plane + idx] = value / 255.0;
            }
        }
    }
}

/// Bilinear RGB sample at continuous coordinates (pixel `i` spans `[i, i+1)`).
fn bilinear(img: &RgbImage, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (img.width() as i64, img.height() as i64);
    if w == 0 || h == 0 {
        return [0.0; 3];
    }
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let (tx, ty) = (fx - x0, fy - y0);
    let clamp = |v: i64, max: i64| v.clamp(0, max - 1) as u32;
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (xa, xb) = (clamp(x0, w), clamp(x0 + 1, w));
    let (ya, yb) = (clamp(y0, h), clamp(y0 + 1, h));
    let p = |xx: u32, yy: u32| img.get_pixel(xx, yy).0;
    let (a, b, c, d) = (p(xa, ya), p(xb, ya), p(xa, yb), p(xb, yb));
    let mut out = [0.0; 3];
    for k in 0..3 {
        let top = a[k] as f32 * (1.0 - tx) + b[k] as f32 * tx;
        let bot = c[k] as f32 * (1.0 - tx) + d[k] as f32 * tx;
        out[k] = top * (1.0 - ty) + bot * ty;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// Left half black, right half white.
    fn split_image() -> RgbImage {
        RgbImage::from_fn(100, 60, |x, _| if x < 50 { Rgb([0, 0, 0]) } else { Rgb([255, 255, 255]) })
    }

    fn sample(img: &RgbImage, angle: f32) -> Vec<f32> {
        let mut out = Vec::new();
        sample_eye_patch(img, (50.0, 30.0), 40.0, angle, &mut out);
        out
    }

    #[test]
    fn patch_has_chw_shape_and_unit_range() {
        let out = sample(&split_image(), 0.0);
        assert_eq!(out.len(), 3 * EYE_INPUT_H * EYE_INPUT_W);
        assert!(out.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn patch_preserves_orientation_and_follows_roll() {
        let img = split_image();
        let row = EYE_INPUT_H / 2 * EYE_INPUT_W;
        let level = sample(&img, 0.0);
        assert!(level[row] < 0.05, "left edge should be black");
        assert!(level[row + EYE_INPUT_W - 1] > 0.95, "right edge should be white");
        // Rotating the sampling frame by 180° mirrors the patch.
        let flipped = sample(&img, std::f32::consts::PI);
        assert!(flipped[row] > 0.95 && flipped[row + EYE_INPUT_W - 1] < 0.05);
    }

    #[test]
    fn patch_clamps_at_image_border() {
        let img = RgbImage::from_pixel(10, 10, Rgb([200, 100, 50]));
        let mut out = Vec::new();
        sample_eye_patch(&img, (0.0, 0.0), 30.0, 0.3, &mut out);
        let plane = EYE_INPUT_H * EYE_INPUT_W;
        assert!(out[..plane].iter().all(|v| (v - 200.0 / 255.0).abs() < 1e-5));
        assert!(out[2 * plane..].iter().all(|v| (v - 50.0 / 255.0).abs() < 1e-5));
    }

    #[test]
    fn geometry_rejects_tiny_faces_and_flags_profiles() {
        assert!(EyeGeometry::from_keypoints((10.0, 10.0), (20.0, 10.0), (15.0, 15.0)).is_none());

        let frontal =
            EyeGeometry::from_keypoints((100.0, 100.0), (160.0, 100.0), (130.0, 130.0)).unwrap();
        assert!(!frontal.profile);
        assert!((frontal.interocular - 60.0).abs() < 1e-4);
        assert!(frontal.angle.abs() < 1e-6);

        // Nose shifted well toward one eye → head turned.
        let turned =
            EyeGeometry::from_keypoints((100.0, 100.0), (160.0, 100.0), (156.0, 130.0)).unwrap();
        assert!(turned.profile);
    }

    #[test]
    fn combine_uses_min_frontal_and_max_profile() {
        assert_eq!(combine_eyes(0.9, 0.1, false), 0.1);
        assert_eq!(combine_eyes(0.9, 0.1, true), 0.9);
    }
}
