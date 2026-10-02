mod decoder;
mod exif;
mod raw_preview;
mod scanner;

pub use decoder::{decode_thumbnail, decode_thumbnail_for, encode_jpeg, ThumbnailSpec};
pub use exif::ExifInfo;
pub use scanner::{
    classify_extension, scan_files, scan_files_with_skips, FsScanner, PhotoSource, Scanner,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Pipeline step at which a photo was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipStage {
    /// Couldn't be listed/opened/hashed, or isn't a supported format.
    Scan,
    /// Opened, but no image could be decoded from it (corrupt file, RAW
    /// without a usable preview, ...).
    Decode,
    /// Decoded, but feature extraction / scoring failed.
    Features,
}

/// A photo the pipeline couldn't process. These used to be dropped with only
/// a log line — neither picked nor rejected — so a scan could silently cover
/// fewer files than the folder holds. Carried in the run report so the UI
/// can say how many were skipped and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedPhoto {
    pub path: PathBuf,
    pub stage: SkipStage,
    pub reason: String,
}

impl SkippedPhoto {
    pub fn new(path: impl Into<PathBuf>, stage: SkipStage, reason: impl std::fmt::Display) -> Self {
        Self { path: path.into(), stage, reason: reason.to_string() }
    }
}

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

/// RAW format families. All decode through rawler's vendor-aware embedded
/// preview extraction (three-tier fallback in `decoder.rs`); the TIFF
/// containers additionally have a legacy EXIF/byte-scan path. CR3/RAF EXIF
/// (timestamp/ISO/orientation) also comes from rawler — kamadak-exif can't
/// walk their containers.
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
    /// Canon CR3 (ISO BMFF container)
    Cr3,
    /// Fujifilm RAF (proprietary container)
    Raf,
}

impl RawKind {
    /// Whether the legacy EXIF-walk / byte-scan preview path (decode tier 2)
    /// can apply. CR3/RAF skip straight from rawler preview to demosaic.
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
