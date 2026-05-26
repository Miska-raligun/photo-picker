import { useState } from "react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { ExternalLink, X } from "lucide-react";
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

/// Full-viewport image preview. Implemented directly on Radix Dialog
/// primitives (not the shadcn `<Dialog>` wrapper) so we can:
///   - render outside the parent Dialog tree via Portal — pointer-down
///     events here don't bubble to the parent's "outside-click → close"
///     detector
///   - skip the auto-rendered shadcn close button (we have our own UX)
///   - keep our own large transparent-overlay layout
/// Radix natively handles nested-dialog focus + ESC routing so closing
/// only dismisses the top-most Dialog (this one).
export function Lightbox({ open, onOpenChange, previewUrl, thumbUrl, filename }: Props) {
  const m = useM();
  const [loaded, setLoaded] = useState(false);
  const [errored, setErrored] = useState(false);

  return (
    <DialogPrimitive.Root
      open={open}
      onOpenChange={(v) => {
        if (!v) {
          setLoaded(false);
          setErrored(false);
        }
        onOpenChange(v);
      }}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-[200] bg-black/85 backdrop-blur-sm data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=open]:fade-in-0 data-[state=closed]:fade-out-0" />
        <DialogPrimitive.Content
          className="fixed inset-0 z-[201] flex items-center justify-center p-4 outline-none"
          onOpenAutoFocus={(e) => e.preventDefault()}
        >
          {/* sr-only title for a11y */}
          <DialogPrimitive.Title className="sr-only">
            {filename ?? "preview"}
          </DialogPrimitive.Title>

          {/* Top bar */}
          <div className="absolute top-4 left-4 right-4 flex items-center justify-between gap-2 text-white">
            <div className="font-mono text-xs sm:text-sm truncate max-w-[60vw] bg-black/40 px-2 py-1 rounded">
              {filename ?? ""}
            </div>
            <div className="flex items-center gap-1.5">
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
              already-loaded thumbnail blurred + scaled as a backdrop. */}
          <div className="relative max-w-[96vw] max-h-[88vh] flex items-center justify-center pointer-events-none">
            {!loaded && !errored && thumbUrl && (
              <img
                src={thumbUrl}
                alt=""
                aria-hidden
                className="max-w-[96vw] max-h-[88vh] object-contain rounded-md shadow-2xl blur-md scale-105 opacity-90"
              />
            )}
            {errored && (
              <div className="text-white/80 text-sm font-mono bg-black/40 px-3 py-2 rounded pointer-events-auto">
                {m.detail.previewFailed ?? "failed to load preview"}
              </div>
            )}
            {previewUrl && (
              <img
                src={previewUrl}
                alt={filename ?? ""}
                onLoad={() => setLoaded(true)}
                onError={() => setErrored(true)}
                className={`max-w-[96vw] max-h-[88vh] object-contain rounded-md shadow-2xl pointer-events-auto ${
                  loaded ? "block" : thumbUrl ? "absolute inset-0 m-auto opacity-0" : "hidden"
                }`}
              />
            )}
          </div>

          <div className="absolute bottom-4 left-1/2 -translate-x-1/2 text-white/60 text-xs">
            ESC · click outside to close
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
