//! Technical quality scoring (M2): exposure, white balance, sharpness, noise.
//!
//! All scorers operate on a 1024px luma thumbnail. `exposure`, `wb`, and `noise`
//! are absolute and self-normalize to `[0, 1]`. `sharpness` returns a raw signal
//! that the pipeline normalizes *within a group* via z-score → sigmoid — "the
//! sharpest one in this burst" matters more than absolute Laplacian variance,
//! which depends on subject and lighting.

pub mod aesthetic;
pub mod composition;
pub mod exposure;
pub mod face;
#[cfg(feature = "onnx")]
pub mod face_yunet;
pub mod noise;
pub mod scene;
pub mod sharpness;
pub mod wb;

pub use aesthetic::{AestheticScorer, HeuristicAestheticScorer, NeutralAestheticStub};
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

/// Plan B.7 face bonus:
///   `face_quality_f = 0.3·size + 0.4·eye + 0.2·sharp + 0.1·smile`
///   `face_bonus = mean(face_quality_f) · coverage`
/// `coverage` is 1.0 when at least `ceil(N·0.8)` faces have `eye_open > 0.5`,
/// otherwise the fraction of open-eye faces (avoids hard-zeroing big groups).
/// Faces whose detector doesn't report an eye-open probability (e.g. YuNet)
/// count as open, so coverage isn't forced to zero by missing data.
pub fn face_bonus_score(face: &FaceInfo) -> f32 {
    let n = face.count();
    if n == 0 {
        return 0.0;
    }
    let mut total = 0.0_f32;
    let mut open_count = 0;
    for f in &face.faces {
        let size = (f.area_ratio() / 0.05).clamp(0.0, 1.0);
        let eye = f.eye_open_prob.unwrap_or(0.5);
        let smile = f.smile_prob.unwrap_or(0.5);
        let sharp = f.local_sharpness.unwrap_or(0.5);
        let q = 0.3 * size + 0.4 * eye.clamp(0.0, 1.0) + 0.2 * sharp.clamp(0.0, 1.0) + 0.1 * smile.clamp(0.0, 1.0);
        total += q;
        // Unknown eye state (detector doesn't provide one, e.g. YuNet) must not
        // count as closed — otherwise coverage hard-zeros the whole bonus.
        if f.eye_open_prob.map_or(true, |p| p > 0.5) {
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
    let scene = classify_scene(face);
    let w = FinalWeights::for_scene(scene);
    let fb = face_bonus_score(face);
    let value =
        (w.tech * tech + w.aesthetic * aesthetic + w.composition * composition + w.face_bonus * fb)
            .clamp(0.0, 1.0);
    FinalScore {
        scene,
        tech,
        aesthetic,
        composition,
        face_bonus: fb,
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
    let sigma = var.sqrt().max(1e-6);

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

/// For each composition group, compute the final scene-aware score and pick K2.
pub fn select_top_k_per_composition(
    composition_groups: &[CompositionGroup],
    features: &HashMap<PhotoId, PhotoFeatures>,
    tech_scores: &HashMap<PhotoId, TechScore>,
    k2: usize,
) -> Vec<CompositionPick> {
    composition_groups
        .iter()
        .map(|cg| {
            let mut ranked: Vec<(PhotoId, FinalScore)> = cg
                .photo_ids
                .iter()
                .filter_map(|pid| {
                    let f = features.get(pid)?;
                    let t = tech_scores.get(pid)?;
                    let aesthetic = f.aesthetic.unwrap_or(0.5);
                    let composition = f.composition.unwrap_or(0.5);
                    let face = f.face.clone().unwrap_or_default();
                    let fs = compute_final_score(t.tech, aesthetic, composition, &face);
                    Some((*pid, fs))
                })
                .collect();
            ranked.sort_by(|a, b| b.1.value.partial_cmp(&a.1.value).unwrap_or(Ordering::Equal));
            let k = k2.min(ranked.len());
            let rejected = ranked.split_off(k);
            CompositionPick {
                group: cg.clone(),
                kept: ranked,
                rejected,
            }
        })
        .collect()
}

/// For each group, rank photos by tech score and split into kept/rejected at K1.
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
        let raws: Vec<(PhotoId, RawTechScores)> = group
            .photo_ids
            .iter()
            .filter_map(|pid| {
                features
                    .get(pid)
                    .and_then(|f| f.raw_tech_scores())
                    .map(|r| (*pid, r))
            })
            .collect();

        if raws.is_empty() {
            // No scoring possible — keep the group's photos unscored, none rejected.
            out.push(SelectedGroup {
                group: group.clone(),
                kept: vec![],
                rejected: vec![],
            });
            continue;
        }

        let tech_map = finalize_group(&raws, weights);
        let mut ranked: Vec<(PhotoId, TechScore)> = raws
            .iter()
            .map(|(id, _)| (*id, tech_map[id]))
            .collect();
        ranked.sort_by(|a, b| b.1.tech.partial_cmp(&a.1.tech).unwrap_or(Ordering::Equal));

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
}
