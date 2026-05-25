mod stage_a;
mod stage_b;
pub(crate) mod unionfind;

pub use stage_a::{cluster_stage_a, StageAParams};
pub use stage_b::{cluster_stage_b, CompositionGroup, StageBParams};

use crate::ingest::PhotoId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Cosine similarity for already L2-normalized vectors — a plain dot product.
/// Both stages compare CLIP embeddings, which `ClipEncoder` L2-normalizes on
/// output, so the norm terms are 1.
pub(crate) fn cosine_normalized(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GroupId(pub Uuid);

impl GroupId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for GroupId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for GroupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    pub photo_ids: Vec<PhotoId>,
    /// M1: simply the first photo of the group. Replaced by highest-scoring photo in M2.
    pub representative: PhotoId,
}

impl Group {
    pub fn singleton(photo_id: PhotoId) -> Self {
        Self {
            id: GroupId::new(),
            photo_ids: vec![photo_id],
            representative: photo_id,
        }
    }
}
