//! Aesthetic / image-quality scorer.
//!
//! **Status (M3.4)**: stub returning a neutral 0.5 for every image. The interface
//! is defined so the rest of the pipeline can wire it in; the model itself is
//! deferred.
//!
//! Two plan-side paths for the real implementation:
//!
//! - **NIMA**: load a pretrained NIMA ONNX (MobileNet/Inception backbone),
//!   integrate as another ort Session like CLIP. Output is a probability
//!   distribution over [1..10]; we report `Σ i·p[i]` then map to [0,1].
//! - **CLIP-IQA**: reuse the loaded CLIP vision encoder, compare each image
//!   embedding against two pre-computed text embeddings (e.g. "a good photo"
//!   vs "a bad photo") via cosine + softmax. No new model file — just
//!   ~4KB of constants we compute once from the CLIP text encoder.
//!
//! The trait and pipeline integration should not change when the real model
//! lands.

use image::DynamicImage;

pub trait AestheticScorer: Send + Sync {
    /// Returns an aesthetic quality score in `[0, 1]` for `thumb` (the 1024px
    /// pre-decoded thumbnail used elsewhere in the pipeline).
    fn score(&self, thumb: &DynamicImage) -> f32;
}

/// Placeholder that returns 0.5 for every image. Replace with NIMA or CLIP-IQA
/// before claiming meaningful aesthetic ranking.
pub struct NeutralAestheticStub;

impl AestheticScorer for NeutralAestheticStub {
    fn score(&self, _thumb: &DynamicImage) -> f32 {
        0.5
    }
}
