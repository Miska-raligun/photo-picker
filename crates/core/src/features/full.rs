use super::{hash::HashOnlyExtractor, FeatureExtractor, PhotoFeatures};
use crate::error::Result;
use crate::ingest::PhotoRef;
use crate::scoring::compute_raw_scores;
use image::DynamicImage;

/// Combines hash extraction with the M2 technical scorers in a single pass over
/// the thumbnail, so we decode only once per photo.
pub struct FullExtractor {
    hashes: HashOnlyExtractor,
}

impl Default for FullExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl FullExtractor {
    pub fn new() -> Self {
        Self { hashes: HashOnlyExtractor::new() }
    }
}

impl FeatureExtractor for FullExtractor {
    fn extract(&self, photo: &PhotoRef, thumb: &DynamicImage) -> Result<PhotoFeatures> {
        let base = self.hashes.extract(photo, thumb)?;
        let raw = compute_raw_scores(thumb, photo);
        Ok(PhotoFeatures {
            photo_id: base.photo_id,
            phash: base.phash,
            dhash: base.dhash,
            exposure: Some(raw.exposure),
            wb: Some(raw.wb),
            sharpness_raw: Some(raw.sharpness_raw),
            noise: Some(raw.noise),
        })
    }
}
