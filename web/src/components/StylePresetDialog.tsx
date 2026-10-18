import { useEffect, useState } from "react";
import { AlertTriangle, Download, ImagePlus, Loader2, Palette, X } from "lucide-react";
import { toast } from "sonner";
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
import { Badge } from "@/components/ui/badge";
import { api } from "@/lib/api";
import { useM } from "@/lib/i18n";
import type { StyleFile, StyleFormat, StylePresetResponse } from "@/lib/types";
import { BrowseDialog, type BrowseResult } from "./BrowseDialog";
import { SliderInput } from "./SliderInput";
import { Thumb } from "./Thumb";

interface Props {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  runId: string;
  /** Folder the browse dialog opens in (the run's source folder). */
  initialPath?: string;
}

const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;

/// Offered export formats, in display order. Adding an app = a new
/// `StyleFormat` on the server plus an entry here and its i18n strings.
const FORMATS: StyleFormat[] = ["lr_preset", "cube_lut"];
const MIME: Record<StyleFormat, string> = {
  lr_preset: "application/rdf+xml",
  cube_lut: "text/plain",
};

/// Learn a look from reference photos and download it as a Lightroom preset.
/// The server pairs references with their originals when they're edits of
/// photos in this run (precise fit); otherwise it falls back to a rough,
/// content-independent fit, and the dialog says which one happened.
export function StylePresetDialog({ open, onOpenChange, runId, initialPath }: Props) {
  const m = useM();
  const t = m.stylePreset;
  const [refs, setRefs] = useState<string[]>([]);
  const [name, setName] = useState("");
  const [strength, setStrength] = useState(100);
  const [formats, setFormats] = useState<StyleFormat[]>(["lr_preset"]);
  const [browseOpen, setBrowseOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<StylePresetResponse | null>(null);
  const [previews, setPreviews] = useState<Record<string, string>>({});

  // Render "after" previews for the suggested photos whenever a new preset
  // arrives; revoke the object URLs when they're replaced or on close.
  useEffect(() => {
    if (!result) return;
    let cancelled = false;
    const urls: Record<string, string> = {};
    (async () => {
      for (const id of result.preview_ids) {
        try {
          urls[id] = await api.stylePreview(runId, result.preset, id);
          if (!cancelled) setPreviews((p) => ({ ...p, [id]: urls[id] }));
        } catch {
          /* a missing preview isn't worth a toast */
        }
      }
    })();
    return () => {
      cancelled = true;
      Object.values(urls).forEach(URL.revokeObjectURL);
      setPreviews({});
    };
  }, [result, runId]);

  function onBrowse(r: BrowseResult) {
    if (r.kind !== "files" || r.files.length === 0) {
      toast.info(t.pickFilesHint);
      return;
    }
    setRefs((prev) => Array.from(new Set([...prev, ...r.files])));
    setResult(null);
  }

  async function generate() {
    setBusy(true);
    try {
      const r = await api.stylePreset(runId, refs, name.trim(), strength / 100, formats);
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
    setResult(null);
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-2xl max-h-[90vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Palette className="h-4 w-4" />
            {t.title}
          </DialogTitle>
          <DialogDescription>{t.desc}</DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <label className="text-xs font-medium text-muted-foreground">
                {t.references} ({refs.length})
              </label>
              <Button variant="outline" size="sm" onClick={() => setBrowseOpen(true)}>
                <ImagePlus className="h-4 w-4" />
                {t.addReferences}
              </Button>
            </div>
            {refs.length === 0 ? (
              <p className="text-xs text-muted-foreground">{t.referencesHint}</p>
            ) : (
              <ul className="max-h-32 overflow-y-auto space-y-1 text-xs font-mono">
                {refs.map((p) => (
                  <li key={p} className="flex items-center gap-2 min-w-0">
                    <span className="truncate flex-1" title={p}>
                      {baseName(p)}
                    </span>
                    <button
                      type="button"
                      aria-label={t.remove}
                      className="text-muted-foreground hover:text-foreground"
                      onClick={() => {
                        setRefs((prev) => prev.filter((x) => x !== p));
                        setResult(null);
                      }}
                    >
                      <X className="h-3.5 w-3.5" />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>

          <div className="grid gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">{t.name}</label>
            <Input value={name} placeholder={t.namePlaceholder} onChange={(e) => setName(e.target.value)} />
          </div>

          <div className="grid gap-1.5">
            <label className="text-xs font-medium text-muted-foreground">{t.strength}</label>
            <SliderInput value={strength} onChange={setStrength} min={0} max={100} step={5} />
          </div>

          <div className="grid gap-1.5">
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
          </div>

          {result && (
            <div className="space-y-3 rounded-md border p-3">
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
              {result.preview_ids.length > 0 && (
                <div className="space-y-1.5">
                  <p className="text-xs text-muted-foreground">{t.previewNote}</p>
                  <div className="grid grid-cols-2 gap-2">
                    {result.preview_ids.map((id) => (
                      <div key={id} className="grid grid-cols-2 gap-1">
                        <div className="aspect-[3/2] rounded overflow-hidden bg-muted">
                          <Thumb src={api.thumbUrl(runId, id)} alt={t.before} />
                        </div>
                        <div className="aspect-[3/2] rounded overflow-hidden bg-muted">
                          {previews[id] ? (
                            <Thumb src={previews[id]} alt={t.after} />
                          ) : (
                            <div className="h-full w-full shimmer" />
                          )}
                        </div>
                      </div>
                    ))}
                  </div>
                  <div className="grid grid-cols-4 text-[10px] text-muted-foreground text-center">
                    <span>{t.before}</span>
                    <span>{t.after}</span>
                    <span>{t.before}</span>
                    <span>{t.after}</span>
                  </div>
                </div>
              )}
              {result.files.map((f) => (
                <p key={f.format} className="text-xs text-muted-foreground whitespace-pre-line">
                  <span className="font-medium text-foreground">{f.file_name}</span>
                  {" — "}
                  {t.format[f.format].howTo}
                </p>
              ))}
              <p className="text-xs text-muted-foreground">{t.settingsNote}</p>
            </div>
          )}
        </div>

        <DialogFooter className="gap-2">
          <Button
            variant="outline"
            onClick={generate}
            disabled={busy || refs.length === 0 || formats.length === 0}
          >
            {busy && <Loader2 className="h-4 w-4 animate-spin" />}
            {result ? t.regenerate : t.generate}
          </Button>
          {result?.files.map((f) => (
            <Button key={f.format} onClick={() => download(f)} disabled={busy}>
              <Download className="h-4 w-4" />
              {t.download(f.file_name.split(".").pop() ?? "")}
            </Button>
          ))}
        </DialogFooter>
      </DialogContent>

      <BrowseDialog
        open={browseOpen}
        onOpenChange={setBrowseOpen}
        mode="source"
        initialPath={initialPath}
        onConfirm={onBrowse}
      />
    </Dialog>
  );
}
