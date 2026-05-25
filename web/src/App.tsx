import { useCallback, useEffect, useRef, useState } from "react";
import { Settings as SettingsIcon } from "lucide-react";
import { ScanForm } from "./components/ScanForm";
import { RunCard } from "./components/RunCard";
import { RunDetailDialog } from "./components/RunDetailDialog";
import { GroupDetailDialog } from "./components/GroupDetailDialog";
import { SettingsDialog } from "./components/SettingsDialog";
import { LanguageToggle } from "./components/LanguageToggle";
import { Button } from "./components/ui/button";
import { Toaster } from "./components/ui/sonner";
import { api } from "./lib/api";
import type { RunRecord, VlmSettings } from "./lib/types";
import { I18nContext, LANG_STORAGE_KEY, messages, type Lang } from "./lib/i18n";
import { loadVlmSettings } from "./lib/vlmStore";

export default function App() {
  const [lang, setLangState] = useState<Lang>(() => {
    const stored = localStorage.getItem(LANG_STORAGE_KEY) as Lang | null;
    if (stored === "en" || stored === "zh") return stored;
    return navigator.language?.toLowerCase().startsWith("zh") ? "zh" : "en";
  });
  const setLang = useCallback((l: Lang) => {
    setLangState(l);
    localStorage.setItem(LANG_STORAGE_KEY, l);
  }, []);
  const m = messages[lang];

  const [vlmSettings, setVlmSettings] = useState<VlmSettings>(() =>
    loadVlmSettings()
  );
  const [settingsOpen, setSettingsOpen] = useState(false);

  const [runs, setRuns] = useState<RunRecord[]>([]);
  const [overrides, setOverrides] = useState<Map<string, Set<string>>>(new Map());
  const [detailRunId, setDetailRunId] = useState<string | null>(null);
  const [groupRun, setGroupRun] = useState<string | null>(null);
  const [groupIdx, setGroupIdx] = useState<number | null>(null);
  const pollingRef = useRef<Set<string>>(new Set());

  const schedulePoll = useCallback((runId: string) => {
    if (pollingRef.current.has(runId)) return;
    pollingRef.current.add(runId);
    (async () => {
      while (true) {
        await new Promise((r) => setTimeout(r, 600));
        try {
          const r = await api.getRun(runId);
          setRuns((prev) => {
            const i = prev.findIndex((x) => x.id === runId);
            if (i === -1) return [r, ...prev];
            const out = [...prev];
            out[i] = r;
            return out;
          });
          if (r.status.state !== "running") break;
        } catch {
          break;
        }
      }
      pollingRef.current.delete(runId);
    })();
  }, []);

  useEffect(() => {
    (async () => {
      try {
        const list = await api.listRuns();
        list.sort((a, b) => a.id.localeCompare(b.id));
        const full = await Promise.all(list.map((r) => api.getRun(r.id)));
        setRuns(full);
        for (const r of full) {
          if (r.status.state === "running") schedulePoll(r.id);
        }
      } catch (e) {
        console.error("initial run load", e);
      }
    })();
  }, [schedulePoll]);

  function handleScanStarted(runId: string, summary: string, output: string) {
    const placeholder: RunRecord = {
      id: runId,
      root: summary,
      output,
      in_place: false,
      status: { state: "running" },
      report: null,
      html_report: null,
      composition_picks: [],
      explanations: {},
    };
    setRuns((prev) => [placeholder, ...prev]);
    schedulePoll(runId);
  }

  function getOverrides(runId: string): Set<string> {
    return overrides.get(runId) ?? new Set();
  }

  function toggleOverride(runId: string, photoId: string) {
    setOverrides((prev) => {
      const next = new Map(prev);
      const set = new Set(next.get(runId) ?? []);
      if (set.has(photoId)) set.delete(photoId);
      else set.add(photoId);
      next.set(runId, set);
      return next;
    });
  }

  const detailRun = detailRunId ? runs.find((r) => r.id === detailRunId) ?? null : null;
  const groupRunRecord = groupRun ? runs.find((r) => r.id === groupRun) : null;

  return (
    <I18nContext.Provider value={{ lang, setLang, m }}>
      <div className="min-h-screen">
        <main className="max-w-5xl mx-auto px-6 pt-10 pb-20">
          <header className="mb-10 flex items-start justify-between gap-4">
            <div>
              <h1 className="text-3xl font-semibold tracking-tight">
                {m.common.appName}
              </h1>
              <p className="text-muted-foreground mt-2 text-sm">
                {m.common.tagline}
              </p>
            </div>
            <div className="flex items-center gap-1">
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setSettingsOpen(true)}
                className="text-muted-foreground hover:text-foreground"
                aria-label={m.common.settings}
              >
                <SettingsIcon className="h-4 w-4" />
              </Button>
              <LanguageToggle />
            </div>
          </header>

          <ScanForm onScanStarted={handleScanStarted} />

          <div className="mt-8 mb-3 text-xs uppercase tracking-wider text-muted-foreground font-semibold">
            {m.common.runsSection}
          </div>

          {runs.length === 0 ? (
            <div className="rounded-xl border border-dashed text-center text-muted-foreground italic text-sm py-12">
              {m.common.emptyRuns}
            </div>
          ) : (
            <div className="space-y-4">
              {runs.map((r) => (
                <RunCard
                  key={r.id}
                  run={r}
                  onOpenDetail={() => setDetailRunId(r.id)}
                />
              ))}
            </div>
          )}
        </main>

        <RunDetailDialog
          open={detailRunId !== null}
          onOpenChange={(v) => !v && setDetailRunId(null)}
          run={detailRun}
          overrides={detailRunId ? getOverrides(detailRunId) : new Set()}
          onOpenGroup={(idx) => {
            setGroupRun(detailRunId);
            setGroupIdx(idx);
          }}
          onApplyDone={() => detailRunId && schedulePoll(detailRunId)}
        />

        <GroupDetailDialog
          open={groupRun !== null && groupIdx !== null}
          onOpenChange={(v) => {
            if (!v) {
              setGroupRun(null);
              setGroupIdx(null);
            }
          }}
          runId={groupRun}
          pickIndex={groupIdx}
          overrides={groupRun ? getOverrides(groupRun) : new Set()}
          inPlace={groupRunRecord?.in_place ?? false}
          vlmSettings={vlmSettings}
          onOpenSettings={() => setSettingsOpen(true)}
          onToggleOverride={(photoId) =>
            groupRun && toggleOverride(groupRun, photoId)
          }
        />

        <SettingsDialog
          open={settingsOpen}
          onOpenChange={setSettingsOpen}
          initial={vlmSettings}
          onChange={setVlmSettings}
        />

        <Toaster richColors closeButton position="bottom-right" />
      </div>
    </I18nContext.Provider>
  );
}
