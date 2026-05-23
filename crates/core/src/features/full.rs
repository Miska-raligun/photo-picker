use super::{hash::HashOnlyExtractor, FeatureExtractor, PhotoFeatures};
use crate::error::{Error, Result};
use crate::ingest::PhotoRef;
use crate::models::ClipEncoder;
use crate::scoring::compute_raw_scores;
use image::DynamicImage;
use std::sync::Mutex;

/// Combines hash extraction + M2 technical scorers + (optionally) CLIP
/// embedding in a single pass over the thumbnail, so the file is decoded once.
///
/// The CLIP session is wrapped in a `Mutex` because `ort::Session` is `Send`
/// but not `Sync`. Inference is internally multi-threaded by onnxruntime, so
/// serializing it across rayon workers loses very little while keeping memory
/// at one model copy (vs N copies for per-thread sessions).
pub struct FullExtractor {
    hashes: HashOnlyExtractor,
    clip: Option<Mutex<ClipEncoder>>,
}

impl Default for FullExtractor {
    fn default() -> Self {
        Self::new(None)
    }
}

impl FullExtractor {
    pub fn new(clip: Option<ClipEncoder>) -> Self {
        Self {
            hashes: HashOnlyExtractor::new(),
            clip: clip.map(Mutex::new),
        }
    }
}

impl FeatureExtractor for FullExtractor {
    fn extract(&self, photo: &PhotoRef, thumb: &DynamicImage) -> Result<PhotoFeatures> {
        let base = self.hashes.extract(photo, thumb)?;
        let raw = compute_raw_scores(thumb, photo);

        let clip_embed = if let Some(mu) = &self.clip {
            let mut guard = mu.lock().map_err(|_| Error::Config("clip mutex poisoned".into()))?;
            Some(guard.embed(thumb)?)
        } else {
            None
        };

        Ok(PhotoFeatures {
            photo_id: base.photo_id,
            phash: base.phash,
            dhash: base.dhash,
            exposure: Some(raw.exposure),
            wb: Some(raw.wb),
            sharpness_raw: Some(raw.sharpness_raw),
            noise: Some(raw.noise),
            clip_embed,
        })
    }
}
