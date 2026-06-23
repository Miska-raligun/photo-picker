/// Toggle for the "scan finished" OS notification. Off by default so a
/// fresh install doesn't surprise the user with a permission prompt the
/// first time a run completes; flipped on from Settings.
///
/// Stored as the literal string "on" / "off" in localStorage so a manual
/// inspection in DevTools reads the same as the UI.

const STORAGE_KEY = "photo-pick.notify";

export function loadNotifyEnabled(): boolean {
  try {
    return localStorage.getItem(STORAGE_KEY) === "on";
  } catch {
    return false;
  }
}

export function saveNotifyEnabled(on: boolean): void {
  try {
    localStorage.setItem(STORAGE_KEY, on ? "on" : "off");
  } catch {
    // Storage disabled / private mode — the toggle will just live for the
    // current tab session, which is acceptable.
  }
}

/// Request Notification permission if we haven't already, then return the
/// final permission state. Caller decides whether to surface a toast on
/// "denied" — for the settings flow we want to know if the user actually
/// granted, because flipping the toggle on without permission is a trap.
export async function ensureNotificationPermission(): Promise<NotificationPermission> {
  if (typeof Notification === "undefined") return "denied";
  if (Notification.permission === "granted" || Notification.permission === "denied") {
    return Notification.permission;
  }
  try {
    return await Notification.requestPermission();
  } catch {
    // Older Safari etc. — treat as denied.
    return "denied";
  }
}

/// Fire a "scan complete" notification. No-ops silently when the toggle is
/// off, the API is unavailable, or permission isn't granted — the caller
/// shouldn't have to know about those edge cases at the call site.
export function fireScanCompleteNotification(body: string): void {
  if (!loadNotifyEnabled()) return;
  if (typeof Notification === "undefined") return;
  if (Notification.permission !== "granted") return;
  try {
    // `tag` collapses multiple completions into one popup if a fast user
    // kicks off back-to-back scans.
    new Notification("photo-pick", { body, tag: "photo-pick-scan-complete" });
  } catch {
    // Some browsers reject construction in non-https contexts even with
    // permission — swallow silently rather than blowing up the SSE handler.
  }
}
