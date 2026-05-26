import { useState } from "react";
import { Eye, EyeOff, Settings as SettingsIcon } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { useM } from "@/lib/i18n";
import type { VlmSettings } from "@/lib/types";
import { saveVlmSettings, clearVlmSettings, VLM_PRESETS } from "@/lib/vlmStore";
import { toast } from "sonner";

interface Props {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  initial: VlmSettings;
  onChange: (s: VlmSettings) => void;
}

export function SettingsDialog({ open, onOpenChange, initial, onChange }: Props) {
  const m = useM();
  const [mode, setMode] = useState<"env" | "custom">(initial.mode);
  const [provider, setProvider] = useState<"openai" | "anthropic">(
    initial.mode === "custom" ? initial.config.provider : "openai"
  );
  const [baseUrl, setBaseUrl] = useState(
    initial.mode === "custom" ? initial.config.base_url : ""
  );
  const [apiKey, setApiKey] = useState(
    initial.mode === "custom" ? initial.config.api_key : ""
  );
  const [model, setModel] = useState(
    initial.mode === "custom" ? initial.config.model : ""
  );
  const [showKey, setShowKey] = useState(false);
  const [preset, setPreset] = useState("");

  function applyPreset(key: string) {
    setPreset(key);
    if (!key) return;
    const p = VLM_PRESETS[key];
    if (!p) return;
    setProvider(p.provider);
    setBaseUrl(p.base_url);
    setModel(p.model);
  }

  function save() {
    if (mode === "custom") {
      if (!baseUrl.trim() || !apiKey.trim() || !model.trim()) {
        toast.error(m.settings.requiredFields);
        return;
      }
      const next: VlmSettings = {
        mode: "custom",
        config: {
          provider,
          base_url: baseUrl.trim(),
          api_key: apiKey,
          model: model.trim(),
        },
      };
      saveVlmSettings(next);
      onChange(next);
    } else {
      saveVlmSettings({ mode: "env" });
      onChange({ mode: "env" });
    }
    onOpenChange(false);
    toast.success(m.common.save + " ✓");
  }

  function clearAndReset() {
    clearVlmSettings();
    setApiKey("");
    setMode("env");
    onChange({ mode: "env" });
    toast.success(m.settings.clearSaved + " ✓");
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <SettingsIcon className="h-4 w-4" />
            {m.settings.title}
          </DialogTitle>
        </DialogHeader>

        <div className="space-y-5 py-2">
          <div>
            <h3 className="font-semibold text-sm mb-1">{m.settings.vlmHeading}</h3>
            <p className="text-xs text-muted-foreground mb-3">
              {m.settings.vlmDesc}
            </p>

            <div className="space-y-2.5">
              <label className="flex items-start gap-2 cursor-pointer">
                <input
                  type="radio"
                  checked={mode === "env"}
                  onChange={() => setMode("env")}
                  className="mt-1 accent-primary"
                />
                <div className="text-sm">
                  <div className="font-medium">{m.settings.modeEnv}</div>
                  <div className="text-xs text-muted-foreground">
                    {m.settings.modeEnvDesc}
                  </div>
                </div>
              </label>

              <label className="flex items-start gap-2 cursor-pointer">
                <input
                  type="radio"
                  checked={mode === "custom"}
                  onChange={() => setMode("custom")}
                  className="mt-1 accent-primary"
                />
                <div className="text-sm">
                  <div className="font-medium">{m.settings.modeCustom}</div>
                </div>
              </label>
            </div>
          </div>

          {mode === "custom" && (
            <>
              <Separator />
              <div className="rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
                ⚠ {m.settings.modeCustomWarning}
              </div>

              <div className="grid gap-3">
                <div className="grid gap-1.5">
                  <Label className="text-xs">{m.settings.preset}</Label>
                  <Select value={preset} onValueChange={applyPreset}>
                    <SelectTrigger>
                      <SelectValue placeholder={m.settings.presetNone} />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="openai-gpt-4o">OpenAI · gpt-4o</SelectItem>
                      <SelectItem value="siliconflow-qwen3-vl-32b">
                        SiliconFlow · Qwen3-VL-32B-Instruct
                      </SelectItem>
                      <SelectItem value="anthropic-claude-opus">
                        Anthropic · claude-opus-4-7
                      </SelectItem>
                      <SelectItem value="anthropic-claude-sonnet">
                        Anthropic · claude-sonnet-4-6
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                <div className="grid gap-1.5">
                  <Label className="text-xs">{m.settings.provider}</Label>
                  <Select
                    value={provider}
                    onValueChange={(v) =>
                      setProvider(v as "openai" | "anthropic")
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="openai">
                        {m.settings.providerOpenai}
                      </SelectItem>
                      <SelectItem value="anthropic">
                        {m.settings.providerAnthropic}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                <div className="grid gap-1.5">
                  <Label className="text-xs">{m.settings.baseUrl}</Label>
                  <Input
                    value={baseUrl}
                    onChange={(e) => setBaseUrl(e.target.value)}
                    placeholder="https://..."
                    className="font-mono text-xs"
                  />
                </div>

                <div className="grid gap-1.5">
                  <Label className="text-xs">{m.settings.apiKey}</Label>
                  <div className="relative">
                    <Input
                      type={showKey ? "text" : "password"}
                      value={apiKey}
                      onChange={(e) => setApiKey(e.target.value)}
                      placeholder="sk-..."
                      className="font-mono text-xs pr-16"
                      autoComplete="off"
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => setShowKey(!showKey)}
                      className="absolute right-1 top-1 h-7 px-2"
                    >
                      {showKey ? (
                        <EyeOff className="h-3 w-3" />
                      ) : (
                        <Eye className="h-3 w-3" />
                      )}
                    </Button>
                  </div>
                </div>

                <div className="grid gap-1.5">
                  <Label className="text-xs">{m.settings.model}</Label>
                  <Input
                    value={model}
                    onChange={(e) => setModel(e.target.value)}
                    placeholder="gpt-4o / Qwen/Qwen3-VL-32B-Instruct / ..."
                    className="font-mono text-xs"
                  />
                </div>
              </div>
            </>
          )}
        </div>

        <DialogFooter className="sm:justify-between gap-2">
          <Button variant="ghost" size="sm" onClick={clearAndReset}>
            {m.settings.clearSaved}
          </Button>
          <div className="flex gap-2">
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              {m.common.cancel}
            </Button>
            <Button onClick={save}>{m.common.save}</Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
