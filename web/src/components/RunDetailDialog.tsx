import { useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { LucideIcon } from "lucide-react";
import {
  CheckCircle2,
  Clock,
  Database,
  ExternalLink,
  FolderClosed,
  Images,
  Layers,
  LayoutGrid,
  Loader2,
  Upload,
  XCircle,
} from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { GroupCard } from "./GroupCard";
import { ApplyBar } from "./ApplyBar";
import { ExportDialog } from "./ExportDialog";
import { api } from "@/lib/api";
import { useM } from "@/lib/i18n";
import type { RunRecord } from "@/lib/types";
import { cn } from "@/lib/utils";

interface Props {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  run: RunRecord | null;
  overrides: Set<string>;
  onOpenGroup: (pickIndex: number) => void;
  onApplyDone: () => void;
}

export function RunDetailDialog({
  open,
  onOpenChange,
  run,
  overrides,
  onOpenGroup,
  onApplyDone,
}: Props) {
  const m = useM();
  const [exportOpen, setExportOpen] = useState(false);
  if (!run) return null;
  const report = run.report;
  const picks = run.composition_picks ?? [];
  const isRunning = run.status.state === "running";
  const isFailed = run.status.state === "failed";
  const isCompleted = run.status.state === "completed";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="!max-w-[98vw] sm:!max-w-[96vw] !w-[96vw] !h-[94vh] max-h-[94vh] flex flex-col gap-0 p-0">
        <DialogHeader className="px-6 pt-5 pb-4 border-b">
          <DialogTitle className="flex items-center gap-2.5 flex-wrap">
            <span>{m.runCard.taskDetails}</span>
            <code className="font-mono text-xs bg-muted px-1.5 py-0.5 rounded text-muted-foreground">
              {run.id.slice(0, 8)}
            </code>
          </DialogTitle>
          <div className="flex items-center gap-1.5 text-xs font-mono text-muted-foreground min-w-0">
            <FolderClosed className="h-3.5 w-3.5 shrink-0" />
            <span className="break-all">{run.root}</span>
          </div>
        </DialogHeader>

        <div className="px-6 py-4 flex-1 overflow-y-auto space-y-5">
          {report && (
            <div className="flex flex-wrap gap-1.5">
              <StatPill
                icon={Images}
                label={m.runCard.statPhotos}
                value={report.photo_count}
              />
              <StatPill
                icon={Database}
                label={m.runCard.statCache}
                value={`${report.cached_count}/${report.photo_count}`}
                accent={report.cached_count > 0 ? "success" : undefined}
              />
              <StatPill
                icon={Layers}
                label={m.runCard.statBursts}
                value={report.stage_a_group_count}
              />
              <StatPill
                icon={LayoutGrid}
                label={m.runCard.statCompGroups}
                value={report.stage_b_group_count}
              />
              <StatPill
                icon={CheckCircle2}
                label={m.runCard.statKept}
                value={report.picked_count}
                accent="success"
              />
              <StatPill
                icon={XCircle}
                label={m.runCard.statRejected}
                value={report.rejected_count}
              />
              <StatPill
                icon={Clock}
                label={m.runCard.statElapsed}
                value={`${(report.elapsed.secs + report.elapsed.nanos / 1e9).toFixed(2)}s`}
              />
            </div>
          )}

          {picks.length > 0 && (
            <>
              <Separator />
              <VirtualGroupStrip
                runId={run.id}
                picks={picks}
                overrides={overrides}
                onOpenGroup={onOpenGroup}
              />
            </>
          )}

          {isRunning && (
            <div className="space-y-3">
              <div className="flex items-center gap-2 text-muted-foreground text-sm">
                <Loader2 className="h-4 w-4 animate-spin" />
                <span>{m.runCard.scanInProgress}…</span>
              </div>
              {/* Skeleton group strip so the dialog isn't empty while the
                  scan runs — uses the shimmer keyframe from the friendly-UI PR. */}
              <div className="flex gap-6 py-1 overflow-hidden">
                {Array.from({ length: 6 }).map((_, i) => (
                  <div key={i} className="shrink-0 w-44">
                    <div className="h-32 w-40 mx-auto rounded-lg shimmer" />
                    <div className="mt-4 mx-auto h-3 w-24 rounded shimmer" />
                    <div className="mt-2 mx-auto h-3 w-28 rounded shimmer" />
                  </div>
                ))}
              </div>
            </div>
          )}

          {!isRunning && !isFailed && picks.length === 0 && (
            <div className="rounded-xl border border-dashed bg-card/40 py-12 px-6 flex flex-col items-center text-center gap-3">
              <span className="grid place-items-center h-12 w-12 rounded-full bg-muted text-muted-foreground">
                <LayoutGrid className="h-6 w-6" />
              </span>
              <p className="text-sm text-muted-foreground">{m.runCard.emptyGroups}</p>
            </div>
          )}

          {isFailed && run.status.state === "failed" && (
            <div className="text-destructive text-sm font-mono bg-destructive/10 border border-destructive/20 rounded-md p-3 whitespace-pre-wrap">
              {run.status.error}
            </div>
          )}

          {run.in_place && picks.length > 0 && (
            <ApplyBar
              runId={run.id}
              picks={picks}
              overrides={overrides}
              sourceRoot={run.root}
              onDone={onApplyDone}
            />
          )}

          <div className="flex items-center justify-between gap-2 pt-2 border-t">
            <Button asChild variant="link" size="sm">
              <a
                href={api.htmlReportUrl(run.id)}
                target="_blank"
                rel="noreferrer"
              >
                {m.runCard.openHtmlReport}
                <ExternalLink className="h-3 w-3" />
              </a>
            </Button>
            {isCompleted && picks.length > 0 && (
              <Button variant="outline" size="sm" onClick={() => setExportOpen(true)}>
                <Upload className="h-4 w-4" />
                {m.export.button}
              </Button>
            )}
          </div>
        </div>
      </DialogContent>

      <ExportDialog
        open={exportOpen}
        onOpenChange={setExportOpen}
        runId={run.id}
        picks={picks}
        overrides={overrides}
      />
    </Dialog>
  );
}

/// Horizontally-virtualized strip of `GroupCard`s. For long shoots the
/// composition-picks list can run into the hundreds; rendering them all
/// builds a multi-MB DOM and stutters scroll. The virtualizer keeps only
/// the visible window mounted (plus a small overscan).
function VirtualGroupStrip({
  runId,
  picks,
  overrides,
  onOpenGroup,
}: {
  runId: string;
  picks: RunRecord["composition_picks"] extends infer T ? (T extends undefined ? never : T) : never;
  overrides: Set<string>;
  onOpenGroup: (pickIndex: number) => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  // GroupCard is `w-44` (176px) inside its button; we add 24px gap → 200px slot.
  const ITEM_W = 200;
  const CARD_AREA_H = 200; // card + label rows
  const virt = useVirtualizer({
    count: picks.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ITEM_W,
    horizontal: true,
    overscan: 6,
  });
  return (
    <div ref={scrollRef} className="w-full overflow-x-auto py-3">
      <div
        style={{
          width: `${virt.getTotalSize()}px`,
          height: `${CARD_AREA_H}px`,
          position: "relative",
        }}
      >
        {virt.getVirtualItems().map((vi) => {
          const p = picks[vi.index];
          return (
            <div
              key={p.index}
              style={{
                position: "absolute",
                left: 0,
                top: 0,
                transform: `translateX(${vi.start}px)`,
                width: `${vi.size}px`,
              }}
            >
              <GroupCard
                runId={runId}
                pick={p}
                overrides={overrides}
                onClick={() => onOpenGroup(p.index)}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}

function StatPill({
  icon: Icon,
  label,
  value,
  accent,
}: {
  icon: LucideIcon;
  label: string;
  value: string | number;
  accent?: "success";
}) {
  return (
    <div
      className={cn(
        "inline-flex items-center gap-1.5 rounded-md border bg-muted/50 px-2.5 py-1 text-xs",
        accent === "success" && "border-[var(--success)]/40 bg-[var(--success)]/5"
      )}
    >
      <Icon
        className={cn(
          "h-3.5 w-3.5",
          accent === "success" ? "text-[var(--success)]" : "text-muted-foreground"
        )}
      />
      <span className="text-muted-foreground">{label}</span>
      <span className="font-semibold tabular-nums">{value}</span>
    </div>
  );
}
