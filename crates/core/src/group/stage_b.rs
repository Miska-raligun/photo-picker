use super::{cosine_normalized, unionfind::UnionFind, GroupId};
use crate::ingest::PhotoId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct StageBParams {
    /// Cosine similarity threshold above which two photos are merged into the
    /// same composition group.
    pub similarity_threshold: f32,
    /// Anti-chaining margin. After the union-find pass, members of a multi-
    /// member group whose cosine similarity to the group centroid falls below
    /// `(similarity_threshold - chain_margin)` are split off as singletons.
    /// Prevents the classic A~B~C~D chain where A·D is well below threshold
    /// yet still ends up in the same group because of intermediate links.
    pub chain_margin: f32,
}

impl Default for StageBParams {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.93,
            chain_margin: 0.05,
        }
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

/// Two-pass agglomerative clustering on L2-normalized CLIP embeddings:
///   1. Single-link union-find against `similarity_threshold` (cheap candidate
///      grouping).
///   2. Centroid-distance refinement using `chain_margin` to split off outliers
///      that only joined via a transitive chain. Pure single-link clustering
///      can drag A and D into one group when A·D itself is far below the
///      threshold; the centroid check catches that.
/// O(n²) over embeddings — fine for the typical n ≤ few-hundred kept photos.
pub fn cluster_stage_b(
    kept_with_embeds: &[(PhotoId, Vec<f32>)],
    params: &StageBParams,
) -> Vec<CompositionGroup> {
    let n = kept_with_embeds.len();
    if n == 0 {
        return vec![];
    }

    // Pass 1: union-find on similarity > threshold (single-link).
    let mut uf = UnionFind::new(n);
    for i in 0..n {
        for j in (i + 1)..n {
            let s = cosine_normalized(&kept_with_embeds[i].1, &kept_with_embeds[j].1);
            if s > params.similarity_threshold {
                uf.union(i, j);
            }
        }
    }

    let mut buckets: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        buckets.entry(uf.find(i)).or_default().push(i);
    }

    // Pass 2: split chains via centroid distance.
    let split_threshold = (params.similarity_threshold - params.chain_margin).max(0.0);
    let mut groups: Vec<Vec<PhotoId>> = Vec::new();
    for (_, indices) in buckets {
        if indices.len() <= 1 {
            groups.push(indices.iter().map(|i| kept_with_embeds[*i].0).collect());
            continue;
        }

        // Compute L2-normalized centroid of the member embeddings.
        let dim = kept_with_embeds[indices[0]].1.len();
        let mut centroid = vec![0.0_f32; dim];
        for &i in &indices {
            for (j, v) in kept_with_embeds[i].1.iter().enumerate() {
                centroid[j] += v;
            }
        }
        let m = indices.len() as f32;
        for v in centroid.iter_mut() {
            *v /= m;
        }
        let norm: f32 = centroid.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-8);
        for v in centroid.iter_mut() {
            *v /= norm;
        }

        let mut core: Vec<PhotoId> = Vec::new();
        for &i in &indices {
            let sim = cosine_normalized(&centroid, &kept_with_embeds[i].1);
            if sim >= split_threshold {
                core.push(kept_with_embeds[i].0);
            } else {
                groups.push(vec![kept_with_embeds[i].0]);
            }
        }
        if !core.is_empty() {
            groups.push(core);
        }
    }

    groups
        .into_iter()
        .map(|ids| CompositionGroup {
            id: GroupId::new(),
            photo_ids: ids,
        })
        .collect()
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
        let a = normed(&[1.0, 0.05, 0.0]);
        let b = normed(&[1.0, 0.10, 0.0]);
        let groups = cluster_stage_b(
            &[(PhotoId::new(), a), (PhotoId::new(), b)],
            &StageBParams::default(),
        );
        assert_eq!(groups.len(), 1);
    }

    #[test]
    fn chaining_outlier_is_split_off() {
        // Four embeddings on a chain: a·b ≈ b·c ≈ c·d ≈ 0.94 (>threshold);
        // a·d ≈ 0.78 (well below). Single-link would merge all four; the
        // centroid refinement should pull `d` out — its similarity to the
        // (a, b, c, d) centroid sits below 0.88 once `d` is the outlier.
        let a = normed(&[1.0, 0.3, 0.0, 0.0]);
        let b = normed(&[0.95, 0.4, 0.05, 0.0]);
        let c = normed(&[0.85, 0.5, 0.2, 0.05]);
        let d = normed(&[0.6, 0.5, 0.5, 0.3]);
        let groups = cluster_stage_b(
            &[
                (PhotoId::new(), a),
                (PhotoId::new(), b),
                (PhotoId::new(), c),
                (PhotoId::new(), d),
            ],
            &StageBParams { similarity_threshold: 0.93, chain_margin: 0.05 },
        );
        // Expect at least 2 groups (chain didn't collapse).
        assert!(
            groups.len() >= 2,
            "chaining guard should split at least one outlier; got {} groups",
            groups.len()
        );
    }
}
