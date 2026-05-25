use super::{unionfind::UnionFind, Group, GroupId};
use crate::features::{hash::hamming, PhotoFeatures};
use crate::ingest::{PhotoId, PhotoRef};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct StageAParams {
    /// Time-window scaling factor: `Δt = clamp(k_time · median_dt, min_dt, max_dt)`.
    pub k_time: f32,
    /// Lower bound for the time window (avoids absurdly tight windows on fast bursts).
    pub min_dt: Duration,
    /// Upper bound for the time window (avoids the small-sample case where a single
    /// large gap inflates the median and merges unrelated photos).
    pub max_dt: Duration,
    /// pHash Hamming distance fallback threshold (used only when CLIP embeddings
    /// aren't available for a pair).
    pub max_hash_dist: u32,
    /// CLIP cosine similarity threshold for "this really is the same burst"
    /// — strictly tighter than Stage B's threshold because true burst frames
    /// are nearly identical. Used preferentially over pHash when CLIP embeds
    /// are present (which they always are once M3.2 is on).
    pub clip_threshold: f32,
}

impl Default for StageAParams {
    fn default() -> Self {
        Self {
            k_time: 3.0,
            min_dt: Duration::from_millis(300),
            max_dt: Duration::from_secs(30),
            max_hash_dist: 6,
            clip_threshold: 0.95,
        }
    }
}

/// Cluster near-duplicate photos by time-window + perceptual hash distance.
///
/// Photos without `captured_at` each become their own singleton group.
pub fn cluster_stage_a(
    photos: &[PhotoRef],
    features: &HashMap<PhotoId, PhotoFeatures>,
    params: &StageAParams,
) -> Vec<Group> {
    // Partition by whether we have a timestamp.
    let (mut timed, untimed): (Vec<&PhotoRef>, Vec<&PhotoRef>) = photos
        .iter()
        .partition(|p| p.captured_at.is_some());

    timed.sort_by_key(|p| p.captured_at.unwrap());

    let mut groups: Vec<Group> = Vec::new();

    if !timed.is_empty() {
        let delta_t = compute_delta_t(&timed, params);
        let mut uf = UnionFind::new(timed.len());

        for i in 0..timed.len().saturating_sub(1) {
            let a = timed[i];
            let b = timed[i + 1];

            // BurstID match forces grouping regardless of time/hash.
            if let (Some(ba), Some(bb)) = (&a.burst_id, &b.burst_id) {
                if ba == bb {
                    uf.union(i, i + 1);
                    continue;
                }
            }

            let dt = seconds_between(a, b);
            if dt > delta_t {
                continue;
            }

            let (Some(fa), Some(fb)) = (features.get(&a.id), features.get(&b.id)) else {
                continue;
            };
            // Prefer CLIP cosine similarity — pHash gives too many false
            // positives (high-contrast scenes hash similarly even when their
            // content is unrelated, and pHash similarity isn't transitive so
            // chained adjacent merges can drag in visually different photos).
            let should_merge = match (&fa.clip_embed, &fb.clip_embed) {
                (Some(ea), Some(eb)) if ea.len() == eb.len() => {
                    cosine_normalized(ea, eb) > params.clip_threshold
                }
                _ => hamming(fa.phash, fb.phash) <= params.max_hash_dist,
            };
            if should_merge {
                uf.union(i, i + 1);
            }
        }

        let mut buckets: HashMap<usize, Vec<PhotoId>> = HashMap::new();
        for (i, p) in timed.iter().enumerate() {
            buckets.entry(uf.find(i)).or_default().push(p.id);
        }
        for ids in buckets.into_values() {
            let representative = ids[0];
            groups.push(Group {
                id: GroupId::new(),
                photo_ids: ids,
                representative,
            });
        }
    }

    for p in untimed {
        groups.push(Group::singleton(p.id));
    }

    groups
}

fn compute_delta_t(timed_sorted: &[&PhotoRef], params: &StageAParams) -> f32 {
    let deltas: Vec<f32> = timed_sorted
        .windows(2)
        .map(|w| seconds_between(w[0], w[1]))
        .collect();
    let median_dt = median(&deltas).unwrap_or(2.0);
    let proposed = params.k_time * median_dt;
    proposed
        .max(params.min_dt.as_secs_f32())
        .min(params.max_dt.as_secs_f32())
}

/// Dot product on L2-normalized vectors == cosine similarity.
fn cosine_normalized(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn seconds_between(a: &PhotoRef, b: &PhotoRef) -> f32 {
    let ta = a.captured_at.unwrap();
    let tb = b.captured_at.unwrap();
    let ms = (tb - ta).num_milliseconds();
    ms as f32 / 1000.0
}

fn median(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::{ImageFormat, PhotoId};
    use chrono::{TimeZone, Utc};

    fn mk_photo(id: PhotoId, t_ms: i64) -> PhotoRef {
        PhotoRef {
            id,
            path: format!("/fake/{}.jpg", id).into(),
            format: ImageFormat::Jpeg,
            captured_at: Some(Utc.timestamp_millis_opt(t_ms).unwrap()),
            file_size: 0,
            sha256_short: [0; 16],
            burst_id: None,
            drive_mode: None,
            iso: None,
            exposure_bias_ev: None,
        }
    }

    fn mk_feat(id: PhotoId, phash: u64) -> PhotoFeatures {
        PhotoFeatures::hashes_only(id, phash, 0)
    }

    #[test]
    fn merges_tight_burst_with_similar_hashes() {
        let ids: Vec<PhotoId> = (0..5).map(|_| PhotoId::new()).collect();
        let photos: Vec<PhotoRef> = ids.iter().enumerate()
            .map(|(i, id)| mk_photo(*id, (i as i64) * 100)) // 100ms apart
            .collect();
        let features: HashMap<PhotoId, PhotoFeatures> =
            ids.iter().map(|id| (*id, mk_feat(*id, 0xFF))).collect();

        let groups = cluster_stage_a(&photos, &features, &StageAParams::default());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].photo_ids.len(), 5);
    }

    #[test]
    fn splits_when_time_gap_too_large() {
        let id_a = PhotoId::new();
        let id_b = PhotoId::new();
        let photos = vec![mk_photo(id_a, 0), mk_photo(id_b, 60_000)];
        let features: HashMap<_, _> =
            [(id_a, mk_feat(id_a, 0)), (id_b, mk_feat(id_b, 0))].into();

        let groups = cluster_stage_a(&photos, &features, &StageAParams::default());
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn splits_when_hash_differs() {
        let id_a = PhotoId::new();
        let id_b = PhotoId::new();
        let photos = vec![mk_photo(id_a, 0), mk_photo(id_b, 100)];
        let features: HashMap<_, _> = [
            (id_a, mk_feat(id_a, 0x0000_0000_0000_0000)),
            (id_b, mk_feat(id_b, 0xFFFF_FFFF_FFFF_FFFF)), // 64 bit difference
        ]
        .into();

        let groups = cluster_stage_a(&photos, &features, &StageAParams::default());
        assert_eq!(groups.len(), 2);
    }
}
