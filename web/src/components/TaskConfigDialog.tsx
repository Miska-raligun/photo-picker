import { useEffect, useMemo, useState } from "react";
import {
  ChevronRight,
  Filter,
  Loader2,
  Play,
  Scale,
  Sparkles,
  Wand2,
} from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
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
import { Checkbox } from "@/components/ui/checkbox";
import { Separator } from "@/components/ui/separator";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { FieldHelp } from "./FieldHelp";
import { SliderInput } from "./SliderInput";
import { PresetCard } from "./PresetCard";
import { api } from "@/lib/api";
import type { ExecutionProvider } from "@/lib/types";
import { useM } from "@/lib/i18n";
import { matchPreset, PRESETS, type PresetId, type PresetParams } from "@/lib/presets";
import { cn } from "@/lib/utils";
import { toast } from "sonner";

interface Props {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  source: string;
  files: string[] | null;
  output: string;
  inPlace: boolean;
  onStarted: (runId: string, summary: string, output: string) => void;
}

export function TaskConfigDialog({
  open,
  onOpenChange,
  source,
  files,
  output,
  inPlace,
  onStarted,
}: Props) {
  const m = useM();

  // Engine knobs as separate state so the existing slider/input controls stay
  // readable; we project them into a PresetParams view for matching against
  // named presets, and apply presets in bulk.
  const initial = PRESETS.balanced;
  const [k1, setK1] = useState(initial.k1);
  // K2 is optional: empty string → auto (server picks per-group via score
  // gaps), positive integer → fixed K2.
  const [k2, setK2] = useState<string>(initial.k2 === null ? "" : String(initial.k2));
  const [timeK, setTimeK] = useState(initial.time_k);
  const [minDt, setMinDt] = useState(0.3);
  const [maxDt, setMaxDt] = useState(30.0);
  const [hashDist, setHashDist] = useState(6);
  const [stageAClip, setStageAClip] = useState(initial.stage_a_clip_threshold);
  const [stageBClip, setStageBClip] = useState(initial.stage_b_threshold);
  const [enableClip, setEnableClip] = useState(initial.enable_clip);
  const [enableFace, setEnableFace] = useState(initial.enable_face);
  const [adaptiveThresholds, setAdaptiveThresholds] = useState(initial.adaptive_thresholds);
  const [linkMode, setLinkMode] = useState<"copy" | "hardlink" | "symlink">("hardlink");
  const [thumbLongEdge, setThumbLongEdge] = useState(1024);
  const [executionProvider, setExecutionProvider] = useState<ExecutionProvider>("cpu");
  const [availableProviders, setAvailableProviders] = useState<ExecutionProvider[]>(["cpu"]);

  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const reduce = useReducedMotion();

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    api
      .listProviders()
      .then((r) => {
        if (cancelled) return;
        const list: ExecutionProvider[] =
          r.providers.length > 0 ? r.providers : ["cpu"];
        setAvailableProviders(list);
        setExecutionProvider((prev) => (list.includes(prev) ? prev : "cpu"));
      })
      .catch(() => {
        if (!cancelled) setAvailableProviders(["cpu"]);
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  // Derived view of preset-relevant fields. If they match a named preset we
  // highlight that card; manual tweaks land in "custom".
  const currentPresetParams: PresetParams = useMemo(() => {
    const k2Trim = k2.trim();
    const k2Parsed = k2Trim === "" ? null : parseInt(k2Trim, 10);
    return {
      k1,
      k2: k2Parsed === null || Number.isNaN(k2Parsed) || k2Parsed <= 0 ? null : k2Parsed,
      time_k: timeK,
      stage_a_clip_threshold: stageAClip,
      stage_b_threshold: stageBClip,
      enable_clip: enableClip,
      enable_face: enableFace,
      adaptive_thresholds: adaptiveThresholds,
    };
  }, [k1, k2, timeK, stageAClip, stageBClip, enableClip, enableFace, adaptiveThresholds]);
  const activePreset = matchPreset(currentPresetParams);

  function applyPreset(id: PresetId) {
    const p = PRESETS[id];
    setK1(p.k1);
    setK2(p.k2 === null ? "" : String(p.k2));
    setTimeK(p.time_k);
    setStageAClip(p.stage_a_clip_threshold);
    setStageBClip(p.stage_b_threshold);
    setEnableClip(p.enable_clip);
    setEnableFace(p.enable_face);
    setAdaptiveThresholds(p.adaptive_thresholds);
  }

  async function start() {
    setSubmitting(true);
    try {
      const k2Trim = k2.trim();
      const k2Parsed = k2Trim === "" ? undefined : parseInt(k2Trim, 10);
      const k2Final =
        k2Parsed === undefined || Number.isNaN(k2Parsed) || k2Parsed <= 0
          ? undefined
          : k2Parsed;
      const req = {
        output: inPlace ? source : output,
        k1,
        ...(k2Final !== undefined ? { k2: k2Final } : {}),
        time_k: timeK,
        min_dt: minDt,
        max_dt: maxDt,
        hash_dist: hashDist,
        stage_a_clip_threshold: stageAClip,
        stage_b_threshold: stageBClip,
        enable_clip: enableClip,
        enable_face: enableFace,
        in_place: inPlace,
        adaptive_thresholds: adaptiveThresholds,
        link_mode: linkMode,
        thumbnail_long_edge: thumbLongEdge,
        execution_provider: executionProvider,
        ...(files && files.length > 0 ? { files } : { root: source }),
      };
      const { run_id } = await api.scan(req);
      const summary =
        files && files.length > 0
          ? `${files.length} photos from ${source}`
          : source;
      onStarted(run_id, summary, req.output);
      onOpenChange(false);
    } catch (e) {
      toast.error(m.errors.failedToStart, {
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl max-h-[90vh] flex flex-col gap-0 p-0">
        <DialogHeader className="px-6 pt-5 pb-3 border-b">
          <DialogTitle className="flex items-center gap-2.5">
            <span className="grid place-items-center h-7 w-7 rounded-md bg-primary/10 text-primary">
              <Wand2 className="h-4 w-4" />
            </span>
            {m.scanForm.configureTaskTitle}
          </DialogTitle>
          <DialogDescription className="text-xs">
            {m.scanForm.configureTaskDesc}
          </DialogDescription>
        </DialogHeader>

        <div className="overflow-y-auto px-6 py-5 flex-1 space-y-5">
          {/* Source summary */}
          <div
            className={cn(
              "rounded-lg border px-3.5 py-2.5 text-xs",
              inPlace
                ? "border-primary/30 bg-accent"
                : "border-border bg-muted/50"
            )}
          >
            <div className="font-mono break-all mb-1">
              <span className="text-muted-foreground">→ </span>
              {files && files.length > 0
                ? `${files.length} photos from ${source}`
                : source}
            </div>
            <div className="text-muted-foreground">
              {inPlace
                ? m.scanForm.inPlaceNotice
                : m.scanForm.withOutputNotice + " " + output}
            </div>
          </div>

          {/* Presets — the friendly path for the common case. */}
          <section className="space-y-2.5">
            <div className="flex items-center justify-between">
              <h3 className="text-[0.78rem] font-semibold uppercase tracking-wider text-muted-foreground">
                {m.scanForm.presetSection}
              </h3>
              {activePreset === "custom" && (
                <span className="text-[0.7rem] text-muted-foreground italic">
                  {m.scanForm.presetCustom}
                </span>
              )}
            </div>
            <div className="grid sm:grid-cols-3 gap-2.5">
              <PresetCard
                selected={activePreset === "aggressive"}
                onSelect={() => applyPreset("aggressive")}
                icon={Filter}
                title={m.scanForm.presetAggressiveTitle}
                hint={m.scanForm.presetAggressiveHint}
              />
              <PresetCard
                selected={activePreset === "balanced"}
                onSelect={() => applyPreset("balanced")}
                icon={Scale}
                title={m.scanForm.presetBalancedTitle}
                hint={m.scanForm.presetBalancedHint}
                accent="primary"
              />
              <PresetCard
                selected={activePreset === "gentle"}
                onSelect={() => applyPreset("gentle")}
                icon={Sparkles}
                title={m.scanForm.presetGentleTitle}
                hint={m.scanForm.presetGentleHint}
              />
            </div>
          </section>

          {/* Plain-language essentials kept visible. */}
          <section>
            <CheckboxRow
              label={m.scanForm.enableFaceLabel}
              desc={m.scanForm.enableFaceDesc}
              checked={enableFace}
              onChange={setEnableFace}
            />
          </section>

          {/* Advanced — every raw engine knob lives here. Closed by default. */}
          <section className="border-t pt-3">
            <button
              type="button"
              onClick={() => setAdvancedOpen((o) => !o)}
              aria-expanded={advancedOpen}
              className="w-full flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground transition-colors"
            >
              <motion.span
                animate={reduce ? undefined : { rotate: advancedOpen ? 90 : 0 }}
                transition={{ duration: 0.18, ease: "easeOut" }}
                className="inline-flex"
              >
                <ChevronRight className="h-3.5 w-3.5" />
              </motion.span>
              <span className="font-medium">{m.scanForm.advancedLabel}</span>
              <span className="text-[0.7rem] text-muted-foreground/80 ml-1">
                {m.scanForm.advancedHint}
              </span>
            </button>

            <AnimatePresence initial={false}>
              {advancedOpen && (
                <motion.div
                  key="advanced"
                  initial={reduce ? false : { height: 0, opacity: 0 }}
                  animate={{ height: "auto", opacity: 1 }}
                  exit={reduce ? { opacity: 0 } : { height: 0, opacity: 0 }}
                  transition={{ duration: 0.22, ease: "easeOut" }}
                  className="overflow-hidden"
                >
                  <div className="pt-5 space-y-5">
                    <div className="grid sm:grid-cols-2 gap-5">
                      <FieldHelp
                        label={m.scanForm.k1Label}
                        htmlFor="k1"
                        desc={m.scanForm.k1Desc}
                      >
                        <Input
                          id="k1"
                          type="number"
                          min={1}
                          max={20}
                          value={k1}
                          onChange={(e) => setK1(parseInt(e.target.value) || 1)}
                          className="text-sm w-24"
                        />
                      </FieldHelp>
                      <FieldHelp
                        label={m.scanForm.k2Label}
                        htmlFor="k2"
                        desc={m.scanForm.k2Desc}
                      >
                        <Input
                          id="k2"
                          type="number"
                          min={1}
                          max={10}
                          value={k2}
                          placeholder={m.scanForm.k2Auto}
                          onChange={(e) => setK2(e.target.value)}
                          className="text-sm w-24"
                        />
                      </FieldHelp>
                    </div>

                    <FieldHelp label={m.scanForm.timeKLabel} desc={m.scanForm.timeKDesc}>
                      <SliderInput
                        value={timeK}
                        onChange={setTimeK}
                        min={0.5}
                        max={10}
                        step={0.1}
                      />
                    </FieldHelp>

                    <FieldHelp
                      label={m.scanForm.stageAClipLabel}
                      desc={m.scanForm.stageAClipDesc}
                    >
                      <SliderInput
                        value={stageAClip}
                        onChange={setStageAClip}
                        min={0.7}
                        max={1}
                        step={0.01}
                      />
                    </FieldHelp>

                    <FieldHelp
                      label={m.scanForm.stageBClipLabel}
                      desc={m.scanForm.stageBClipDesc}
                    >
                      <SliderInput
                        value={stageBClip}
                        onChange={setStageBClip}
                        min={0.7}
                        max={1}
                        step={0.01}
                      />
                    </FieldHelp>

                    <div className="grid sm:grid-cols-2 gap-5">
                      <FieldHelp label={m.scanForm.minDtLabel} desc={m.scanForm.minDtDesc}>
                        <SliderInput
                          value={minDt}
                          onChange={setMinDt}
                          min={0}
                          max={5}
                          step={0.05}
                        />
                      </FieldHelp>
                      <FieldHelp label={m.scanForm.maxDtLabel} desc={m.scanForm.maxDtDesc}>
                        <SliderInput
                          value={maxDt}
                          onChange={setMaxDt}
                          min={1}
                          max={120}
                          step={1}
                        />
                      </FieldHelp>
                    </div>

                    <FieldHelp
                      label={m.scanForm.hashDistLabel}
                      htmlFor="hash_dist"
                      desc={m.scanForm.hashDistDesc}
                    >
                      <Input
                        id="hash_dist"
                        type="number"
                        min={0}
                        max={32}
                        value={hashDist}
                        onChange={(e) => setHashDist(parseInt(e.target.value) || 0)}
                        className="text-sm w-24"
                      />
                    </FieldHelp>

                    <Separator />

                    <div className="space-y-4">
                      <CheckboxRow
                        label={m.scanForm.enableClipLabel}
                        desc={m.scanForm.enableClipDesc}
                        checked={enableClip}
                        onChange={setEnableClip}
                      />
                      <CheckboxRow
                        label={m.scanForm.adaptiveLabel}
                        desc={m.scanForm.adaptiveDesc}
                        checked={adaptiveThresholds}
                        onChange={setAdaptiveThresholds}
                      />
                    </div>

                    <Separator />

                    <div className="grid sm:grid-cols-2 gap-5">
                      <FieldHelp
                        label={m.scanForm.providerLabel}
                        desc={m.scanForm.providerDesc}
                      >
                        <Select
                          value={executionProvider}
                          onValueChange={(v) =>
                            setExecutionProvider(v as ExecutionProvider)
                          }
                        >
                          <SelectTrigger className="text-sm w-full">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            {availableProviders.includes("cpu") && (
                              <SelectItem value="cpu">CPU</SelectItem>
                            )}
                            {availableProviders.includes("cuda") && (
                              <SelectItem value="cuda">CUDA (NVIDIA)</SelectItem>
                            )}
                            {availableProviders.includes("coreml") && (
                              <SelectItem value="coreml">CoreML (macOS)</SelectItem>
                            )}
                            {availableProviders.includes("directml") && (
                              <SelectItem value="directml">DirectML (Windows)</SelectItem>
                            )}
                          </SelectContent>
                        </Select>
                      </FieldHelp>
                      <FieldHelp
                        label={m.scanForm.thumbEdgeLabel}
                        htmlFor="thumb_long_edge"
                        desc={m.scanForm.thumbEdgeDesc}
                      >
                        <Input
                          id="thumb_long_edge"
                          type="number"
                          min={512}
                          max={4096}
                          step={64}
                          value={thumbLongEdge}
                          onChange={(e) =>
                            setThumbLongEdge(
                              Math.max(512, Math.min(4096, parseInt(e.target.value) || 1024))
                            )
                          }
                          className="text-sm w-32"
                        />
                      </FieldHelp>
                    </div>

                    {!inPlace && (
                      <FieldHelp
                        label={m.scanForm.linkModeLabel}
                        desc={m.scanForm.linkModeDesc}
                      >
                        <Select
                          value={linkMode}
                          onValueChange={(v) => setLinkMode(v as typeof linkMode)}
                        >
                          <SelectTrigger className="text-sm w-full sm:w-64">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="hardlink">{m.scanForm.linkHardlink}</SelectItem>
                            <SelectItem value="copy">{m.scanForm.linkCopy}</SelectItem>
                            <SelectItem value="symlink">{m.scanForm.linkSymlink}</SelectItem>
                          </SelectContent>
                        </Select>
                      </FieldHelp>
                    )}
                  </div>
                </motion.div>
              )}
            </AnimatePresence>
          </section>
        </div>

        <DialogFooter className="px-6 py-4 border-t bg-muted/30">
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {m.common.cancel}
          </Button>
          <motion.div
            whileTap={reduce ? undefined : { scale: 0.98 }}
            className="inline-flex"
          >
            <Button onClick={start} disabled={submitting}>
              {submitting ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <Play className="h-4 w-4" />
              )}
              {m.scanForm.startTask}
            </Button>
          </motion.div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function CheckboxRow({
  label,
  desc,
  checked,
  onChange,
}: {
  label: string;
  desc: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <label className="flex items-start gap-3 cursor-pointer">
      <Checkbox
        checked={checked}
        onCheckedChange={(c) => onChange(c === true)}
        className="mt-0.5"
      />
      <div className="grid gap-1 text-sm">
        <span className="font-medium">{label}</span>
        <span className="text-muted-foreground text-[0.78rem] leading-snug">
          {desc}
        </span>
      </div>
    </label>
  );
}
