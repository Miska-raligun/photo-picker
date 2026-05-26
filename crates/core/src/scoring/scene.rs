//! Scene detection (portrait / landscape / mixed) → per-scene weight selection.
//!
//! Driven by face presence/size per design decision #2:
//! ```text
//! N ≥ 1 ∧ A ≥ 5%  → Portrait
//! N == 0          → Landscape
//! otherwise       → Mixed
//! ```

use super::FaceInfo;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scene {
    Portrait,
    Landscape,
    Mixed,
}

pub const PORTRAIT_AREA_THRESHOLD: f32 = 0.05;
/// Below this face area we treat the photo as Mixed for the discrete label.
/// Between `SOFT_PORTRAIT_AREA_MIN` and `PORTRAIT_AREA_THRESHOLD` the
/// graded-weights path linearly interpolates between Mixed and Portrait so
/// a 3 %-area face doesn't sit at the same weighting as a 0.5 % one.
pub const SOFT_PORTRAIT_AREA_MIN: f32 = 0.02;

pub fn classify_scene(face: &FaceInfo) -> Scene {
    let n = face.count();
    let a = face.max_area_ratio();
    if n >= 1 && a >= PORTRAIT_AREA_THRESHOLD {
        Scene::Portrait
    } else if n == 0 {
        Scene::Landscape
    } else {
        Scene::Mixed
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FinalWeights {
    pub tech: f32,
    pub aesthetic: f32,
    pub composition: f32,
    pub face_bonus: f32,
}

impl FinalWeights {
    pub fn for_scene(s: Scene) -> Self {
        match s {
            Scene::Portrait => Self {
                tech: 0.30,
                aesthetic: 0.20,
                composition: 0.15,
                face_bonus: 0.35,
            },
            Scene::Landscape => Self {
                tech: 0.35,
                aesthetic: 0.40,
                composition: 0.25,
                face_bonus: 0.00,
            },
            Scene::Mixed => Self {
                tech: 0.32,
                aesthetic: 0.30,
                composition: 0.20,
                face_bonus: 0.18,
            },
        }
    }

    /// Graded weights driven by actual face presence/area. Avoids the hard
    /// 5 % cliff in `classify_scene`: faces between 2 % and 5 % of the frame
    /// get weights interpolated between Mixed and Portrait. Outside that
    /// band behaves the same as `for_scene(classify_scene(face))`.
    pub fn for_face(face: &FaceInfo) -> Self {
        let n = face.count();
        if n == 0 {
            return Self::for_scene(Scene::Landscape);
        }
        let a = face.max_area_ratio();
        if a >= PORTRAIT_AREA_THRESHOLD {
            return Self::for_scene(Scene::Portrait);
        }
        if a <= SOFT_PORTRAIT_AREA_MIN {
            return Self::for_scene(Scene::Mixed);
        }
        // Linear blend Mixed → Portrait across [SOFT_PORTRAIT_AREA_MIN,
        // PORTRAIT_AREA_THRESHOLD]. t=0 → Mixed, t=1 → Portrait.
        let t = (a - SOFT_PORTRAIT_AREA_MIN)
            / (PORTRAIT_AREA_THRESHOLD - SOFT_PORTRAIT_AREA_MIN);
        let mixed = Self::for_scene(Scene::Mixed);
        let portrait = Self::for_scene(Scene::Portrait);
        Self {
            tech: mixed.tech + t * (portrait.tech - mixed.tech),
            aesthetic: mixed.aesthetic + t * (portrait.aesthetic - mixed.aesthetic),
            composition: mixed.composition + t * (portrait.composition - mixed.composition),
            face_bonus: mixed.face_bonus + t * (portrait.face_bonus - mixed.face_bonus),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::FaceBox;

    fn face(area: f32) -> FaceBox {
        let s = area.sqrt();
        FaceBox { x: 0.5 - s / 2.0, y: 0.5 - s / 2.0, w: s, h: s,
                  eye_open_prob: None, smile_prob: None, local_sharpness: None }
    }

    #[test]
    fn no_face_is_landscape() {
        assert_eq!(classify_scene(&FaceInfo::default()), Scene::Landscape);
    }

    #[test]
    fn large_face_is_portrait() {
        let fi = FaceInfo { faces: vec![face(0.25)] };
        assert_eq!(classify_scene(&fi), Scene::Portrait);
    }

    #[test]
    fn tiny_face_is_mixed() {
        let fi = FaceInfo { faces: vec![face(0.01)] };
        assert_eq!(classify_scene(&fi), Scene::Mixed);
    }

    #[test]
    fn soft_portrait_band_interpolates() {
        let mixed = FinalWeights::for_scene(Scene::Mixed);
        let portrait = FinalWeights::for_scene(Scene::Portrait);
        // Mid-band ≈ halfway between Mixed and Portrait.
        let fi = FaceInfo { faces: vec![face(0.035)] };
        let w = FinalWeights::for_face(&fi);
        let mid_fb = (mixed.face_bonus + portrait.face_bonus) / 2.0;
        assert!(
            (w.face_bonus - mid_fb).abs() < 0.01,
            "expected ~{} got {}",
            mid_fb,
            w.face_bonus
        );
        // Outside band: matches discrete weights.
        let small = FinalWeights::for_face(&FaceInfo { faces: vec![face(0.01)] });
        assert_eq!(small.face_bonus, mixed.face_bonus);
        let big = FinalWeights::for_face(&FaceInfo { faces: vec![face(0.20)] });
        assert_eq!(big.face_bonus, portrait.face_bonus);
    }
}
