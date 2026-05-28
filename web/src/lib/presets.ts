// Curation presets shown as the primary task-creation choice. Each preset
// hides the engine knobs (k1/k2/thresholds/etc.) behind one plain-language
// picture of intent. Users who want the raw knobs open "Advanced settings".
//
// Tunable: these mappings are starting points, not gospel. Keep the BALANCED
// preset identical to the previous server defaults so behavior is unchanged
// for anyone migrating; the other two shift just enough to be useful.

export type PresetId = "aggressive" | "balanced" | "gentle";

export interface PresetParams {
  k1: number;
  /// `null` ⇒ auto-K2 (server picks per-group via score gaps).
  k2: number | null;
  time_k: number;
  stage_a_clip_threshold: number;
  stage_b_threshold: number;
  enable_clip: boolean;
  enable_face: boolean;
  adaptive_thresholds: boolean;
}

export const PRESETS: Record<PresetId, PresetParams> = {
  // 严格精选: keep fewer per burst & per composition group. Lower thresholds
  // merge more aggressively (so groups absorb near-duplicates rather than
  // becoming separate keepers).
  aggressive: {
    k1: 2,
    k2: 1,
    time_k: 3.0,
    stage_a_clip_threshold: 0.93,
    stage_b_threshold: 0.90,
    enable_clip: true,
    enable_face: true,
    adaptive_thresholds: true,
  },
  // 平衡 (default): the server's previous defaults, unchanged.
  balanced: {
    k1: 3,
    k2: null,
    time_k: 3.0,
    stage_a_clip_threshold: 0.95,
    stage_b_threshold: 0.93,
    enable_clip: true,
    enable_face: true,
    adaptive_thresholds: true,
  },
  // 宽松保留: keep more — higher thresholds split more, k1 lets more frames
  // survive per burst. Auto-K2 still decides per composition group.
  gentle: {
    k1: 5,
    k2: null,
    time_k: 3.0,
    stage_a_clip_threshold: 0.97,
    stage_b_threshold: 0.95,
    enable_clip: true,
    enable_face: true,
    adaptive_thresholds: true,
  },
};

/// Compare candidate params to a preset's so the UI can re-select the right
/// card when a user manually matches one (and show "Custom" otherwise).
export function matchPreset(p: PresetParams): PresetId | "custom" {
  const eq = (a: PresetParams, b: PresetParams) =>
    a.k1 === b.k1 &&
    a.k2 === b.k2 &&
    a.time_k === b.time_k &&
    Math.abs(a.stage_a_clip_threshold - b.stage_a_clip_threshold) < 1e-6 &&
    Math.abs(a.stage_b_threshold - b.stage_b_threshold) < 1e-6 &&
    a.enable_clip === b.enable_clip &&
    a.enable_face === b.enable_face &&
    a.adaptive_thresholds === b.adaptive_thresholds;
  for (const id of ["aggressive", "balanced", "gentle"] as PresetId[]) {
    if (eq(p, PRESETS[id])) return id;
  }
  return "custom";
}
