//! Disk-persistent JPEG thumbnail cache, keyed by `sha256_short`.
//!
//! Sits between the pipeline's feature extractor (which already decodes a
//! `DynamicImage` per photo and previously threw it away) and the consumers
//! that re-decode the source: the HTML report's `build_thumbnail_map` and the
//! server's `/thumb` + `/preview` handlers.
//!
//! Layout: `<cache_root>/<hex_sha256_short>.jpg`. Content-addressed by
//! `sha256_short`, so re-scans of the same directory hit a warm cache and
//! the file survives across server restarts.

use crate::error::{Error, Result};
use crate::ingest::{decode_thumbnail_for, encode_jpeg, PhotoRef, ThumbnailSpec};
use image::DynamicImage;
use std::fs;
use std::path::{Path, PathBuf};

/// Conservative defaults — match `output/html.rs`'s embedded thumb so the
/// pipeline's persisted bytes can be reused verbatim for the report.
pub const DEFAULT_THUMB_LONG_EDGE: u32 = 480;
pub const DEFAULT_THUMB_QUALITY: u8 = 78;

/// Bumped when the rendering of a cached thumbnail changes (so stale files from
/// older builds are ignored rather than served). v2: EXIF orientation applied.
const THUMB_CACHE_VERSION: u32 = 2;

/// Filesystem-backed thumb cache. Cheap to clone (just a `PathBuf`).
#[derive(Debug, Clone)]
pub struct ThumbDiskCache {
    dir: PathBuf,
    spec: ThumbnailSpec,
    quality: u8,
}

impl ThumbDiskCache {
    /// Create or open the cache at `dir`. Does NOT create the directory eagerly;
    /// it's created lazily on the first successful `persist`.
    pub fn new(dir: PathBuf, long_edge: u32, quality: u8) -> Self {
        Self {
            dir,
            spec: ThumbnailSpec { long_edge },
            quality,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn spec(&self) -> ThumbnailSpec {
        self.spec
    }

    pub fn quality(&self) -> u8 {
        self.quality
    }

    fn path_for(&self, sha256_short: &[u8; 16]) -> PathBuf {
        // Versioned filename so a change in how thumbnails are rendered (e.g.
        // EXIF orientation, v2) invalidates cached bytes from older builds
        // instead of serving stale (sideways) images.
        self.dir
            .join(format!("{}.v{}.jpg", hex::encode(sha256_short), THUMB_CACHE_VERSION))
    }

    /// Read encoded JPEG bytes if present on disk.
    pub fn read(&self, sha256_short: &[u8; 16]) -> Option<Vec<u8>> {
        fs::read(self.path_for(sha256_short)).ok()
    }

    /// Encode the supplied (already-decoded) thumbnail and persist it. Used
    /// during feature extraction where we already hold the `DynamicImage`.
    /// Errors during write are logged but not surfaced — the cache is a
    /// best-effort optimisation, not part of the pipeline's correctness path.
    pub fn persist(&self, sha256_short: &[u8; 16], thumb: &DynamicImage) {
        if self.read(sha256_short).is_some() {
            return; // already cached
        }
        let thumb_small = if thumb.width().max(thumb.height()) > self.spec.long_edge {
            // Down-render to the cache's spec so we don't store a 4K thumb
            // when a 480px one was requested.
            let (w, h) = (thumb.width(), thumb.height());
            let scale = self.spec.long_edge as f32 / w.max(h) as f32;
            let nw = ((w as f32 * scale).round() as u32).max(1);
            let nh = ((h as f32 * scale).round() as u32).max(1);
            thumb.resize_exact(nw, nh, image::imageops::FilterType::Triangle)
        } else {
            thumb.clone()
        };
        let Ok(bytes) = encode_jpeg(&thumb_small, self.quality) else {
            return;
        };
        if let Err(err) = fs::create_dir_all(&self.dir) {
            tracing::debug!(dir = %self.dir.display(), %err, "thumb cache mkdir failed");
            return;
        }
        let dest = self.path_for(sha256_short);
        // Atomic-ish write via temp + rename so a concurrent reader never sees
        // a partial file. Different procs on the same cache dir race the rename
        // — last writer wins, harmless because content is content-addressed.
        let tmp = dest.with_extension("jpg.tmp");
        if let Err(err) = fs::write(&tmp, &bytes) {
            tracing::debug!(path = %tmp.display(), %err, "thumb cache write failed");
            return;
        }
        if let Err(err) = fs::rename(&tmp, &dest) {
            let _ = fs::remove_file(&tmp);
            tracing::debug!(path = %dest.display(), %err, "thumb cache rename failed");
        }
    }

    /// Read from disk, or decode the source and render fresh. Successful renders
    /// are persisted back so the next call hits the cache. Used by consumers
    /// (html report, server) that may run against a partially-populated cache.
    pub fn read_or_render(&self, photo: &PhotoRef) -> Result<Vec<u8>> {
        if let Some(bytes) = self.read(&photo.sha256_short) {
            return Ok(bytes);
        }
        let img = decode_thumbnail_for(photo, self.spec)?;
        let bytes = encode_jpeg(&img, self.quality)?;
        // Persist for next time — best effort.
        if let Err(err) = fs::create_dir_all(&self.dir) {
            tracing::debug!(dir = %self.dir.display(), %err, "thumb cache mkdir failed");
        } else {
            let dest = self.path_for(&photo.sha256_short);
            let tmp = dest.with_extension("jpg.tmp");
            if fs::write(&tmp, &bytes).is_ok() && fs::rename(&tmp, &dest).is_err() {
                let _ = fs::remove_file(&tmp);
            }
        }
        Ok(bytes)
    }
}

#[allow(dead_code)]
fn _unused_error_path_marker(_: Error) {}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};
    use tempfile::TempDir;

    fn dummy_thumb(size: u32) -> DynamicImage {
        let mut img = RgbImage::new(size, size);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x as u8).wrapping_mul(3), (y as u8).wrapping_mul(5), 128]);
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn persist_then_read_roundtrip() {
        let td = TempDir::new().unwrap();
        let cache = ThumbDiskCache::new(td.path().join("thumbs"), 480, 78);
        let sha = [7u8; 16];
        cache.persist(&sha, &dummy_thumb(640));
        let bytes = cache.read(&sha).expect("should be cached");
        assert!(bytes.starts_with(&[0xFF, 0xD8])); // JPEG SOI marker
    }

    #[test]
    fn read_missing_returns_none() {
        let td = TempDir::new().unwrap();
        let cache = ThumbDiskCache::new(td.path().to_path_buf(), 480, 78);
        assert!(cache.read(&[0u8; 16]).is_none());
    }
}
