pub mod hash;

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

    // Populated starting M2; serialized as null when absent.
    pub exposure:  Option<f32>,
    pub wb:        Option<f32>,
    pub sharpness: Option<f32>,
    pub noise:     Option<f32>,
}

impl PhotoFeatures {
    pub fn hashes_only(id: PhotoId, phash: u64, dhash: u64) -> Self {
        Self {
            photo_id: id,
            phash,
            dhash,
            exposure: None,
            wb: None,
            sharpness: None,
            noise: None,
        }
    }
}

pub trait FeatureExtractor: Send + Sync {
    fn extract(&self, photo: &PhotoRef, thumb: &DynamicImage) -> Result<PhotoFeatures>;
}
