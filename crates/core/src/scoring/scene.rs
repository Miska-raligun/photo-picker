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
}
