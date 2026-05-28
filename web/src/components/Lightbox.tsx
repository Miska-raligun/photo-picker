import { useEffect, useRef, useState } from "react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { ExternalLink, Minus, Plus, RotateCcw, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useM } from "@/lib/i18n";

interface Props {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  previewUrl: string | null;
  /// Optional low-res image (typically the already-loaded thumbnail) used as
  /// a blurred backdrop while the full preview decodes — much nicer than a
  /// blank spinner, especially for RAW where the preview can take seconds.
  thumbUrl?: string | null;
  filename: string | null;
}

const ZOOM_MIN = 1;
const ZOOM_MAX = 8;
const WHEEL_STEP = 0.0015;
const DOUBLE_CLICK_ZOOM = 2.5;

/// Full-viewport image preview with zoom + pan.
///
/// - Mouse wheel: zoom in/out anchored at the cursor
/// - Drag (when zoomed): pan
/// - Double-click: toggle 1× ↔ 2.5× anchored at the cursor
/// - + / – / 0 keys: zoom in / out / reset
/// - Buttons on the top bar: zoom in / out / reset
///
/// Built on Radix Dialog primitives so ESC/outside-click routing works
/// when stacked over the parent dialog.
export function Lightbox({ open, onOpenChange, previewUrl, thumbUrl, filename }: Props) {
  const m = useM();
  const [loaded, setLoaded] = useState(false);
  const [errored, setErrored] = useState(false);

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

  // Reset on new image or close.
  useEffect(() => {
    if (open) resetZoom();
  }, [open, previewUrl]);

  // Keyboard: + / - / 0 / arrow pan when zoomed.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      // Ignore if focus is in an input (none here today, defensive).
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
      } else if (scale > 1) {
        const step = 60;
        if (e.key === "ArrowLeft") setPos((p) => ({ ...p, x: p.x + step }));
        else if (e.key === "ArrowRight") setPos((p) => ({ ...p, x: p.x - step }));
        else if (e.key === "ArrowUp") setPos((p) => ({ ...p, y: p.y + step }));
        else if (e.key === "ArrowDown") setPos((p) => ({ ...p, y: p.y - step }));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, scale]);

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
      // If we just zoomed back to 1, snap pan back to (0, 0) so the image
      // re-centers exactly.
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
    // Only drag when zoomed in — at 1× we want clicks-on-backdrop to close
    // via Radix (outside-click on the actual Content boundary).
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
          // When zoomed in, pointer-down on the content shouldn't bubble
          // into Radix's outside-click detector (we want to pan, not close).
          onPointerDownOutside={(e) => {
            if (zoomed) e.preventDefault();
          }}
        >
          {/* sr-only title for a11y */}
          <DialogPrimitive.Title className="sr-only">
            {filename ?? "preview"}
          </DialogPrimitive.Title>

          {/* Top bar */}
          <div className="absolute top-4 left-4 right-4 flex items-center justify-between gap-2 text-white z-10">
            <div className="font-mono text-xs sm:text-sm truncate max-w-[60vw] bg-black/40 px-2 py-1 rounded">
              {filename ?? ""}
            </div>
            <div className="flex items-center gap-1.5">
              {/* Zoom controls — always rendered so the user discovers them
                  without needing to know wheel/double-click. */}
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

          {/* Image area — blur-up: while the full preview decodes, show the
              already-loaded thumbnail blurred + scaled as a backdrop.
              Zoom + pan are applied via transform on the inner wrapper. */}
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
            {/* No thumbnail to blur-up yet: soft shimmer placeholder so the
                preview never opens to an empty void. */}
            {!loaded && !errored && !thumbUrl && (
              <div
                aria-hidden
                className="shimmer absolute w-[60vw] h-[70vh] max-w-[96vw] max-h-[88vh] rounded-md opacity-20"
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
          </div>

          <div className="absolute bottom-4 left-1/2 -translate-x-1/2 text-white/60 text-xs select-none">
            {m.detail.lightboxHint}
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
