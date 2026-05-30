import { useEffect, useState } from "react";
import { ArrowUp, Check, Folder, ImageIcon, Loader2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Checkbox } from "@/components/ui/checkbox";
import { Badge } from "@/components/ui/badge";
import { api } from "@/lib/api";
import { useM } from "@/lib/i18n";
import type { BrowseResponse } from "@/lib/types";
import { cn } from "@/lib/utils";

export type BrowseResult =
  | { kind: "folder"; path: string }
  | { kind: "files"; files: string[]; sourceDir: string };

interface Props {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  mode: "source" | "output";
  initialPath?: string;
  onConfirm: (result: BrowseResult) => void;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

export function BrowseDialog({
  open,
  onOpenChange,
  mode,
  initialPath,
  onConfirm,
}: Props) {
  const m = useM();
  const [data, setData] = useState<BrowseResponse | null>(null);
  const [pathInput, setPathInput] = useState(initialPath ?? "");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function load(path?: string) {
    setLoading(true);
    setError(null);
    try {
      const d = await api.browse(path);
      setData(d);
      setPathInput(d.current);
      setSelected(new Set());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    if (open) load(initialPath || undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  function toggleFile(p: string, checked: boolean) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (checked) next.add(p);
      else next.delete(p);
      return next;
    });
  }

  function toggleAll() {
    if (!data) return;
    const allSelected = data.files.every((f) => selected.has(f.path));
    setSelected(allSelected ? new Set() : new Set(data.files.map((f) => f.path)));
  }

  // `data.parent === ""` is a valid target on Windows (= the drives view),
  // so only `undefined`/`null` means "no parent".
  const hasParent = data?.parent !== undefined && data?.parent !== null;

  function goUp() {
    if (hasParent) load(data!.parent as string);
  }

  function confirm() {
    if (!data) return;
    if (mode === "output") {
      onConfirm({ kind: "folder", path: data.current });
    } else if (selected.size > 0) {
      onConfirm({
        kind: "files",
        files: Array.from(selected),
        sourceDir: data.current,
      });
    } else {
      onConfirm({ kind: "folder", path: data.current });
    }
    onOpenChange(false);
  }

  const total = data?.files.length ?? 0;
  const selCount = selected.size;
  const confirmLabel =
    mode === "output"
      ? m.browse.useThisFolder
      : selCount > 0
      ? m.browse.useSelectionN(selCount)
      : total > 0
      ? m.browse.useFolderN(total)
      : m.browse.useThisFolder;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-3xl gap-0 p-0">
        <DialogHeader className="px-6 pt-5 pb-3">
          <DialogTitle className="text-base">
            {mode === "source" ? m.browse.sourceTitle : m.browse.outputTitle}
          </DialogTitle>
          <DialogDescription className="text-xs">
            {mode === "source"
              ? m.browse.sourceDescription
              : m.browse.outputDescription}
          </DialogDescription>
        </DialogHeader>

        <div className="border-y bg-muted/50 px-6 py-3 flex gap-2 items-center">
          <Button
            variant="outline"
            size="sm"
            onClick={goUp}
            disabled={!hasParent}
          >
            <ArrowUp className="h-4 w-4" />
          </Button>
          <Input
            value={pathInput}
            onChange={(e) => setPathInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                load(pathInput);
              }
            }}
            className="font-mono text-xs h-8"
            placeholder={data?.current === "" ? m.browse.thisPc : "/path"}
          />
          <Button size="sm" variant="outline" onClick={() => load(pathInput)}>
            Go
          </Button>
        </div>

        <ScrollArea className="h-[420px]">
          <div className="px-6 py-4 space-y-4">
            {loading && (
              <div className="flex items-center justify-center text-muted-foreground text-sm py-8">
                <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                {m.browse.loading}
              </div>
            )}
            {error && (
              <div className="text-destructive text-sm font-mono bg-destructive/10 border border-destructive/20 rounded-md p-3">
                {error}
              </div>
            )}

            {data && !loading && (
              <>
                <section>
                  <h4 className="text-[0.7rem] uppercase tracking-wider text-muted-foreground font-semibold mb-2">
                    {m.browse.foldersHeading}
                  </h4>
                  {data.dirs.length === 0 ? (
                    <p className="text-sm text-muted-foreground italic">
                      {m.browse.noSubfolders}
                    </p>
                  ) : (
                    <div className="grid grid-cols-2 gap-1">
                      {data.dirs.map((d) => (
                        <button
                          key={d.path}
                          onClick={() => load(d.path)}
                          className="flex items-center gap-2 px-3 py-2 rounded-md text-sm text-left hover:bg-accent transition-colors"
                        >
                          <Folder className="h-4 w-4 text-primary" />
                          <span className="truncate">{d.name}</span>
                        </button>
                      ))}
                    </div>
                  )}
                </section>

                {mode === "source" && (
                  <section>
                    <div className="flex items-center justify-between mb-2">
                      <h4 className="text-[0.7rem] uppercase tracking-wider text-muted-foreground font-semibold">
                        {m.browse.photosHeading} ({data.files.length})
                      </h4>
                      {data.files.length > 0 && (
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={toggleAll}
                          className="text-xs h-7"
                        >
                          {m.browse.toggleAll}
                        </Button>
                      )}
                    </div>
                    {data.files.length === 0 ? (
                      <p className="text-sm text-muted-foreground italic">
                        {m.browse.noPhotos}
                      </p>
                    ) : (
                      <div className="space-y-0.5">
                        {data.files.map((f) => {
                          const isSel = selected.has(f.path);
                          return (
                            <label
                              key={f.path}
                              className={cn(
                                "flex items-center gap-3 px-3 py-1.5 rounded-md text-sm cursor-pointer transition-colors",
                                isSel
                                  ? "bg-accent text-accent-foreground"
                                  : "hover:bg-muted/60"
                              )}
                            >
                              <Checkbox
                                checked={isSel}
                                onCheckedChange={(c) =>
                                  toggleFile(f.path, c === true)
                                }
                              />
                              <ImageIcon className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                              <span className="flex-1 truncate font-mono text-xs">
                                {f.name}
                              </span>
                              <Badge variant="outline" className="font-mono text-[0.65rem] h-5">
                                {f.format}
                              </Badge>
                              <span className="text-xs text-muted-foreground font-mono w-16 text-right">
                                {formatBytes(f.size)}
                              </span>
                            </label>
                          );
                        })}
                      </div>
                    )}
                  </section>
                )}
              </>
            )}
          </div>
        </ScrollArea>

        <DialogFooter className="px-6 py-4 border-t bg-muted/30 sm:justify-between">
          <span className="text-xs text-muted-foreground">
            {mode === "source" && data
              ? selCount > 0
                ? m.browse.selectionOfN(selCount, total)
                : total > 0
                ? m.browse.photosInFolder(total)
                : ""
              : ""}
          </span>
          <div className="flex gap-2">
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              {m.common.cancel}
            </Button>
            <Button onClick={confirm}>
              <Check className="h-4 w-4" />
              {confirmLabel}
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
