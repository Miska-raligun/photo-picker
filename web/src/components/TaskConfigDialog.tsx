import { useState } from "react";
import { Loader2, Play } from "lucide-react";
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
import { FieldHelp } from "./FieldHelp";
import { SliderInput } from "./SliderInput";
import { api } from "@/lib/api";
import { useM } from "@/lib/i18n";
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
  const [k1, setK1] = useState(3);
  const [k2, setK2] = useState(1);
  const [timeK, setTimeK] = useState(3.0);
  const [minDt, setMinDt] = useState(0.3);
  const [maxDt, setMaxDt] = useState(30.0);
  const [hashDist, setHashDist] = useState(6);
  const [stageAClip, setStageAClip] = useState(0.95);
  const [stageBClip, setStageBClip] = useState(0.93);
  const [enableClip, setEnableClip] = useState(true);
  const [enableFace, setEnableFace] = useState(true);
  const [submitting, setSubmitting] = useState(false);

  async function start() {
    setSubmitting(true);
    try {
      const req = {
        output: inPlace ? source : output,
        k1,
        k2,
        time_k: timeK,
        min_dt: minDt,
        max_dt: maxDt,
        hash_dist: hashDist,
        stage_a_clip_threshold: stageAClip,
        stage_b_threshold: stageBClip,
        enable_clip: enableClip,
        enable_face: enableFace,
        in_place: inPlace,
        ...(files && files.length > 0 ? { files } : { root: source }),
      };
      // When in-place mode, we still need an output dir for cache + reports;
      // default it to the source folder so cache stays alongside the photos.
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
          <DialogTitle>{m.scanForm.configureTaskTitle}</DialogTitle>
          <DialogDescription className="text-xs">
            {m.scanForm.configureTaskDesc}
          </DialogDescription>
        </DialogHeader>

        <div className="overflow-y-auto px-6 py-5 flex-1 space-y-5">
          <div
            className={`rounded-lg border px-3.5 py-2.5 text-xs ${
              inPlace
                ? "border-primary/30 bg-accent"
                : "border-border bg-muted/50"
            }`}
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
                onChange={(e) => setK2(parseInt(e.target.value) || 1)}
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
              label={m.scanForm.enableFaceLabel}
              desc={m.scanForm.enableFaceDesc}
              checked={enableFace}
              onChange={setEnableFace}
            />
          </div>
        </div>

        <DialogFooter className="px-6 py-4 border-t bg-muted/30">
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {m.common.cancel}
          </Button>
          <Button onClick={start} disabled={submitting}>
            {submitting ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Play className="h-4 w-4" />
            )}
            {m.scanForm.startTask}
          </Button>
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
