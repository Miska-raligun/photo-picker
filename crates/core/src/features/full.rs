use super::{hash::HashOnlyExtractor, FeatureExtractor, PhotoFeatures};
use crate::error::{Error, Result};
use crate::ingest::PhotoRef;
use crate::models::ClipEncoder;
use crate::scoring::{
    compute_raw_scores, AestheticScorer, CompositionScorer, FaceDetector,
    HeuristicAestheticScorer, HeuristicCompositionScorer, NoFaceDetectorStub,
};
use image::DynamicImage;
use std::sync::Mutex;

/// Combines hash extraction + M2 technical scorers + (optionally) CLIP
/// embedding + M3 model-driven scorers in a single pass over the thumbnail,
/// so the file is decoded once per photo.
///
/// The CLIP session is wrapped in `Mutex` because `ort::Session` is `Send` but
/// not `Sync`. Inference is internally multi-threaded by onnxruntime, so
/// serializing it across rayon workers loses little while keeping memory at
/// one model copy.
pub struct FullExtractor {
    hashes: HashOnlyExtractor,
    clip: Option<Mutex<ClipEncoder>>,
    face: Box<dyn FaceDetector>,
    aesthetic: Box<dyn AestheticScorer>,
    composition: Box<dyn CompositionScorer>,
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
            face: Box::new(NoFaceDetectorStub),
            aesthetic: Box::new(HeuristicAestheticScorer),
            composition: Box::new(HeuristicCompositionScorer),
        }
    }

    /// Builder-style override for the face detector. Use to swap the stub for
    /// a real ONNX-backed detector once one is wired in.
    pub fn with_face_detector(mut self, fd: Box<dyn FaceDetector>) -> Self {
        self.face = fd;
        self
    }

    pub fn with_aesthetic(mut self, a: Box<dyn AestheticScorer>) -> Self {
        self.aesthetic = a;
        self
    }

    pub fn with_composition(mut self, c: Box<dyn CompositionScorer>) -> Self {
        self.composition = c;
        self
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

        let aesthetic = self.aesthetic.score(thumb);
        let composition = self.composition.score(thumb);
        let face = self.face.detect(thumb);

        Ok(PhotoFeatures {
            photo_id: base.photo_id,
            phash: base.phash,
            dhash: base.dhash,
            exposure: Some(raw.exposure),
            wb: Some(raw.wb),
            sharpness_raw: Some(raw.sharpness_raw),
            noise: Some(raw.noise),
            clip_embed,
            aesthetic: Some(aesthetic),
            composition: Some(composition),
            face: Some(face),
        })
    }
}
