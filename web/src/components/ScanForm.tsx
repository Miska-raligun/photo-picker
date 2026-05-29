import { useState } from "react";
import { ArrowRight, FolderOpen, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { BrowseDialog, type BrowseResult } from "./BrowseDialog";
import { TaskConfigDialog } from "./TaskConfigDialog";
import { useM } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { toast } from "sonner";

interface Props {
  onScanStarted: (runId: string, summary: string) => void;
  /// Compact bar form (used once runs exist); otherwise the large hero input.
  compact?: boolean;
}

export function ScanForm({ onScanStarted, compact = false }: Props) {
  const m = useM();
  const [root, setRoot] = useState("");
  const [explicitFiles, setExplicitFiles] = useState<{
    files: string[];
    sourceDir: string;
  } | null>(null);

  const [browseOpen, setBrowseOpen] = useState(false);
  const [configOpen, setConfigOpen] = useState(false);

  function handleBrowseConfirm(result: BrowseResult) {
    if (result.kind === "files") {
      // Keep any typed `root` so clearing the file selection restores it; the
      // input visually yields to the selection while explicitFiles is set.
      setExplicitFiles({ files: result.files, sourceDir: result.sourceDir });
    } else {
      setRoot(result.path);
      setExplicitFiles(null);
    }
  }

  function createTask() {
    if (!root && !explicitFiles) {
      toast.error(m.errors.pickSource);
      return;
    }
    setConfigOpen(true);
  }

  const effectiveSource = explicitFiles ? explicitFiles.sourceDir : root;

  return (
    <div className={cn("w-full", compact ? "" : "max-w-2xl mx-auto")}>
      <div
        className={cn(
          "flex items-center gap-1.5 rounded-full border bg-card shadow-sm transition-shadow focus-within:shadow-md focus-within:border-primary/40",
          compact ? "px-2 py-1.5" : "px-3 py-2.5"
        )}
      >
        <FolderOpen className="ml-2 h-4 w-4 text-muted-foreground shrink-0" />
        <input
          value={explicitFiles ? "" : root}
          onChange={(e) => {
            setRoot(e.target.value);
            setExplicitFiles(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              createTask();
            }
          }}
          placeholder={
            explicitFiles
              ? `${explicitFiles.files.length} photos selected`
              : m.scanForm.sourcePlaceholder
          }
          className={cn(
            "flex-1 bg-transparent outline-none font-mono min-w-0 placeholder:text-muted-foreground/70",
            compact ? "text-sm" : "text-sm sm:text-base"
          )}
        />
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="shrink-0 text-muted-foreground hover:text-foreground"
          onClick={() => setBrowseOpen(true)}
        >
          <FolderOpen className="h-4 w-4" />
          <span className="hidden sm:inline">{m.scanForm.browse}</span>
        </Button>
        <Button
          type="button"
          onClick={createTask}
          size={compact ? "sm" : "default"}
          className="rounded-full shrink-0"
        >
          {m.scanForm.createTask}
          <ArrowRight className="h-4 w-4" />
        </Button>
      </div>

      {explicitFiles && (
        <div className="mt-2 flex justify-center">
          <Badge
            variant="secondary"
            className="bg-accent text-accent-foreground gap-1.5"
          >
            {explicitFiles.files.length} photos from {explicitFiles.sourceDir}
            <button
              type="button"
              onClick={() => setExplicitFiles(null)}
              className="ml-1 hover:opacity-70"
              aria-label="clear selection"
            >
              <X className="h-3 w-3" />
            </button>
          </Badge>
        </div>
      )}

      <BrowseDialog
        open={browseOpen}
        onOpenChange={setBrowseOpen}
        mode="source"
        initialPath={root}
        onConfirm={handleBrowseConfirm}
      />

      <TaskConfigDialog
        open={configOpen}
        onOpenChange={setConfigOpen}
        source={effectiveSource}
        files={explicitFiles?.files ?? null}
        onStarted={onScanStarted}
      />
    </div>
  );
}
