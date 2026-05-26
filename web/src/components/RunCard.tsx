import { ArrowRight, Loader2, AlertCircle, CheckCircle2 } from "lucide-react";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useM } from "@/lib/i18n";
import type { RunProgress, RunRecord } from "@/lib/types";
import { cn } from "@/lib/utils";

interface Props {
  run: RunRecord;
  progress?: RunProgress | null;
  onOpenDetail: () => void;
}

export function RunCard({ run, progress, onOpenDetail }: Props) {
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
        "transition-shadow",
        state === "completed" && "cursor-pointer hover:shadow-md"
      )}
      onClick={() => state === "completed" && onOpenDetail()}
    >
      <CardHeader className="space-y-2 pb-3">
        <div className="flex items-center gap-2.5 flex-wrap">
          {statusBadge}
          <span className="font-semibold text-sm">{heading}</span>
          <code className="font-mono text-xs bg-muted px-1.5 py-0.5 rounded text-muted-foreground">
            {run.id.slice(0, 8)}
          </code>
          {run.in_place && (
            <Badge variant="outline" className="text-[0.65rem] h-5">
              {m.runCard.inPlace}
            </Badge>
          )}
        </div>
        <div className="text-xs font-mono text-muted-foreground break-all">
          {run.root} <span className="text-foreground">→</span> {run.output}
        </div>
      </CardHeader>

      <CardContent className="space-y-3">
        {state === "running" && progress && (
          <div className="space-y-1.5">
            <div className="flex items-center justify-between text-xs text-muted-foreground">
              <span className="font-mono">{progress.stage}</span>
              <span className="tabular-nums">
                {progress.total > 0
                  ? `${progress.done} / ${progress.total}`
                  : `${progress.done}`}
              </span>
            </div>
            <div className="h-1.5 rounded bg-muted overflow-hidden">
              <div
                className="h-full bg-primary transition-[width] duration-200"
                style={{
                  width:
                    progress.total > 0
                      ? `${Math.min(100, (progress.done / progress.total) * 100)}%`
                      : "20%",
                }}
              />
            </div>
          </div>
        )}

        {report && (
          <div className="flex flex-wrap gap-1.5">
            <StatPill label={m.runCard.statPhotos} value={report.photo_count} />
            <StatPill
              label={m.runCard.statCache}
              value={`${report.cached_count}/${report.photo_count}`}
              accent={report.cached_count > 0 ? "success" : undefined}
            />
            <StatPill
              label={m.runCard.statBursts}
              value={report.stage_a_group_count}
            />
            <StatPill
              label={m.runCard.statCompGroups}
              value={report.stage_b_group_count}
            />
            <StatPill label={m.runCard.statKept} value={report.picked_count} />
            <StatPill
              label={m.runCard.statRejected}
              value={report.rejected_count}
            />
            <StatPill
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
                onOpenDetail();
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

function StatPill({
  label,
  value,
  accent,
}: {
  label: string;
  value: string | number;
  accent?: "success";
}) {
  return (
    <div
      className={cn(
        "inline-flex items-center gap-1.5 rounded-md border bg-muted/50 px-2.5 py-1 text-xs",
        accent === "success" && "border-[var(--success)]"
      )}
    >
      <span className="text-muted-foreground">{label}</span>
      <span className="font-semibold tabular-nums">{value}</span>
    </div>
  );
}
