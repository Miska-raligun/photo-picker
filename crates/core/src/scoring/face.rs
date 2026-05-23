//! Face detection + per-face quality (eye-open, smile, local sharpness).
//!
//! **Status (M3.5)**: stub returning no faces for every image. The interface
//! lets the rest of the pipeline branch on "portrait vs landscape" scene
//! detection — when a real detector is wired in, no other code changes.
//!
//! Candidate models (none integrated yet — choice deferred):
//! - **YuNet** (~500KB ONNX from opencv_zoo): bbox + 5 keypoints; would need
//!   manual NMS. Tiny + reliable.
//! - **SCRFD / RetinaFace** (insightface): bbox + 5 keypoints, higher quality.
//! - Eye-open / smile: separate small classifiers on the face crop, or compute
//!   eye aspect ratio from a richer landmark model (68-point or denser).

use image::DynamicImage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FaceBox {
    /// Normalized bbox in `[0, 1]`, relative to thumbnail dimensions.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// `Some` once a real detector lands; the stub leaves it `None`.
    pub eye_open_prob: Option<f32>,
    pub smile_prob: Option<f32>,
    /// Local sharpness on the face crop (Laplacian variance), `None` when the
    /// stub is in use.
    pub local_sharpness: Option<f32>,
}

impl FaceBox {
    pub fn area_ratio(&self) -> f32 {
        (self.w * self.h).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FaceInfo {
    pub faces: Vec<FaceBox>,
}

impl FaceInfo {
    pub fn count(&self) -> usize {
        self.faces.len()
    }

    /// Largest face's area ratio in `[0, 1]`. `0.0` when no faces are present.
    pub fn max_area_ratio(&self) -> f32 {
        self.faces
            .iter()
            .map(|f| f.area_ratio())
            .fold(0.0_f32, f32::max)
    }
}

pub trait FaceDetector: Send + Sync {
    fn detect(&self, thumb: &DynamicImage) -> FaceInfo;
}

/// Placeholder that detects no faces. Pipeline treats every photo as a
/// landscape scene until a real detector is wired in.
pub struct NoFaceDetectorStub;

impl FaceDetector for NoFaceDetectorStub {
    fn detect(&self, _thumb: &DynamicImage) -> FaceInfo {
        FaceInfo::default()
    }
}
