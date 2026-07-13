// Mirrors the Rust types in crates/server/src/{state,handlers}.rs and
// crates/core/src/scoring/mod.rs. Keep in sync when the server schemas change.

export type Scene = "portrait" | "landscape" | "mixed";

export interface FinalScore {
  scene: Scene;
  tech: number;
  aesthetic: number;
  composition: number;
  face_bonus: number;
  value: number;
}

export interface PhotoView {
  photo_id: string;
  filename: string | null;
  captured_at: string | null;
  iso: number | null;
  final_score: FinalScore | null;
}

export interface CompositionPickView {
  index: number;
  id: string;
  scene: Scene;
  kept: PhotoView[];
  rejected: PhotoView[];
}

export interface PipelineReport {
  photo_count: number;
  cached_count: number;
  extracted_count: number;
  stage_a_group_count: number;
  stage_b_group_count: number;
  picked_count: number;
  rejected_count: number;
  elapsed: { secs: number; nanos: number };
}

export type RunStatus =
  | { state: "running" }
  | { state: "completed" }
  | { state: "failed"; error: string }
  | { state: "cancelled" };

export interface RunRecord {
  id: string;
  root: string;
  output: string;
  in_place: boolean;
  status: RunStatus;
  report: PipelineReport | null;
  html_report: string | null;
  composition_picks?: CompositionPickView[];
  explanations: Record<number, ExplanationRecord>;
}

export interface ExplanationRecord {
  provider: string;
  model: string;
  text: string;
}

export interface ScanRequest {
  root?: string;
  files?: string[];
  /// Optional: when omitted the server stores the run's internal artifacts in
  /// a managed per-source dir. Runs never copy photos here — export is deferred.
  output?: string;
  k1?: number;
  k2?: number;
  time_k?: number;
  min_dt?: number;
  max_dt?: number;
  hash_dist?: number;
  // The Rust side reads `min_dt` / `max_dt` as f32 seconds; the form passes them.
  stage_a_clip_threshold?: number;
  stage_b_threshold?: number;
  /// Stage B structural gate — max dHash Hamming distance (0-64) for two
  /// photos to merge on top of the CLIP check. 0 disables the gate.
  stage_b_structural_max?: number;
  enable_clip?: boolean;
  enable_face?: boolean;
  in_place?: boolean;
  adaptive_thresholds?: boolean;
  link_mode?: "copy" | "hardlink" | "symlink";
  thumbnail_long_edge?: number;
  execution_provider?: ExecutionProvider;
}

export type ExecutionProvider = "cpu" | "cuda" | "coreml" | "directml";

/// Live progress event delivered over SSE from `/api/runs/:id/events`.
/// `Done` is terminal — the server drops the channel right after.
export type ProgressEvent =
  | { kind: "stage"; stage: string; total: number }
  | { kind: "tick"; stage: string; done: number }
  | { kind: "finish"; stage: string }
  | { kind: "done"; ok: boolean };

/// Client-side derived progress for the currently-running stage. Built from
/// the SSE stream; not part of the server-side `RunRecord` JSON.
export interface RunProgress {
  stage: string;
  done: number;
  total: number;
}

export interface BrowseFile {
  name: string;
  path: string;
  size: number;
  format: string;
}

export interface BrowseEntry {
  name: string;
  path: string;
}

export interface BrowseResponse {
  current: string;
  parent: string | null;
  dirs: BrowseEntry[];
  files: BrowseFile[];
}

export interface VlmConfig {
  provider: "openai" | "anthropic";
  base_url: string;
  api_key: string;
  model: string;
}

export type VlmSettings =
  | { mode: "env" }
  | { mode: "custom"; config: VlmConfig };

export interface ApplyResult {
  requested: number;
  deleted: number;
  failed: ApplyFailure[];
  used_trash: boolean;
  dry_run: boolean;
  would_delete: ApplyTarget[];
  /// Photo ids that were actually deleted on this apply. Subset of the
  /// request's `delete_ids` (excludes anything that hit `failed`). Used by
  /// the UI to clear stale verdict overrides without touching the overrides
  /// on files the apply skipped — those keep their context for a retry.
  deleted_ids: string[];
  /// Path of the on-disk delete manifest (audit trail) when at least one
  /// file was actually deleted. `null` on dry runs and zero-delete results.
  manifest_path: string | null;
}

export interface ApplyTarget {
  photo_id: string;
  path: string;
}

export interface ApplyFailure {
  photo_id: string;
  path: string;
  error: string;
}

export interface ExportResult {
  requested: number;
  exported: number;
  failed: ApplyFailure[];
  target_dir: string;
}

/// One side of a cross-run keep-set comparison. `run_id` names the run the
/// photo (and its thumb URL) belongs to.
export interface RunDiffEntry {
  run_id: string;
  photo_id: string;
  filename: string | null;
  /// false ⇒ the file isn't in the other run at all (added/removed on disk);
  /// true ⇒ same content present in both, but the verdict differs.
  present_in_other: boolean;
}

export interface RunDiff {
  run_id: string;
  other_id: string;
  kept_here: number;
  kept_there: number;
  added_kept: RunDiffEntry[];
  removed_kept: RunDiffEntry[];
  photos_only_here: number;
  photos_only_there: number;
}

/// User-attached metadata for one photo, stored per-run in localStorage
/// (same lifecycle as verdict overrides).
export interface PhotoTag {
  flag?: boolean;
  note?: string;
}
