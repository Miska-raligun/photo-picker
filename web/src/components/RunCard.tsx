import { useEffect, useMemo, useState } from "react";
import type { LucideIcon } from "lucide-react";
import {
  AlertCircle,
  ArrowRight,
  Ban,
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
import { toast } from "sonner";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";
import { useM } from "@/lib/i18n";
import type { RunProgress, RunRecord } from "@/lib/types";
import { cn } from "@/lib/utils";

/// Sliding-window ETA. Holds the last N tick samples (timestamp + done) so
/// we can fit a recent throughput line through them instead of averaging
/// over an entire stage — the first few ticks are usually slower (warmup,
/// model load) and drag a global ETA way off.
const ETA_WINDOW = 5;
interface EtaSample {
  ts: number;
  done: number;
  stage: string;
}

function formatEta(sec: number): string {
  if (sec >= 3600) {
    const h = Math.floor(sec / 3600);
    const m = Math.floor((sec % 3600) / 60);
    return `~${h}h ${m}m`;
  }
  if (sec >= 60) {
    const min = Math.floor(sec / 60);
    const s = sec % 60;
    return `~${min}m ${s}s`;
  }
  return `~${Math.max(1, sec)}s`;
}

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

  // Throughput samples drive the ETA estimate. Reset on stage change so a
  // fast Score phase doesn't get extrapolated into a slow Features phase.
  const [samples, setSamples] = useState<EtaSample[]>([]);
  useEffect(() => {
    if (!progress || progress.total === 0) {
      if (samples.length !== 0) setSamples([]);
      return;
    }
    setSamples((prev) => {
      const last = prev[prev.length - 1];
      if (last && last.stage !== progress.stage) {
        return [{ ts: Date.now(), done: progress.done, stage: progress.stage }];
      }
      if (last && last.done === progress.done) return prev;
      const next = [...prev, { ts: Date.now(), done: progress.done, stage: progress.stage }];
      return next.length > ETA_WINDOW ? next.slice(-ETA_WINDOW) : next;
    });
    // samples intentionally not depended on — we only react to incoming ticks.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [progress?.done, progress?.total, progress?.stage]);

  const etaSec = useMemo<number | null>(() => {
    if (!progress || progress.total <= 0 || samples.length < 2) return null;
    const first = samples[0];
    const last = samples[samples.length - 1];
    if (last.stage !== progress.stage) return null;
    const dt = (last.ts - first.ts) / 1000;
    const dd = last.done - first.done;
    if (dd <= 0 || dt <= 0) return null;
    const rate = dd / dt;
    const remaining = progress.total - progress.done;
    if (remaining <= 0) return null;
    return Math.round(remaining / rate);
  }, [samples, progress?.total, progress?.done, progress?.stage]);

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
    if (state === "cancelled")
      return (
        <Badge variant="secondary" className="gap-1.5">
          <Ban className="h-3 w-3" />
          {m.runCard.cancelled}
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
      : state === "cancelled"
      ? m.runCard.scanCancelled
      : m.runCard.scanInProgress;

  const [cancelling, setCancelling] = useState(false);
  function requestCancel(e: React.MouseEvent) {
    e.stopPropagation();
    setCancelling(true);
    api.cancelRun(run.id).catch((err) => {
      setCancelling(false);
      toast.error(m.runCard.cancelFailed, {
        description: err instanceof Error ? err.message : String(err),
      });
    });
    // No success toast — the card's status flips to "cancelled" on its own
    // via the SSE done event, which is the real confirmation.
  }

  return (
    <Card
      className={cn(
        "transition-all duration-200",
        state === "completed" &&
          "cursor-pointer hover:shadow-md hover:-translate-y-0.5 hover:border-primary/20"
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
        </div>
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground min-w-0">
          <FolderClosed className="h-3.5 w-3.5 shrink-0" />
          <span className="font-mono truncate" title={run.root}>
            {run.root}
          </span>
        </div>
      </CardHeader>

      <CardContent className="space-y-3">
        {state === "running" && (
          <div className="space-y-1.5">
            <div className="flex items-center justify-between text-xs text-muted-foreground">
              <span className="font-mono flex items-center gap-2">
                {progress?.stage ?? m.runCard.starting}
                <button
                  type="button"
                  onClick={requestCancel}
                  disabled={cancelling}
                  className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 border border-border text-muted-foreground hover:text-destructive hover:border-destructive/40 disabled:opacity-50 transition-colors"
                  title={m.runCard.cancelTitle}
                >
                  <Ban className="h-3 w-3" />
                  {cancelling ? m.runCard.cancelling : m.runCard.cancel}
                </button>
              </span>
              {progress && progress.total > 0 && (
                <span className="tabular-nums">
                  {progress.done} / {progress.total}
                  {etaSec != null && etaSec > 1 && (
                    <span className="ml-2 text-muted-foreground/60">
                      {formatEta(etaSec)}
                    </span>
                  )}
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
