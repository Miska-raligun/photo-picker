import { useState } from "react";
import { FolderOpen, Loader2, Upload } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
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
import { BrowseDialog, type BrowseResult } from "./BrowseDialog";
import { api } from "@/lib/api";
import { useM } from "@/lib/i18n";
import { finalKeptIds } from "@/lib/selection";
import type { CompositionPickView } from "@/lib/types";
import { toast } from "sonner";

interface Props {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  runId: string;
  picks: CompositionPickView[];
  overrides: Set<string>;
}

type LinkMode = "copy" | "hardlink" | "symlink";

export function ExportDialog({ open, onOpenChange, runId, picks, overrides }: Props) {
  const m = useM();
  const keptIds = finalKeptIds(picks, overrides);
  const [target, setTarget] = useState<string | null>(null);
  const [linkMode, setLinkMode] = useState<LinkMode>("copy");
  const [browseOpen, setBrowseOpen] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  function handleBrowse(result: BrowseResult) {
    if (result.kind === "folder") setTarget(result.path);
  }

  async function execute() {
    if (!target || keptIds.length === 0) return;
    setSubmitting(true);
    try {
      const r = await api.export(runId, keptIds, target, linkMode);
      const msg = `${r.exported} / ${r.requested} ${m.export.toastDone}`;
      if (r.failed.length === 0) {
        toast.success(msg, { description: r.target_dir });
      } else {
        toast.warning(`${msg} — ${r.failed.length} ${m.export.toastFailedSuffix}`, {
          description: r.failed
            .slice(0, 3)
            .map((f) => `${f.path.split("/").pop()}: ${f.error}`)
            .join("\n"),
        });
      }
      onOpenChange(false);
    } catch (e) {
      toast.error(m.export.toastFailed, {
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="max-w-lg">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <Upload className="h-4 w-4 text-primary" />
              {m.export.title}
            </DialogTitle>
            <DialogDescription>{m.export.desc}</DialogDescription>
          </DialogHeader>

          <div className="space-y-4 py-2">
            <div className="text-sm text-muted-foreground">
              {m.export.willExport(keptIds.length)}
            </div>

            <div className="flex gap-2 items-center">
              <div className="flex-1 min-w-0 rounded-md border bg-muted/40 px-3 py-2 font-mono text-xs truncate">
                {target ?? (
                  <span className="text-muted-foreground italic">
                    {m.export.noFolder}
                  </span>
                )}
              </div>
              <Button variant="outline" onClick={() => setBrowseOpen(true)}>
                <FolderOpen className="h-4 w-4" />
                {m.export.chooseFolder}
              </Button>
            </div>

            <div className="grid gap-1.5">
              <label className="text-xs font-medium text-muted-foreground">
                {m.export.linkMode}
              </label>
              <Select value={linkMode} onValueChange={(v) => setLinkMode(v as LinkMode)}>
                <SelectTrigger className="text-sm w-full sm:w-72">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="copy">{m.export.linkCopy}</SelectItem>
                  <SelectItem value="hardlink">{m.export.linkHardlink}</SelectItem>
                  <SelectItem value="symlink">{m.export.linkSymlink}</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              {m.common.cancel}
            </Button>
            <Button
              onClick={execute}
              disabled={submitting || !target || keptIds.length === 0}
            >
              {submitting ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <Upload className="h-4 w-4" />
              )}
              {keptIds.length === 0
                ? m.export.nothingToExport
                : m.export.run(keptIds.length)}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <BrowseDialog
        open={browseOpen}
        onOpenChange={setBrowseOpen}
        mode="output"
        initialPath={target ?? undefined}
        onConfirm={handleBrowse}
      />
    </>
  );
}
