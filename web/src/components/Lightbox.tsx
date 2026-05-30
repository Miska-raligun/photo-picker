import { useEffect, useRef, useState } from "react";
import { Dialog as DialogPrimitive } from "radix-ui";
import {
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  Info,
  Minus,
  Plus,
  RotateCcw,
  Sparkles,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { useM } from "@/lib/i18n";
import { cn } from "@/lib/utils";

/// Per-scene weights — mirrors `FinalWeights::for_scene` in the Rust core
/// so the details panel can grey out terms that don't contribute (e.g.
/// `face_bonus` for landscape) and highlight the dominant contributor.
const SCENE_WEIGHTS: Record<
  string,
  { tech: number; aesthetic: number; composition: number; face_bonus: number }
> = {
  portrait: { tech: 0.3, aesthetic: 0.2, composition: 0.15, face_bonus: 0.35 },
  landscape: { tech: 0.35, aesthetic: 0.4, composition: 0.25, face_bonus: 0 },
  mixed: { tech: 0.32, aesthetic: 0.3, composition: 0.2, face_bonus: 0.18 },
};

export interface LightboxFinalScore {
  scene: string;
  tech: number;
  aesthetic: number;
  composition: number;
  face_bonus: number;
  value: number;
}

export interface LightboxDetails {
  /// Algorithm verdict — true if this photo was in `pick.kept`.
  kept: boolean;
  /// 1-based rank inside the composition group (display order: kept first,
  /// then rejected).
  algoRank: number;
  finalScore: LightboxFinalScore;
  /// VLM's independent rank, if a VLM analysis has been run.
  aiRank?: number;
  /// One-sentence reason from the VLM.
  aiReason?: string;
}

interface Props {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  previewUrl: string | null;
  /// Optional low-res image (typically the already-loaded thumbnail) used as
  /// a blurred backdrop while the full preview decodes — much nicer than a
  /// blank spinner, especially for RAW where the preview can take seconds.
  thumbUrl?: string | null;
  filename: string | null;
  /// Navigation within the surrounding group. Both `onPrev` and `onNext`
  /// may be undefined to disable the corresponding control (e.g. at the
  /// first/last photo, or when there is no group context).
  onPrev?: () => void;
  onNext?: () => void;
  /// "3 / 8" position indicator. Optional — omitted ⇒ no counter.
  position?: { index: number; total: number };
  /// Per-photo details for the right-hand info panel. Pass null when not
  /// available (e.g. preview-only contexts).
  details?: LightboxDetails | null;
}

const ZOOM_MIN = 1;
const ZOOM_MAX = 8;
const WHEEL_STEP = 0.0015;
const DOUBLE_CLICK_ZOOM = 2.5;

/// Full-viewport image preview with zoom, pan, group navigation, and an
/// optional details panel.
///
/// Image controls:
/// - Mouse wheel: zoom in/out anchored at the cursor
/// - Drag (when zoomed): pan
/// - Double-click: toggle 1× ↔ 2.5× anchored at the cursor
/// - + / – / 0 keys: zoom in / out / reset
/// - Buttons on the top bar: zoom in / out / reset
///
/// Navigation:
/// - ← / → keys, on-screen chevrons on the image edges (only at 1× zoom so
///   they don't interfere with pan)
///
/// Details panel:
/// - 'I' key or the info button toggles a right-side overlay showing the
///   algorithm verdict + scene-weighted score breakdown + VLM annotation.
///
/// Built on Radix Dialog primitives so ESC / outside-click routing works
/// when stacked over the parent dialog.
export function Lightbox({
  open,
  onOpenChange,
  previewUrl,
  thumbUrl,
  filename,
  onPrev,
  onNext,
  position,
  details,
}: Props) {
  const m = useM();
  const [loaded, setLoaded] = useState(false);
  const [errored, setErrored] = useState(false);
  const [showDetails, setShowDetails] = useState(false);

  // Zoom + pan state. (x, y) is the image's translation in screen pixels
  // BEFORE the scale is applied (transform-origin: center). Reset whenever
  // a new preview is loaded or the dialog re-opens.
  const [scale, setScale] = useState(1);
  const [pos, setPos] = useState({ x: 0, y: 0 });
  const containerRef = useRef<HTMLDivElement>(null);
  const dragOrigin = useRef<{ pointerX: number; pointerY: number; x: number; y: number } | null>(
    null
  );

  const resetZoom = () => {
    setScale(1);
    setPos({ x: 0, y: 0 });
  };

  // Reset on new image / open. Loading state too — a re-render with a new
  // previewUrl means we're waiting on a fresh decode.
  useEffect(() => {
    if (open) {
      setLoaded(false);
      setErrored(false);
      resetZoom();
    }
  }, [open, previewUrl]);

  // Keyboard: + / - / 0 / arrow nav / arrow pan / 'i' details.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement | null)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") return;
      if (e.key === "+" || e.key === "=") {
        e.preventDefault();
        zoomBy(1.25, null);
      } else if (e.key === "-" || e.key === "_") {
        e.preventDefault();
        zoomBy(1 / 1.25, null);
      } else if (e.key === "0") {
        e.preventDefault();
        resetZoom();
      } else if (e.key === "i" || e.key === "I") {
        if (details) {
          e.preventDefault();
          setShowDetails((v) => !v);
        }
      } else if (scale > 1) {
        // Zoomed → arrows pan (existing behavior).
        const step = 60;
        if (e.key === "ArrowLeft") setPos((p) => ({ ...p, x: p.x + step }));
        else if (e.key === "ArrowRight") setPos((p) => ({ ...p, x: p.x - step }));
        else if (e.key === "ArrowUp") setPos((p) => ({ ...p, y: p.y + step }));
        else if (e.key === "ArrowDown") setPos((p) => ({ ...p, y: p.y - step }));
      } else {
        // Not zoomed → ← / → navigate within the group.
        if (e.key === "ArrowLeft" && onPrev) {
          e.preventDefault();
          onPrev();
        } else if (e.key === "ArrowRight" && onNext) {
          e.preventDefault();
          onNext();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, scale, onPrev, onNext, details]);

  /// Zoom by `factor` around `anchor` in container coords. When anchor is
  /// null, zooms around the container center (i.e. keeps the image centered).
  function zoomBy(factor: number, anchor: { x: number; y: number } | null) {
    const rect = containerRef.current?.getBoundingClientRect();
    const cx = anchor && rect ? anchor.x - rect.left - rect.width / 2 : 0;
    const cy = anchor && rect ? anchor.y - rect.top - rect.height / 2 : 0;
    setScale((s) => {
      const next = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, s * factor));
      if (next === s) return s;
      const k = next / s;
      setPos((p) => ({ x: cx - (cx - p.x) * k, y: cy - (cy - p.y) * k }));
      if (Math.abs(next - 1) < 1e-3) {
        setPos({ x: 0, y: 0 });
      }
      return next;
    });
  }

  function onWheel(e: React.WheelEvent) {
    if (!previewUrl) return;
    e.preventDefault();
    const factor = 1 + -e.deltaY * WHEEL_STEP;
    zoomBy(factor, { x: e.clientX, y: e.clientY });
  }

  function onDoubleClick(e: React.MouseEvent) {
    if (!previewUrl) return;
    if (scale > 1.05) {
      resetZoom();
    } else {
      zoomBy(DOUBLE_CLICK_ZOOM, { x: e.clientX, y: e.clientY });
    }
  }

  function onPointerDown(e: React.PointerEvent) {
    if (scale <= 1) return;
    if (e.button !== 0) return;
    dragOrigin.current = {
      pointerX: e.clientX,
      pointerY: e.clientY,
      x: pos.x,
      y: pos.y,
    };
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: React.PointerEvent) {
    if (!dragOrigin.current) return;
    setPos({
      x: dragOrigin.current.x + (e.clientX - dragOrigin.current.pointerX),
      y: dragOrigin.current.y + (e.clientY - dragOrigin.current.pointerY),
    });
  }

  function onPointerUp(e: React.PointerEvent) {
    if (!dragOrigin.current) return;
    dragOrigin.current = null;
    try {
      (e.currentTarget as Element).releasePointerCapture(e.pointerId);
    } catch {
      /* ignore */
    }
  }

  const zoomed = scale > 1.001;
  const transformStyle: React.CSSProperties = {
    transform: `translate(${pos.x}px, ${pos.y}px) scale(${scale})`,
    transformOrigin: "center center",
    transition: dragOrigin.current ? "none" : "transform 80ms ease-out",
    cursor: zoomed ? (dragOrigin.current ? "grabbing" : "grab") : "zoom-in",
  };

  return (
    <DialogPrimitive.Root
      open={open}
      onOpenChange={(v) => {
        if (!v) {
          setLoaded(false);
          setErrored(false);
          setShowDetails(false);
          resetZoom();
        }
        onOpenChange(v);
      }}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-[200] bg-black/85 backdrop-blur-sm data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=open]:fade-in-0 data-[state=closed]:fade-out-0" />
        <DialogPrimitive.Content
          className="fixed inset-0 z-[201] flex items-center justify-center p-4 outline-none"
          onOpenAutoFocus={(e) => e.preventDefault()}
          onPointerDownOutside={(e) => {
            if (zoomed) e.preventDefault();
          }}
        >
          <DialogPrimitive.Title className="sr-only">
            {filename ?? "preview"}
          </DialogPrimitive.Title>

          {/* Top bar */}
          <div className="absolute top-4 left-4 right-4 flex items-center justify-between gap-2 text-white z-10">
            <div className="flex items-center gap-2 min-w-0">
              <div className="font-mono text-xs sm:text-sm truncate max-w-[40vw] bg-black/40 px-2 py-1 rounded">
                {filename ?? ""}
              </div>
              {position && (
                <div className="font-mono text-xs tabular-nums bg-black/40 px-2 py-1 rounded shrink-0">
                  {position.index + 1} / {position.total}
                </div>
              )}
            </div>
            <div className="flex items-center gap-1.5">
              <div className="flex items-center bg-black/40 rounded">
                <Button
                  variant="ghost"
                  size="sm"
                  className="text-white hover:bg-white/10 px-2"
                  onClick={() => zoomBy(1 / 1.25, null)}
                  disabled={scale <= ZOOM_MIN + 1e-3}
                  aria-label="zoom out"
                >
                  <Minus className="h-4 w-4" />
                </Button>
                <span className="text-xs font-mono tabular-nums px-2 select-none w-12 text-center">
                  {Math.round(scale * 100)}%
                </span>
                <Button
                  variant="ghost"
                  size="sm"
                  className="text-white hover:bg-white/10 px-2"
                  onClick={() => zoomBy(1.25, null)}
                  disabled={scale >= ZOOM_MAX - 1e-3}
                  aria-label="zoom in"
                >
                  <Plus className="h-4 w-4" />
                </Button>
                {zoomed && (
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-white hover:bg-white/10 px-2"
                    onClick={resetZoom}
                    aria-label="reset zoom"
                  >
                    <RotateCcw className="h-4 w-4" />
                  </Button>
                )}
              </div>
              {details && (
                <Button
                  variant="ghost"
                  size="sm"
                  className={cn(
                    "text-white hover:bg-white/10",
                    showDetails && "bg-white/15"
                  )}
                  onClick={() => setShowDetails((v) => !v)}
                  aria-label={m.detail.toggleDetails}
                  title={m.detail.toggleDetails + " (I)"}
                >
                  <Info className="h-4 w-4" />
                </Button>
              )}
              {previewUrl && (
                <Button
                  asChild
                  variant="ghost"
                  size="sm"
                  className="text-white hover:bg-white/10"
                >
                  <a href={previewUrl} target="_blank" rel="noreferrer">
                    <ExternalLink className="h-4 w-4" />
                    {m.detail.openInNewTab}
                  </a>
                </Button>
              )}
              <DialogPrimitive.Close asChild>
                <Button
                  variant="ghost"
                  size="sm"
                  className="text-white hover:bg-white/10"
                  aria-label="close"
                >
                  <X className="h-5 w-5" />
                </Button>
              </DialogPrimitive.Close>
            </div>
          </div>

          {/* Image area — blur-up + transform-based zoom/pan. */}
          <div
            ref={containerRef}
            className="relative w-[96vw] h-[88vh] flex items-center justify-center overflow-hidden touch-none"
            onWheel={onWheel}
            onDoubleClick={onDoubleClick}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
            onPointerCancel={onPointerUp}
          >
            {!loaded && !errored && thumbUrl && (
              <img
                src={thumbUrl}
                alt=""
                aria-hidden
                className="absolute max-w-[96vw] max-h-[88vh] object-contain rounded-md shadow-2xl blur-md scale-105 opacity-90 pointer-events-none"
              />
            )}
            {errored && (
              <div className="text-white/80 text-sm font-mono bg-black/40 px-3 py-2 rounded">
                {m.detail.previewFailed ?? "failed to load preview"}
              </div>
            )}
            {previewUrl && (
              <img
                src={previewUrl}
                alt={filename ?? ""}
                draggable={false}
                onLoad={() => setLoaded(true)}
                onError={() => setErrored(true)}
                style={transformStyle}
                className={`max-w-[96vw] max-h-[88vh] object-contain rounded-md shadow-2xl select-none ${
                  loaded ? "block" : "opacity-0"
                }`}
              />
            )}

            {/* Prev/next chevrons — only at 1× zoom so they don't interfere
                with panning. Hidden when the corresponding handler is unset. */}
            {!zoomed && onPrev && (
              <button
                type="button"
                onClick={onPrev}
                aria-label="previous"
                className="absolute left-2 top-1/2 -translate-y-1/2 h-12 w-12 grid place-items-center rounded-full bg-black/40 text-white hover:bg-black/60 transition-colors pointer-events-auto"
              >
                <ChevronLeft className="h-6 w-6" />
              </button>
            )}
            {!zoomed && onNext && (
              <button
                type="button"
                onClick={onNext}
                aria-label="next"
                className="absolute right-2 top-1/2 -translate-y-1/2 h-12 w-12 grid place-items-center rounded-full bg-black/40 text-white hover:bg-black/60 transition-colors pointer-events-auto"
              >
                <ChevronRight className="h-6 w-6" />
              </button>
            )}
          </div>

          {/* Details panel — slides in from the right when toggled. Sits
              above the image (z-20) so it overlays without resizing it. */}
          {details && showDetails && (
            <DetailsPanel
              details={details}
              filename={filename}
              onClose={() => setShowDetails(false)}
            />
          )}

          <div className="absolute bottom-4 left-1/2 -translate-x-1/2 text-white/60 text-xs select-none">
            {m.detail.lightboxHint}
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}

/// Right-side floating panel rendered above the image when toggled.
/// Matches the lightbox's translucent-black aesthetic; scrolls if it
/// outgrows the viewport on small screens.
function DetailsPanel({
  details,
  filename,
  onClose,
}: {
  details: LightboxDetails;
  filename: string | null;
  onClose: () => void;
}) {
  const m = useM();
  const fs = details.finalScore;
  const weights = SCENE_WEIGHTS[fs.scene] ?? SCENE_WEIGHTS.mixed;
  const components = [
    { key: "tech", label: m.detail.scoreTech, value: fs.tech, weight: weights.tech },
    { key: "aesthetic", label: m.detail.scoreAesthetic, value: fs.aesthetic, weight: weights.aesthetic },
    {
      key: "composition",
      label: m.detail.scoreComposition,
      value: fs.composition,
      weight: weights.composition,
    },
    {
      key: "face_bonus",
      label: m.detail.scoreFaceBonus,
      value: fs.face_bonus,
      weight: weights.face_bonus,
    },
  ];
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
    <div className="absolute top-20 right-4 z-20 w-[300px] max-h-[calc(88vh-1rem)] overflow-y-auto rounded-lg bg-black/65 backdrop-blur-md text-white shadow-2xl border border-white/10">
      <div className="flex items-center justify-between px-4 py-3 border-b border-white/10">
        <span className="text-sm font-semibold">{m.detail.detailsTitle}</span>
        <button
          type="button"
          onClick={onClose}
          aria-label="close details"
          className="text-white/70 hover:text-white"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      <div className="px-4 py-3 space-y-4 text-sm">
        {/* Verdict + ranks */}
        <div className="space-y-2">
          {filename && (
            <div className="font-mono text-xs text-white/60 break-all">{filename}</div>
          )}
          <div className="flex flex-wrap items-center gap-1.5">
            <Badge
              className={cn(
                "text-[0.65rem] font-semibold uppercase tracking-wider",
                details.kept
                  ? "bg-[var(--success)] text-white"
                  : "bg-foreground/40 text-background"
              )}
            >
              {details.kept ? m.detail.verdictWillKeep : m.detail.verdictWillDrop}
            </Badge>
            <Badge
              variant="outline"
              className="text-[0.65rem] font-mono border-white/20 text-white/80"
            >
              {fs.scene}
            </Badge>
          </div>

          <div className="grid grid-cols-2 gap-2 text-xs">
            <div className="rounded bg-white/5 px-2.5 py-2">
              <div className="text-white/50 text-[0.65rem] uppercase tracking-wider">
                {m.detail.algoRank}
              </div>
              <div className="font-mono tabular-nums text-base">#{details.algoRank}</div>
            </div>
            <div className="rounded bg-white/5 px-2.5 py-2">
              <div className="text-white/50 text-[0.65rem] uppercase tracking-wider flex items-center gap-1">
                <Sparkles className="h-3 w-3" />
                {m.detail.aiRank}
              </div>
              <div className="font-mono tabular-nums text-base">
                {details.aiRank != null ? `#${details.aiRank}` : "—"}
              </div>
            </div>
          </div>
        </div>

        {/* Score breakdown */}
        <div className="space-y-2">
          <div className="flex items-baseline justify-between">
            <span className="text-white/60 text-xs font-mono">
              {m.detail.scoreFinal}
            </span>
            <span className="font-mono font-semibold text-primary text-lg tabular-nums">
              {Math.round(fs.value * 100)}
            </span>
          </div>
          <div className="space-y-1.5">
            {components.map((c) => (
              <ScoreRow
                key={c.key}
                label={c.label}
                value={c.value}
                disabled={c.weight === 0}
                dominant={c.key === dominant && c.weight > 0}
              />
            ))}
          </div>
        </div>

        {/* AI reason */}
        {details.aiReason && (
          <div className="space-y-1.5 pt-2 border-t border-white/10">
            <div className="text-white/60 text-xs font-mono flex items-center gap-1">
              <Sparkles className="h-3 w-3" />
              {m.detail.aiReasonLabel}
            </div>
            <p className="text-xs leading-relaxed text-white/85 whitespace-pre-wrap">
              {details.aiReason}
            </p>
          </div>
        )}
      </div>
    </div>
  );
}

function ScoreRow({
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
  return (
    <div className="flex items-center gap-2 text-[0.7rem] font-mono">
      <span
        className={cn(
          "w-16 shrink-0",
          disabled ? "text-white/30 line-through" : "text-white/60"
        )}
      >
        {label}
      </span>
      <div className="flex-1 h-1.5 rounded-full bg-white/10 overflow-hidden">
        <div
          className={cn(
            "h-full transition-[width] duration-300 ease-out",
            disabled
              ? "bg-white/20"
              : dominant
              ? "bg-primary"
              : "bg-white/55"
          )}
          style={{ width: `${pct}%` }}
        />
      </div>
      <span
        className={cn(
          "tabular-nums w-7 text-right",
          disabled ? "text-white/30" : "text-white/80"
        )}
      >
        {Math.round(value * 100)}
      </span>
    </div>
  );
}
