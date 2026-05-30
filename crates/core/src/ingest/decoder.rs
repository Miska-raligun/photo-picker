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
        ImageFormat::Raw(_kind) => {
            // Primary: rawler — knows each vendor's MakerNote layout, so the
            // largest embedded preview (often full-res JPEG) comes out cleanly
            // for NEF/ARW/CR2/CR3/RAF/ORF/DNG/… without our brittle byte-scan
            // fallback. Drops the per-kind container restriction since rawler
            // covers CR3/RAF (ISO-BMFF and Fuji-specific containers) too.
            match super::raw_preview::extract_embedded_preview(path) {
                Ok(img) => img,
                Err(rawler_err) => {
                    // Legacy fallback: EXIF SubIFD JPEG, then a raw byte scan
                    // for SOI/EOI markers. Keep for any oddball format rawler
                    // doesn't recognize on this build.
                    tracing::debug!(
                        path = %path.display(),
                        %rawler_err,
                        "rawler preview failed; falling back to EXIF/byte-scan"
                    );
                    let jpeg_bytes = extract_tiff_preview_jpeg(path)?;
                    image::load_from_memory_with_format(&jpeg_bytes, image::ImageFormat::Jpeg)
                        .map_err(|e| Error::Decode { path: path.to_path_buf(), source: e })?
                }
            }
        }
    };
    // Upright the image per its EXIF orientation before downscaling. Cameras
    // store pixels in sensor order + an orientation tag; without this, portrait
    // shots are processed and displayed sideways (and YuNet, trained on upright
    // faces, misses them). RAW embedded previews follow the same convention.
    let img = apply_exif_orientation(img, super::exif::read_orientation(path));
    Ok(downscale(img, spec.long_edge))
}

/// Apply an EXIF orientation value (1–8) to an image. Value 1 (and anything
/// unrecognized) is a no-op.
fn apply_exif_orientation(img: DynamicImage, orientation: u16) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
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

/// Find the largest embedded JPEG inside a TIFF-container RAW.
///
/// Strategy: EXIF first (cheap, exact), then a brute byte-scan fallback.
/// The fallback exists because vendor RAW formats (notably Nikon NEF) park
/// the full-resolution preview JPEG in a SubIFD that kamadak-exif's top-level
/// `In(N)` iterator doesn't reach. Scanning for SOI/EOI markers and keeping
/// the largest hit is format-agnostic and very robust.
fn extract_tiff_preview_jpeg(path: &Path) -> Result<Vec<u8>> {
    if let Ok(bytes) = extract_via_exif(path) {
        return Ok(bytes);
    }
    extract_via_jpeg_scan(path)
}

fn extract_via_exif(path: &Path) -> Result<Vec<u8>> {
    use exif::{In, Tag};

    let mut file = File::open(path).map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    let mut buf = BufReader::new(&mut file);
    let exif_data = exif::Reader::new()
        .read_from_container(&mut buf)
        .map_err(|e| Error::Exif { path: path.to_path_buf(), source: e })?;

    let mut best: Option<(u64, u64)> = None;
    for ifd in [In::PRIMARY, In(1), In(2)] {
        let offset = exif_data.get_field(Tag::JPEGInterchangeFormat, ifd);
        let length = exif_data.get_field(Tag::JPEGInterchangeFormatLength, ifd);
        if let (Some(off_f), Some(len_f)) = (offset, length) {
            if let (Some(o), Some(l)) = (first_long(&off_f.value), first_long(&len_f.value)) {
                if best.map(|(_, bl)| l > bl).unwrap_or(true) {
                    best = Some((o, l));
                }
            }
        }
    }
    let (offset, length) = best.ok_or_else(|| Error::Config("no preview in top-level IFDs".into()))?;

    let mut file = File::open(path).map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    let mut bytes = vec![0u8; length as usize];
    file.read_exact(&mut bytes)
        .map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    Ok(bytes)
}

/// Scan the file bytes for `FF D8 ... FF D9` JPEG segments and return the
/// largest one. Reads up to 128MB; covers virtually all consumer RAWs without
/// pulling the whole file when files are huge.
fn extract_via_jpeg_scan(path: &Path) -> Result<Vec<u8>> {
    const MAX_SCAN_BYTES: usize = 128 * 1024 * 1024;

    let mut file = File::open(path).map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    let meta = file.metadata().map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    let to_read = (meta.len() as usize).min(MAX_SCAN_BYTES);
    let mut buf = vec![0u8; to_read];
    file.read_exact(&mut buf)
        .map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;

    let mut best: Option<(usize, usize)> = None;
    let mut i = 0usize;
    while i + 1 < buf.len() {
        if buf[i] == 0xFF && buf[i + 1] == 0xD8 {
            // Found SOI; scan forward for matching EOI. Skip past entropy data
            // by stepping byte-by-byte (some markers contain 0xFF padding which
            // doesn't matter for finding the final 0xFFD9).
            let start = i;
            let mut j = i + 2;
            let mut found_eoi = false;
            while j + 1 < buf.len() {
                if buf[j] == 0xFF && buf[j + 1] == 0xD9 {
                    let end = j + 2;
                    let len = end - start;
                    // Embedded thumbnails in NEFs/CR2s are usually 5-50KB; the
                    // full-resolution preview is multiple MB. Tracking the max
                    // naturally lands on the preview.
                    if best.map(|(s, e)| len > e - s).unwrap_or(true) {
                        best = Some((start, end));
                    }
                    i = end;
                    found_eoi = true;
                    break;
                }
                j += 1;
            }
            if !found_eoi {
                break; // Unterminated; nothing useful past here.
            }
        } else {
            i += 1;
        }
    }

    match best {
        Some((s, e)) => Ok(buf[s..e].to_vec()),
        None => Err(Error::Config(format!(
            "{}: no embedded JPEG preview found in first {}MB",
            path.display(),
            to_read / 1024 / 1024
        ))),
    }
}

fn first_long(v: &exif::Value) -> Option<u64> {
    match v {
        exif::Value::Long(xs) => xs.first().map(|x| *x as u64),
        exif::Value::Short(xs) => xs.first().map(|x| *x as u64),
        _ => None,
    }
}

/// Encode an in-memory image to JPEG bytes at the given quality (0-100).
pub fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(40_000);
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    img.write_with_encoder(enc)
        .map_err(|e| Error::Config(format!("jpeg encode: {e}")))?;
    Ok(buf)
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgb, RgbImage};

    fn sample() -> DynamicImage {
        // 3 wide × 2 tall, asymmetric so transforms are distinguishable.
        let mut img = RgbImage::new(3, 2);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgb([x as u8 * 40, y as u8 * 40, 0]);
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn orientation_1_is_identity() {
        let img = sample();
        let out = apply_exif_orientation(img.clone(), 1);
        assert_eq!(out.to_rgb8(), img.to_rgb8());
    }

    #[test]
    fn orientation_swaps_dimensions_for_rotations() {
        let img = sample(); // 3×2
        for o in [5u16, 6, 7, 8] {
            let out = apply_exif_orientation(img.clone(), o);
            assert_eq!((out.width(), out.height()), (2, 3), "orientation {o} should swap dims");
        }
        for o in [1u16, 2, 3, 4] {
            let out = apply_exif_orientation(img.clone(), o);
            assert_eq!((out.width(), out.height()), (3, 2), "orientation {o} keeps dims");
        }
    }

    #[test]
    fn orientation_3_is_self_inverse() {
        let img = sample();
        let twice = apply_exif_orientation(apply_exif_orientation(img.clone(), 3), 3);
        assert_eq!(twice.to_rgb8(), img.to_rgb8());
    }
}
