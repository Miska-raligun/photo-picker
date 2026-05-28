import type { CompositionPickView } from "./types";

/// Resolve the user's final keep/drop decision across all composition groups,
/// folding in manual overrides. A photo's algorithmic verdict (kept vs
/// rejected) is flipped when its id is present in `overrides`.
///
/// - `finalKeptIds`  → what survives review (export target).
/// - `finalDropIds`  → what gets deleted in-place (ApplyBar target).
///
/// The two are exact complements over the run's photos, so they share this
/// single walk to stay consistent.

export function finalKeptIds(
  picks: CompositionPickView[],
  overrides: Set<string>
): string[] {
  const ids: string[] = [];
  for (const p of picks) {
    for (const k of p.kept) {
      if (!overrides.has(k.photo_id)) ids.push(k.photo_id);
    }
    for (const r of p.rejected) {
      if (overrides.has(r.photo_id)) ids.push(r.photo_id);
    }
  }
  return ids;
}

export function finalDropIds(
  picks: CompositionPickView[],
  overrides: Set<string>
): string[] {
  const ids: string[] = [];
  for (const p of picks) {
    for (const r of p.rejected) {
      if (!overrides.has(r.photo_id)) ids.push(r.photo_id);
    }
    for (const k of p.kept) {
      if (overrides.has(k.photo_id)) ids.push(k.photo_id);
    }
  }
  return ids;
}
