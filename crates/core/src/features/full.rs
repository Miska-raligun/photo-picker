use super::{hash::HashOnlyExtractor, FeatureExtractor, PhotoFeatures};
use crate::error::Result;
use crate::ingest::PhotoRef;
use crate::scoring::{
    compute_raw_scores, AestheticScorer, CompositionScorer, FaceDetector,
    HeuristicAestheticScorer, HeuristicCompositionScorer, NoFaceDetectorStub,
};
use image::DynamicImage;
#[cfg(feature = "onnx")]
use crate::models::{ClipEncoder, SessionPool};

/// Combines hash extraction + M2 technical scorers + (optionally) CLIP
/// embedding + M3 model-driven scorers in a single pass over the thumbnail,
/// so the file is decoded once per photo.
///
/// CLIP runs through a `SessionPool` so rayon workers can run multiple
/// inferences concurrently instead of serializing on a single `Mutex`. Pool
/// size defaults to 2 (configurable via `PHOTO_PICK_INFERENCE_POOL_SIZE`) —
/// each session is an independent ONNX model copy (~150 MB).
pub struct FullExtractor {
    hashes: HashOnlyExtractor,
    #[cfg(feature = "onnx")]
    clip: Option<SessionPool<ClipEncoder>>,
    face: Box<dyn FaceDetector>,
    aesthetic: Box<dyn AestheticScorer>,
    composition: Box<dyn CompositionScorer>,
}

impl Default for FullExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl FullExtractor {
    pub fn new() -> Self {
        Self {
            hashes: HashOnlyExtractor::new(),
            #[cfg(feature = "onnx")]
            clip: None,
            face: Box::new(NoFaceDetectorStub),
            aesthetic: Box::new(HeuristicAestheticScorer),
            composition: Box::new(HeuristicCompositionScorer),
        }
    }

    /// Attach a CLIP encoder pool for embedding extraction (Stage B input).
    /// The caller decides how many sessions to load; `None` disables CLIP.
    #[cfg(feature = "onnx")]
    pub fn with_clip_pool(mut self, pool: Option<SessionPool<ClipEncoder>>) -> Self {
        self.clip = pool;
        self
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

        #[cfg(feature = "onnx")]
        let clip_embed = if let Some(pool) = &self.clip {
            Some(pool.with(|enc| enc.embed(thumb))?)
        } else {
            None
        };
        #[cfg(not(feature = "onnx"))]
        let clip_embed: Option<Vec<f32>> = None;

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
