//! Technical quality scoring (M2): exposure, white balance, sharpness, noise.
//!
//! All scorers operate on a 1024px luma thumbnail. `exposure`, `wb`, and `noise`
//! are absolute and self-normalize to `[0, 1]`. `sharpness` returns a raw signal
//! that the pipeline normalizes *within a group* via z-score → sigmoid — "the
//! sharpest one in this burst" matters more than absolute Laplacian variance,
//! which depends on subject and lighting.

pub mod aesthetic;
mod aesthetic_laion_weights;
pub mod composition;
pub mod eye_state;
pub mod exposure;
pub mod face;
#[cfg(feature = "onnx")]
pub mod face_yunet;
pub mod noise;
pub mod scene;
pub mod sharpness;
pub mod wb;

pub use aesthetic::{
    laion_raw_to_unit, learned_aesthetic, AestheticScorer, HeuristicAestheticScorer,
    NeutralAestheticStub,
};
pub use composition::{CompositionScorer, HeuristicCompositionScorer, NeutralCompositionStub};
pub use face::{FaceBox, FaceDetector, FaceInfo, NoFaceDetectorStub};
#[cfg(feature = "onnx")]
pub use face_yunet::YunetFaceDetector;
pub use scene::{classify_scene, FinalWeights, Scene};

use crate::features::PhotoFeatures;
use crate::group::{CompositionGroup, Group};
use crate::ingest::{PhotoId, PhotoRef};
use image::DynamicImage;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RawTechScores {
    pub exposure: f32,
    pub wb: f32,
    /// Raw multi-ROI sharpness signal; not yet normalized.
    pub sharpness_raw: f32,
    pub noise: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TechWeights {
    pub w_exposure: f32,
    pub w_wb: f32,
    pub w_sharpness: f32,
    pub w_noise: f32,
}

impl Default for TechWeights {
    fn default() -> Self {
        // Plan B.8: tech = 0.30·exp + 0.20·wb + 0.35·sharp + 0.15·noise
        Self {
            w_exposure: 0.30,
            w_wb: 0.20,
            w_sharpness: 0.35,
            w_noise: 0.15,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TechScore {
    pub exposure: f32,
    pub wb: f32,
    /// Group-normalized sharpness in `[0, 1]`.
    pub sharpness: f32,
    pub noise: f32,
    pub tech: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FinalScore {
    pub scene: Scene,
    pub tech: f32,
    pub aesthetic: f32,
    pub composition: f32,
    pub face_bonus: f32,
    /// Final weighted score in `[0, 1]`.
    pub value: f32,
}

/// Plan B.7 face bonus. Per-face quality is a weighted average over the signals
/// the detector actually provides — `size` (always), plus normalized local
/// `sharp`, `eye`-open, and `smile` when available (base weights 0.3/0.2/0.4/0.1).
/// Unknown signals are *excluded* rather than defaulted to 0.5, so a detector
/// that only reports bbox + size (e.g. current YuNet) yields a size-driven
/// bonus instead of one diluted toward a constant.
///
/// `face_bonus = mean(face_quality) · coverage`, where `coverage` is 1.0 when at
/// least `ceil(N·0.8)` faces have `eye_open > 0.5`, else the open-eye fraction
/// (avoids hard-zeroing big groups). Faces with no eye-open probability count as
/// open, so coverage isn't forced to zero by missing data.
pub fn face_bonus_score(face: &FaceInfo) -> f32 {
    let sharp: Vec<Option<f32>> = face.faces.iter().map(|f| f.local_sharpness).collect();
    face_bonus_score_with_sharp(face, &sharp)
}

/// An eye-open probability below this counts as *confidently* closed for the
/// coverage gate. The OCEC classifier is sharp (near 0 or 1), but the
/// fallback heuristic (Laplacian energy around an eye keypoint, used when the
/// model is unavailable or the face is tiny) is uncalibrated: a soft or small
/// but genuinely open eye can land in the ambiguous 0.3–0.5 band, and treating
/// that as "closed" would hard-cut the whole bonus via coverage. Only a
/// clearly-low estimate triggers the penalty.
const EYE_CLOSED_CONFIDENCE: f32 = 0.3;

/// Like [`face_bonus_score`] but with per-face local sharpness supplied
/// externally (already normalized to `[0,1]`), so a caller with group context
/// can z-score face sharpness across the group instead of using the raw,
/// unbounded Laplacian variance which would saturate the `[0,1]` clamp.
fn face_bonus_score_with_sharp(face: &FaceInfo, norm_sharp: &[Option<f32>]) -> f32 {
    let n = face.count();
    if n == 0 {
        return 0.0;
    }
    let mut total = 0.0_f32;
    let mut open_count = 0;
    for (i, f) in face.faces.iter().enumerate() {
        let size = (f.area_ratio() / 0.05).clamp(0.0, 1.0);
        // Weighted average over present signals only (size is always present).
        let mut num = 0.3 * size;
        let mut den = 0.3_f32;
        if let Some(s) = norm_sharp.get(i).copied().flatten() {
            num += 0.2 * s.clamp(0.0, 1.0);
            den += 0.2;
        }
        if let Some(e) = f.eye_open_prob {
            num += 0.4 * e.clamp(0.0, 1.0);
            den += 0.4;
        }
        if let Some(sm) = f.smile_prob {
            num += 0.1 * sm.clamp(0.0, 1.0);
            den += 0.1;
        }
        total += num / den;
        // Coverage gate: only a *confidently* closed eye counts against
        // coverage. Unknown eye state (None) and ambiguous-but-not-low estimates
        // count as open, so a noisy eye-open heuristic can't hard-zero the bonus.
        if !f.eye_open_prob.is_some_and(|p| p < EYE_CLOSED_CONFIDENCE) {
            open_count += 1;
        }
    }
    let need = ((n as f32) * 0.8).ceil() as usize;
    let coverage = if open_count >= need { 1.0 } else { open_count as f32 / n as f32 };
    (total / n as f32 * coverage).clamp(0.0, 1.0)
}

pub fn compute_final_score(
    tech: f32,
    aesthetic: f32,
    composition: f32,
    face: &FaceInfo,
) -> FinalScore {
    let fb = face_bonus_score(face);
    final_score_from_parts(tech, aesthetic, composition, face, fb)
}

/// Blend the four scene-weighted components into a `FinalScore`. Separated from
/// [`compute_final_score`] so callers that compute `face_bonus` with group
/// context (see [`score_group_final`]) can reuse the weighting.
fn final_score_from_parts(
    tech: f32,
    aesthetic: f32,
    composition: f32,
    face: &FaceInfo,
    face_bonus: f32,
) -> FinalScore {
    let scene = classify_scene(face);
    // `for_face` matches `for_scene(classify_scene)` outside the 2–5 % soft
    // band, so the canonical `scene` label stays meaningful for the report.
    let w = FinalWeights::for_face(face);
    let value = (w.tech * tech
        + w.aesthetic * aesthetic
        + w.composition * composition
        + w.face_bonus * face_bonus)
        .clamp(0.0, 1.0);
    FinalScore {
        scene,
        tech,
        aesthetic,
        composition,
        face_bonus,
        value,
    }
}

pub fn compute_raw_scores(thumb: &DynamicImage, photo: &PhotoRef) -> RawTechScores {
    // Convert to luma once and share across the three luma-based scorers
    // (exposure, sharpness, noise) instead of each re-deriving it.
    let gray = thumb.to_luma8();
    RawTechScores {
        exposure: exposure::score(&gray, photo.exposure_bias_ev),
        wb: wb::score(thumb),
        sharpness_raw: sharpness::raw(&gray),
        noise: noise::score(&gray, photo.iso),
    }
}

/// Normalize raw sharpness within a group (z-score → sigmoid) and compute the
/// weighted tech score for each photo in the group.
pub fn finalize_group(
    raw_per_photo: &[(PhotoId, RawTechScores)],
    weights: &TechWeights,
) -> HashMap<PhotoId, TechScore> {
    let n = raw_per_photo.len();
    if n == 0 {
        return HashMap::new();
    }

    let n_f = n as f32;
    let mu = raw_per_photo.iter().map(|(_, r)| r.sharpness_raw).sum::<f32>() / n_f;
    let var = raw_per_photo
        .iter()
        .map(|(_, r)| (r.sharpness_raw - mu).powi(2))
        .sum::<f32>()
        / n_f;
    let sigma = norm_sigma(var.sqrt(), mu);

    raw_per_photo
        .iter()
        .map(|(id, r)| {
            // Singletons land at 0.5 (no within-group contrast available).
            let sharpness = if n == 1 { 0.5 } else { sigmoid((r.sharpness_raw - mu) / sigma) };
            let tech = (weights.w_exposure * r.exposure
                + weights.w_wb * r.wb
                + weights.w_sharpness * sharpness
                + weights.w_noise * r.noise)
                .clamp(0.0, 1.0);
            (
                *id,
                TechScore {
                    exposure: r.exposure,
                    wb: r.wb,
                    sharpness,
                    noise: r.noise,
                    tech,
                },
            )
        })
        .collect()
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Smallest spread, as a fraction of the group mean, that within-group
/// normalization will assume. Burst frames are often equally sharp; their
/// sharpness values then differ by a few percent of pure measurement noise,
/// and a bare z-score would stretch that into a 0.27-vs-0.73 split that
/// decides the pick. With this floor a 3 % difference stays a ~0.05 nudge,
/// while a genuinely blurred frame (typically 50 %+ lower) still drops hard.
const RELATIVE_SIGMA_FLOOR: f32 = 0.15;

/// Spread used for within-group z-scores: the observed standard deviation,
/// but never less than [`RELATIVE_SIGMA_FLOOR`] of the mean.
fn norm_sigma(std: f32, mean: f32) -> f32 {
    std.max(RELATIVE_SIGMA_FLOOR * mean.abs()).max(1e-6)
}

/// z-score → sigmoid normalization of `x` against a population `(mu, sigma)`.
fn zsig(x: f32, mu: f32, sigma: f32) -> f32 {
    sigmoid((x - mu) / sigma)
}

/// Per-photo input to [`score_group_final`]: raw tech signals plus the absolute
/// aesthetic/composition scores and the detected faces.
pub type GroupMember = (PhotoId, RawTechScores, f32, f32, FaceInfo);

/// Compute the scene-aware `FinalScore` for every photo in a group, normalizing
/// the *relative* signals (global sharpness and per-face local sharpness) WITHIN
/// the group. Used at both selection stages so the same blended score decides
/// the burst keeper (K1) and the composition keeper (K2); the only difference is
/// the group the relative signals are normalized against (burst vs composition).
///
/// Also returns each photo's `TechScore` so K1 can keep storing it for reports
/// without recomputing.
pub fn score_group_final(
    members: &[GroupMember],
    weights: &TechWeights,
) -> HashMap<PhotoId, (TechScore, FinalScore)> {
    let n = members.len();
    if n == 0 {
        return HashMap::new();
    }

    // Global sharpness: z-score → sigmoid within the group.
    let n_f = n as f32;
    let mu = members.iter().map(|(_, r, ..)| r.sharpness_raw).sum::<f32>() / n_f;
    let var =
        members.iter().map(|(_, r, ..)| (r.sharpness_raw - mu).powi(2)).sum::<f32>() / n_f;
    let sigma = norm_sigma(var.sqrt(), mu);

    // Per-face local sharpness: pool every face's raw Laplacian variance across
    // the whole group and normalize that pool. Raw variance is unbounded, so
    // per-photo normalization would just saturate the [0,1] clamp.
    let pool: Vec<f32> = members
        .iter()
        .flat_map(|(_, _, _, _, face)| face.faces.iter().filter_map(|f| f.local_sharpness))
        .collect();
    let face_norm = if pool.len() >= 2 {
        let m = pool.iter().sum::<f32>() / pool.len() as f32;
        let v = pool.iter().map(|x| (x - m).powi(2)).sum::<f32>() / pool.len() as f32;
        Some((m, norm_sigma(v.sqrt(), m)))
    } else {
        None
    };

    members
        .iter()
        .map(|(id, r, aesthetic, composition, face)| {
            let sharpness = if n == 1 { 0.5 } else { zsig(r.sharpness_raw, mu, sigma) };
            let tech = (weights.w_exposure * r.exposure
                + weights.w_wb * r.wb
                + weights.w_sharpness * sharpness
                + weights.w_noise * r.noise)
                .clamp(0.0, 1.0);
            let ts = TechScore { exposure: r.exposure, wb: r.wb, sharpness, noise: r.noise, tech };

            let norm_sharp: Vec<Option<f32>> = face
                .faces
                .iter()
                .map(|f| {
                    f.local_sharpness.map(|v| match face_norm {
                        Some((m, s)) => zsig(v, m, s),
                        None => 0.5,
                    })
                })
                .collect();
            let fb = face_bonus_score_with_sharp(face, &norm_sharp);
            let fs = final_score_from_parts(tech, *aesthetic, *composition, face, fb);
            (*id, (ts, fs))
        })
        .collect()
}

/// Pull a group's [`GroupMember`]s out of the feature map (skipping photos whose
/// technical scores never got computed).
fn collect_members(
    photo_ids: &[PhotoId],
    features: &HashMap<PhotoId, PhotoFeatures>,
) -> Vec<GroupMember> {
    photo_ids
        .iter()
        .filter_map(|pid| {
            let f = features.get(pid)?;
            let r = f.raw_tech_scores()?;
            Some((
                *pid,
                r,
                f.aesthetic.unwrap_or(0.5),
                f.composition.unwrap_or(0.5),
                f.face.clone().unwrap_or_default(),
            ))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct SelectedGroup {
    /// The original Stage A cluster with every photo it contained.
    pub group: Group,
    /// Top-K1 photos sorted by tech score (descending). First entry becomes the
    /// new representative.
    pub kept: Vec<(PhotoId, TechScore)>,
    /// Remaining photos sorted by tech score (descending) — kept for the report.
    pub rejected: Vec<(PhotoId, TechScore)>,
}

#[derive(Debug, Clone)]
pub struct CompositionPick {
    pub group: CompositionGroup,
    /// Top-K2 photos by final score (descending). First entry is the new
    /// "representative" pick.
    pub kept: Vec<(PhotoId, FinalScore)>,
    /// Stage A kept photos that lost the composition contest to a better
    /// peer in the same Stage B group.
    pub rejected: Vec<(PhotoId, FinalScore)>,
}

/// K2 selection policy. `Fixed(k)` keeps a constant K2 across every
/// composition group. `Auto` adapts per-group: keep the top photo, plus
/// any subsequent photo whose score is within `AUTO_K2_TOP_RATIO` of the
/// best AND whose drop from the previous rank is within `AUTO_K2_STEP_DROP`
/// (relative to the top score). Capped at `AUTO_K2_MAX`.
#[derive(Debug, Clone, Copy)]
pub enum K2Policy {
    Fixed(usize),
    Auto,
}

/// Within `AUTO_K2_TOP_RATIO` of the top score → still a contender.
const AUTO_K2_TOP_RATIO: f32 = 0.92;
/// Successive-rank score drop (relative to the top score) above which the
/// "next" photo is treated as clearly worse and we stop including more.
const AUTO_K2_STEP_DROP: f32 = 0.05;
/// Hard cap to avoid runaway in pathological groups where every photo
/// scores ~identically.
const AUTO_K2_MAX: usize = 5;

/// For each composition group, compute the final scene-aware score and pick K2.
///
/// Sharpness (global and per-face) is re-normalized WITHIN the composition group
/// here — not carried over from the Stage A burst normalization — so photos from
/// different bursts are compared on a consistent scale.
pub fn select_top_k_per_composition(
    composition_groups: &[CompositionGroup],
    features: &HashMap<PhotoId, PhotoFeatures>,
    policy: K2Policy,
    weights: &TechWeights,
) -> Vec<CompositionPick> {
    composition_groups
        .iter()
        .map(|cg| {
            let members = collect_members(&cg.photo_ids, features);
            let scored = score_group_final(&members, weights);
            let mut ranked: Vec<(PhotoId, FinalScore)> =
                members.iter().map(|(id, ..)| (*id, scored[id].1)).collect();
            ranked.sort_by(|a, b| b.1.value.partial_cmp(&a.1.value).unwrap_or(Ordering::Equal));
            let k = match policy {
                K2Policy::Fixed(k2) => k2.min(ranked.len()),
                K2Policy::Auto => auto_k(&ranked),
            };
            let rejected = ranked.split_off(k);
            CompositionPick {
                group: cg.clone(),
                kept: ranked,
                rejected,
            }
        })
        .collect()
}

/// Auto-K policy: always keep ≥1 photo. Then walk down the sorted list,
/// keeping each photo while it's within `AUTO_K2_TOP_RATIO` of the top AND
/// the per-rank drop from its predecessor stays within `AUTO_K2_STEP_DROP`
/// of the top. Capped at `AUTO_K2_MAX`.
fn auto_k(ranked: &[(PhotoId, FinalScore)]) -> usize {
    if ranked.is_empty() {
        return 0;
    }
    let top = ranked[0].1.value.max(1e-6);
    let mut k = 1usize;
    while k < ranked.len().min(AUTO_K2_MAX) {
        let cur = ranked[k].1.value;
        let prev = ranked[k - 1].1.value;
        let close_to_top = cur / top >= AUTO_K2_TOP_RATIO;
        let small_step = (prev - cur) / top <= AUTO_K2_STEP_DROP;
        if close_to_top && small_step {
            k += 1;
        } else {
            break;
        }
    }
    k
}

/// For each burst, rank photos by the full scene-aware final score (sharpness
/// normalized within the burst) and split into kept/rejected at K1. Aesthetic,
/// composition, and face quality therefore influence the burst keeper instead of
/// it being chosen on technical merit alone. The stored `TechScore` is unchanged
/// so reports keep working; only the ranking key differs.
///
/// Photos missing tech scores (e.g. feature extraction failed) fall through with
/// the group untouched.
pub fn select_top_k_per_group(
    groups: &[Group],
    features: &HashMap<PhotoId, PhotoFeatures>,
    k1: usize,
    weights: &TechWeights,
) -> Vec<SelectedGroup> {
    let mut out = Vec::with_capacity(groups.len());
    for group in groups {
        let members = collect_members(&group.photo_ids, features);

        if members.is_empty() {
            // No scoring possible — keep the group's photos unscored, none rejected.
            out.push(SelectedGroup {
                group: group.clone(),
                kept: vec![],
                rejected: vec![],
            });
            continue;
        }

        let scored = score_group_final(&members, weights);
        let mut ranked: Vec<(PhotoId, TechScore)> =
            members.iter().map(|(id, ..)| (*id, scored[id].0)).collect();
        // Rank by the blended final value, not tech alone.
        ranked.sort_by(|a, b| {
            scored[&b.0]
                .1
                .value
                .partial_cmp(&scored[&a.0].1.value)
                .unwrap_or(Ordering::Equal)
        });

        let k = k1.min(ranked.len());
        let rejected = ranked.split_off(k);

        let mut updated = group.clone();
        if let Some((first, _)) = ranked.first() {
            updated.representative = *first;
        }

        out.push(SelectedGroup {
            group: updated,
            kept: ranked,
            rejected,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(sharp: f32) -> RawTechScores {
        RawTechScores { exposure: 0.8, wb: 0.8, sharpness_raw: sharp, noise: 0.8 }
    }

    #[test]
    fn singleton_sharpness_is_neutral() {
        let id = PhotoId::new();
        let m = finalize_group(&[(id, raw(123.4))], &TechWeights::default());
        assert!((m[&id].sharpness - 0.5).abs() < 1e-3);
    }

    fn face(area: f32, eye_open_prob: Option<f32>) -> FaceBox {
        let side = area.sqrt();
        FaceBox {
            x: 0.0,
            y: 0.0,
            w: side,
            h: side,
            eye_open_prob,
            smile_prob: None,
            local_sharpness: None,
        }
    }

    #[test]
    fn face_bonus_nonzero_when_eye_state_unknown() {
        // YuNet leaves eye_open_prob = None; a detected face must still earn a
        // bonus instead of being zeroed by the coverage gate.
        let info = FaceInfo { faces: vec![face(0.1, None)] };
        assert!(face_bonus_score(&info) > 0.0);
    }

    #[test]
    fn face_bonus_zero_without_faces() {
        assert_eq!(face_bonus_score(&FaceInfo::default()), 0.0);
    }

    #[test]
    fn face_bonus_penalizes_closed_eyes() {
        let open = FaceInfo { faces: vec![face(0.1, Some(0.9)), face(0.1, Some(0.9))] };
        let one_closed = FaceInfo { faces: vec![face(0.1, Some(0.9)), face(0.1, Some(0.1))] };
        assert!(face_bonus_score(&one_closed) < face_bonus_score(&open));
    }

    #[test]
    fn ambiguous_eye_open_is_not_hard_cut_by_coverage() {
        // A soft/small but open eye lands in the uncalibrated 0.3–0.5 band; it
        // must NOT trigger the coverage penalty the way a confidently-closed eye
        // (single face, coverage → 0 → bonus 0) does.
        let soft_open = FaceInfo { faces: vec![face(0.25, Some(0.4))] };
        let confident_closed = FaceInfo { faces: vec![face(0.25, Some(0.1))] };
        assert_eq!(face_bonus_score(&confident_closed), 0.0);
        assert!(face_bonus_score(&soft_open) > 0.0);
    }

    #[test]
    fn face_bonus_all_none_reduces_to_size() {
        // With no eye/smile/sharp signal, the bonus is purely size-driven
        // (size = area_ratio / 0.05); it must not be diluted toward a constant.
        let info = FaceInfo { faces: vec![face(0.025, None)] };
        assert!((face_bonus_score(&info) - 0.5).abs() < 1e-4);
    }

    fn member(id: PhotoId, sharp: f32, aes: f32, comp: f32, face: FaceInfo) -> GroupMember {
        (id, RawTechScores { exposure: 0.8, wb: 0.8, sharpness_raw: sharp, noise: 0.8 }, aes, comp, face)
    }

    #[test]
    fn score_group_final_normalizes_sharpness_within_group() {
        let ids: Vec<PhotoId> = (0..3).map(|_| PhotoId::new()).collect();
        let members: Vec<GroupMember> = ids
            .iter()
            .zip([10.0, 50.0, 100.0])
            .map(|(id, s)| member(*id, s, 0.5, 0.5, FaceInfo::default()))
            .collect();
        let scored = score_group_final(&members, &TechWeights::default());
        let sharp: Vec<f32> = ids.iter().map(|id| scored[id].0.sharpness).collect();
        assert!(sharp[0] < sharp[1] && sharp[1] < sharp[2], "sharpness monotone within group");
        // Landscape (no faces) → value rises with tech (aes/comp constant).
        let val: Vec<f32> = ids.iter().map(|id| scored[id].1.value).collect();
        assert!(val[0] < val[1] && val[1] < val[2]);
    }

    #[test]
    fn final_value_favors_open_eyes_over_sharper_closed_eyes() {
        // The K1 fix: a softer frame with open eyes should beat a sharper frame
        // with closed eyes within the same portrait burst.
        let sharp_closed = PhotoId::new();
        let soft_open = PhotoId::new();
        let members = vec![
            member(sharp_closed, 100.0, 0.5, 0.5, FaceInfo { faces: vec![face(0.25, Some(0.1))] }),
            member(soft_open, 10.0, 0.5, 0.5, FaceInfo { faces: vec![face(0.25, Some(0.9))] }),
        ];
        let scored = score_group_final(&members, &TechWeights::default());
        assert!(
            scored[&soft_open].1.value > scored[&sharp_closed].1.value,
            "open-eyes frame should win despite lower sharpness"
        );
    }

    /// Procedural stand-in for a photo: smooth sky gradient, hard-edged shapes
    /// and fine texture on the ground, and a small blown highlight (~3 %).
    fn synthetic_photo() -> DynamicImage {
        let (w, h) = (1024u32, 768u32);
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let mut v = 60.0 + 120.0 * (1.0 - fy);
            if ((x / 96) + (y / 96)) % 2 == 0 && fy > 0.45 {
                v -= 45.0;
            }
            if fy > 0.45 {
                // Textured "ground"; the sky above stays smooth, as in most
                // real frames (the noise estimator reads the flattest patches).
                v += 18.0 * ((x as f32 * 0.9).sin() * (y as f32 * 0.7).cos());
            }
            let (dx, dy) = (fx - 0.72, fy - 0.2);
            if dx * dx + dy * dy < 0.012 {
                v = 255.0;
            }
            let v = v.clamp(0.0, 255.0);
            image::Rgb([v as u8, (v * 0.95) as u8, (v * 0.85) as u8])
        }))
    }

    fn degrade(img: &DynamicImage, f: impl Fn(u32, u32, f32) -> f32) -> DynamicImage {
        let rgb = img.to_rgb8();
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(rgb.width(), rgb.height(), |x, y| {
            let p = rgb.get_pixel(x, y).0;
            image::Rgb(p.map(|c| f(x, y, c as f32).clamp(0.0, 255.0) as u8))
        }))
    }

    /// End-to-end on the tech signals: in a "burst" of the same frame, the
    /// clean original must outrank noisy, darker and softer copies. Before the
    /// noise-robust sharpness and graded clipping fixes, the noisy copy read
    /// as sharpest and the darkened copy as best exposed.
    #[test]
    fn clean_frame_beats_noisy_dark_and_soft_copies() {
        let photo = PhotoRef {
            id: PhotoId::new(),
            path: "x.jpg".into(),
            format: crate::ingest::ImageFormat::Jpeg,
            captured_at: None,
            file_size: 0,
            sha256_short: [0; 16],
            burst_id: None,
            drive_mode: None,
            iso: None,
            exposure_bias_ev: None,
        };
        let clean = synthetic_photo();
        // Deterministic pseudo-noise, σ≈12.
        let noisy = degrade(&clean, |x, y, c| {
            let h = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) % 1000;
            c + (h as f32 / 1000.0 - 0.5) * 41.0
        });
        let dark = degrade(&clean, |_, _, c| c * 0.6);
        let soft = image::DynamicImage::ImageRgb8(image::imageops::blur(&clean.to_rgb8(), 1.6));

        let frames = [("clean", clean), ("noisy", noisy), ("dark", dark), ("soft", soft)];
        let ids: Vec<PhotoId> = frames.iter().map(|_| PhotoId::new()).collect();
        let members: Vec<GroupMember> = frames
            .iter()
            .zip(&ids)
            .map(|((_, img), id)| (*id, compute_raw_scores(img, &photo), 0.5, 0.5, FaceInfo::default()))
            .collect();
        let scored = score_group_final(&members, &TechWeights::default());
        let clean_v = scored[&ids[0]].1.value;
        for (i, (name, _)) in frames.iter().enumerate().skip(1) {
            let v = scored[&ids[i]].1.value;
            assert!(clean_v > v + 0.01, "clean={clean_v:.3} should beat {name}={v:.3}");
        }
    }

    #[test]
    fn near_equal_sharpness_is_not_amplified_into_a_big_gap() {
        // Two equally-sharp burst frames whose measurements differ by 3 %.
        let (a, b) = (PhotoId::new(), PhotoId::new());
        let m = finalize_group(&[(a, raw(100.0)), (b, raw(103.0))], &TechWeights::default());
        let gap = m[&b].sharpness - m[&a].sharpness;
        assert!(gap > 0.0 && gap < 0.1, "3 % difference became a {gap} gap");
        // A clearly blurred frame still separates strongly.
        let (s, blur) = (PhotoId::new(), PhotoId::new());
        let m = finalize_group(&[(s, raw(100.0)), (blur, raw(40.0))], &TechWeights::default());
        assert!(m[&s].sharpness - m[&blur].sharpness > 0.4);
    }

    #[test]
    fn group_normalization_orders_by_raw_sharpness() {
        let ids: Vec<PhotoId> = (0..4).map(|_| PhotoId::new()).collect();
        let raws: Vec<(PhotoId, RawTechScores)> = ids.iter().zip([10.0, 30.0, 50.0, 100.0])
            .map(|(id, s)| (*id, raw(s))).collect();
        let m = finalize_group(&raws, &TechWeights::default());
        let s: Vec<f32> = ids.iter().map(|id| m[id].sharpness).collect();
        for w in s.windows(2) {
            assert!(w[1] >= w[0], "sharpness should be monotone with raw");
        }
        // Tech score should also be monotone since other scores are equal.
        let t: Vec<f32> = ids.iter().map(|id| m[id].tech).collect();
        for w in t.windows(2) {
            assert!(w[1] >= w[0]);
        }
    }

    fn fake_pair(value: f32) -> (PhotoId, FinalScore) {
        (
            PhotoId::new(),
            FinalScore {
                scene: Scene::Mixed,
                tech: 0.5,
                aesthetic: 0.5,
                composition: 0.5,
                face_bonus: 0.0,
                value,
            },
        )
    }

    #[test]
    fn auto_k_keeps_one_when_clear_winner() {
        let ranked = vec![fake_pair(0.9), fake_pair(0.5), fake_pair(0.4)];
        assert_eq!(auto_k(&ranked), 1);
    }

    #[test]
    fn auto_k_keeps_near_ties() {
        // 0.90, 0.88, 0.86, 0.40 — first three are within the 92% ratio AND
        // step drops are ≤5% of the top; the 0.40 outlier breaks both
        // conditions, so we stop at 3.
        let ranked = vec![fake_pair(0.90), fake_pair(0.88), fake_pair(0.86), fake_pair(0.40)];
        assert_eq!(auto_k(&ranked), 3);
    }

    #[test]
    fn auto_k_caps_at_max() {
        // 10 identical scores → cap at AUTO_K2_MAX.
        let ranked: Vec<_> = (0..10).map(|_| fake_pair(0.85)).collect();
        assert_eq!(auto_k(&ranked), AUTO_K2_MAX);
    }

    #[test]
    fn auto_k_handles_empty() {
        assert_eq!(auto_k(&[]), 0);
    }

    #[test]
    fn auto_k_singleton_keeps_one() {
        let ranked = vec![fake_pair(0.6)];
        assert_eq!(auto_k(&ranked), 1);
    }
}
