//! Composition score from a saliency mask (plan B.5).
//!
//! **Status (M3.6)**: returns a neutral 0.5 for every image. A real
//! implementation needs a saliency segmenter to find the subject's centroid;
//! once that lands, this module computes the rule-of-thirds, subject-size, and
//! edge-clipping sub-scores per the plan.
//!
//! Candidate saliency models (deferred):
//! - **U²-Net lite** (~4.7MB ONNX)
//! - **TRACER** (lighter, similar quality)
//! - **MODNet** (designed for portraits, alpha-channel output)

use image::DynamicImage;

pub trait CompositionScorer: Send + Sync {
    fn score(&self, thumb: &DynamicImage) -> f32;
}

pub struct NeutralCompositionStub;

impl CompositionScorer for NeutralCompositionStub {
    fn score(&self, _thumb: &DynamicImage) -> f32 {
        0.5
    }
}
