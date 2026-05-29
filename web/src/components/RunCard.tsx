import { memo } from "react";
import type { LucideIcon } from "lucide-react";
import {
  AlertCircle,
  ArrowRight,
  CheckCircle2,
  Clock,
  Database,
  FolderClosed,
  Images,
  Layers,
  LayoutGrid,
  Loader2,
  XCircle,
} from "lucide-react";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useM } from "@/lib/i18n";
import type { RunProgress, RunRecord } from "@/lib/types";
import { cn } from "@/lib/utils";

interface Props {
  run: RunRecord;
  progress?: RunProgress | null;
  onOpenDetail: (runId: string) => void;
}

function RunCardImpl({ run, progress, onOpenDetail }: Props) {
  const m = useM();
  const state = run.status.state;
  const error = state === "failed" ? run.status.error : null;
  const report = run.report;

  const statusBadge = (() => {
    if (state === "running")
      return (
        <Badge className="bg-accent text-accent-foreground gap-1.5">
          <Loader2 className="h-3 w-3 animate-spin" />
          {m.runCard.running}
        </Badge>
      );
    if (state === "completed")
      return (
        <Badge className="bg-[var(--success)] text-white gap-1.5">
          <CheckCircle2 className="h-3 w-3" />
          {m.runCard.completed}
        </Badge>
      );
    return (
      <Badge variant="destructive" className="gap-1.5">
        <AlertCircle className="h-3 w-3" />
        {m.runCard.failed}
      </Badge>
    );
  })();

  const heading =
    state === "completed"
      ? m.runCard.scanComplete
      : state === "failed"
      ? m.runCard.scanFailed
      : m.runCard.scanInProgress;

  return (
    <Card
      className={cn(
        "transition-all duration-200",
        state === "completed" &&
          "cursor-pointer hover:shadow-md hover:-translate-y-0.5 hover:border-primary/20"
      )}
      onClick={() => state === "completed" && onOpenDetail(run.id)}
    >
      <CardHeader className="space-y-2 pb-3">
        <div className="flex items-center gap-2.5 flex-wrap">
          {statusBadge}
          <span className="font-semibold text-sm">{heading}</span>
          <code className="font-mono text-xs bg-muted px-1.5 py-0.5 rounded text-muted-foreground">
            {run.id.slice(0, 8)}
          </code>
        </div>
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground min-w-0">
          <FolderClosed className="h-3.5 w-3.5 shrink-0" />
          <span className="font-mono truncate">{run.root}</span>
        </div>
      </CardHeader>

      <CardContent className="space-y-3">
        {state === "running" && (
          <div className="space-y-1.5">
            <div className="flex items-center justify-between text-xs text-muted-foreground">
              <span className="font-mono">{progress?.stage ?? m.runCard.starting}</span>
              {progress && progress.total > 0 && (
                <span className="tabular-nums">
                  {progress.done} / {progress.total}
                </span>
              )}
            </div>
            <div className="h-1.5 rounded bg-muted overflow-hidden">
              {progress && progress.total > 0 ? (
                <div
                  className="h-full bg-primary transition-[width] duration-200"
                  style={{
                    width: `${Math.min(100, (progress.done / progress.total) * 100)}%`,
                  }}
                />
              ) : (
                // Indeterminate bar for stages without per-item progress
                // (Cluster/Score/StageB/FinalSelect) and for the brief
                // window before the first SSE event lands.
                <div className="h-full w-1/3 bg-primary animate-[indeterminate_1.2s_ease-in-out_infinite] rounded" />
              )}
            </div>
          </div>
        )}

        {report && (
          <div className="flex flex-wrap gap-1.5">
            <StatPill icon={Images} label={m.runCard.statPhotos} value={report.photo_count} />
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

        {error && (
          <div className="text-destructive text-sm font-mono bg-destructive/10 border border-destructive/20 rounded-md p-3 whitespace-pre-wrap">
            {error}
          </div>
        )}

        {state === "completed" && (
          <div className="flex justify-end">
            <Button
              variant="outline"
              size="sm"
              onClick={(e) => {
                e.stopPropagation();
                onOpenDetail(run.id);
              }}
            >
              {m.runCard.viewResults}
              <ArrowRight className="h-3 w-3" />
            </Button>
          </div>
        )}
      </CardContent>
    </Card>
  );
}

/// Memoized: with a stable `onOpenDetail` and `EMPTY_OVERRIDES`, non-running
/// cards skip re-render on every SSE progress tick of a sibling run.
export const RunCard = memo(RunCardImpl);

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
