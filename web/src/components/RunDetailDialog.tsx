import { useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { LucideIcon } from "lucide-react";
import {
  CheckCircle2,
  Clock,
  ClipboardCopy,
  Database,
  Download,
  ExternalLink,
  FolderClosed,
  GitCompareArrows,
  Images,
  Layers,
  LayoutGrid,
  Copy,
  Loader2,
  Upload,
  XCircle,
} from "lucide-react";
import { toast } from "sonner";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { GroupCard } from "./GroupCard";
import { ApplyBar } from "./ApplyBar";
import { ExportDialog } from "./ExportDialog";
import { Thumb } from "./Thumb";
import { api } from "@/lib/api";
import { useM } from "@/lib/i18n";
import type {
  ApplyResult,
  DuplicateReport,
  PhotoTag,
  RunDiff,
  RunDiffEntry,
  RunRecord,
} from "@/lib/types";
import { cn } from "@/lib/utils";

interface Props {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  run: RunRecord | null;
  /// Other completed runs, offered as comparison targets ("did my parameter
  /// tweak keep different photos?").
  otherRuns: RunRecord[];
  overrides: Set<string>;
  /// User flags/notes for this run — forwarded to the export dialog for
  /// optional XMP sidecars.
  tags: Map<string, PhotoTag>;
  onOpenGroup: (pickIndex: number) => void;
  onApplyDone: (result: ApplyResult) => void;
}

export function RunDetailDialog({
  open,
  onOpenChange,
  run,
  otherRuns,
  overrides,
  tags,
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

          {isCompleted && <DuplicatesPanel runId={run.id} />}

          {isCompleted && picks.length > 0 && otherRuns.length > 0 && (
            <RunCompare runId={run.id} otherRuns={otherRuns} />
          )}

          <div className="flex items-center justify-between gap-2 pt-2 border-t">
            <div className="flex items-center gap-1 flex-wrap">
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
              {/* Direct download of the canonical on-disk report.json.
                  Useful when sharing a run with someone for debugging or
                  feeding the picks into a downstream tool. */}
              <Button asChild variant="link" size="sm">
                <a
                  href={api.reportJsonUrl(run.id)}
                  download={`photo-pick-${run.id}.report.json`}
                >
                  <Download className="h-3 w-3" />
                  {m.runDetail.downloadJson}
                </a>
              </Button>
              {/* Copy the path of the on-disk HTML report — handy when
                  the user wants to open it in a different browser /
                  share the artifact location with a teammate. */}
              {run.html_report && (
                <Button
                  variant="link"
                  size="sm"
                  onClick={async () => {
                    try {
                      await navigator.clipboard.writeText(run.html_report!);
                      toast.success(m.runDetail.copiedHtmlPath);
                    } catch {
                      toast.error(m.runDetail.copyFailed);
                    }
                  }}
                >
                  <ClipboardCopy className="h-3 w-3" />
                  {m.runDetail.copyHtmlPath}
                </Button>
              )}
            </div>
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
        tags={tags}
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

/// Cross-run keep-set comparison. Photos are matched across runs by content
/// hash on the server, so "same file, different verdict" and "file only in
/// one run" are distinguished (the latter gets a dashed ring).
function RunCompare({ runId, otherRuns }: { runId: string; otherRuns: RunRecord[] }) {
  const m = useM();
  const [otherId, setOtherId] = useState<string>("");
  const [diff, setDiff] = useState<RunDiff | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function selectOther(id: string) {
    setOtherId(id);
    setDiff(null);
    setError(null);
    if (!id) return;
    setLoading(true);
    api
      .diffRuns(runId, id)
      .then(setDiff)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }

  return (
    <div className="rounded-lg border bg-card/50 px-4 py-3 space-y-3">
      <div className="flex items-center gap-2 flex-wrap">
        <GitCompareArrows className="h-4 w-4 text-muted-foreground shrink-0" />
        <span className="text-sm font-medium">{m.compare.title}</span>
        <Select value={otherId} onValueChange={selectOther}>
          <SelectTrigger className="h-8 w-72 text-xs">
            <SelectValue placeholder={m.compare.pickRun} />
          </SelectTrigger>
          <SelectContent>
            {otherRuns.map((r) => (
              <SelectItem key={r.id} value={r.id}>
                <span className="font-mono">{r.id.slice(0, 8)}</span>
                <span className="text-muted-foreground"> · {r.root}</span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {loading && <Loader2 className="h-3.5 w-3.5 animate-spin text-muted-foreground" />}
        {diff && (
          <span className="text-xs text-muted-foreground font-mono tabular-nums ml-auto">
            {m.compare.keptCounts(diff.kept_here, diff.kept_there)}
          </span>
        )}
      </div>
      {error && (
        <div className="text-xs text-muted-foreground bg-muted/60 rounded px-2.5 py-1.5">
          {error}
        </div>
      )}
      {diff && diff.added_kept.length === 0 && diff.removed_kept.length === 0 && (
        <div className="text-xs text-muted-foreground">{m.compare.identical}</div>
      )}
      {diff && diff.added_kept.length > 0 && (
        <DiffStrip
          label={m.compare.addedKept(diff.added_kept.length)}
          entries={diff.added_kept}
          tone="add"
          onlyHereHint={m.compare.notInOther}
        />
      )}
      {diff && diff.removed_kept.length > 0 && (
        <DiffStrip
          label={m.compare.removedKept(diff.removed_kept.length)}
          entries={diff.removed_kept}
          tone="remove"
          onlyHereHint={m.compare.notInOther}
        />
      )}
      {diff && (diff.photos_only_here > 0 || diff.photos_only_there > 0) && (
        <div className="text-[0.7rem] text-muted-foreground">
          {m.compare.fileDelta(diff.photos_only_here, diff.photos_only_there)}
        </div>
      )}
    </div>
  );
}

function DiffStrip({
  label,
  entries,
  tone,
  onlyHereHint,
}: {
  label: string;
  entries: RunDiffEntry[];
  tone: "add" | "remove";
  onlyHereHint: string;
}) {
  const MAX_SHOWN = 24;
  return (
    <div className="space-y-1.5">
      <div
        className={cn(
          "text-xs font-medium",
          tone === "add" ? "text-[var(--success)]" : "text-destructive"
        )}
      >
        {label}
      </div>
      <div className="flex gap-2 overflow-x-auto pb-1">
        {entries.slice(0, MAX_SHOWN).map((e) => (
          <div
            key={`${e.run_id}:${e.photo_id}`}
            className={cn(
              "relative shrink-0 w-20 h-20 rounded-md overflow-hidden border-2",
              tone === "add" ? "border-[var(--success)]/60" : "border-destructive/60",
              !e.present_in_other && "border-dashed"
            )}
            title={
              (e.filename ?? e.photo_id) + (e.present_in_other ? "" : ` — ${onlyHereHint}`)
            }
          >
            <Thumb src={api.thumbUrl(e.run_id, e.photo_id)} alt={e.filename ?? e.photo_id} />
          </div>
        ))}
        {entries.length > MAX_SHOWN && (
          <div className="shrink-0 w-20 h-20 rounded-md border border-dashed grid place-items-center text-xs text-muted-foreground">
            +{entries.length - MAX_SHOWN}
          </div>
        )}
      </div>
    </div>
  );
}


/// Byte-identical duplicate sets in this run. Lazy — the scan already hashed
/// every file, but users shouldn't pay a request unless they ask.
function DuplicatesPanel({ runId }: { runId: string }) {
  const m = useM();
  const [report, setReport] = useState<DuplicateReport | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function scan() {
    setLoading(true);
    setError(null);
    api
      .duplicates(runId)
      .then(setReport)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }

  return (
    <div className="rounded-lg border bg-card/50 px-4 py-3 space-y-2">
      <div className="flex items-center gap-2 flex-wrap">
        <Copy className="h-4 w-4 text-muted-foreground shrink-0" />
        <span className="text-sm font-medium">{m.duplicates.title}</span>
        <Button variant="outline" size="sm" className="h-7 text-xs" onClick={scan} disabled={loading}>
          {loading ? <Loader2 className="h-3 w-3 animate-spin" /> : null}
          {report ? m.duplicates.rescan : m.duplicates.scan}
        </Button>
        {report && (
          <span className="text-xs text-muted-foreground font-mono tabular-nums ml-auto">
            {m.duplicates.summary(report.groups.length, report.redundant_count)}
          </span>
        )}
      </div>
      {error && <div className="text-xs text-muted-foreground">{error}</div>}
      {report && report.groups.length === 0 && (
        <div className="text-xs text-muted-foreground">{m.duplicates.none}</div>
      )}
      {report && report.groups.length > 0 && (
        <div className="space-y-2 max-h-64 overflow-y-auto">
          {report.groups.slice(0, 20).map((g, i) => (
            <div key={i} className="flex gap-2 items-start">
              <div className="shrink-0 w-16 h-16 rounded overflow-hidden border">
                <Thumb
                  src={api.thumbUrl(runId, g.photos[0].photo_id)}
                  alt={g.photos[0].filename ?? ""}
                />
              </div>
              <ul className="text-[0.7rem] font-mono text-muted-foreground min-w-0 flex-1 space-y-0.5">
                {g.photos.map((ph) => (
                  <li key={ph.photo_id} className="truncate" title={ph.path}>
                    {ph.path}
                  </li>
                ))}
              </ul>
            </div>
          ))}
          {report.groups.length > 20 && (
            <div className="text-xs text-muted-foreground italic">
              … +{report.groups.length - 20}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
