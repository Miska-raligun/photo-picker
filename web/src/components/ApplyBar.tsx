import { useEffect, useState } from "react";
import { AlertTriangle, Loader2, ShieldAlert, Trash2 } from "lucide-react";
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
import { finalDropIds } from "@/lib/selection";
import type { ApplyResult, CompositionPickView } from "@/lib/types";
import { toast } from "sonner";

interface Props {
  runId: string;
  picks: CompositionPickView[];
  overrides: Set<string>;
  sourceRoot: string;
  /// Called after a successful apply. Receives the server response so the
  /// caller can clear overrides on the photo ids that were actually deleted
  /// (and leave the rest alone for retry).
  onDone: (result: ApplyResult) => void;
}

export function ApplyBar({ runId, picks, overrides, sourceRoot, onDone }: Props) {
  const m = useM();
  // Final delete set across all groups, folding in user overrides.
  const toDelete = finalDropIds(picks, overrides);
  // Filenames for the same set, so the confirm dialog can show actual names.
  const dropSet = new Set(toDelete);
  const deleteFilenames = picks
    .flatMap((p) => [...p.kept, ...p.rejected])
    .filter((ph) => dropSet.has(ph.photo_id))
    .map((ph) => ph.filename)
    .filter((n): n is string => !!n);
  const overrideCount = overrides.size;

  const [confirmOpen, setConfirmOpen] = useState(false);
  const [mode, setMode] = useState<"trash" | "delete">("trash");
  const [submitting, setSubmitting] = useState(false);
  const [done, setDone] = useState(false);
  // Server-side dry-run preflight: resolves every path + applies the
  // symlink-escape / missing-file safety check before any destructive call.
  // Lets the dialog show "5 of 8 will be deleted; 3 failed safety check" and
  // the specific reasons, rather than the user finding out post-mortem.
  const [preflight, setPreflight] = useState<ApplyResult | null>(null);
  const [preflightLoading, setPreflightLoading] = useState(false);

  useEffect(() => {
    if (!confirmOpen || toDelete.length === 0) {
      setPreflight(null);
      return;
    }
    let cancelled = false;
    setPreflightLoading(true);
    setPreflight(null);
    api
      .apply(runId, toDelete, mode === "trash", true)
      .then((r) => {
        if (!cancelled) setPreflight(r);
      })
      .catch(() => {
        // Network errors here aren't fatal — the user can still click
        // delete; the real apply will surface a clearer error. Just
        // clear the spinner.
      })
      .finally(() => {
        if (!cancelled) setPreflightLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [confirmOpen, runId, toDelete, mode]);

  async function execute() {
    setSubmitting(true);
    try {
      const r = await api.apply(runId, toDelete, mode === "trash", false);
      const verb = r.used_trash
        ? m.applyBar.toastMovedToTrash
        : m.applyBar.toastDeleted;
      const msg = `${r.deleted} / ${r.requested} ${verb}`;
      const manifest = r.manifest_path;
      // Audit-trail action: only useful when an on-disk manifest was
      // actually written (skipped on dry runs / zero-delete results).
      const action = manifest
        ? {
            label: m.applyBar.toastReveal,
            onClick: () => {
              api.reveal(manifest).catch((err) => {
                toast.error(m.applyBar.toastRevealFailed, {
                  description: err instanceof Error ? err.message : String(err),
                });
              });
            },
          }
        : undefined;
      const description = manifest ? m.applyBar.toastManifestSaved(manifest) : undefined;
      if (r.failed.length === 0) {
        toast.success(msg, { description, action });
      } else {
        const failureSummary = r.failed
          .slice(0, 3)
          .map((f) => `${f.path.split("/").pop()}: ${f.error}`)
          .join("\n");
        toast.warning(`${msg} — ${r.failed.length} ${m.applyBar.toastFailedSuffix}`, {
          description: description
            ? `${description}\n${failureSummary}`
            : failureSummary,
          action,
        });
      }
      setDone(true);
      setConfirmOpen(false);
      onDone(r);
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
        <DialogContent className="max-w-lg">
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
          {deleteFilenames.length > 0 && (
            <div className="border border-border bg-muted/40 rounded-md max-h-48 overflow-auto p-2 font-mono text-[0.72rem] leading-relaxed">
              {deleteFilenames.slice(0, 50).map((name) => (
                <div key={name} className="truncate text-muted-foreground">
                  {name}
                </div>
              ))}
              {deleteFilenames.length > 50 && (
                <div className="text-muted-foreground italic">
                  … +{deleteFilenames.length - 50} more
                </div>
              )}
            </div>
          )}
          {/* Preflight banner: surfaces server-side safety check results
              (missing files, symlinks resolving outside the run root). When
              the preflight is in flight we render a quieter spinner so the
              dialog doesn't shift around. */}
          {preflightLoading && (
            <div className="flex items-center gap-2 text-xs text-muted-foreground">
              <Loader2 className="h-3 w-3 animate-spin" />
              checking paths…
            </div>
          )}
          {preflight && preflight.failed.length > 0 && (
            <div className="rounded-md border border-amber-500/40 bg-amber-500/10 p-2 text-xs">
              <div className="flex items-center gap-1.5 font-medium text-amber-700 dark:text-amber-400">
                <ShieldAlert className="h-3.5 w-3.5" />
                {preflight.failed.length} of {preflight.requested} will be skipped
              </div>
              <ul className="mt-1 space-y-0.5 text-muted-foreground max-h-24 overflow-auto">
                {preflight.failed.slice(0, 5).map((f) => (
                  <li key={f.photo_id} className="truncate">
                    <span className="font-mono">{f.path.split("/").pop()}</span>: {f.error}
                  </li>
                ))}
                {preflight.failed.length > 5 && (
                  <li className="italic">… +{preflight.failed.length - 5} more</li>
                )}
              </ul>
            </div>
          )}
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
