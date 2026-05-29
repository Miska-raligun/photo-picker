import { memo } from "react";
import { cn } from "@/lib/utils";
import type { CompositionPickView } from "@/lib/types";
import { api } from "@/lib/api";
import { useM } from "@/lib/i18n";
import { Thumb } from "./Thumb";

interface Props {
  runId: string;
  pick: CompositionPickView;
  overrides: Set<string>;
  onClick: () => void;
}

function GroupCardImpl({ runId, pick, overrides, onClick }: Props) {
  const m = useM();
  const total = pick.kept.length + pick.rejected.length;
  const rep = pick.kept[0] || pick.rejected[0];
  const thumb = rep ? api.thumbUrl(runId, rep.photo_id) : "";
  // Count of any user flips in this group (both directions).
  const overrideCount =
    pick.rejected.filter((r) => overrides.has(r.photo_id)).length +
    pick.kept.filter((k) => overrides.has(k.photo_id)).length;

  return (
    <button
      onClick={onClick}
      className="group shrink-0 w-44 cursor-pointer text-left transition-transform hover:-translate-y-1"
    >
      <div className="relative h-32 w-40 mx-auto">
        {total >= 3 && (
          // Use --muted (dark-mode-aware) instead of a hardcoded light oklch
          // so the back layer stays visible against dark backgrounds.
          <div className="absolute inset-0 rounded-lg border border-border bg-muted shadow-sm rotate-[-1.8deg] translate-x-[11px] translate-y-[9px]" />
        )}
        {total >= 2 && (
          <div className="absolute inset-0 rounded-lg border border-border bg-card shadow-sm rotate-[2.5deg] translate-x-[6px] translate-y-[5px]" />
        )}
        <div className="absolute inset-0 rounded-lg border border-border overflow-hidden bg-muted shadow-md z-10">
          {thumb && <Thumb src={thumb} />}
        </div>
        <span className="absolute z-20 top-2 right-2 bg-foreground/80 text-background text-[0.7rem] font-semibold font-mono px-2 py-0.5 rounded-full tabular-nums">
          {total}
        </span>
        <span
          className={cn(
            "absolute z-20 bottom-2 left-2 text-[0.65rem] font-semibold uppercase tracking-wider px-2 py-0.5 rounded text-white",
            pick.kept.length > 0 ? "bg-[var(--success)]" : "bg-foreground/60"
          )}
        >
          {pick.kept.length} {m.groupCard.keptSuffix}
        </span>
        {overrideCount > 0 && (
          <span className="absolute z-20 bottom-2 right-2 bg-primary text-primary-foreground text-[0.65rem] font-semibold uppercase tracking-wider px-2 py-0.5 rounded">
            +{overrideCount}
          </span>
        )}
      </div>
      <div className="mt-4 text-center space-y-0.5">
        <div className="text-xs font-mono text-muted-foreground">
          #{pick.index + 1} · {pick.scene}
        </div>
        <div className="text-xs text-foreground truncate px-1">
          {rep?.filename ?? "(empty)"}
        </div>
      </div>
    </button>
  );
}

/// Memoized: this card lives in a virtualized strip; without memo every
/// unrelated parent re-render (e.g. an override flip on a different group)
/// re-renders all visible cards. Cheap equality on `pick` (stable ref from
/// the report) + `overrides` (a Set — stable until `setOverrides` runs).
export const GroupCard = memo(GroupCardImpl);
