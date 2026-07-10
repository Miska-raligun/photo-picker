import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearOverrides,
  loadOverrides,
  removeOverrides,
  saveOverrides,
} from "./overridesStore";

// Minimal localStorage stand-in — vitest runs in node where the global is
// absent. Only the three methods the store touches.
function installLocalStorage() {
  const map = new Map<string, string>();
  const stub = {
    getItem: (k: string) => map.get(k) ?? null,
    setItem: (k: string, v: string) => void map.set(k, v),
    removeItem: (k: string) => void map.delete(k),
  };
  vi.stubGlobal("localStorage", stub);
  return map;
}

describe("overridesStore", () => {
  beforeEach(() => {
    installLocalStorage();
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.runOnlyPendingTimers();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("save is debounced, then load round-trips the set", () => {
    saveOverrides("run1", new Set(["a", "b"]));
    // Before the debounce window elapses nothing is persisted.
    expect(loadOverrides("run1").size).toBe(0);
    vi.advanceTimersByTime(400);
    expect([...loadOverrides("run1")].sort()).toEqual(["a", "b"]);
  });

  it("saving an empty set removes the key entirely", () => {
    saveOverrides("run1", new Set(["a"]));
    vi.advanceTimersByTime(400);
    saveOverrides("run1", new Set());
    vi.advanceTimersByTime(400);
    expect(loadOverrides("run1").size).toBe(0);
  });

  it("removeOverrides drops only the listed ids (post-apply cleanup)", () => {
    saveOverrides("run1", new Set(["deleted1", "deleted2", "failed1"]));
    vi.advanceTimersByTime(400);
    // Simulates the apply response: two files actually deleted, one failed.
    removeOverrides("run1", ["deleted1", "deleted2"]);
    vi.advanceTimersByTime(400);
    expect([...loadOverrides("run1")]).toEqual(["failed1"]);
  });

  it("removeOverrides with no matching ids leaves storage untouched", () => {
    saveOverrides("run1", new Set(["keep-me"]));
    vi.advanceTimersByTime(400);
    removeOverrides("run1", ["not-present"]);
    vi.advanceTimersByTime(400);
    expect([...loadOverrides("run1")]).toEqual(["keep-me"]);
  });

  it("clearOverrides cancels a pending debounced write", () => {
    saveOverrides("run1", new Set(["a"]));
    clearOverrides("run1"); // before the timer fires
    vi.advanceTimersByTime(400);
    expect(loadOverrides("run1").size).toBe(0);
  });

  it("malformed stored payloads load as an empty set instead of throwing", () => {
    const map = installLocalStorage();
    map.set("photo-pick.overrides.run1", "{not json");
    expect(loadOverrides("run1").size).toBe(0);
    map.set("photo-pick.overrides.run1", JSON.stringify({ nope: 1 }));
    expect(loadOverrides("run1").size).toBe(0);
  });
});
