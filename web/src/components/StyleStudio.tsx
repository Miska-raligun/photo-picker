import { useEffect, useState } from "react";
import {
  AlertTriangle,
  Download,
  FolderOpen,
  ImagePlus,
  Loader2,
  Palette,
  X,
} from "lucide-react";
import { toast } from "sonner";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { api } from "@/lib/api";
import { useM } from "@/lib/i18n";
import type { StyleFile, StyleFormat, StylePresetResponse } from "@/lib/types";
import { BrowseDialog, type BrowseResult } from "./BrowseDialog";
import { SliderInput } from "./SliderInput";
import { Thumb } from "./Thumb";

const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;

/// Offered export formats, in display order. Adding an app = a new
/// `StyleFormat` on the server plus an entry here and its i18n strings.
const FORMATS: StyleFormat[] = ["lr_preset", "cube_lut"];
const MIME: Record<StyleFormat, string> = {
  lr_preset: "application/rdf+xml",
  cube_lut: "text/plain",
};

type Picking = "references" | "targets" | null;

/// Style extraction: a standalone tool next to scanning. Pick reference
/// photos with the look you want and the photos to restyle; get a Lightroom
/// preset and/or 3D LUT. When a reference is an edit of one of the photos to
/// restyle, the server pairs them for a precise fit; otherwise it falls back
/// to a rough, content-independent fit, and the result says which happened.
export function StyleStudio() {
  const m = useM();
  const t = m.stylePreset;
  const [refs, setRefs] = useState<string[]>([]);
  const [targetDir, setTargetDir] = useState<string | null>(null);
  const [targetFiles, setTargetFiles] = useState<string[]>([]);
  const [name, setName] = useState("");
  const [strength, setStrength] = useState(100);
  const [formats, setFormats] = useState<StyleFormat[]>(["lr_preset"]);
  const [picking, setPicking] = useState<Picking>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<StylePresetResponse | null>(null);
  const [previews, setPreviews] = useState<Record<string, { before?: string; after?: string }>>({});

  const hasTargets = targetDir !== null || targetFiles.length > 0;

  // Before/after previews for the suggested photos whenever a new preset
  // arrives; object URLs are revoked when replaced or on unmount.
  useEffect(() => {
    if (!result) return;
    let cancelled = false;
    const urls: string[] = [];
    (async () => {
      for (const path of result.preview_paths) {
        for (const which of ["before", "after"] as const) {
          try {
            const url = await api.stylePreview(path, which === "after" ? result.preset : null);
            urls.push(url);
            if (!cancelled) {
              setPreviews((p) => ({ ...p, [path]: { ...p[path], [which]: url } }));
            }
          } catch {
            /* a missing preview isn't worth a toast */
          }
        }
      }
    })();
    return () => {
      cancelled = true;
      urls.forEach(URL.revokeObjectURL);
      setPreviews({});
    };
  }, [result]);

  function invalidate() {
    setResult(null);
  }

  function onBrowse(r: BrowseResult) {
    if (picking === "references") {
      if (r.kind !== "files" || r.files.length === 0) {
        toast.info(t.pickFilesHint);
        return;
      }
      setRefs((prev) => Array.from(new Set([...prev, ...r.files])));
    } else if (picking === "targets") {
      if (r.kind === "folder") {
        setTargetDir(r.path);
      } else {
        setTargetFiles((prev) => Array.from(new Set([...prev, ...r.files])));
      }
    }
    invalidate();
  }

  async function generate() {
    setBusy(true);
    try {
      const r = await api.stylePreset({
        referencePaths: refs,
        targetDir,
        targetPaths: targetFiles,
        name: name.trim(),
        strength: strength / 100,
        formats,
      });
      setResult(r);
    } catch (e) {
      toast.error(t.failed, { description: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  }

  function download(file: StyleFile) {
    const url = URL.createObjectURL(new Blob([file.content], { type: MIME[file.format] }));
    const a = document.createElement("a");
    a.href = url;
    a.download = file.file_name;
    a.click();
    URL.revokeObjectURL(url);
  }

  function toggleFormat(f: StyleFormat, on: boolean) {
    setFormats((prev) => FORMATS.filter((x) => (x === f ? on : prev.includes(x))));
    invalidate();
  }

  const fileList = (items: string[], remove: (p: string) => void) => (
    <ul className="max-h-32 overflow-y-auto space-y-1 text-xs font-mono">
      {items.map((p) => (
        <li key={p} className="flex items-center gap-2 min-w-0">
          <span className="truncate flex-1" title={p}>
            {baseName(p)}
          </span>
          <button
            type="button"
            aria-label={t.remove}
            className="text-muted-foreground hover:text-foreground"
            onClick={() => {
              remove(p);
              invalidate();
            }}
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </li>
      ))}
    </ul>
  );

  return (
    <div className="space-y-4">
      <Card>
        <CardHeader className="pb-2">
          <div className="flex items-center gap-2 font-semibold">
            <Palette className="h-4 w-4 text-primary" />
            {t.title}
          </div>
          <p className="text-sm text-muted-foreground">{t.desc}</p>
        </CardHeader>
        <CardContent className="space-y-5">
          <section className="space-y-2">
            <div className="flex items-center justify-between gap-2">
              <span className="text-sm font-medium">
                1. {t.references} ({refs.length})
              </span>
              <Button variant="outline" size="sm" onClick={() => setPicking("references")}>
                <ImagePlus className="h-4 w-4" />
                {t.addReferences}
              </Button>
            </div>
            {refs.length === 0 ? (
              <p className="text-xs text-muted-foreground">{t.referencesHint}</p>
            ) : (
              fileList(refs, (p) => setRefs((prev) => prev.filter((x) => x !== p)))
            )}
          </section>

          <section className="space-y-2">
            <div className="flex items-center justify-between gap-2">
              <span className="text-sm font-medium">2. {t.targets}</span>
              <Button variant="outline" size="sm" onClick={() => setPicking("targets")}>
                <FolderOpen className="h-4 w-4" />
                {t.chooseTargets}
              </Button>
            </div>
            {!hasTargets ? (
              <p className="text-xs text-muted-foreground">{t.targetsHint}</p>
            ) : (
              <div className="space-y-1">
                {targetDir && (
                  <div className="flex items-center gap-2 text-xs font-mono min-w-0">
                    <FolderOpen className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                    <span className="truncate flex-1" title={targetDir}>
                      {targetDir}
                    </span>
                    <button
                      type="button"
                      aria-label={t.remove}
                      className="text-muted-foreground hover:text-foreground"
                      onClick={() => {
                        setTargetDir(null);
                        invalidate();
                      }}
                    >
                      <X className="h-3.5 w-3.5" />
                    </button>
                  </div>
                )}
                {targetFiles.length > 0 &&
                  fileList(targetFiles, (p) => setTargetFiles((prev) => prev.filter((x) => x !== p)))}
              </div>
            )}
          </section>

          <section className="grid gap-4 sm:grid-cols-2">
            <div className="grid gap-1.5">
              <label className="text-xs font-medium text-muted-foreground">{t.name}</label>
              <Input
                value={name}
                placeholder={t.namePlaceholder}
                onChange={(e) => setName(e.target.value)}
              />
            </div>
            <div className="grid gap-1.5">
              <label className="text-xs font-medium text-muted-foreground">{t.strength}</label>
              <SliderInput value={strength} onChange={setStrength} min={0} max={100} step={5} />
            </div>
          </section>

          <section className="grid gap-1.5">
            <span className="text-xs font-medium text-muted-foreground">{t.formats}</span>
            {FORMATS.map((f) => (
              <label key={f} className="flex items-start gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={formats.includes(f)}
                  onChange={(e) => toggleFormat(f, e.target.checked)}
                  className="mt-0.5 accent-primary"
                />
                <div className="text-sm">
                  <div className="font-medium">{t.format[f].label}</div>
                  <div className="text-xs text-muted-foreground">{t.format[f].desc}</div>
                </div>
              </label>
            ))}
          </section>

          <div className="flex flex-wrap justify-end gap-2">
            <Button
              onClick={generate}
              disabled={busy || refs.length === 0 || !hasTargets || formats.length === 0}
            >
              {busy && <Loader2 className="h-4 w-4 animate-spin" />}
              {result ? t.regenerate : t.generate}
            </Button>
          </div>
          {busy && <p className="text-xs text-muted-foreground text-right">{t.working}</p>}
        </CardContent>
      </Card>

      {result && (
        <Card>
          <CardContent className="space-y-4 pt-6">
            <div className="flex items-start gap-2 text-sm">
              <Badge variant={result.mode === "paired" ? "default" : "secondary"}>
                {result.mode === "paired" ? t.modePaired : t.modeUnpaired}
              </Badge>
              <p className="text-xs text-muted-foreground">
                {result.mode === "paired"
                  ? t.pairedExplain(result.references_used)
                  : t.unpairedExplain}
              </p>
            </div>
            <p className="text-xs text-muted-foreground">
              {t.targetsRead(result.targets_read, result.targets_total)}
            </p>
            {(result.references_unpaired.length > 0 || result.references_failed.length > 0) && (
              <div className="flex gap-1.5 text-xs text-amber-700 dark:text-amber-400">
                <AlertTriangle className="h-3.5 w-3.5 shrink-0 mt-0.5" />
                <div className="space-y-0.5">
                  {result.references_unpaired.length > 0 && (
                    <p>{t.leftOut(result.references_unpaired.map(baseName).join(", "))}</p>
                  )}
                  {result.references_failed.map((f) => (
                    <p key={f.path}>
                      {baseName(f.path)}: {f.reason}
                    </p>
                  ))}
                </div>
              </div>
            )}

            {result.preview_paths.length > 0 && (
              <div className="space-y-1.5">
                <p className="text-xs text-muted-foreground">{t.previewNote}</p>
                <div className="grid gap-3 sm:grid-cols-2">
                  {result.preview_paths.map((path) => (
                    <figure key={path} className="space-y-1">
                      <div className="grid grid-cols-2 gap-1">
                        {(["before", "after"] as const).map((which) => (
                          <div key={which} className="aspect-[3/2] rounded overflow-hidden bg-muted">
                            {previews[path]?.[which] ? (
                              <Thumb
                                src={previews[path]![which]!}
                                alt={which === "before" ? t.before : t.after}
                              />
                            ) : (
                              <div className="h-full w-full shimmer" />
                            )}
                          </div>
                        ))}
                      </div>
                      <figcaption className="grid grid-cols-2 text-[10px] text-muted-foreground text-center">
                        <span>{t.before}</span>
                        <span title={path}>
                          {t.after} · {baseName(path)}
                        </span>
                      </figcaption>
                    </figure>
                  ))}
                </div>
              </div>
            )}

            <div className="space-y-2">
              {result.files.map((f) => (
                <div key={f.format} className="flex flex-col sm:flex-row sm:items-start gap-2 rounded-md border p-3">
                  <div className="flex-1 min-w-0">
                    <div className="text-sm font-medium truncate">{f.file_name}</div>
                    <p className="text-xs text-muted-foreground whitespace-pre-line">
                      {t.format[f.format].howTo}
                    </p>
                  </div>
                  <Button size="sm" onClick={() => download(f)}>
                    <Download className="h-4 w-4" />
                    {t.download(f.file_name.split(".").pop() ?? "")}
                  </Button>
                </div>
              ))}
            </div>
            <p className="text-xs text-muted-foreground">{t.settingsNote}</p>
          </CardContent>
        </Card>
      )}

      <BrowseDialog
        open={picking !== null}
        onOpenChange={(v) => !v && setPicking(null)}
        mode="source"
        initialPath={picking === "targets" ? targetDir ?? undefined : undefined}
        onConfirm={onBrowse}
      />
    </div>
  );
}
