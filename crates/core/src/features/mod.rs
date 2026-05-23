pub mod full;
pub mod hash;

pub use full::FullExtractor;
pub use hash::HashOnlyExtractor;

use crate::error::Result;
use crate::ingest::{PhotoId, PhotoRef};
use image::DynamicImage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoFeatures {
    pub photo_id: PhotoId,
    pub phash: u64,
    pub dhash: u64,

    /// All four populated starting M2; `None` when only a hash-only extractor ran.
    /// `sharpness_raw` is the unnormalized signal — the pipeline z-scores it per
    /// group before producing the final tech score.
    pub exposure:      Option<f32>,
    pub wb:            Option<f32>,
    pub sharpness_raw: Option<f32>,
    pub noise:         Option<f32>,

    /// L2-normalized CLIP embedding (M3+). Skipped from serialization — 512
    /// f32 per photo would bloat the JSON report by ~2KB/photo without value
    /// to humans; recompute from the cache if you need it.
    #[serde(skip)]
    pub clip_embed: Option<Vec<f32>>,
}

impl PhotoFeatures {
    pub fn hashes_only(id: PhotoId, phash: u64, dhash: u64) -> Self {
        Self {
            photo_id: id,
            phash,
            dhash,
            exposure: None,
            wb: None,
            sharpness_raw: None,
            noise: None,
            clip_embed: None,
        }
    }

    /// Returns the raw technical scores if all four are populated.
    pub fn raw_tech_scores(&self) -> Option<crate::scoring::RawTechScores> {
        Some(crate::scoring::RawTechScores {
            exposure: self.exposure?,
            wb: self.wb?,
            sharpness_raw: self.sharpness_raw?,
            noise: self.noise?,
        })
    }
}

pub trait FeatureExtractor: Send + Sync {
    fn extract(&self, photo: &PhotoRef, thumb: &DynamicImage) -> Result<PhotoFeatures>;
}
