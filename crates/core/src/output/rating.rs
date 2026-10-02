//! Star ratings from aesthetic scores, for XMP sidecars.
//!
//! Selection only ever compares photos *within* a group, so a trip of mostly
//! one-off shots (every group a singleton, every photo kept) gets no help from
//! the aesthetic score. Writing it out as an `xmp:Rating` lets the user filter
//! "the best of this trip" in Lightroom / Capture One / digiKam instead.
//!
//! Stars are **relative to the run** (percentile bands), not absolute
//! thresholds: "★4 = top 15 % of what you shot" is what a cull filter wants,
//! and it doesn't depend on the exact calibration of the score.

use std::collections::HashMap;
use std::hash::Hash;

/// `(upper percentile bound, stars)`, best first. Percentile 0 is the best
/// photo in the run and 1 the worst; anything past the last bound gets 1★.
/// 5★ is deliberately left for the user's own flags.
const STAR_BANDS: [(f32, u8); 3] = [(0.15, 4), (0.45, 3), (0.80, 2)];

/// Map each photo's aesthetic score to 1–4 stars by its rank in `scores`.
///
/// Equal scores share a rank (the best of the tied positions), so identical
/// photos never land in different bands. Non-finite scores are ignored. A
/// single scored photo gets 3★ — there's nothing to rank it against.
pub fn aesthetic_star_ratings<K: Copy + Eq + Hash>(scores: &[(K, f32)]) -> HashMap<K, u8> {
    let mut ranked: Vec<(K, f32)> = scores.iter().copied().filter(|(_, s)| s.is_finite()).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let n = ranked.len();
    let mut out = HashMap::with_capacity(n);
    if n == 1 {
        out.insert(ranked[0].0, 3);
        return out;
    }
    let mut rank = 0usize;
    for (i, (id, score)) in ranked.iter().enumerate() {
        if i > 0 && *score < ranked[i - 1].1 {
            rank = i;
        }
        let pct = rank as f32 / (n - 1) as f32;
        let stars = STAR_BANDS
            .iter()
            .find(|(bound, _)| pct < *bound)
            .map_or(1, |(_, stars)| *stars);
        out.insert(*id, stars);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_split_a_large_run_by_percentile() {
        // 100 photos with distinct scores, id = position from best (0) to worst (99).
        let scores: Vec<(usize, f32)> = (0..100).map(|i| (i, 1.0 - i as f32 / 100.0)).collect();
        let r = aesthetic_star_ratings(&scores);
        let count = |s: u8| r.values().filter(|&&v| v == s).count();
        assert_eq!(r[&0], 4);
        assert_eq!(r[&99], 1);
        // pct = i/99: <0.15 → i ≤ 14, <0.45 → i ≤ 44, <0.80 → i ≤ 79.
        assert_eq!((count(4), count(3), count(2), count(1)), (15, 30, 35, 20));
    }

    #[test]
    fn ties_share_a_band_and_order_is_input_independent() {
        let r = aesthetic_star_ratings(&[("a", 0.9), ("b", 0.2), ("c", 0.9), ("d", 0.5)]);
        assert_eq!(r["a"], r["c"]);
        assert_eq!(r["a"], 4);
        assert_eq!(r["b"], 1);
        let shuffled = aesthetic_star_ratings(&[("d", 0.5), ("c", 0.9), ("b", 0.2), ("a", 0.9)]);
        assert_eq!(r, shuffled);
    }

    #[test]
    fn small_and_degenerate_inputs() {
        assert!(aesthetic_star_ratings::<u8>(&[]).is_empty());
        assert_eq!(aesthetic_star_ratings(&[(1, 0.7)])[&1], 3);
        let two = aesthetic_star_ratings(&[(1, 0.7), (2, 0.6)]);
        assert_eq!((two[&1], two[&2]), (4, 1));
        let nan = aesthetic_star_ratings(&[(1, f32::NAN), (2, 0.6), (3, 0.4)]);
        assert!(!nan.contains_key(&1));
        assert_eq!(nan.len(), 2);
    }
}
