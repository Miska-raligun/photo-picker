import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { authHeaders, loadToken, saveToken, syncTokenCookie } from "./tokenStore";

function installStorage() {
  const map = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => map.get(k) ?? null,
    setItem: (k: string, v: string) => void map.set(k, v),
    removeItem: (k: string) => void map.delete(k),
  });
  return map;
}

/// `document.cookie` is a setter that appends; emulate just enough of that
/// to assert what the store writes.
function installCookieJar() {
  const jar: string[] = [];
  vi.stubGlobal("document", {
    get cookie() {
      return jar.join("; ");
    },
    set cookie(v: string) {
      jar.push(v);
    },
  });
  return jar;
}

describe("tokenStore", () => {
  beforeEach(() => {
    installStorage();
    installCookieJar();
  });
  afterEach(() => vi.unstubAllGlobals());

  it("round-trips the token and mirrors it into a cookie", () => {
    saveToken("s3cret");
    expect(loadToken()).toBe("s3cret");
    expect(document.cookie).toContain("photo_pick_token=s3cret");
    // Header-less requests rely on the cookie, so it must be scoped to the
    // whole app, not just the current path.
    expect(document.cookie).toContain("path=/");
  });

  it("saving an empty token clears storage and expires the cookie", () => {
    saveToken("gone-soon");
    saveToken("");
    expect(loadToken()).toBe("");
    expect(document.cookie).toContain("Max-Age=0");
  });

  it("authHeaders is empty without a token and Bearer with one", () => {
    expect(authHeaders()).toEqual({});
    saveToken("abc");
    expect(authHeaders()).toEqual({ authorization: "Bearer abc" });
  });

  it("syncTokenCookie re-asserts a stored token (cookies are session-scoped)", () => {
    localStorage.setItem("photo-pick.token", "restored");
    syncTokenCookie();
    expect(document.cookie).toContain("photo_pick_token=restored");
  });

  it("token values are URL-encoded so punctuation can't break the cookie", () => {
    saveToken("a b;c");
    expect(document.cookie).toContain("photo_pick_token=a%20b%3Bc");
  });
});
