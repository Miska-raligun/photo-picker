import { useState } from "react";
import { AlertTriangle, Loader2, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { api } from "@/lib/api";
import { useM } from "@/lib/i18n";
import type { CompositionPickView } from "@/lib/types";
import { toast } from "sonner";

interface Props {
  runId: string;
  picks: CompositionPickView[];
  overrides: Set<string>;
  sourceRoot: string;
  onDone: () => void;
}

export function ApplyBar({ runId, picks, overrides, sourceRoot, onDone }: Props) {
  const m = useM();
  // Final delete list = (algo-rejected & not flipped) + (algo-kept & flipped)
  const algoRejected = picks.flatMap((p) => p.rejected.map((r) => r.photo_id));
  const algoKept = picks.flatMap((p) => p.kept.map((k) => k.photo_id));
  const toDelete = [
    ...algoRejected.filter((id) => !overrides.has(id)),
    ...algoKept.filter((id) => overrides.has(id)),
  ];
  const overrideCount = overrides.size;

  const [confirmOpen, setConfirmOpen] = useState(false);
  const [mode, setMode] = useState<"trash" | "delete">("trash");
  const [submitting, setSubmitting] = useState(false);
  const [done, setDone] = useState(false);

  async function execute() {
    setSubmitting(true);
    try {
      const r = await api.apply(runId, toDelete, mode === "trash");
      const verb = r.used_trash
        ? m.applyBar.toastMovedToTrash
        : m.applyBar.toastDeleted;
      const msg = `${r.deleted} / ${r.requested} ${verb}`;
      if (r.failed.length === 0) {
        toast.success(msg);
      } else {
        toast.warning(`${msg} — ${r.failed.length} ${m.applyBar.toastFailedSuffix}`, {
          description: r.failed
            .slice(0, 3)
            .map((f) => `${f.path.split("/").pop()}: ${f.error}`)
            .join("\n"),
        });
      }
      setDone(true);
      setConfirmOpen(false);
      onDone();
    } catch (e) {
      toast.error(m.applyBar.toastApplyFailed, {
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <>
      <div className="rounded-lg border border-primary/30 bg-accent px-4 py-3 flex items-center gap-3 flex-wrap">
        <AlertTriangle className="h-4 w-4 text-primary shrink-0" />
        <div className="text-sm flex-1 min-w-0">
          {m.applyBar.willDelete}{" "}
          <strong className="text-accent-foreground">{toDelete.length}</strong>{" "}
          {toDelete.length === 1
            ? m.applyBar.rejectedFile
            : m.applyBar.rejectedFiles}{" "}
          {m.applyBar.fromSource}
          {overrideCount > 0 && (
            <span className="text-muted-foreground">
              {" "}
              ({overrideCount} {m.applyBar.keptByOverride})
            </span>
          )}
        </div>
        <Button
          variant="default"
          onClick={() => setConfirmOpen(true)}
          disabled={toDelete.length === 0 || done}
        >
          <Trash2 className="h-4 w-4" />
          {done ? m.applyBar.applied : m.applyBar.applyN(toDelete.length)}
        </Button>
      </div>

      <Dialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>{m.applyBar.confirmTitle}</DialogTitle>
            <DialogDescription>
              {m.applyBar.confirmDescPrefix}{" "}
              <code className="font-mono text-xs bg-muted px-1.5 py-0.5 rounded break-all">
                {sourceRoot}
              </code>{" "}
              {m.applyBar.confirmDescSuffix}{" "}
              <strong>{toDelete.length}</strong>{" "}
              {toDelete.length === 1 ? m.applyBar.rejectedFile : m.applyBar.rejectedFiles}.
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-2 py-2">
            <p className="text-xs text-muted-foreground">
              {overrideCount}{" "}
              {m.applyBar.confirmOverrideNote}{" "}
              <strong>{m.applyBar.confirmKeptWord}</strong>
              {m.applyBar.confirmDueOverride}
            </p>
            <div className="space-y-2 pt-2">
              <Label className="text-sm flex items-center gap-2">
                <input
                  type="radio"
                  name="apply-mode"
                  checked={mode === "trash"}
                  onChange={() => setMode("trash")}
                  className="accent-primary"
                />
                {m.applyBar.sendToTrash}
              </Label>
              <Label className="text-sm flex items-center gap-2">
                <input
                  type="radio"
                  name="apply-mode"
                  checked={mode === "delete"}
                  onChange={() => setMode("delete")}
                  className="accent-primary"
                />
                {m.applyBar.deletePermanent}
              </Label>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setConfirmOpen(false)}>
              {m.common.cancel}
            </Button>
            <Button onClick={execute} disabled={submitting}>
              {submitting && <Loader2 className="h-4 w-4 animate-spin" />}
              {m.applyBar.applyToFiles(toDelete.length)}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
