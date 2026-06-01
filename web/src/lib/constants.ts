/// Per-scene final-score weights — mirrors `FinalWeights::for_scene` in the
/// Rust core (`crates/core/src/scoring/mod.rs`). The details panels use these
/// to grey out terms that don't contribute for the scene (e.g. `face_bonus`
/// for landscape) and to highlight the dominant contributor.
///
/// Keep these in sync with the Rust side. They're duplicated rather than
/// fetched from the server because score breakdowns render at 60fps in the
/// lightbox and a network round-trip per render would stall the UI.
export type SceneWeights = {
  tech: number;
  aesthetic: number;
  composition: number;
  face_bonus: number;
};

export const SCENE_WEIGHTS: Record<string, SceneWeights> = {
  portrait: { tech: 0.30, aesthetic: 0.20, composition: 0.15, face_bonus: 0.35 },
  landscape: { tech: 0.35, aesthetic: 0.40, composition: 0.25, face_bonus: 0.00 },
  mixed: { tech: 0.32, aesthetic: 0.30, composition: 0.20, face_bonus: 0.18 },
};
