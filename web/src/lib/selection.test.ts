import { describe, expect, it } from "vitest";
import { finalDropIds, finalKeptIds } from "./selection";
import type { CompositionPickView, PhotoView } from "./types";

function photo(id: string): PhotoView {
  return { photo_id: id, filename: `${id}.jpg`, captured_at: null, iso: null, final_score: null };
}

function pick(index: number, kept: string[], rejected: string[]): CompositionPickView {
  return {
    index,
    id: `g${index}`,
    scene: "mixed",
    kept: kept.map(photo),
    rejected: rejected.map(photo),
  };
}

// These two functions decide which files the ApplyBar DELETES — the single
// most destructive derivation in the frontend. Their invariant: over the
// same picks + overrides, kept ∪ drop covers every photo exactly once.

describe("finalDropIds / finalKeptIds", () => {
  it("with no overrides, mirrors the algorithm's verdicts", () => {
    const picks = [pick(0, ["a", "b"], ["c"]), pick(1, ["d"], ["e", "f"])];
    const none = new Set<string>();
    expect(finalKeptIds(picks, none).sort()).toEqual(["a", "b", "d"]);
    expect(finalDropIds(picks, none).sort()).toEqual(["c", "e", "f"]);
  });

  it("an override flips a rejected photo into the kept set (and out of drop)", () => {
    const picks = [pick(0, ["a"], ["b"])];
    const ov = new Set(["b"]);
    expect(finalKeptIds(picks, ov).sort()).toEqual(["a", "b"]);
    expect(finalDropIds(picks, ov)).toEqual([]);
  });

  it("an override flips a kept photo into the drop set (without keeping the rejected one)", () => {
    const picks = [pick(0, ["a"], ["b"])];
    const ov = new Set(["a"]);
    // "b" stays rejected — an override on "a" says nothing about "b".
    expect(finalKeptIds(picks, ov)).toEqual([]);
    expect(finalDropIds(picks, ov).sort()).toEqual(["a", "b"]);
  });

  it("kept and drop are exact complements for arbitrary override mixes", () => {
    const picks = [pick(0, ["a", "b"], ["c", "d"]), pick(1, ["e"], ["f"])];
    const all = ["a", "b", "c", "d", "e", "f"];
    for (const ov of [new Set<string>(), new Set(["a", "d"]), new Set(all)]) {
      const kept = finalKeptIds(picks, ov);
      const drop = finalDropIds(picks, ov);
      expect([...kept, ...drop].sort()).toEqual(all);
      expect(kept.filter((k) => drop.includes(k))).toEqual([]);
    }
  });

  it("an override id not present in any pick changes nothing", () => {
    const picks = [pick(0, ["a"], ["b"])];
    const ov = new Set(["zz-not-a-photo"]);
    expect(finalKeptIds(picks, ov)).toEqual(["a"]);
    expect(finalDropIds(picks, ov)).toEqual(["b"]);
  });
});
