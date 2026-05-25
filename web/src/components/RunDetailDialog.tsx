import { ExternalLink, Loader2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { ScrollArea, ScrollBar } from "@/components/ui/scroll-area";
import { GroupCard } from "./GroupCard";
import { ApplyBar } from "./ApplyBar";
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
  if (!run) return null;
  const report = run.report;
  const picks = run.composition_picks ?? [];
  const isRunning = run.status.state === "running";
  const isFailed = run.status.state === "failed";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="!max-w-[98vw] sm:!max-w-[96vw] !w-[96vw] !h-[94vh] max-h-[94vh] flex flex-col gap-0 p-0">
        <DialogHeader className="px-6 pt-5 pb-4 border-b">
          <DialogTitle className="flex items-center gap-2.5 flex-wrap">
            <span>{m.runCard.taskDetails}</span>
            <code className="font-mono text-xs bg-muted px-1.5 py-0.5 rounded text-muted-foreground">
              {run.id.slice(0, 8)}
            </code>
            {run.in_place && (
              <Badge variant="outline" className="text-[0.65rem] h-5">
                {m.runCard.inPlace}
              </Badge>
            )}
          </DialogTitle>
          <div className="text-xs font-mono text-muted-foreground break-all">
            {run.root} <span className="text-foreground">→</span> {run.output}
          </div>
        </DialogHeader>

        <div className="px-6 py-4 flex-1 overflow-y-auto space-y-5">
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

          {picks.length > 0 && (
            <>
              <Separator />
              <ScrollArea className="w-full">
                <div className="flex gap-6 py-3">
                  {picks.map((p) => (
                    <GroupCard
                      key={p.index}
                      runId={run.id}
                      pick={p}
                      overrides={overrides}
                      onClick={() => onOpenGroup(p.index)}
                    />
                  ))}
                </div>
                <ScrollBar orientation="horizontal" />
              </ScrollArea>
            </>
          )}

          {isRunning && (
            <div className="flex items-center justify-center gap-2 text-muted-foreground py-12 border border-dashed rounded-xl">
              <Loader2 className="h-4 w-4 animate-spin" />
              <span className="text-sm">{m.runCard.scanInProgress}…</span>
            </div>
          )}

          {!isRunning && !isFailed && picks.length === 0 && (
            <div className="rounded-xl border border-dashed text-center text-muted-foreground italic text-sm py-12">
              {m.runCard.emptyGroups}
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

          <div className="flex justify-end pt-2 border-t">
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
          </div>
        </div>
      </DialogContent>
    </Dialog>
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
