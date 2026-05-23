use super::{unionfind::UnionFind, GroupId};
use crate::ingest::PhotoId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct StageBParams {
    /// Cosine similarity threshold above which two photos are merged into the
    /// same composition group.
    pub similarity_threshold: f32,
}

impl Default for StageBParams {
    fn default() -> Self {
        Self { similarity_threshold: 0.93 }
    }
}

/// A Stage B "composition group" — a cluster of photos with very similar CLIP
/// embeddings (≈ same composition / scene framing). Stage B groups draw their
/// members from across Stage A clusters, since two distinct bursts of the same
/// subject should still be ranked against each other.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositionGroup {
    pub id: GroupId,
    pub photo_ids: Vec<PhotoId>,
}

/// Single-link agglomerative clustering on L2-normalized CLIP embeddings.
/// O(n²) — fine for the typical n ≤ few-hundred kept photos.
pub fn cluster_stage_b(
    kept_with_embeds: &[(PhotoId, Vec<f32>)],
    params: &StageBParams,
) -> Vec<CompositionGroup> {
    let n = kept_with_embeds.len();
    if n == 0 {
        return vec![];
    }

    let mut uf = UnionFind::new(n);
    for i in 0..n {
        for j in (i + 1)..n {
            let s = cosine_normalized(&kept_with_embeds[i].1, &kept_with_embeds[j].1);
            if s > params.similarity_threshold {
                uf.union(i, j);
            }
        }
    }

    let mut buckets: HashMap<usize, Vec<PhotoId>> = HashMap::new();
    for (i, (pid, _)) in kept_with_embeds.iter().enumerate() {
        buckets.entry(uf.find(i)).or_default().push(*pid);
    }
    buckets
        .into_values()
        .map(|ids| CompositionGroup { id: GroupId::new(), photo_ids: ids })
        .collect()
}

/// Cosine similarity for already L2-normalized vectors (dot product).
fn cosine_normalized(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normed(values: &[f32]) -> Vec<f32> {
        let n: f32 = values.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-8);
        values.iter().map(|x| x / n).collect()
    }

    #[test]
    fn identical_embeddings_merge() {
        let id_a = PhotoId::new();
        let id_b = PhotoId::new();
        let e = normed(&[1.0, 0.0, 0.0]);
        let groups = cluster_stage_b(
            &[(id_a, e.clone()), (id_b, e)],
            &StageBParams::default(),
        );
        assert_eq!(groups.len(), 1);
    }

    #[test]
    fn orthogonal_embeddings_split() {
        let groups = cluster_stage_b(
            &[
                (PhotoId::new(), normed(&[1.0, 0.0, 0.0])),
                (PhotoId::new(), normed(&[0.0, 1.0, 0.0])),
            ],
            &StageBParams::default(),
        );
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn near_neighbors_below_threshold_split() {
        // Cosine ~0.92 → below default 0.93 → should split.
        let a = normed(&[1.0, 0.4, 0.0]);
        let b = normed(&[0.6, 1.0, 0.0]);
        let groups = cluster_stage_b(
            &[(PhotoId::new(), a), (PhotoId::new(), b)],
            &StageBParams::default(),
        );
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn near_neighbors_above_threshold_merge() {
        // Cosine ~0.998 → above default 0.93 → should merge.
        let a = normed(&[1.0, 0.05, 0.0]);
        let b = normed(&[1.0, 0.10, 0.0]);
        let groups = cluster_stage_b(
            &[(PhotoId::new(), a), (PhotoId::new(), b)],
            &StageBParams::default(),
        );
        assert_eq!(groups.len(), 1);
    }
}
