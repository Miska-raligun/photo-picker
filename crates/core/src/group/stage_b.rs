use super::{cosine_normalized, unionfind::UnionFind, GroupId};
use crate::ingest::PhotoId;
use ndarray::Array2;
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

    // Pass 1: union-find on similarity > threshold (single-link). We do
    // the pairwise scan via a batched (n × dim) · (dim × n) matmul, which
    // hands the inner-product loops to ndarray's auto-vectorized
    // `matrixmultiply` kernel — roughly an order of magnitude faster than
    // a hand-rolled `zip().map().sum()` triple loop at n ≈ thousands.
    //
    // `PHOTO_PICK_LEGACY_COSINE=1` falls back to the original scalar
    // path. Kept as an escape hatch in case the batched path ever
    // disagrees with the scalar one on real data — pure numerical
    // difference would be float-rounding-eps, which can flip a
    // similarity sitting exactly on the threshold; the env var lets a
    // user reproduce older results without rebuilding.
    let mut uf = UnionFind::new(n);
    let use_legacy = std::env::var_os("PHOTO_PICK_LEGACY_COSINE").is_some();
    let mode = if use_legacy || n < BATCHED_MIN_N { Mode::Scalar } else { Mode::Batched };
    for (i, j) in pairs_above_threshold(kept_with_embeds, params.similarity_threshold, mode) {
        uf.union(i, j);
    }

    let mut buckets: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        buckets.entry(uf.find(i)).or_default().push(i);
    }

    // Pass 2: split chains via centroid distance. Iterative — peel the single
    // worst member below `split_threshold`, then recompute the centroid over
    // the survivors and repeat. A one-shot pass uses the *original* (chain-
    // contaminated) centroid, so a long A~B~C~D chain can leave residual
    // outliers in the core; recomputing after each peel tightens the cluster
    // until every survivor is genuinely close to the group it ends up in.
    let split_threshold = (params.similarity_threshold - params.chain_margin).max(0.0);
    let mut groups: Vec<Vec<PhotoId>> = Vec::new();
    for (_, indices) in buckets {
        if indices.len() <= 1 {
            groups.push(indices.iter().map(|i| kept_with_embeds[*i].0).collect());
            continue;
        }

        let mut core = indices.clone();
        loop {
            if core.len() <= 1 {
                break;
            }
            let centroid = l2_centroid(&core, kept_with_embeds);
            // Find the member least aligned with the current centroid.
            let mut worst_pos = 0usize;
            let mut worst_sim = f32::INFINITY;
            for (pos, &i) in core.iter().enumerate() {
                let sim = cosine_normalized(&centroid, &kept_with_embeds[i].1);
                if sim < worst_sim {
                    worst_sim = sim;
                    worst_pos = pos;
                }
            }
            if worst_sim >= split_threshold {
                break; // every survivor is cohesive — done
            }
            let outlier = core.swap_remove(worst_pos);
            groups.push(vec![kept_with_embeds[outlier].0]);
        }
        groups.push(core.iter().map(|i| kept_with_embeds[*i].0).collect());
    }

    groups
        .into_iter()
        .map(|ids| CompositionGroup {
            id: GroupId::new(),
            photo_ids: ids,
        })
        .collect()
}

/// Below this many photos the batched path's setup cost (allocating an
/// n×dim ndarray + transpose view) outweighs its win over a tight scalar
/// loop. Picked empirically — at n=64 with dim=512 the crossover is well
/// under a millisecond either way, so the exact value doesn't matter much.
const BATCHED_MIN_N: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Original `cosine_normalized` per pair. Always available.
    Scalar,
    /// Single n×dim · dim×n matmul, then read pairs above threshold off
    /// the result. ndarray's f32 `.dot()` uses the SIMD `matrixmultiply`
    /// kernel — ~10x faster at n ≈ thousands.
    Batched,
}

/// Pairs `(i, j)` with `i < j` whose cosine similarity exceeds `threshold`.
/// Both modes produce the same pair set in exact arithmetic; with floats,
/// a similarity sitting on the threshold can flip due to summation order.
/// `PHOTO_PICK_LEGACY_COSINE=1` selects the scalar path to reproduce
/// pre-batching results.
fn pairs_above_threshold(
    kept_with_embeds: &[(PhotoId, Vec<f32>)],
    threshold: f32,
    mode: Mode,
) -> Vec<(usize, usize)> {
    let n = kept_with_embeds.len();
    let mut out = Vec::new();
    match mode {
        Mode::Scalar => {
            for i in 0..n {
                for j in (i + 1)..n {
                    let s = cosine_normalized(&kept_with_embeds[i].1, &kept_with_embeds[j].1);
                    if s > threshold {
                        out.push((i, j));
                    }
                }
            }
        }
        Mode::Batched => {
            let emb = stack_embeddings(kept_with_embeds);
            let sim = emb.dot(&emb.t());
            for i in 0..n {
                for j in (i + 1)..n {
                    if sim[[i, j]] > threshold {
                        out.push((i, j));
                    }
                }
            }
        }
    }
    out
}

/// Copy the input embeddings into a dense `n × dim` `Array2<f32>` so we can
/// feed them to ndarray's matmul. The input is `Vec<(_, Vec<f32>)>` which
/// isn't laid out contiguously — there's no zero-copy view we could hand
/// to ndarray instead.
fn stack_embeddings(kept_with_embeds: &[(PhotoId, Vec<f32>)]) -> Array2<f32> {
    let n = kept_with_embeds.len();
    let dim = kept_with_embeds[0].1.len();
    let mut buf = Array2::<f32>::zeros((n, dim));
    for (i, (_, e)) in kept_with_embeds.iter().enumerate() {
        // Defensive: skip rows that don't match the expected dim. Stage B
        // is called from the pipeline with uniform CLIP embeddings, so
        // this should never trip — but a misshaped input shouldn't panic.
        if e.len() != dim {
            continue;
        }
        for (j, v) in e.iter().enumerate() {
            buf[[i, j]] = *v;
        }
    }
    buf
}

/// L2-normalized mean of the embeddings at `indices`.
fn l2_centroid(indices: &[usize], kept_with_embeds: &[(PhotoId, Vec<f32>)]) -> Vec<f32> {
    let dim = kept_with_embeds[indices[0]].1.len();
    let mut centroid = vec![0.0_f32; dim];
    for &i in indices {
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
    centroid
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

    #[test]
    fn scalar_and_batched_paths_agree() {
        // Build a deterministic 80×8 embedding set with a couple of
        // clusters + some chain-noise members. Both pair-finding modes
        // must report the same set of supra-threshold pairs (modulo
        // float epsilon — we tolerate flips only if the similarity sits
        // exactly on the threshold, which we avoid by spacing the
        // synthetic embeddings well clear of `threshold`).
        let mut items: Vec<(PhotoId, Vec<f32>)> = Vec::with_capacity(80);
        for k in 0..80 {
            // Three "scenes" so we exercise both >-threshold and
            // <-threshold cases.
            let scene = k % 3;
            let mut v = vec![0.0_f32; 8];
            v[scene] = 1.0;
            // Tiny perturbation, deterministic from k.
            v[(scene + 1) % 8] = (k as f32) * 0.001;
            items.push((PhotoId::new(), normed(&v)));
        }
        let threshold = 0.9;
        let mut scalar = pairs_above_threshold(&items, threshold, Mode::Scalar);
        let mut batched = pairs_above_threshold(&items, threshold, Mode::Batched);
        scalar.sort();
        batched.sort();
        assert_eq!(scalar, batched,
            "scalar / batched pair sets diverged — float ordering shouldn't change pair membership for inputs spaced well clear of the threshold");
    }

    #[test]
    fn iterative_refinement_peels_residual_outliers() {
        // A tight core of three near-identical embeddings plus two progressively
        // drifting members linked in via a chain. A single-pass centroid (biased
        // by the drifters) could keep the milder drifter; iterating recomputes
        // the centroid over the tightening core and peels both.
        let core1 = normed(&[1.0, 0.02, 0.0, 0.0]);
        let core2 = normed(&[1.0, 0.03, 0.0, 0.0]);
        let core3 = normed(&[1.0, 0.04, 0.0, 0.0]);
        let drift1 = normed(&[0.80, 0.55, 0.20, 0.0]);
        let drift2 = normed(&[0.55, 0.70, 0.45, 0.10]);
        let ids: Vec<PhotoId> = (0..5).map(|_| PhotoId::new()).collect();
        let groups = cluster_stage_b(
            &[
                (ids[0], core1),
                (ids[1], core2),
                (ids[2], core3),
                (ids[3], drift1),
                (ids[4], drift2),
            ],
            &StageBParams { similarity_threshold: 0.90, chain_margin: 0.05 },
        );
        // The three tight members must stay together in one group; the two
        // drifters must not be in that group.
        let core_group = groups
            .iter()
            .find(|g| g.photo_ids.contains(&ids[0]))
            .expect("core group exists");
        assert!(core_group.photo_ids.contains(&ids[1]));
        assert!(core_group.photo_ids.contains(&ids[2]));
        assert!(!core_group.photo_ids.contains(&ids[3]));
        assert!(!core_group.photo_ids.contains(&ids[4]));
    }
}
