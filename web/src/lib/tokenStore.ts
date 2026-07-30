/// Access token for a server started with PHOTO_PICK_TOKEN.
///
/// Three transports are needed because the browser can't set headers
/// everywhere:
///   - `fetch` calls send `Authorization: Bearer <token>`
///   - `<img src>` (thumbnails/previews) and `EventSource` (SSE progress)
///     can't set headers at all, so we also drop a `photo_pick_token`
///     cookie that the server accepts
///
/// The token lives in localStorage — same trust model as the VLM key that
/// already lives there. On a shared machine, don't use either.

const STORAGE_KEY = "photo-pick.token";
const COOKIE_NAME = "photo_pick_token";

export function loadToken(): string {
  try {
    return localStorage.getItem(STORAGE_KEY) ?? "";
  } catch {
    return "";
  }
}

/// Persist the token and mirror it into a session cookie so header-less
/// requests (img, EventSource) authenticate too. `SameSite=Strict` keeps
/// the cookie off cross-site requests; the server is same-origin with the
/// UI so nothing legitimate is lost.
export function saveToken(token: string): void {
  try {
    if (token) localStorage.setItem(STORAGE_KEY, token);
    else localStorage.removeItem(STORAGE_KEY);
  } catch {
    // storage disabled — the cookie below still covers this tab
  }
  try {
    if (token) {
      document.cookie = `${COOKIE_NAME}=${encodeURIComponent(token)}; path=/; SameSite=Strict`;
    } else {
      document.cookie = `${COOKIE_NAME}=; path=/; Max-Age=0; SameSite=Strict`;
    }
  } catch {
    // non-browser context (tests) — nothing to do
  }
}

/// Re-assert the cookie from the stored token. Called once at startup
/// because cookies are session-scoped while localStorage survives restarts.
export function syncTokenCookie(): void {
  const t = loadToken();
  if (t) saveToken(t);
}

export function authHeaders(): Record<string, string> {
  const t = loadToken();
  return t ? { authorization: `Bearer ${t}` } : {};
}
