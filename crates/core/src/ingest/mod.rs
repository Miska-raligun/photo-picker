mod decoder;
mod exif;
mod scanner;

pub use decoder::{decode_thumbnail, decode_thumbnail_for, ThumbnailSpec};
pub use exif::ExifInfo;
pub use scanner::{FsScanner, Scanner};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PhotoId(pub Uuid);

impl PhotoId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PhotoId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for PhotoId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFormat {
    Jpeg,
    Raw(RawKind),
    // future: Heic
}

/// RAW format families. Only the TIFF-container ones are actually decodable in
/// M2 (we extract the embedded full-resolution JPEG preview rather than
/// demosaicing the raw sensor data).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RawKind {
    /// Canon CR2 (TIFF container with embedded JPEG)
    Cr2,
    /// Nikon NEF (TIFF container)
    Nef,
    /// Sony ARW (TIFF container)
    Arw,
    /// Adobe DNG (TIFF container)
    Dng,
    /// Pentax PEF (TIFF container)
    Pef,
    /// Olympus ORF (TIFF container)
    Orf,
    /// Canon CR3 — ISOBMFF, not supported in M2
    Cr3,
    /// Fujifilm RAF — custom container, not supported in M2
    Raf,
}

impl RawKind {
    /// Whether M2's TIFF-embedded-preview extractor can handle this format.
    pub fn is_tiff_container(self) -> bool {
        matches!(self, Self::Cr2 | Self::Nef | Self::Arw | Self::Dng | Self::Pef | Self::Orf)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriveMode {
    Single,
    ContinuousLow,
    ContinuousHigh,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoRef {
    pub id: PhotoId,
    pub path: PathBuf,
    pub format: ImageFormat,
    pub captured_at: Option<DateTime<Utc>>,
    pub file_size: u64,
    /// First 16 bytes of SHA-256 — enough for in-batch dedup.
    pub sha256_short: [u8; 16],
    pub burst_id: Option<String>,
    pub drive_mode: Option<DriveMode>,
    pub iso: Option<u32>,
    pub exposure_bias_ev: Option<f32>,
}
