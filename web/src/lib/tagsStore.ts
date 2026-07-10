/// Persistence for user-attached photo tags (flag + free-text note), keyed
/// per run — the "this one's my favorite" / "fix the crop later" layer that
/// the algorithmic verdict can't capture. Same localStorage + debounce
/// pattern as overridesStore so the two stay behaviorally identical.

import type { PhotoTag } from "./types";

const PREFIX = "photo-pick.tags.";
const WRITE_DEBOUNCE_MS = 300;

const pendingWrites = new Map<string, ReturnType<typeof setTimeout>>();

function key(runId: string): string {
  return `${PREFIX}${runId}`;
}

function isMeaningful(tag: PhotoTag): boolean {
  return !!tag.flag || !!tag.note?.trim();
}

/// Load the persisted tag map for `runId`. Malformed payloads load as empty
/// rather than throwing (same contract as overridesStore).
export function loadTags(runId: string): Map<string, PhotoTag> {
  try {
    const raw = localStorage.getItem(key(runId));
    if (!raw) return new Map();
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      const out = new Map<string, PhotoTag>();
      for (const [id, tag] of Object.entries(parsed)) {
        if (tag && typeof tag === "object") {
          const t = tag as PhotoTag;
          if (isMeaningful(t)) out.set(id, { flag: !!t.flag, note: t.note });
        }
      }
      return out;
    }
    return new Map();
  } catch {
    return new Map();
  }
}

/// Persist `tags` for `runId`, debounced. Entries that carry no information
/// (unflagged, empty note) are dropped so storage doesn't accumulate husks.
export function saveTags(runId: string, tags: Map<string, PhotoTag>): void {
  const existing = pendingWrites.get(runId);
  if (existing) clearTimeout(existing);
  const t = setTimeout(() => {
    pendingWrites.delete(runId);
    try {
      const obj: Record<string, PhotoTag> = {};
      let n = 0;
      for (const [id, tag] of tags) {
        if (isMeaningful(tag)) {
          obj[id] = tag;
          n++;
        }
      }
      if (n === 0) localStorage.removeItem(key(runId));
      else localStorage.setItem(key(runId), JSON.stringify(obj));
    } catch {
      // Quota exceeded / storage disabled — losing tags is not worth a crash.
    }
  }, WRITE_DEBOUNCE_MS);
  pendingWrites.set(runId, t);
}

/// Drop tags for photos that no longer exist (post-apply cleanup, same
/// selective semantics as removeOverrides).
export function removeTags(runId: string, ids: readonly string[]): void {
  if (ids.length === 0) return;
  const current = loadTags(runId);
  if (current.size === 0) return;
  let touched = false;
  for (const id of ids) {
    if (current.delete(id)) touched = true;
  }
  if (touched) saveTags(runId, current);
}
