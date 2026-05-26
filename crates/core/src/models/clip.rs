use super::cache::{ensure_model, ModelDescriptor};
use super::registry::{build_session, ExecutionProvider};
use crate::error::{Error, Result};
use image::DynamicImage;
use ndarray::Array4;
use ort::session::Session;
use ort::value::Tensor;

pub const CLIP_VIT_B32_VISION: ModelDescriptor = ModelDescriptor {
    name: "clip-vit-b32-vision-quantized",
    filename: "clip-vit-b32-vision-quantized.onnx",
    url: "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/onnx/vision_model_quantized.onnx",
    sha256_hex: "583fd1110a514667812fee7d684952aaf82a99b959760c8d7dca7e0ab9839299",
    size_bytes: 89_117_001,
};

pub const CLIP_INPUT_SIZE: u32 = 224;
pub const CLIP_EMBED_DIM: usize = 512;

/// L2-normalized 512-d image embedding from CLIP ViT-B/32.
///
/// Stage B compares photos with cosine similarity, which on L2-normalized
/// vectors reduces to a dot product. Pre-normalizing here keeps the hot path
/// branch-free.
pub struct ClipEncoder {
    session: Session,
}

impl ClipEncoder {
    pub fn load(ep: ExecutionProvider) -> Result<Self> {
        let path = ensure_model(&CLIP_VIT_B32_VISION)?;
        let session = build_session(&path, ep)?;
        Ok(Self { session })
    }

    pub fn embed(&mut self, img: &DynamicImage) -> Result<Vec<f32>> {
        let arr = preprocess(img);
        let input = Tensor::from_array(arr)
            .map_err(|e| Error::Config(format!("clip input tensor: {e}")))?;
        let outputs = self
            .session
            .run(ort::inputs!["pixel_values" => input])
            .map_err(|e| Error::Config(format!("clip inference: {e}")))?;
        let (_shape, data) = outputs["image_embeds"]
            .try_extract_tensor::<f32>()
            .map_err(|e| Error::Config(format!("clip output: {e}")))?;

        let mut emb: Vec<f32> = data.to_vec();
        l2_normalize(&mut emb);
        Ok(emb)
    }
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-8);
    for x in v.iter_mut() {
        *x /= norm;
    }
}

/// CLIP preprocessing: resize so the shorter edge is 224, center-crop to
/// 224×224, convert to CHW float32, normalize with the CLIP-pretrained mean/std.
///
/// Builds the [1, 3, H, W] tensor by writing each channel in one linear pass
/// over the source's HWC buffer — auto-vectorisable, avoids `s*s` per-pixel
/// `get_pixel` + 3D `arr[[..]]` index calls (which the borrow checker can't
/// hoist). On a 224×224 input this is the difference between ~200µs and a
/// handful of µs per photo.
fn preprocess(img: &DynamicImage) -> Array4<f32> {
    let resized = resize_center_crop(img, CLIP_INPUT_SIZE);
    let rgb = resized.to_rgb8();
    let mean = [0.48145466_f32, 0.4578275, 0.40821073];
    let std = [0.26862954_f32, 0.26130258, 0.27577711];
    let s = CLIP_INPUT_SIZE as usize;
    let pixels = s * s;
    let raw = rgb.as_raw(); // [R, G, B, R, G, B, ...]

    let mut data: Vec<f32> = Vec::with_capacity(3 * pixels);
    // Per-channel sequential write — CHW layout: all R, then all G, then all B.
    for c in 0..3 {
        let m = mean[c];
        let sd = std[c];
        for p in 0..pixels {
            let v = raw[p * 3 + c] as f32 * (1.0 / 255.0);
            data.push((v - m) / sd);
        }
    }
    // Safe: `data.len() == 3*pixels`, shape matches.
    Array4::from_shape_vec((1, 3, s, s), data).expect("shape matches data len")
}

fn resize_center_crop(img: &DynamicImage, size: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let short = w.min(h).max(1);
    let scale = size as f32 / short as f32;
    let new_w = ((w as f32 * scale).round() as u32).max(size);
    let new_h = ((h as f32 * scale).round() as u32).max(size);
    let resized = img.resize_exact(new_w, new_h, image::imageops::FilterType::Lanczos3);
    let crop_x = (new_w - size) / 2;
    let crop_y = (new_h - size) / 2;
    resized.crop_imm(crop_x, crop_y, size, size)
}
