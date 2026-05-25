import { useEffect, useState } from "react";
import { ExternalLink, Loader2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useM } from "@/lib/i18n";

interface Props {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  previewUrl: string | null;
  filename: string | null;
}

/// Full-viewport image preview. ESC or click-outside closes. Has a corner
/// link to open the same URL in a new tab so the user can save / right-click
/// it natively.
export function Lightbox({ open, onOpenChange, previewUrl, filename }: Props) {
  const m = useM();
  const [loaded, setLoaded] = useState(false);
  const [errored, setErrored] = useState(false);

  useEffect(() => {
    if (!open) return;
    setLoaded(false);
    setErrored(false);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onOpenChange(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onOpenChange]);

  if (!open || !previewUrl) return null;

  return (
    <div
      className="fixed inset-0 z-[100] bg-black/85 backdrop-blur-sm flex items-center justify-center p-4"
      onClick={() => onOpenChange(false)}
      role="dialog"
      aria-modal="true"
    >
      {/* Top bar — filename + actions */}
      <div className="absolute top-4 left-4 right-4 flex items-center justify-between gap-2 z-10 text-white">
        <div className="font-mono text-xs sm:text-sm truncate max-w-[60vw] bg-black/40 px-2 py-1 rounded">
          {filename ?? ""}
        </div>
        <div className="flex items-center gap-1.5">
          <Button
            asChild
            variant="ghost"
            size="sm"
            className="text-white hover:bg-white/10"
            onClick={(e) => e.stopPropagation()}
          >
            <a href={previewUrl} target="_blank" rel="noreferrer">
              <ExternalLink className="h-4 w-4" />
              {m.detail.openInNewTab}
            </a>
          </Button>
          <Button
            variant="ghost"
            size="sm"
            className="text-white hover:bg-white/10"
            onClick={(e) => {
              e.stopPropagation();
              onOpenChange(false);
            }}
            aria-label="close"
          >
            <X className="h-5 w-5" />
          </Button>
        </div>
      </div>

      {/* Image area — stopPropagation so clicking the image doesn't close */}
      <div
        className="max-w-[96vw] max-h-[88vh] flex items-center justify-center"
        onClick={(e) => e.stopPropagation()}
      >
        {!loaded && !errored && (
          <div className="text-white flex items-center gap-2">
            <Loader2 className="h-5 w-5 animate-spin" />
            <span className="text-sm">loading…</span>
          </div>
        )}
        {errored && (
          <div className="text-white/80 text-sm font-mono bg-black/40 px-3 py-2 rounded">
            failed to load preview
          </div>
        )}
        <img
          src={previewUrl}
          alt={filename ?? ""}
          onLoad={() => setLoaded(true)}
          onError={() => setErrored(true)}
          className={`max-w-[96vw] max-h-[88vh] object-contain rounded-md shadow-2xl ${
            loaded ? "block" : "hidden"
          }`}
        />
      </div>

      <div className="absolute bottom-4 left-1/2 -translate-x-1/2 text-white/60 text-xs">
        ESC · click outside to close
      </div>
    </div>
  );
}
