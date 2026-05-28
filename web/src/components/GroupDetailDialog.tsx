import { memo, useEffect, useMemo, useState } from "react";
import {
  ChevronLeft,
  ChevronRight,
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
import { ScrollArea, ScrollBar } from "@/components/ui/scroll-area";
import { api } from "@/lib/api";
import { useI18n, useM } from "@/lib/i18n";
// Button import retained for header/footer use elsewhere in the dialog.
import type {
  CompositionPickView,
  ExplanationRecord,
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
  const [lightbox, setLightbox] = useState<{
    url: string;
    name: string;
    thumbUrl?: string;
  } | null>(null);

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
    if (!open || lightbox) return;
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
  }, [open, lightbox, canPrev, canNext, onNavigate]);

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
    const patterns = [
      // Rank-first
      /(?:Rank|排名|第)\s*[#]?\s*(\d+)[^\n]{0,40}?Image\s*[#]?\s*(\d+)\s*\)?\s*[:\-—–]\s*([^\n]+)/gi,
      // Image-first
      /Image\s*[#]?\s*(\d+)[^\n]{0,40}?(?:Rank|排名|第)\s*[#]?\s*(\d+)\s*\)?\s*[:\-—–]\s*([^\n]+)/gi,
    ];
    for (let pi = 0; pi < patterns.length; pi++) {
      const re = patterns[pi];
      let match;
      while ((match = re.exec(vlmResult.text)) !== null) {
        const a = parseInt(match[1]);
        const b = parseInt(match[2]);
        const reason = match[3]?.trim() ?? "";
        if (Number.isNaN(a) || Number.isNaN(b)) continue;
        // pi=0 → (rank, image). pi=1 → (image, rank).
        const [rank, imageNum] = pi === 0 ? [a, b] : [b, a];
        if (!map.has(imageNum)) map.set(imageNum, { rank, reason });
      }
    }
    return map.size > 0 ? map : null;
  }, [vlmResult]);

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
              #{pickIndex}
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
            <ScrollArea className="flex-1 w-full min-h-0">
              <div className="flex gap-4 px-6 py-4 w-max">
                {[
                  ...pick.kept.map((p) => ({ p, kept: true })),
                  ...pick.rejected.map((p) => ({ p, kept: false })),
                ].map(({ p, kept }, i) => (
                  <PhotoCard
                    key={p.photo_id}
                    runId={runId}
                    photo={p}
                    kept={kept}
                    overridden={overrides.has(p.photo_id)}
                    inPlace={inPlace}
                    aiRank={aiAnnotations?.get(i + 1)?.rank}
                    aiReason={aiAnnotations?.get(i + 1)?.reason}
                    onToggleOverride={() => onToggleOverride(p.photo_id)}
                    onViewOriginal={() =>
                      setLightbox({
                        url: api.previewUrl(runId, p.photo_id),
                        name: p.filename ?? p.photo_id,
                        thumbUrl: api.thumbUrl(runId, p.photo_id),
                      })
                    }
                  />
                ))}
              </div>
              <ScrollBar orientation="horizontal" />
            </ScrollArea>
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

      <Lightbox
        open={lightbox !== null}
        onOpenChange={(v) => !v && setLightbox(null)}
        previewUrl={lightbox?.url ?? null}
        thumbUrl={lightbox?.thumbUrl ?? null}
        filename={lightbox?.name ?? null}
      />
    </Dialog>
  );
}

/// Per-scene weights — must mirror `FinalWeights::for_scene` in the Rust
/// scoring crate. Used to identify the dominant contributor.
const SCENE_WEIGHTS: Record<string, { tech: number; aesthetic: number; composition: number; face_bonus: number }> = {
  portrait: { tech: 0.30, aesthetic: 0.20, composition: 0.15, face_bonus: 0.35 },
  landscape: { tech: 0.35, aesthetic: 0.40, composition: 0.25, face_bonus: 0.00 },
  mixed:    { tech: 0.32, aesthetic: 0.30, composition: 0.20, face_bonus: 0.18 },
};

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
  inPlace: boolean;
  /// The VLM's independent rank (1 = best) — shown as overlay badge when present.
  aiRank?: number;
  /// One-sentence reason from the VLM, shown below the score grid.
  aiReason?: string;
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
  inPlace,
  aiRank,
  aiReason,
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
      onClick={inPlace ? onToggleOverride : undefined}
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
        "group w-80 shrink-0 rounded-lg border bg-card overflow-hidden flex flex-col transition-all",
        // Border + opacity reflect the FINAL state, not the raw algo verdict.
        willKeep && !overridden && "border-[var(--success)] border-2",
        !willKeep && !overridden && "opacity-80",
        overridden && "border-primary border-2",
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
          alt={photo.filename ?? ""}
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
      </div>
      <div className="p-3 space-y-2 flex flex-col flex-1">
        <div className="text-xs font-medium text-foreground break-all leading-tight">
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
      </div>
    </div>
  );
}

const PhotoCard = memo(PhotoCardImpl);
