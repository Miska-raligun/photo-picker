//! YuNet face detector backend (M3.5 real implementation).
//!
//! YuNet is a small (~230KB) anchor-free face detector from OpenCV's model zoo
//! (`opencv_zoo/face_detection_yunet_2023mar.onnx`, Apache 2.0). It outputs
//! per-stride classification, objectness, bbox, and 5-keypoint tensors at
//! strides 8/16/32 over a 640×640 input.
//!
//! Eye-open / smile / local sharpness aren't computed yet — YuNet only gives
//! bbox + 5 keypoints (eyes, nose, mouth corners). A future iteration can add:
//! - eye-aspect-ratio from a richer landmark model, or
//! - a dedicated eye-state classifier on the eye crop.

use super::face::{FaceBox, FaceDetector, FaceInfo};
use crate::error::{Error, Result};
use crate::models::cache::ensure_model;
use crate::models::registry::build_session;
use crate::models::{ExecutionProvider, ModelDescriptor};
use image::DynamicImage;
use ndarray::Array4;
use ort::session::Session;
use ort::value::Tensor;
use std::sync::Mutex;

pub const YUNET_FACE: ModelDescriptor = ModelDescriptor {
    name: "yunet-face-2023mar",
    filename: "face_detection_yunet_2023mar.onnx",
    url: "https://github.com/opencv/opencv_zoo/raw/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx",
    sha256_hex: "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
    size_bytes: 232_589,
};

const INPUT_SIZE: u32 = 640;
const STRIDES: [u32; 3] = [8, 16, 32];
const SCORE_THRESHOLD: f32 = 0.6;
const NMS_IOU_THRESHOLD: f32 = 0.3;

pub struct YunetFaceDetector {
    session: Mutex<Session>,
}

impl YunetFaceDetector {
    pub fn load(ep: ExecutionProvider) -> Result<Self> {
        let path = ensure_model(&YUNET_FACE)?;
        let session = build_session(&path, ep)?;
        Ok(Self {
            session: Mutex::new(session),
        })
    }
}

impl FaceDetector for YunetFaceDetector {
    fn detect(&self, thumb: &DynamicImage) -> FaceInfo {
        match self.detect_inner(thumb) {
            Ok(faces) => faces,
            Err(err) => {
                tracing::warn!(%err, "yunet face detection failed");
                FaceInfo::default()
            }
        }
    }
}

impl YunetFaceDetector {
    fn detect_inner(&self, thumb: &DynamicImage) -> Result<FaceInfo> {
        let (tensor, meta) = preprocess(thumb);
        let input = Tensor::from_array(tensor)
            .map_err(|e| Error::Config(format!("yunet input: {e}")))?;

        // Run inference and copy the output tensors out before releasing the
        // session guard — the SessionOutputs borrows from the guard.
        let stride_data: Vec<(u32, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>)> = {
            let mut guard = self
                .session
                .lock()
                .map_err(|_| Error::Config("yunet mutex poisoned".into()))?;
            let outputs = guard
                .run(ort::inputs!["input" => input])
                .map_err(|e| Error::Config(format!("yunet inference: {e}")))?;
            STRIDES
                .iter()
                .map(|&s| -> Result<_> {
                    Ok((
                        s,
                        read_f32_output(&outputs, &format!("cls_{s}"))?,
                        read_f32_output(&outputs, &format!("obj_{s}"))?,
                        read_f32_output(&outputs, &format!("bbox_{s}"))?,
                        read_f32_output(&outputs, &format!("kps_{s}"))?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?
        };

        let mut candidates: Vec<RawFace> = Vec::new();
        for (stride, cls, obj, bbox, kps) in &stride_data {
            decode_stride(*stride, cls, obj, bbox, kps, &mut candidates);
        }

        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let kept = nms(&candidates, NMS_IOU_THRESHOLD);

        let faces = kept
            .into_iter()
            .map(|f| project_to_source(&f, &meta))
            .collect();
        Ok(FaceInfo { faces })
    }
}

fn read_f32_output(
    outputs: &ort::session::SessionOutputs,
    name: &str,
) -> Result<Vec<f32>> {
    let (_, data) = outputs[name]
        .try_extract_tensor::<f32>()
        .map_err(|e| Error::Config(format!("yunet output {name}: {e}")))?;
    Ok(data.to_vec())
}

#[derive(Debug, Clone)]
struct RawFace {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    score: f32,
    #[allow(dead_code)]
    kps: [(f32, f32); 5],
}

struct LetterboxMeta {
    src_w: u32,
    src_h: u32,
    /// Uniform scale src → input.
    scale: f32,
    /// Padding added in input pixel space.
    pad_x: u32,
    pad_y: u32,
}

/// Letterbox-resize to 640×640, convert to BGR float (raw 0-255), CHW layout.
fn preprocess(img: &DynamicImage) -> (Array4<f32>, LetterboxMeta) {
    let (src_w, src_h) = (img.width(), img.height());
    let scale = INPUT_SIZE as f32 / src_w.max(src_h) as f32;
    let new_w = ((src_w as f32 * scale).round() as u32).max(1);
    let new_h = ((src_h as f32 * scale).round() as u32).max(1);
    let resized = img.resize_exact(new_w, new_h, image::imageops::FilterType::Triangle);
    let pad_x = (INPUT_SIZE - new_w) / 2;
    let pad_y = (INPUT_SIZE - new_h) / 2;

    let rgb = resized.to_rgb8();
    let isize = INPUT_SIZE as usize;
    let mut arr = Array4::<f32>::zeros((1, 3, isize, isize));
    for y in 0..new_h {
        for x in 0..new_w {
            let p = rgb.get_pixel(x, y);
            let tx = (x + pad_x) as usize;
            let ty = (y + pad_y) as usize;
            // YuNet (per OpenCV's wrapper) consumes BGR pixel values 0-255 as
            // float without further normalization.
            arr[[0, 0, ty, tx]] = p.0[2] as f32;
            arr[[0, 1, ty, tx]] = p.0[1] as f32;
            arr[[0, 2, ty, tx]] = p.0[0] as f32;
        }
    }
    (
        arr,
        LetterboxMeta { src_w, src_h, scale, pad_x, pad_y },
    )
}

fn decode_stride(
    stride: u32,
    cls: &[f32],
    obj: &[f32],
    bbox: &[f32],
    kps: &[f32],
    out: &mut Vec<RawFace>,
) {
    let fm = (INPUT_SIZE / stride) as usize;
    let s = stride as f32;
    for y in 0..fm {
        for x in 0..fm {
            let idx = y * fm + x;
            // OpenCV's reference impl does score = cls * obj (no sigmoid; the
            // model export already includes sigmoid activations).
            let score = cls[idx] * obj[idx];
            if score < SCORE_THRESHOLD {
                continue;
            }

            let cx = (x as f32 + 0.5) * s;
            let cy = (y as f32 + 0.5) * s;

            let l = bbox[idx * 4] * s;
            let t = bbox[idx * 4 + 1] * s;
            let r = bbox[idx * 4 + 2] * s;
            let b = bbox[idx * 4 + 3] * s;

            let mut kp_arr = [(0.0_f32, 0.0_f32); 5];
            for i in 0..5 {
                let dx = kps[idx * 10 + i * 2] * s;
                let dy = kps[idx * 10 + i * 2 + 1] * s;
                kp_arr[i] = (cx + dx, cy + dy);
            }

            out.push(RawFace {
                x1: cx - l,
                y1: cy - t,
                x2: cx + r,
                y2: cy + b,
                score,
                kps: kp_arr,
            });
        }
    }
}

fn nms(sorted: &[RawFace], iou_threshold: f32) -> Vec<RawFace> {
    let mut keep: Vec<RawFace> = Vec::new();
    'outer: for cand in sorted {
        for k in &keep {
            if iou(cand, k) > iou_threshold {
                continue 'outer;
            }
        }
        keep.push(cand.clone());
    }
    keep
}

fn iou(a: &RawFace, b: &RawFace) -> f32 {
    let ix1 = a.x1.max(b.x1);
    let iy1 = a.y1.max(b.y1);
    let ix2 = a.x2.min(b.x2);
    let iy2 = a.y2.min(b.y2);
    let iw = (ix2 - ix1).max(0.0);
    let ih = (iy2 - iy1).max(0.0);
    let inter = iw * ih;
    let aarea = (a.x2 - a.x1).max(0.0) * (a.y2 - a.y1).max(0.0);
    let barea = (b.x2 - b.x1).max(0.0) * (b.y2 - b.y1).max(0.0);
    let union = aarea + barea - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Map a 640×640 input-pixel bbox back to a normalized [0,1] bbox over the
/// source thumbnail.
fn project_to_source(face: &RawFace, m: &LetterboxMeta) -> FaceBox {
    let undo = |v: f32, pad: u32| (v - pad as f32) / m.scale;
    let sx1 = undo(face.x1, m.pad_x).clamp(0.0, m.src_w as f32);
    let sy1 = undo(face.y1, m.pad_y).clamp(0.0, m.src_h as f32);
    let sx2 = undo(face.x2, m.pad_x).clamp(0.0, m.src_w as f32);
    let sy2 = undo(face.y2, m.pad_y).clamp(0.0, m.src_h as f32);
    let nw = m.src_w as f32;
    let nh = m.src_h as f32;
    FaceBox {
        x: (sx1 / nw).clamp(0.0, 1.0),
        y: (sy1 / nh).clamp(0.0, 1.0),
        w: ((sx2 - sx1) / nw).clamp(0.0, 1.0),
        h: ((sy2 - sy1) / nh).clamp(0.0, 1.0),
        eye_open_prob: None,
        smile_prob: None,
        local_sharpness: None,
    }
}
