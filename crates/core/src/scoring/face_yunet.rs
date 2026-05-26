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
use crate::models::pool::{default_size as default_pool_size, SessionPool};
use crate::models::registry::build_session;
use crate::models::{ExecutionProvider, ModelDescriptor};
use image::DynamicImage;
use ndarray::Array4;
use ort::session::Session;
use ort::value::Tensor;

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
    sessions: SessionPool<Session>,
}

impl YunetFaceDetector {
    /// Load a single session (back-compat for tests / callers that don't
    /// care about throughput). Prefer `load_pool` for pipeline use.
    pub fn load(ep: ExecutionProvider) -> Result<Self> {
        Self::load_pool(ep, 1)
    }

    /// Load `n` independent sessions into a pool. Each adds ~1 MB of RAM
    /// (YuNet is tiny). Falls back to a single session if `n == 0`.
    pub fn load_pool(ep: ExecutionProvider, n: usize) -> Result<Self> {
        let path = ensure_model(&YUNET_FACE)?;
        let n = n.max(1);
        let mut sessions = Vec::with_capacity(n);
        for _ in 0..n {
            sessions.push(build_session(&path, ep)?);
        }
        Ok(Self {
            sessions: SessionPool::new(sessions),
        })
    }

    /// Convenience: read the pool size from the env var.
    pub fn load_pool_from_env(ep: ExecutionProvider) -> Result<Self> {
        Self::load_pool(ep, default_pool_size())
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
        // session guard — the SessionOutputs borrows from the session.
        let stride_data: Vec<(u32, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>)> =
            self.sessions.with(|session| -> Result<_> {
                let outputs = session
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
                    .collect::<Result<Vec<_>>>()
            })?;

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

        let mut faces: Vec<FaceBox> = kept
            .iter()
            .map(|f| project_to_source(f, &meta))
            .collect();

        // Derive per-face signals from the projected bbox + keypoints.
        if !faces.is_empty() {
            let gray = thumb.to_luma8();
            let (tw, th) = (gray.width(), gray.height());
            for (f, raw) in faces.iter_mut().zip(kept.iter()) {
                // (a) bbox-local sharpness (group-normalized downstream).
                let bx = ((f.x * tw as f32) as u32).min(tw.saturating_sub(1));
                let by = ((f.y * th as f32) as u32).min(th.saturating_sub(1));
                let bw = ((f.w * tw as f32) as u32).min(tw - bx);
                let bh = ((f.h * th as f32) as u32).min(th - by);
                if bw >= 8 && bh >= 8 {
                    let roi = image::imageops::crop_imm(&gray, bx, by, bw, bh).to_image();
                    f.local_sharpness =
                        Some(crate::scoring::sharpness::laplacian_variance(&roi));
                }

                // (b) Project keypoints from 640×640 input space to source.
                let project = |kx: f32, ky: f32| -> Option<(u32, u32)> {
                    let sx = (kx - meta.pad_x as f32) / meta.scale;
                    let sy = (ky - meta.pad_y as f32) / meta.scale;
                    if sx < 0.0 || sy < 0.0 {
                        return None;
                    }
                    let sxu = (sx as u32).min(tw.saturating_sub(1));
                    let syu = (sy as u32).min(th.saturating_sub(1));
                    Some((sxu, syu))
                };
                let (re, le, rm, lm) = (
                    project(raw.kps[0].0, raw.kps[0].1),
                    project(raw.kps[1].0, raw.kps[1].1),
                    project(raw.kps[3].0, raw.kps[3].1),
                    project(raw.kps[4].0, raw.kps[4].1),
                );

                // (c) Eye-open heuristic: Laplacian variance on small luma
                //     patches around each eye keypoint. Open eyes have far
                //     more high-frequency detail (lashes, iris, sclera
                //     boundary) than closed lids. tanh-normalize to [0,1]
                //     against an empirical baseline (~40 lap-var).
                let eye_patch = (bw / 8).clamp(8, 64);
                let crop_patch = |c: (u32, u32)| -> Option<image::GrayImage> {
                    let (cx, cy) = c;
                    let half = eye_patch / 2;
                    let x = cx.saturating_sub(half);
                    let y = cy.saturating_sub(half);
                    let w = eye_patch.min(tw - x);
                    let h = eye_patch.min(th - y);
                    if w < 4 || h < 4 {
                        return None;
                    }
                    Some(image::imageops::crop_imm(&gray, x, y, w, h).to_image())
                };
                if let (Some(re), Some(le)) = (re, le) {
                    if let (Some(re_img), Some(le_img)) = (crop_patch(re), crop_patch(le)) {
                        let rv = crate::scoring::sharpness::laplacian_variance(&re_img);
                        let lv = crate::scoring::sharpness::laplacian_variance(&le_img);
                        // Both eyes need to be open; min is more conservative
                        // than mean (a half-closed eye drops the score).
                        let avg = rv.min(lv);
                        f.eye_open_prob = Some((avg / 40.0).tanh().clamp(0.0, 1.0));
                    }
                }

                // (d) Smile heuristic: mouth-corner spread normalized by
                //     interocular distance. Neutral ≈ 0.6–0.7, smile ≈ 0.85+.
                if let (Some((rex, rey)), Some((lex, ley)), Some((rmx, rmy)), Some((lmx, lmy))) =
                    (re, le, rm, lm)
                {
                    let inter = (((lex as f32 - rex as f32).powi(2)
                        + (ley as f32 - rey as f32).powi(2))
                        as f32)
                        .sqrt();
                    if inter > 8.0 {
                        let mouth = (((lmx as f32 - rmx as f32).powi(2)
                            + (lmy as f32 - rmy as f32).powi(2))
                            as f32)
                            .sqrt();
                        let ratio = mouth / inter;
                        // Centered sigmoid: ratio 0.6 → ~0.12, 0.8 → 0.5, 1.0 → ~0.88.
                        let smile = 1.0 / (1.0 + (-(ratio - 0.8) * 10.0).exp());
                        f.smile_prob = Some(smile.clamp(0.0, 1.0));
                    }
                }
            }
        }
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
    /// 5 keypoints in 640×640 input pixel space.
    /// Order (per YuNet/InsightFace convention):
    ///   0: right eye, 1: left eye, 2: nose,
    ///   3: right mouth corner, 4: left mouth corner.
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

    // Build the BGR letterboxed [1, 3, H, W] tensor in one pass. The active
    // region is a `new_w × new_h` crop padded by `pad_x`/`pad_y`; outside that
    // area the buffer stays zeroed (Array4::zeros). For 640×640 inputs this
    // avoids ~410k per-pixel `get_pixel` + 3D `arr[[..]]` index calls —
    // measurable on CPU.
    let rgb = resized.to_rgb8();
    let raw = rgb.as_raw(); // [R,G,B, R,G,B, ...]
    let isize_v = INPUT_SIZE as usize;
    let plane = isize_v * isize_v;
    let mut data = vec![0f32; 3 * plane];
    let new_w_us = new_w as usize;
    let new_h_us = new_h as usize;
    let row_stride_src = new_w_us * 3;
    let pad_x_us = pad_x as usize;
    let pad_y_us = pad_y as usize;
    // YuNet (per OpenCV's wrapper) consumes BGR pixel values 0-255 as f32.
    // CHW layout → plane 0 = B, plane 1 = G, plane 2 = R.
    for y in 0..new_h_us {
        let src = &raw[y * row_stride_src..(y + 1) * row_stride_src];
        let row_off = (pad_y_us + y) * isize_v + pad_x_us;
        let b_row = &mut data[row_off..row_off + new_w_us];
        for (i, b) in b_row.iter_mut().enumerate() {
            *b = src[i * 3 + 2] as f32;
        }
        let g_row = &mut data[plane + row_off..plane + row_off + new_w_us];
        for (i, g) in g_row.iter_mut().enumerate() {
            *g = src[i * 3 + 1] as f32;
        }
        let r_row = &mut data[2 * plane + row_off..2 * plane + row_off + new_w_us];
        for (i, r) in r_row.iter_mut().enumerate() {
            *r = src[i * 3] as f32;
        }
    }
    let arr = Array4::from_shape_vec((1, 3, isize_v, isize_v), data)
        .expect("shape matches data len");
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
