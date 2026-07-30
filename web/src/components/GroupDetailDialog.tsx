import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ChevronLeft,
  ChevronRight,
  Flag,
  ImageOff,
  Loader2,
  Maximize2,
  Settings as SettingsIcon,
  Sparkles,
} from "lucide-react";
import { motion, useReducedMotion } from "motion/react";
import { Lightbox } from "./Lightbox";
import { Thumb } from "./Thumb";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ScrollArea } from "@/components/ui/scroll-area";
import { api } from "@/lib/api";
import { CARD_WIDTH_CLASS, CARD_WIDTH_PX, loadCardSize, saveCardSize, type CardSize } from "@/lib/uiPrefs";
import { SCENE_WEIGHTS } from "@/lib/constants";
import { useI18n, useM } from "@/lib/i18n";
// Button import retained for header/footer use elsewhere in the dialog.
import type {
  CompositionPickView,
  SimilarPhoto,
  ExplanationRecord,
  PhotoTag,
  PhotoView,
  RunRecord,
  VlmSettings,
} from "@/lib/types";
import { cn } from "@/lib/utils";

interface Props {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  runId: string | null;
  pickIndex: number | null;
  /// Total number of composition groups in this run — drives the prev/next
  /// position indicator and disables the controls at either end.
  groupCount: number;
  /// Step to an adjacent group (delta ±1) without closing the dialog.
  onNavigate: (delta: number) => void;
  /// Set of photo ids whose algorithmic verdict the user has flipped.
  /// A flipped kept→drop. A flipped rejected→keep.
  overrides: Set<string>;
  /// User flags/notes per photo id (persisted per-run, like overrides).
  tags: Map<string, PhotoTag>;
  onSetTag: (photoId: string, tag: PhotoTag) => void;
  inPlace: boolean;
  vlmSettings: VlmSettings;
  onOpenSettings: () => void;
  onToggleOverride: (photoId: string) => void;
}

export function GroupDetailDialog({
  open,
  onOpenChange,
  runId,
  pickIndex,
  groupCount,
  onNavigate,
  overrides,
  tags,
  onSetTag,
  inPlace,
  vlmSettings,
  onOpenSettings,
  onToggleOverride,
}: Props) {
  const m = useM();
  const { lang } = useI18n();
  const [run, setRun] = useState<RunRecord | null>(null);
  const [loading, setLoading] = useState(false);
  // Provider only used in env mode; in custom mode we read it from settings.
  const [envProvider, setEnvProvider] = useState<"openai" | "anthropic">("openai");
  const [vlmLoading, setVlmLoading] = useState(false);
  const [vlmResult, setVlmResult] = useState<ExplanationRecord | null>(null);
  const [vlmError, setVlmError] = useState<string | null>(null);
  // Index into the (kept ++ rejected) display order for the currently-open
  // lightbox. `null` ⇒ closed. Holding an index (not a frozen url/name) lets
  // us drive prev/next + AI annotations + score breakdown straight from the
  // current pick without resyncing on every change.
  const [lightboxIndex, setLightboxIndex] = useState<number | null>(null);

  useEffect(() => {
    if (!open || !runId) return;
    setLoading(true);
    setVlmResult(null);
    setVlmError(null);
    api
      .getRun(runId)
      .then((r) => setRun(r))
      .catch(() => setRun(null))
      .finally(() => setLoading(false));
  }, [open, runId]);

  const pick: CompositionPickView | undefined =
    pickIndex == null ? undefined : run?.composition_picks?.[pickIndex];

  // Switching groups must drop the previous group's explanation so a stale
  // VLM answer doesn't appear attached to the new group.
  useEffect(() => {
    setVlmResult(null);
    setVlmError(null);
  }, [pickIndex]);

  const canPrev = pickIndex != null && pickIndex > 0;
  const canNext = pickIndex != null && pickIndex < groupCount - 1;

  // ←/→ step between groups. Skip when the lightbox is open (it owns arrows for
  // panning) or focus sits in a form control (provider <select>, etc.).
  useEffect(() => {
    if (!open || lightboxIndex !== null) return;
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement | null)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      if (e.key === "ArrowLeft" && canPrev) {
        e.preventDefault();
        onNavigate(-1);
      } else if (e.key === "ArrowRight" && canNext) {
        e.preventDefault();
        onNavigate(1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, lightboxIndex, canPrev, canNext, onNavigate]);

  async function askVlm() {
    if (!runId || pickIndex == null) return;
    setVlmLoading(true);
    setVlmError(null);
    setVlmResult(null);
    try {
      const fallbackProvider =
        vlmSettings.mode === "custom" ? vlmSettings.config.provider : envProvider;
      const override =
        vlmSettings.mode === "custom" ? vlmSettings.config : undefined;
      const r = await api.explain(runId, pickIndex, fallbackProvider, override, lang);
      setVlmResult(r);
    } catch (e) {
      setVlmError(e instanceof Error ? e.message : String(e));
    } finally {
      setVlmLoading(false);
    }
  }

  // Parse the model's response for "Rank N (Image X): reason..." lines so
  // both the rank and the per-photo reason can be shown next to each card.
  // Tolerant of variations:
  //   "Rank 1 (Image 3): reason"
  //   "Rank 1 (Image 3) — reason"
  //   "排名 1 (Image 3): reason"
  //   "Image 3 (Rank 1): reason"
  type Ann = { rank: number; reason: string };
  const aiAnnotations = useMemo<Map<number, Ann> | null>(() => {
    if (!vlmResult) return null;
    const map = new Map<number, Ann>();
    // The separator class tolerates markdown/paren noise the model often emits
    // around the bracket, e.g. `**Rank 1 (Image 2)**:` or `Rank 1 (Image 2) —`.
    const patterns = [
      // Rank-first
      /(?:Rank|排名|第)\s*[#]?\s*(\d+)[^\n]{0,40}?Image\s*[#]?\s*(\d+)[\s)*_]*[:\-—–]\s*([^\n]+)/gi,
      // Image-first
      /Image\s*[#]?\s*(\d+)[^\n]{0,40}?(?:Rank|排名|第)\s*[#]?\s*(\d+)[\s)*_]*[:\-—–]\s*([^\n]+)/gi,
    ];
    for (let pi = 0; pi < patterns.length; pi++) {
      const re = patterns[pi];
      let match;
      while ((match = re.exec(vlmResult.text)) !== null) {
        const a = parseInt(match[1]);
        const b = parseInt(match[2]);
        const reason = (match[3] ?? "").replace(/^[\s*_]+|[\s*_]+$/g, "");
        if (Number.isNaN(a) || Number.isNaN(b)) continue;
        // pi=0 → (rank, image). pi=1 → (image, rank).
        const [rank, imageNum] = pi === 0 ? [a, b] : [b, a];
        if (!map.has(imageNum)) map.set(imageNum, { rank, reason });
      }
    }
    return map.size > 0 ? map : null;
  }, [vlmResult]);

  // Combined kept-first list in the SCAN's original order. This is what
  // the VLM saw when it answered, so its "Image N" indexes line up with
  // positions here — keep this stable across sort/filter changes.
  const rawDisplayList = useMemo(
    () =>
      pick
        ? [
            ...pick.kept.map((p) => ({ p, kept: true })),
            ...pick.rejected.map((p) => ({ p, kept: false })),
          ]
        : [],
    [pick]
  );

  // Re-key AI annotations by photo_id so sort/filter don't desync them
  // from their cards. Built once per VLM response from the raw list's
  // original positions (which is what the model's reply references).
  const aiByPhotoId = useMemo<Map<string, Ann> | null>(() => {
    if (!aiAnnotations) return null;
    const out = new Map<string, Ann>();
    rawDisplayList.forEach((item, i) => {
      const ann = aiAnnotations.get(i + 1);
      if (ann) out.set(item.p.photo_id, ann);
    });
    return out.size > 0 ? out : null;
  }, [rawDisplayList, aiAnnotations]);

  // Sort + filter controls for the grid. Defaults reproduce the legacy
  // "algorithm order, show everything" behaviour, so nothing surprises
  // users who don't touch the toolbar.
  type SortMode = "algo" | "ai" | "score" | "time";
  type FilterMode = "all" | "kept" | "rejected" | "overridden" | "flagged" | "lowiso";
  const [sortMode, setSortMode] = useState<SortMode>("algo");
  const [filterMode, setFilterMode] = useState<FilterMode>("all");

  // Reset toolbar state when the user moves between groups so we don't
  // carry "show only overridden" into a group that has none and end up
  // with an empty grid.
  useEffect(() => {
    setSortMode("algo");
    setFilterMode("all");
  }, [pickIndex]);

  // Multi-select for bulk verdict flips. Plain click still toggles a single
  // verdict (legacy muscle memory); Shift selects a range and Ctrl/Cmd
  // toggles individual cards into a working set the toolbar can act on.
  // Cleared on every group change so cross-group selections — which would
  // be ambiguous to apply — can't accidentally form.
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const lastClickIndexRef = useRef<number | null>(null);
  // Card width preference (global, persisted). Also feeds the virtualizer's
  // size estimate so the two can't disagree.
  const [cardSize, setCardSize] = useState<CardSize>(() => loadCardSize());
  // "Find similar" results for the photo currently open in the lightbox.
  // Cleared whenever the lightbox target changes so a stale list can't be
  // attributed to the wrong photo.
  const [similar, setSimilar] = useState<SimilarPhoto[] | null>(null);
  const [similarLoading, setSimilarLoading] = useState(false);
  const [similarError, setSimilarError] = useState<string | null>(null);
  function changeCardSize(next: CardSize) {
    setCardSize(next);
    saveCardSize(next);
  }
  useEffect(() => {
    setSelectedIds(new Set());
    lastClickIndexRef.current = null;
  }, [pickIndex]);

  useEffect(() => {
    setSimilar(null);
    setSimilarError(null);
  }, [lightboxIndex, pickIndex]);

  // Esc clears the selection without closing the dialog. Skipped while
  // typing (provider <select> etc.) so users don't lose a working set to
  // an accidental Esc on a form control.
  useEffect(() => {
    if (!open || lightboxIndex !== null) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const tag = (e.target as HTMLElement | null)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      if (selectedIds.size > 0) {
        e.preventDefault();
        e.stopPropagation();
        setSelectedIds(new Set());
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, lightboxIndex, selectedIds.size]);

  const handleCardClick = (e: React.MouseEvent, photoId: string, index: number) => {
    if (e.shiftKey && lastClickIndexRef.current != null) {
      const a = Math.min(lastClickIndexRef.current, index);
      const b = Math.max(lastClickIndexRef.current, index);
      const rangeIds = displayList.slice(a, b + 1).map((it) => it.p.photo_id);
      setSelectedIds((prev) => {
        const next = new Set(prev);
        for (const id of rangeIds) next.add(id);
        return next;
      });
      lastClickIndexRef.current = index;
      return;
    }
    if (e.ctrlKey || e.metaKey) {
      setSelectedIds((prev) => {
        const next = new Set(prev);
        if (next.has(photoId)) next.delete(photoId);
        else next.add(photoId);
        return next;
      });
      lastClickIndexRef.current = index;
      return;
    }
    // Plain click: legacy single-toggle, unrelated to the bulk-select set.
    lastClickIndexRef.current = index;
    if (inPlace) onToggleOverride(photoId);
  };

  /// Bulk verdict actions for the current selection.
  ///   - "keep"   : ensure each selected photo ends up KEPT (clear override
  ///                on algo-kept, set override on algo-rejected)
  ///   - "reject" : opposite — ensure each ends up REJECTED
  ///   - "reset"  : clear override on every selected photo (restore algo
  ///                verdict regardless of current state)
  /// Each action walks the selection one id at a time through the
  /// existing single-photo toggle so it composes cleanly with
  /// overridesStore's per-id persistence path and the apply manifest.
  const applyBulk = (action: "keep" | "reject" | "reset") => {
    if (selectedIds.size === 0) return;
    // Index algo-verdict by photo id so we can compute the desired
    // override state per photo without flipping any twice.
    const algoKept = new Set<string>();
    for (const item of rawDisplayList) {
      if (item.kept) algoKept.add(item.p.photo_id);
    }
    for (const id of selectedIds) {
      const isAlgoKept = algoKept.has(id);
      const isOverridden = overrides.has(id);
      const wantKeep = action === "keep";
      const wantReject = action === "reject";
      const wantReset = action === "reset";
      // Resolved final verdict after applying the override toggle once.
      // We toggle only when the toggle would move us toward the target.
      if (wantReset) {
        if (isOverridden) onToggleOverride(id);
        continue;
      }
      const finalKept = isOverridden ? !isAlgoKept : isAlgoKept;
      if (wantKeep && !finalKept) onToggleOverride(id);
      else if (wantReject && finalKept) onToggleOverride(id);
    }
    setSelectedIds(new Set());
  };

  const displayList = useMemo(() => {
    const filtered = rawDisplayList.filter((item) => {
      switch (filterMode) {
        case "kept":
          return item.kept;
        case "rejected":
          return !item.kept;
        case "overridden":
          return overrides.has(item.p.photo_id);
        case "flagged":
          return !!tags.get(item.p.photo_id)?.flag;
        case "lowiso":
          return item.p.iso != null && item.p.iso <= 800;
        default:
          return true;
      }
    });
    if (sortMode === "algo") return filtered;
    const sorted = [...filtered];
    sorted.sort((a, b) => {
      switch (sortMode) {
        case "ai": {
          const ar = aiByPhotoId?.get(a.p.photo_id)?.rank ?? Number.POSITIVE_INFINITY;
          const br = aiByPhotoId?.get(b.p.photo_id)?.rank ?? Number.POSITIVE_INFINITY;
          return ar - br;
        }
        case "score": {
          const av = a.p.final_score?.value ?? -1;
          const bv = b.p.final_score?.value ?? -1;
          return bv - av;
        }
        case "time": {
          const at = a.p.captured_at
            ? Date.parse(a.p.captured_at)
            : Number.POSITIVE_INFINITY;
          const bt = b.p.captured_at
            ? Date.parse(b.p.captured_at)
            : Number.POSITIVE_INFINITY;
          return at - bt;
        }
        default:
          return 0;
      }
    });
    return sorted;
  }, [rawDisplayList, filterMode, sortMode, aiByPhotoId, overrides, tags]);

  function findSimilar(photoId: string) {
    if (!runId) return;
    setSimilarLoading(true);
    setSimilarError(null);
    api
      .similar(runId, photoId)
      .then((r) => setSimilar(r.similar))
      .catch((e) => setSimilarError(e instanceof Error ? e.message : String(e)))
      .finally(() => setSimilarLoading(false));
  }

  // Horizontal virtualizer over the (filtered, sorted) display list. Gap is
  // folded into the estimate so absolute offsets line up with the CSS.
  const stripRef = useRef<HTMLDivElement | null>(null);
  const cardStride = CARD_WIDTH_PX[cardSize] + 16;
  const virtualizer = useVirtualizer({
    horizontal: true,
    count: displayList.length,
    getScrollElement: () => stripRef.current,
    estimateSize: useCallback(() => cardStride, [cardStride]),
    overscan: 4,
    paddingStart: 24,
    paddingEnd: 24,
  });

  const total = pick ? pick.kept.length + pick.rejected.length : 0;
  // Final "will be kept" count after user flips: algo-kept minus flipped-kept,
  // plus flipped-rejected.
  const finalKept = pick
    ? (inPlace
        ? pick.kept.filter((k) => !overrides.has(k.photo_id)).length +
          pick.rejected.filter((r) => overrides.has(r.photo_id)).length
        : pick.kept.length)
    : 0;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="!max-w-[98vw] sm:!max-w-[96vw] !w-[96vw] !h-[94vh] max-h-[94vh] gap-0 p-0 grid grid-rows-[auto_minmax(0,1fr)_auto]">
        <DialogHeader className="px-6 pt-5 pb-3 border-b">
          <DialogTitle className="flex items-center gap-2 text-base">
            <div className="flex items-center gap-1 mr-1">
              <Button
                variant="ghost"
                size="icon"
                className="h-7 w-7"
                disabled={!canPrev}
                onClick={() => onNavigate(-1)}
                aria-label={m.detail.prevGroup}
                title={m.detail.prevGroup}
              >
                <ChevronLeft className="h-4 w-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                className="h-7 w-7"
                disabled={!canNext}
                onClick={() => onNavigate(1)}
                aria-label={m.detail.nextGroup}
                title={m.detail.nextGroup}
              >
                <ChevronRight className="h-4 w-4" />
              </Button>
            </div>
            <span className="tabular-nums">
              #{(pickIndex ?? 0) + 1}
              {groupCount > 0 && (
                <span className="text-muted-foreground font-normal text-sm">
                  {" "}
                  · {(pickIndex ?? 0) + 1}/{groupCount}
                </span>
              )}
            </span>
            {pick && (
              <>
                <Badge variant="outline" className="text-[0.65rem] font-normal">
                  {pick.scene}
                </Badge>
                <Badge variant="secondary" className="text-[0.7rem] font-mono">
                  {total} {m.detail.photos} · {finalKept} {m.detail.kept}
                </Badge>
              </>
            )}
          </DialogTitle>
        </DialogHeader>

        <div className="overflow-hidden flex flex-col min-h-0">
          {loading && (
            <div className="flex items-center justify-center text-muted-foreground py-12">
              <Loader2 className="h-4 w-4 mr-2 animate-spin" />
              {m.browse.loading}
            </div>
          )}
          {!loading && pick && runId && (
            <>
              <div className="px-6 pt-3 pb-2 flex items-center gap-2 flex-wrap text-xs text-muted-foreground border-b bg-background shrink-0">
                <span className="font-mono uppercase tracking-wider text-[0.65rem]">
                  {m.detail.sortBy}
                </span>
                <Select
                  value={sortMode}
                  onValueChange={(v) => setSortMode(v as SortMode)}
                >
                  <SelectTrigger className="h-7 w-36 text-xs">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="algo">{m.detail.sortAlgo}</SelectItem>
                    <SelectItem value="ai" disabled={!aiByPhotoId}>
                      {m.detail.sortAi}
                    </SelectItem>
                    <SelectItem value="score">{m.detail.sortScore}</SelectItem>
                    <SelectItem value="time">{m.detail.sortTime}</SelectItem>
                  </SelectContent>
                </Select>
                <span className="font-mono uppercase tracking-wider text-[0.65rem] ml-2">
                  {m.detail.filter}
                </span>
                {(
                  [
                    ["all", m.detail.filterAll],
                    ["kept", m.detail.filterKept],
                    ["rejected", m.detail.filterRejected],
                    ["overridden", m.detail.filterOverridden],
                    ["flagged", m.detail.filterFlagged],
                    ["lowiso", m.detail.filterLowIso],
                  ] as const
                ).map(([key, label]) => (
                  <Button
                    key={key}
                    variant={filterMode === key ? "default" : "outline"}
                    size="sm"
                    className="h-7 text-xs px-2"
                    onClick={() => setFilterMode(key)}
                  >
                    {label}
                  </Button>
                ))}
                <div className="flex items-center gap-0.5 ml-2">
                  {(["s", "m", "l"] as const).map((sz) => (
                    <Button
                      key={sz}
                      variant={cardSize === sz ? "default" : "outline"}
                      size="sm"
                      className="h-7 w-7 p-0 text-[0.65rem] font-mono"
                      onClick={() => changeCardSize(sz)}
                      title={m.detail.cardSize}
                      aria-label={`${m.detail.cardSize}: ${sz.toUpperCase()}`}
                      aria-pressed={cardSize === sz}
                    >
                      {sz.toUpperCase()}
                    </Button>
                  ))}
                </div>
                {selectedIds.size > 0 && inPlace ? (
                  <div className="ml-auto flex items-center gap-1.5 px-2 py-1 rounded-md bg-sky-50 dark:bg-sky-950/30 border border-sky-200/60 dark:border-sky-800/60">
                    <span className="font-mono text-sky-900 dark:text-sky-200">
                      {m.detail.bulkSelected(selectedIds.size)}
                    </span>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-6 text-xs px-2"
                      onClick={() => applyBulk("keep")}
                    >
                      {m.detail.bulkKeep}
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-6 text-xs px-2"
                      onClick={() => applyBulk("reject")}
                    >
                      {m.detail.bulkReject}
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-6 text-xs px-2"
                      onClick={() => applyBulk("reset")}
                    >
                      {m.detail.bulkReset}
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-6 text-xs px-1.5"
                      onClick={() => setSelectedIds(new Set())}
                      aria-label={m.detail.bulkClear}
                      title={m.detail.bulkClear}
                    >
                      ×
                    </Button>
                  </div>
                ) : (
                  <span className="ml-auto font-mono tabular-nums">
                    {displayList.length}/{rawDisplayList.length}
                  </span>
                )}
              </div>
              {displayList.length === 0 ? (
                <div className="flex-1 text-sm text-muted-foreground py-6 px-6">
                  {m.detail.filterEmpty}
                </div>
              ) : (
                // Horizontally virtualized strip: a 200-photo group used to
                // mount 200 cards (each with its own <img> + score bars),
                // which stutters badly on low-end machines. Only the visible
                // window plus overscan is rendered now.
                <div ref={stripRef} className="flex-1 min-h-0 overflow-x-auto overflow-y-hidden">
                  <div
                    className="relative h-full py-4"
                    style={{ width: `${virtualizer.getTotalSize()}px` }}
                  >
                    {virtualizer.getVirtualItems().map((v) => {
                      const { p, kept } = displayList[v.index];
                      return (
                        <div
                          key={p.photo_id}
                          className="absolute top-0 h-full py-4"
                          style={{ left: `${v.start}px`, width: `${v.size}px` }}
                        >
                          <PhotoCard
                            runId={runId}
                            photo={p}
                            kept={kept}
                            overridden={overrides.has(p.photo_id)}
                            selected={selectedIds.has(p.photo_id)}
                            tag={tags.get(p.photo_id)}
                            inPlace={inPlace}
                            widthClass={CARD_WIDTH_CLASS[cardSize]}
                            aiRank={aiByPhotoId?.get(p.photo_id)?.rank}
                            aiReason={aiByPhotoId?.get(p.photo_id)?.reason}
                            onCardClick={(e) => handleCardClick(e, p.photo_id, v.index)}
                            onToggleFlag={() => {
                              const cur = tags.get(p.photo_id) ?? {};
                              onSetTag(p.photo_id, { ...cur, flag: !cur.flag });
                            }}
                            onToggleOverride={() => onToggleOverride(p.photo_id)}
                            onViewOriginal={() => setLightboxIndex(v.index)}
                          />
                        </div>
                      );
                    })}
                  </div>
                </div>
              )}
            </>
          )}
          {!loading && (!pick || !runId) && (
            <div className="flex-1 flex flex-col items-center justify-center gap-3 text-muted-foreground py-12">
              <ImageOff className="h-8 w-8 opacity-50" />
              <span className="text-sm">{m.detail.groupUnavailable}</span>
            </div>
          )}
        </div>

        <div className="px-6 py-4 border-t bg-muted/30 flex flex-col gap-2 max-h-[40vh] min-h-0">
          <div className="flex items-center gap-2 flex-wrap shrink-0">
            {vlmSettings.mode === "env" ? (
              <Select
                value={envProvider}
                onValueChange={(v) => setEnvProvider(v as "openai" | "anthropic")}
              >
                <SelectTrigger className="w-32 h-9">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="openai">OpenAI</SelectItem>
                  <SelectItem value="anthropic">Anthropic</SelectItem>
                </SelectContent>
              </Select>
            ) : (
              <span className="text-xs text-muted-foreground font-mono px-2 py-1 rounded bg-muted">
                {vlmSettings.config.model}
              </span>
            )}
            <Button
              variant="outline"
              size="sm"
              onClick={askVlm}
              disabled={vlmLoading}
            >
              {vlmLoading ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <Sparkles className="h-4 w-4" />
              )}
              {m.detail.askVlm}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={onOpenSettings}
              title={m.common.settings}
            >
              <SettingsIcon className="h-4 w-4" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="ml-auto"
              onClick={() => onOpenChange(false)}
            >
              {m.common.close}
            </Button>
          </div>
          {vlmResult && !aiAnnotations && (
            // Couldn't parse per-photo reasons → fall back to showing the
            // full response in the bottom panel.
            <div className="border-l-2 border-primary bg-card rounded-md text-sm leading-relaxed flex-1 min-h-0 flex flex-col overflow-hidden">
              <div className="px-3 pt-3 pb-1.5 text-[0.7rem] font-mono uppercase tracking-wider text-muted-foreground shrink-0">
                {vlmResult.provider} / {vlmResult.model}
              </div>
              <ScrollArea className="flex-1 min-h-0">
                <div className="px-3 pb-3 whitespace-pre-wrap">
                  {vlmResult.text}
                </div>
              </ScrollArea>
            </div>
          )}
          {vlmResult && aiAnnotations && (
            <div className="text-xs text-muted-foreground shrink-0">
              <span className="font-mono">
                {vlmResult.provider} / {vlmResult.model}
              </span>
              <span className="mx-2">·</span>
              <span>
                {aiAnnotations.size}{" "}
                {m.detail.photos}{" "}
                {m.detail.aiRank}
              </span>
            </div>
          )}
          {vlmError && (
            <div className="border-l-2 border-destructive bg-destructive/5 rounded-md text-sm font-mono text-destructive flex-1 min-h-0 flex flex-col overflow-hidden">
              <div className="px-3 pt-3 pb-1.5 text-[0.7rem] uppercase tracking-wider shrink-0">
                {vlmSettings.mode === "custom"
                  ? vlmSettings.config.provider
                  : envProvider}{" "}
                · {m.detail.failed}
              </div>
              <ScrollArea className="flex-1 min-h-0">
                <div className="px-3 pb-3 whitespace-pre-wrap">{vlmError}</div>
              </ScrollArea>
            </div>
          )}
        </div>
      </DialogContent>

      {(() => {
        const open = lightboxIndex !== null && runId !== null && pick != null;
        const cur = open
          ? displayList[lightboxIndex as number]
          : undefined;
        const photo = cur?.p;
        const aiAnn =
          photo && aiByPhotoId ? aiByPhotoId.get(photo.photo_id) : undefined;
        return (
          <Lightbox
            open={open}
            onOpenChange={(v) => !v && setLightboxIndex(null)}
            previewUrl={
              open && photo ? api.previewUrl(runId as string, photo.photo_id) : null
            }
            thumbUrl={
              open && photo ? api.thumbUrl(runId as string, photo.photo_id) : null
            }
            filename={photo?.filename ?? photo?.photo_id ?? null}
            position={
              open
                ? { index: lightboxIndex as number, total: displayList.length }
                : undefined
            }
            onPrev={
              open && (lightboxIndex as number) > 0
                ? () => setLightboxIndex((i) => (i != null ? i - 1 : i))
                : undefined
            }
            onNext={
              open && (lightboxIndex as number) < displayList.length - 1
                ? () => setLightboxIndex((i) => (i != null ? i + 1 : i))
                : undefined
            }
            onFirst={
              open && (lightboxIndex as number) > 0
                ? () => setLightboxIndex(0)
                : undefined
            }
            onLast={
              open && (lightboxIndex as number) < displayList.length - 1
                ? () => setLightboxIndex(displayList.length - 1)
                : undefined
            }
            onPrevGroup={
              open && canPrev
                ? () => {
                    onNavigate(-1);
                    setLightboxIndex(0);
                  }
                : undefined
            }
            onNextGroup={
              open && canNext
                ? () => {
                    onNavigate(1);
                    setLightboxIndex(0);
                  }
                : undefined
            }
            details={
              open && photo && photo.final_score && cur
                ? {
                    kept: cur.kept,
                    overridden: overrides.has(photo.photo_id),
                    algoRank: (lightboxIndex as number) + 1,
                    finalScore: photo.final_score,
                    aiRank: aiAnn?.rank,
                    aiReason: aiAnn?.reason,
                  }
                : null
            }
            inPlace={inPlace}
            onToggleVerdict={
              open && inPlace && photo
                ? () => onToggleOverride(photo.photo_id)
                : undefined
            }
            tag={open && photo ? tags.get(photo.photo_id) ?? null : null}
            similar={similar}
            similarLoading={similarLoading}
            similarError={similarError}
            onFindSimilar={
              open && photo ? () => findSimilar(photo.photo_id) : undefined
            }
            similarThumbUrl={
              runId ? (pid: string) => api.thumbUrl(runId, pid) : undefined
            }
            onSetTag={
              open && photo
                ? (t) => onSetTag(photo.photo_id, t)
                : undefined
            }
          />
        );
      })()}
    </Dialog>
  );
}

function ScoreBreakdown({ fs }: { fs: NonNullable<PhotoView["final_score"]> }) {
  const m = useM();
  const weights = SCENE_WEIGHTS[fs.scene] ?? SCENE_WEIGHTS.mixed;
  const components = [
    { key: "tech", label: m.detail.scoreTech, value: fs.tech, weight: weights.tech },
    { key: "aesthetic", label: m.detail.scoreAesthetic, value: fs.aesthetic, weight: weights.aesthetic },
    { key: "composition", label: m.detail.scoreComposition, value: fs.composition, weight: weights.composition },
    { key: "face_bonus", label: m.detail.scoreFaceBonus, value: fs.face_bonus, weight: weights.face_bonus },
  ];
  // Dominant = largest weight×value contribution to the final score.
  let dominant = "";
  let best = -1;
  for (const c of components) {
    const contrib = c.value * c.weight;
    if (contrib > best) {
      best = contrib;
      dominant = c.key;
    }
  }
  return (
    <div className="space-y-1.5">
      <div className="flex items-baseline justify-between text-xs">
        <span className="text-muted-foreground font-mono">{m.detail.scoreFinal}</span>
        <span className="text-primary font-mono font-semibold tabular-nums">
          {Math.round(fs.value * 100)}
        </span>
      </div>
      <div className="space-y-1">
        {components.map((c) => (
          <ScoreBar
            key={c.key}
            label={c.label}
            value={c.value}
            disabled={c.weight === 0}
            dominant={c.key === dominant && c.weight > 0}
          />
        ))}
      </div>
    </div>
  );
}

function ScoreBar({
  label,
  value,
  disabled,
  dominant,
}: {
  label: string;
  value: number;
  disabled: boolean;
  dominant: boolean;
}) {
  const pct = Math.max(0, Math.min(100, value * 100));
  const reduce = useReducedMotion();
  return (
    <div className="flex items-center gap-2 text-[0.7rem] font-mono">
      <span
        className={`w-16 shrink-0 ${
          disabled ? "text-muted-foreground/50 line-through" : "text-muted-foreground"
        }`}
      >
        {label}
      </span>
      <div className="flex-1 h-1.5 rounded-full bg-muted overflow-hidden">
        <motion.div
          className={`h-full ${
            disabled
              ? "bg-muted-foreground/20"
              : dominant
              ? "bg-primary"
              : "bg-foreground/40"
          }`}
          initial={reduce ? false : { width: 0 }}
          animate={{ width: `${pct}%` }}
          transition={{ duration: 0.45, ease: [0.22, 0.6, 0.36, 1] }}
        />
      </div>
      <span
        className={`tabular-nums w-9 text-right ${
          disabled ? "text-muted-foreground/50" : ""
        }`}
      >
        {Math.round(value * 100)}
      </span>
    </div>
  );
}

interface PhotoCardProps {
  runId: string;
  photo: PhotoView;
  /// Algorithm's verdict — true if this photo was in `pick.kept`.
  kept: boolean;
  /// Whether the user has flipped the verdict for this photo.
  overridden: boolean;
  /// Whether the card is part of the current multi-select working set.
  /// Visual-only — bulk actions live in the parent's toolbar.
  selected: boolean;
  /// Tailwind width class from the user's card-size preference.
  widthClass: string;
  /// The user's flag/note for this photo, if any.
  tag?: PhotoTag;
  onToggleFlag: () => void;
  inPlace: boolean;
  /// The VLM's independent rank (1 = best) — shown as overlay badge when present.
  aiRank?: number;
  /// One-sentence reason from the VLM, shown below the score grid.
  aiReason?: string;
  /// Click handler with the raw event so the parent can branch on
  /// Shift / Ctrl / Cmd modifiers to drive multi-select without the card
  /// having to know about it.
  onCardClick: (e: React.MouseEvent) => void;
  onToggleOverride: () => void;
  onViewOriginal: () => void;
}

/// Memoized below; rendered in a grid up to ~K1·burst-size per composition
/// group (single-digit to mid-double-digits in practice). Memo avoids
/// re-render churn when an unrelated VLM annotation lands on a sibling card.
function PhotoCardImpl({
  runId,
  photo,
  kept,
  overridden,
  selected,
  widthClass,
  tag,
  onToggleFlag,
  inPlace,
  aiRank,
  aiReason,
  onCardClick,
  onToggleOverride,
  onViewOriginal,
}: PhotoCardProps) {
  const m = useM();
  const fs = photo.final_score;

  // Final state after flip:
  //   algo kept + not flipped → keep
  //   algo kept + flipped → force delete
  //   algo rejected + not flipped → reject (will delete)
  //   algo rejected + flipped → force keep
  const willKeep = overridden ? !kept : kept;
  const verdictText = !overridden
    ? willKeep
      ? m.detail.verdictWillKeep
      : m.detail.verdictWillDrop
    : willKeep
    ? m.detail.verdictForceKeep
    : m.detail.verdictForceDrop;
  const verdictColor = !overridden
    ? willKeep
      ? "bg-[var(--success)] text-white"
      : "bg-foreground/60 text-background"
    : "bg-primary text-primary-foreground";

  return (
    <div
      role={inPlace ? "button" : undefined}
      tabIndex={inPlace ? 0 : undefined}
      onClick={(e) => onCardClick(e)}
      onKeyDown={
        inPlace
          ? (e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onToggleOverride();
              }
            }
          : undefined
      }
      className={cn(
        "group shrink-0 rounded-lg border bg-card overflow-hidden flex flex-col transition-all",
        widthClass,
        // Border + opacity reflect the FINAL state, not the raw algo verdict.
        willKeep && !overridden && !selected && "border-[var(--success)] border-2",
        !willKeep && !overridden && !selected && "opacity-80",
        overridden && !selected && "border-primary border-2",
        // Selection ring wins visually over verdict styling — it's the
        // user's most-recent intent and they need to see what's in the set.
        selected && "border-sky-500 border-2 ring-2 ring-sky-300/60",
        inPlace &&
          "cursor-pointer hover:shadow-md hover:-translate-y-0.5 focus-visible:ring-2 focus-visible:ring-primary focus-visible:outline-none"
      )}
      title={
        inPlace
          ? willKeep
            ? m.detail.toggleToReject
            : m.detail.toggleToKeep
          : undefined
      }
    >
      <div className="relative aspect-[4/3] bg-muted">
        <Thumb
          src={api.thumbUrl(runId, photo.photo_id)}
          alt={
            photo.filename
              ? `${photo.filename} — ${verdictText}`
              : verdictText
          }
        />
        <Badge
          className={cn(
            "absolute top-2 right-2 text-[0.62rem] font-semibold uppercase tracking-wider",
            verdictColor
          )}
        >
          {verdictText}
        </Badge>
        {aiRank != null && (
          <Badge
            className="absolute top-2 left-2 text-[0.65rem] font-semibold uppercase tracking-wider bg-primary text-primary-foreground gap-1"
            title={`AI ranking: #${aiRank}`}
          >
            {m.detail.aiRank} #{aiRank}
          </Badge>
        )}
        {/* View-original button: stops propagation so it doesn't toggle keep/drop */}
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onViewOriginal();
          }}
          onKeyDown={(e) => e.stopPropagation()}
          title={m.detail.viewOriginal}
          aria-label={m.detail.viewOriginal}
          className="absolute bottom-2 right-2 rounded-md bg-black/55 backdrop-blur-sm text-white p-1.5 opacity-0 group-hover:opacity-100 focus:opacity-100 transition-opacity hover:bg-black/75"
        >
          <Maximize2 className="h-4 w-4" />
        </button>
        {/* Flag toggle: persistent when flagged, hover-revealed otherwise.
            Stops propagation so flagging never flips the verdict. */}
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onToggleFlag();
          }}
          onKeyDown={(e) => e.stopPropagation()}
          title={tag?.flag ? m.detail.unflag : m.detail.flag}
          aria-label={tag?.flag ? m.detail.unflag : m.detail.flag}
          aria-pressed={!!tag?.flag}
          className={cn(
            "absolute bottom-2 left-2 rounded-md backdrop-blur-sm p-1.5 transition-opacity",
            tag?.flag
              ? "bg-amber-500/90 text-white opacity-100"
              : "bg-black/55 text-white opacity-0 group-hover:opacity-100 focus:opacity-100 hover:bg-black/75"
          )}
        >
          <Flag className={cn("h-4 w-4", tag?.flag && "fill-current")} />
        </button>
      </div>
      <div className="p-3 space-y-2 flex flex-col flex-1">
        <div
          className="text-xs font-medium text-foreground truncate leading-tight"
          title={photo.filename ?? photo.photo_id}
        >
          {photo.filename ?? photo.photo_id.slice(0, 8)}
        </div>
        {fs && <ScoreBreakdown fs={fs} />}
        {/* aiReason block below */}
        {aiReason && (
          <div className="border-l-2 border-primary bg-primary/5 rounded-r-md px-2.5 py-1.5 text-xs leading-snug text-foreground/80 mt-1">
            <span className="font-mono font-semibold text-primary mr-1.5">
              {m.detail.aiRank}{aiRank != null ? ` #${aiRank}` : ""}:
            </span>
            <span className="italic">{aiReason}</span>
          </div>
        )}
        {tag?.note?.trim() && (
          <div className="border-l-2 border-amber-500 bg-amber-500/5 rounded-r-md px-2.5 py-1.5 text-xs leading-snug text-foreground/80 mt-1 whitespace-pre-wrap">
            {tag.note}
          </div>
        )}
      </div>
    </div>
  );
}

const PhotoCard = memo(PhotoCardImpl);
