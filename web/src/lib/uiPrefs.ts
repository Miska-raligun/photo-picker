/// Local UI preferences (card size today; anything else purely cosmetic
/// later). Kept out of `vlmStore`/`overridesStore` because these are
/// global, not per-run, and losing them costs the user nothing.

const CARD_SIZE_KEY = "photo-pick.cardSize";

export type CardSize = "s" | "m" | "l";

/// Tailwind width class per size — used by the group detail grid. Kept
/// here (not inline) so the card and any future consumer agree.
export const CARD_WIDTH_CLASS: Record<CardSize, string> = {
  s: "w-56",
  m: "w-80",
  l: "w-[26rem]",
};

/// Pixel width matching the classes above, for the virtualizer's size
/// estimate. Must stay in sync with CARD_WIDTH_CLASS.
export const CARD_WIDTH_PX: Record<CardSize, number> = {
  s: 224,
  m: 320,
  l: 416,
};

export function loadCardSize(): CardSize {
  try {
    const v = localStorage.getItem(CARD_SIZE_KEY);
    return v === "s" || v === "m" || v === "l" ? v : "m";
  } catch {
    return "m";
  }
}

export function saveCardSize(size: CardSize): void {
  try {
    localStorage.setItem(CARD_SIZE_KEY, size);
  } catch {
    // storage disabled — preference lives for this tab only
  }
}
