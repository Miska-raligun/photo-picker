import { useCallback, useEffect, useRef, useState } from "react";
import { Aperture, Settings as SettingsIcon } from "lucide-react";
import { ScanForm } from "./components/ScanForm";
import { RunCard } from "./components/RunCard";
import { RunDetailDialog } from "./components/RunDetailDialog";
import { GroupDetailDialog } from "./components/GroupDetailDialog";
import { SettingsDialog } from "./components/SettingsDialog";
import { LanguageToggle } from "./components/LanguageToggle";
import { ThemeToggle } from "./components/ThemeToggle";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { FadeUp } from "./components/Motion";
import { Button } from "./components/ui/button";
import { Toaster } from "./components/ui/sonner";
import { api } from "./lib/api";
import type { ProgressEvent, RunProgress, RunRecord, VlmSettings } from "./lib/types";
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
  const [progress, setProgress] = useState<Map<string, RunProgress>>(new Map());
  const [overrides, setOverrides] = useState<Map<string, Set<string>>>(new Map());
  const [detailRunId, setDetailRunId] = useState<string | null>(null);
  const [groupRun, setGroupRun] = useState<string | null>(null);
  const [groupIdx, setGroupIdx] = useState<number | null>(null);
  // Per-run EventSource — lets us close all live SSE streams on unmount or
  // when the run finishes. Replaces the previous 600ms polling loop.
  const sseRef = useRef<Map<string, EventSource>>(new Map());

  const refreshRun = useCallback(async (runId: string) => {
    try {
      const r = await api.getRun(runId);
      setRuns((prev) => {
        const i = prev.findIndex((x) => x.id === runId);
        if (i === -1) return [r, ...prev];
        const out = [...prev];
        out[i] = r;
        return out;
      });
    } catch (e) {
      console.error("refresh run", runId, e);
    }
  }, []);

  const subscribeProgress = useCallback(
    (runId: string) => {
      if (sseRef.current.has(runId)) return;
      // EventSource follows current-origin; works against the same axum host
      // that serves the app. Cookies/CORS not in play because same-origin.
      const es = new EventSource(`/api/runs/${runId}/events`);
      sseRef.current.set(runId, es);

      const cleanup = () => {
        es.close();
        sseRef.current.delete(runId);
        setProgress((prev) => {
          if (!prev.has(runId)) return prev;
          const next = new Map(prev);
          next.delete(runId);
          return next;
        });
      };

      es.addEventListener("progress", (raw) => {
        let ev: ProgressEvent;
        try {
          ev = JSON.parse((raw as MessageEvent).data);
        } catch {
          return;
        }
        if (ev.kind === "stage") {
          setProgress((prev) => {
            const next = new Map(prev);
            next.set(runId, { stage: ev.stage, done: 0, total: ev.total });
            return next;
          });
        } else if (ev.kind === "tick") {
          setProgress((prev) => {
            const next = new Map(prev);
            const cur = next.get(runId);
            next.set(runId, {
              stage: ev.stage,
              done: ev.done,
              total: cur?.stage === ev.stage ? cur.total : 0,
            });
            return next;
          });
        } else if (ev.kind === "finish") {
          // Don't clear — keep the bar full until the next stage starts.
        } else if (ev.kind === "done") {
          // Terminal: refresh the full record, then close.
          refreshRun(runId).finally(cleanup);
        }
      });

      es.addEventListener("error", () => {
        // Server closed the channel (run already finished) or transient
        // network blip — fall back to a single refresh so the UI doesn't
        // get stuck in `running` forever.
        refreshRun(runId).finally(cleanup);
      });
    },
    [refreshRun]
  );

  // On unmount, close every active SSE stream.
  useEffect(() => {
    return () => {
      for (const es of sseRef.current.values()) es.close();
      sseRef.current.clear();
    };
  }, []);

  useEffect(() => {
    (async () => {
      try {
        const list = await api.listRuns();
        list.sort((a, b) => a.id.localeCompare(b.id));
        const full = await Promise.all(list.map((r) => api.getRun(r.id)));
        setRuns(full);
        for (const r of full) {
          if (r.status.state === "running") subscribeProgress(r.id);
        }
      } catch (e) {
        console.error("initial run load", e);
      }
    })();
  }, [subscribeProgress]);

  function handleScanStarted(runId: string, summary: string) {
    const placeholder: RunRecord = {
      id: runId,
      root: summary,
      output: "",
      in_place: true,
      status: { state: "running" },
      report: null,
      html_report: null,
      composition_picks: [],
      explanations: {},
    };
    setRuns((prev) => [placeholder, ...prev]);
    subscribeProgress(runId);
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
  const groupCount = groupRunRecord?.composition_picks?.length ?? 0;

  // Step through composition groups without closing the dialog. groupIdx is the
  // pick's array position (== CompositionPickView.index), so clamp to the list.
  function navigateGroup(delta: number) {
    setGroupIdx((cur) => {
      if (cur === null || groupCount === 0) return cur;
      return Math.min(groupCount - 1, Math.max(0, cur + delta));
    });
  }

  const hasRuns = runs.length > 0;

  return (
    <I18nContext.Provider value={{ lang, setLang, m }}>
      <div className="app-shell min-h-screen relative">
        {/* Toggles float in the corner so the hero stays clean. */}
        <div className="absolute top-4 right-4 z-10 flex items-center gap-1">
          <ThemeToggle />
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

        <main className="max-w-5xl mx-auto px-6 pb-20">
          {!hasRuns ? (
            // Empty: a Google-like centered hero — brand + one prominent input.
            <FadeUp className="min-h-[78vh] flex flex-col items-center justify-center text-center gap-7">
              <div className="flex flex-col items-center gap-3">
                <span className="grid place-items-center h-16 w-16 rounded-2xl bg-primary/10 text-primary ring-1 ring-primary/15 shadow-sm">
                  <Aperture className="h-8 w-8" />
                </span>
                <h1 className="text-4xl font-semibold tracking-tight">
                  {m.common.appName}
                </h1>
                <p className="text-muted-foreground text-sm max-w-md">
                  {m.common.tagline}
                </p>
              </div>
              <ScanForm onScanStarted={handleScanStarted} />
            </FadeUp>
          ) : (
            // With runs: brand shrinks into a top bar with a compact input.
            <>
              <header className="pt-8 mb-8 flex items-center gap-3">
                <span className="grid place-items-center h-10 w-10 rounded-xl bg-primary/10 text-primary ring-1 ring-primary/15 shadow-sm shrink-0">
                  <Aperture className="h-5 w-5" />
                </span>
                <h1 className="text-xl font-semibold tracking-tight shrink-0">
                  {m.common.appName}
                </h1>
                <div className="flex-1 min-w-0 max-w-xl ml-auto">
                  <ScanForm onScanStarted={handleScanStarted} compact />
                </div>
              </header>

              <div className="mb-3 text-xs uppercase tracking-wider text-muted-foreground font-semibold">
                {m.common.runsSection}
              </div>

              <div className="space-y-4">
                {runs.map((r, i) => (
                  <FadeUp key={r.id} delay={Math.min(i, 6) * 0.04}>
                    <ErrorBoundary resetKey={r.status.state}>
                      <RunCard
                        run={r}
                        progress={progress.get(r.id) ?? null}
                        onOpenDetail={() => setDetailRunId(r.id)}
                      />
                    </ErrorBoundary>
                  </FadeUp>
                ))}
              </div>
            </>
          )}
        </main>

        {/* Wrap each dialog in its own ErrorBoundary: a malformed run record
            or a render bug inside the detail / group view would otherwise
            white-screen the entire app. resetKey rebinds the boundary when
            the underlying run changes so users can recover by re-opening. */}
        <ErrorBoundary resetKey={detailRunId ?? "none"}>
          <RunDetailDialog
            open={detailRunId !== null}
            onOpenChange={(v) => !v && setDetailRunId(null)}
            run={detailRun}
            overrides={detailRunId ? getOverrides(detailRunId) : new Set()}
            onOpenGroup={(idx) => {
              setGroupRun(detailRunId);
              setGroupIdx(idx);
            }}
            onApplyDone={() => detailRunId && subscribeProgress(detailRunId)}
          />
        </ErrorBoundary>

        <ErrorBoundary resetKey={`${groupRun ?? "none"}:${groupIdx ?? "none"}`}>
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
            groupCount={groupCount}
            onNavigate={navigateGroup}
            overrides={groupRun ? getOverrides(groupRun) : new Set()}
            inPlace={groupRunRecord?.in_place ?? false}
            vlmSettings={vlmSettings}
            onOpenSettings={() => setSettingsOpen(true)}
            onToggleOverride={(photoId) =>
              groupRun && toggleOverride(groupRun, photoId)
            }
          />
        </ErrorBoundary>

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
