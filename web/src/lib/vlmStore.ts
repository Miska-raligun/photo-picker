// localStorage-backed VLM settings store.
//
// Security: API keys live in browser localStorage. That means anyone with
// access to this browser profile (or XSS into this page) can read them.
// For a single-user local app this is acceptable; never expose this UI to
// a multi-user / remote deployment without rethinking storage.

import type { VlmSettings } from "./types";

const KEY = "photo-pick.vlm";

export function loadVlmSettings(): VlmSettings {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return { mode: "env" };
    const parsed = JSON.parse(raw);
    if (parsed?.mode === "custom" && parsed?.config?.api_key) return parsed;
    if (parsed?.mode === "env") return parsed;
    return { mode: "env" };
  } catch {
    return { mode: "env" };
  }
}

export function saveVlmSettings(s: VlmSettings) {
  localStorage.setItem(KEY, JSON.stringify(s));
}

export function clearVlmSettings() {
  localStorage.removeItem(KEY);
}

/** Convenience presets — URL + model only, never embed a real API key here. */
export const VLM_PRESETS: Record<string, { provider: "openai" | "anthropic"; base_url: string; model: string }> = {
  "openai-gpt-4o": {
    provider: "openai",
    base_url: "https://api.openai.com/v1/chat/completions",
    model: "gpt-4o",
  },
  "siliconflow-qwen3-vl-32b": {
    provider: "openai",
    base_url: "https://api.siliconflow.cn/v1/chat/completions",
    model: "Qwen/Qwen3-VL-32B-Instruct",
  },
  "anthropic-claude-opus": {
    provider: "anthropic",
    base_url: "https://api.anthropic.com/v1/messages",
    model: "claude-opus-4-7",
  },
  "anthropic-claude-sonnet": {
    provider: "anthropic",
    base_url: "https://api.anthropic.com/v1/messages",
    model: "claude-sonnet-4-6",
  },
};
