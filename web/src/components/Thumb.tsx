import { useState } from "react";
import { ImageOff } from "lucide-react";
import { cn } from "@/lib/utils";

/// Thumbnail image with a shimmer skeleton until it decodes and a graceful
/// broken-image state on error. Fills its parent (the parent owns sizing /
/// aspect-ratio / rounding). Reuses the `.shimmer` keyframe from index.css —
/// which the friendly-UI PR added but never wired to anything.
export function Thumb({
  src,
  alt,
  className,
  imgClassName,
}: {
  src: string;
  alt?: string;
  /// Wrapper classes — positioning / sizing. Defaults to filling the parent.
  className?: string;
  /// Extra classes on the <img> (object-fit etc.). Defaults to object-cover.
  imgClassName?: string;
}) {
  const [loaded, setLoaded] = useState(false);
  const [errored, setErrored] = useState(false);

  return (
    <div className={cn("relative h-full w-full overflow-hidden", className)}>
      {!loaded && !errored && (
        <div className="absolute inset-0 shimmer" aria-hidden />
      )}
      {errored ? (
        <div className="absolute inset-0 grid place-items-center bg-muted text-muted-foreground/50">
          <ImageOff className="h-5 w-5" />
        </div>
      ) : (
        <img
          loading="lazy"
          src={src}
          alt={alt ?? ""}
          draggable={false}
          onLoad={() => setLoaded(true)}
          onError={() => setErrored(true)}
          className={cn(
            "h-full w-full object-cover transition-opacity duration-300 ease-out",
            loaded ? "opacity-100" : "opacity-0",
            imgClassName
          )}
        />
      )}
    </div>
  );
}
