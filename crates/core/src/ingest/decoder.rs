use super::{ImageFormat, PhotoRef};
use crate::error::{Error, Result};
use image::DynamicImage;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
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

/// Decode any supported file to a downscaled thumbnail.
///
/// For RAW formats stored in TIFF containers (CR2/NEF/ARW/DNG/PEF/ORF) we
/// extract the embedded full-resolution JPEG preview — much faster than
/// demosaicing, and sufficient for culling decisions. CR3/RAF use proprietary
/// containers and aren't supported in M2.
pub fn decode_thumbnail(path: &Path, spec: ThumbnailSpec) -> Result<DynamicImage> {
    decode_thumbnail_with_format(path, classify_or_jpeg(path), spec)
}

/// Decode a known format. Use when the caller already has a `PhotoRef`.
pub fn decode_thumbnail_for(photo: &PhotoRef, spec: ThumbnailSpec) -> Result<DynamicImage> {
    decode_thumbnail_with_format(&photo.path, photo.format, spec)
}

fn decode_thumbnail_with_format(
    path: &Path,
    format: ImageFormat,
    spec: ThumbnailSpec,
) -> Result<DynamicImage> {
    let img = match format {
        ImageFormat::Jpeg => decode_image_file(path)?,
        ImageFormat::Raw(kind) => {
            if !kind.is_tiff_container() {
                return Err(Error::Config(format!(
                    "RAW format {:?} not supported in M2 (CR3/RAF need a dedicated parser)",
                    kind
                )));
            }
            let jpeg_bytes = extract_tiff_preview_jpeg(path)?;
            image::load_from_memory_with_format(&jpeg_bytes, image::ImageFormat::Jpeg)
                .map_err(|e| Error::Decode { path: path.to_path_buf(), source: e })?
        }
    };
    Ok(downscale(img, spec.long_edge))
}

fn classify_or_jpeg(path: &Path) -> ImageFormat {
    use super::RawKind;
    match path.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("cr2") => ImageFormat::Raw(RawKind::Cr2),
        Some("cr3") => ImageFormat::Raw(RawKind::Cr3),
        Some("nef") => ImageFormat::Raw(RawKind::Nef),
        Some("arw") => ImageFormat::Raw(RawKind::Arw),
        Some("dng") => ImageFormat::Raw(RawKind::Dng),
        Some("pef") => ImageFormat::Raw(RawKind::Pef),
        Some("orf") => ImageFormat::Raw(RawKind::Orf),
        Some("raf") => ImageFormat::Raw(RawKind::Raf),
        _ => ImageFormat::Jpeg,
    }
}

fn decode_image_file(path: &Path) -> Result<DynamicImage> {
    let reader = image::ImageReader::open(path)
        .map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?
        .with_guessed_format()
        .map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;

    reader
        .decode()
        .map_err(|e| Error::Decode { path: path.to_path_buf(), source: e })
}

/// Walk all IFDs of a TIFF-container RAW, find the largest embedded JPEG, and
/// return its bytes. The "largest" rule is what most RAW formats use to mark
/// the full-resolution preview vs. the tiny thumbnail-strip preview.
fn extract_tiff_preview_jpeg(path: &Path) -> Result<Vec<u8>> {
    use exif::{In, Tag};

    let mut file = File::open(path).map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    let mut buf = BufReader::new(&mut file);
    let exif_data = exif::Reader::new()
        .read_from_container(&mut buf)
        .map_err(|e| Error::Exif { path: path.to_path_buf(), source: e })?;

    // Find the IFD whose JPEGInterchangeFormat points at the largest JPEG blob.
    let mut best: Option<(u64, u64)> = None; // (offset, length)
    for ifd in [In::PRIMARY, In(1), In(2)] {
        let offset = exif_data.get_field(Tag::JPEGInterchangeFormat, ifd);
        let length = exif_data.get_field(Tag::JPEGInterchangeFormatLength, ifd);
        if let (Some(off_f), Some(len_f)) = (offset, length) {
            let off = first_long(&off_f.value);
            let len = first_long(&len_f.value);
            if let (Some(o), Some(l)) = (off, len) {
                if best.map(|(_, bl)| l > bl).unwrap_or(true) {
                    best = Some((o, l));
                }
            }
        }
    }

    let (offset, length) = best.ok_or_else(|| Error::Config(format!(
        "{}: no embedded JPEG preview found (RAW may lack a preview block)",
        path.display()
    )))?;

    let mut file = File::open(path).map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    let mut bytes = vec![0u8; length as usize];
    file.read_exact(&mut bytes)
        .map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    Ok(bytes)
}

fn first_long(v: &exif::Value) -> Option<u64> {
    match v {
        exif::Value::Long(xs) => xs.first().map(|x| *x as u64),
        exif::Value::Short(xs) => xs.first().map(|x| *x as u64),
        _ => None,
    }
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
