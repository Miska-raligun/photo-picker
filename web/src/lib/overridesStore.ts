/// Persistence layer for the user's per-photo verdict overrides
/// (keep-this-photo / drop-this-photo flips made via Space or the lightbox
/// button).
///
/// Without this, the overrides live in `App`'s React state and get
/// destroyed on full page reload, navigation, or container restart — the
/// user has to redo every flip when they come back to a long-running cull.
/// Saving each run's set to `localStorage` keyed on `runId` makes them
/// resilient. localStorage is per-origin so the photo-pick LAN sharing
/// case still works.
///
/// Write coalescing: callers fire on every toggle; we debounce to one
/// write per 300ms so a rapid keyboard flurry doesn't hammer the disk
/// (browsers throttle localStorage but every write still incurs a
/// JSON.stringify + structured clone).

const PREFIX = "photo-pick.overrides.";
const WRITE_DEBOUNCE_MS = 300;

const pendingWrites = new Map<string, ReturnType<typeof setTimeout>>();

function key(runId: string): string {
  return `${PREFIX}${runId}`;
}

/// Load the persisted override set for `runId`. Returns an empty Set if
/// nothing was saved (first visit) or if the saved payload is malformed
/// (don't crash the UI on corrupted storage — just start fresh).
export function loadOverrides(runId: string): Set<string> {
  try {
    const raw = localStorage.getItem(key(runId));
    if (!raw) return new Set();
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed)) {
      return new Set(parsed.filter((x): x is string => typeof x === "string"));
    }
    return new Set();
  } catch {
    return new Set();
  }
}

/// Persist `set` for `runId`. Debounced — the actual write happens
/// `WRITE_DEBOUNCE_MS` after the last call for this runId. Safe to call
/// many times in quick succession (keyboard mashing won't queue work).
export function saveOverrides(runId: string, set: Set<string>): void {
  const existing = pendingWrites.get(runId);
  if (existing) clearTimeout(existing);
  const t = setTimeout(() => {
    pendingWrites.delete(runId);
    try {
      if (set.size === 0) {
        localStorage.removeItem(key(runId));
      } else {
        localStorage.setItem(key(runId), JSON.stringify([...set]));
      }
    } catch {
      // Quota exceeded or storage disabled — silently skip. Losing
      // overrides is annoying but not worth crashing the UI for.
    }
  }, WRITE_DEBOUNCE_MS);
  pendingWrites.set(runId, t);
}

/// Drop the persisted overrides for `runId`. Called after `apply` so a
/// finished cull doesn't haunt the next visit to the same run.
export function clearOverrides(runId: string): void {
  const existing = pendingWrites.get(runId);
  if (existing) {
    clearTimeout(existing);
    pendingWrites.delete(runId);
  }
  try {
    localStorage.removeItem(key(runId));
  } catch {
    // ignore
  }
}

/// Drop only the listed photo ids from the persisted overrides for `runId`.
/// Used after a successful apply so the ids whose files are now gone stop
/// haunting the UI, while overrides on files the apply skipped (failed
/// safety check, missing on disk, etc.) keep their verdict so the user can
/// review and retry without losing context.
export function removeOverrides(runId: string, ids: readonly string[]): void {
  if (ids.length === 0) return;
  const current = loadOverrides(runId);
  if (current.size === 0) return;
  let touched = false;
  for (const id of ids) {
    if (current.delete(id)) touched = true;
  }
  if (touched) saveOverrides(runId, current);
}
