use crate::error::{Error, Result};
use image::DynamicImage;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub struct ThumbnailSpec {
    /// Longest-edge target size in pixels.
    pub long_edge: u32,
}

impl Default for ThumbnailSpec {
    fn default() -> Self {
        Self { long_edge: 1024 }
    }
}

/// Decode a JPEG and downscale to a `long_edge`-pixel thumbnail.
///
/// Downscaling happens during decode by enforcing the limit; the result is the
/// largest image with longest edge <= `long_edge` that we can produce cheaply.
pub fn decode_thumbnail(path: &Path, spec: ThumbnailSpec) -> Result<DynamicImage> {
    let reader = image::ImageReader::open(path)
        .map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?
        .with_guessed_format()
        .map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;

    let img = reader
        .decode()
        .map_err(|e| Error::Decode { path: path.to_path_buf(), source: e })?;

    Ok(downscale(img, spec.long_edge))
}

fn downscale(img: DynamicImage, long_edge: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let longest = w.max(h);
    if longest <= long_edge {
        return img;
    }
    let scale = long_edge as f32 / longest as f32;
    let new_w = (w as f32 * scale).round().max(1.0) as u32;
    let new_h = (h as f32 * scale).round().max(1.0) as u32;
    img.resize_exact(new_w, new_h, image::imageops::FilterType::Triangle)
}
