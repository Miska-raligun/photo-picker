import type {
  ApplyResult,
  BrowseResponse,
  ExecutionProvider,
  ExplanationRecord,
  ExportResult,
  RunRecord,
  ScanRequest,
  VlmConfig,
} from "./types";

class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function request<T>(input: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(input, init);
  if (!resp.ok) {
    const text = await resp.text().catch(() => resp.statusText);
    throw new ApiError(resp.status, text || resp.statusText);
  }
  // GET /thumb returns binary; everything else is JSON. Caller picks the
  // right helper.
  return (await resp.json()) as T;
}

export const api = {
  async scan(req: ScanRequest): Promise<{ run_id: string }> {
    return request("/api/scan", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(req),
    });
  },

  async listRuns(): Promise<RunRecord[]> {
    return request("/api/runs");
  },

  /// Server build info (version + providers + managed data dir). Loaded once
  /// on startup so the UI can show "vX.Y.Z" and ops can grab it for issues.
  async info(): Promise<{
    name: string;
    version: string;
    providers: ExecutionProvider[];
    data_dir: string;
  }> {
    return request("/api/info");
  },

  /// Which ONNX execution providers this server build actually has. The UI
  /// uses the result to hide GPU options that would silently fall back to
  /// CPU. CPU is always present.
  async listProviders(): Promise<{ providers: ExecutionProvider[] }> {
    return request("/api/providers");
  },

  async getRun(id: string): Promise<RunRecord> {
    return request(`/api/runs/${id}`);
  },

  async browse(path?: string): Promise<BrowseResponse> {
    // `path === ""` is meaningful on Windows (= "This PC" drives view), so
    // pass it through explicitly instead of treating empty as omitted.
    const qs =
      path !== undefined
        ? `?path=${encodeURIComponent(path)}`
        : "";
    return request(`/api/browse${qs}`);
  },

  /// Apply (delete) a set of photos. When `dryRun` is true the server resolves
  /// + safety-checks every path but skips the actual delete and returns the
  /// preview in the result's `would_delete` / `failed` fields. The UI uses
  /// the preview to render a confirm-before-destruction dialog that reflects
  /// real path state (missing files, symlinks pointing outside the run root)
  /// instead of guessing from in-memory state.
  async apply(
    runId: string,
    deleteIds: string[],
    useTrash: boolean,
    dryRun = false
  ): Promise<ApplyResult> {
    return request(`/api/runs/${runId}/apply`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        delete_ids: deleteIds,
        use_trash: useTrash,
        dry_run: dryRun,
      }),
    });
  },

  async export(
    runId: string,
    photoIds: string[],
    targetDir: string,
    linkMode: "copy" | "hardlink" | "symlink" = "copy"
  ): Promise<ExportResult> {
    return request(`/api/runs/${runId}/export`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        photo_ids: photoIds,
        target_dir: targetDir,
        link_mode: linkMode,
      }),
    });
  },

  async explain(
    runId: string,
    compositionIndex: number,
    provider: "openai" | "anthropic",
    vlmOverride?: VlmConfig,
    language?: "en" | "zh"
  ): Promise<ExplanationRecord> {
    return request(`/api/runs/${runId}/explain`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        composition_index: compositionIndex,
        provider,
        ...(vlmOverride ? { vlm: vlmOverride } : {}),
        ...(language ? { language } : {}),
      }),
    });
  },

  thumbUrl(runId: string, photoId: string): string {
    return `/api/runs/${runId}/thumb/${photoId}`;
  },

  previewUrl(runId: string, photoId: string, size = 1920): string {
    return `/api/runs/${runId}/preview/${photoId}?size=${size}`;
  },

  htmlReportUrl(runId: string): string {
    return `/api/runs/${runId}/html`;
  },
};

export { ApiError };
