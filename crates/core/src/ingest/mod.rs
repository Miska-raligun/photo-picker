mod decoder;
mod exif;
mod scanner;

pub use decoder::{decode_thumbnail, ThumbnailSpec};
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
    // future: Heic, Raw(RawKind)
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
}
