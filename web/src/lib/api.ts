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
    const qs = path ? `?path=${encodeURIComponent(path)}` : "";
    return request(`/api/browse${qs}`);
  },

  async apply(
    runId: string,
    deleteIds: string[],
    useTrash: boolean
  ): Promise<ApplyResult> {
    return request(`/api/runs/${runId}/apply`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ delete_ids: deleteIds, use_trash: useTrash }),
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
