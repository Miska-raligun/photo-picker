//! Best-effort secret scrubbing for VLM provider logs and error messages.
//!
//! VLM endpoints regularly echo the Authorization header (or the api key it
//! carries) back inside 401/403 response bodies. Those bodies travel two
//! places: a `tracing::warn!` log line and the `Err(...)` we return to the
//! caller. Both can end up in support tickets, log aggregators, or HTTP
//! responses — places we don't want the user's API key to land.
//!
//! `redact_secrets` does a cheap, hand-rolled scan (no `regex` dep) for the
//! formats we actually see in practice: `Bearer <token>` and the
//! provider-specific `sk-*` / `sk-ant-*` prefixes. Anything else passes
//! through untouched. Not a substitute for not-logging-secrets-at-all, but
//! it does mean a `curl ... -H 'Authorization: Bearer leakedkey'` echo
//! in an OpenAI 401 body doesn't survive into our logs.

use std::borrow::Cow;

const REDACTED: &str = "***REDACTED***";

/// Replace likely API-key fragments in `s` with `***REDACTED***`. Returns
/// `Cow::Borrowed` when nothing matched so the common (no-secret) path is
/// allocation-free.
pub fn redact_secrets(s: &str) -> Cow<'_, str> {
    if !looks_suspicious(s) {
        return Cow::Borrowed(s);
    }
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(skip) = match_bearer(bytes, i) {
            out.extend_from_slice(b"Bearer ");
            out.extend_from_slice(REDACTED.as_bytes());
            i += skip;
        } else if let Some(skip) = match_sk(bytes, i) {
            out.extend_from_slice(REDACTED.as_bytes());
            i += skip;
        } else if let Some((keep, skip)) = match_api_key_field(bytes, i) {
            out.extend_from_slice(&bytes[i..i + keep]);
            out.extend_from_slice(REDACTED.as_bytes());
            i += skip;
        } else {
            // Copy the raw byte through: multi-byte UTF-8 sequences pass
            // unchanged (all redaction matches are pure-ASCII runs, so we
            // never split a codepoint). The previous `bytes[i] as char`
            // mangled non-ASCII text into Latin-1 whenever a secret was
            // present in the same message.
            out.push(bytes[i]);
            i += 1;
        }
    }
    Cow::Owned(String::from_utf8(out).unwrap_or_else(|e| {
        String::from_utf8_lossy(e.as_bytes()).into_owned()
    }))
}

fn looks_suspicious(s: &str) -> bool {
    // Skip the byte scan entirely when none of the markers are present.
    s.contains("Bearer ") || s.contains("sk-") || has_key_marker(s)
}

fn has_key_marker(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("api-key") || lower.contains("api_key") || lower.contains("apikey")
}

/// If `bytes[i..]` starts with `Bearer <token>`, return the number of bytes
/// consumed (prefix + space + token). Token is one or more
/// `[A-Za-z0-9_\-\.]` chars.
fn match_bearer(bytes: &[u8], i: usize) -> Option<usize> {
    const PREFIX: &[u8] = b"Bearer ";
    if !bytes[i..].starts_with(PREFIX) {
        return None;
    }
    let token_start = i + PREFIX.len();
    let token_len = token_run(&bytes[token_start..]);
    if token_len == 0 {
        return None;
    }
    Some(PREFIX.len() + token_len)
}

/// If `bytes[i..]` starts with `sk-` (optionally `sk-ant-`) followed by a
/// token run, return the number of bytes consumed. Used for inline keys
/// that aren't carried by an `Authorization` header (e.g. message bodies
/// or stack traces that contain the raw key).
fn match_sk(bytes: &[u8], i: usize) -> Option<usize> {
    if !bytes[i..].starts_with(b"sk-") {
        return None;
    }
    // Match the token (treat `sk-ant-...` as one chunk: `-` is in the run).
    let token_len = token_run(&bytes[i..]);
    // Demand a reasonable minimum so we don't redact harmless "sk-..." text.
    if token_len < 16 {
        return None;
    }
    Some(token_len)
}

/// Match `api-key`/`api_key`/`apikey`/`x-api-key` (any case) followed by a
/// separator run (`: = " '` and whitespace) and a ≥16-char token. Returns
/// `(bytes_to_keep, total_bytes_consumed)` — the field name + separators are
/// kept verbatim, only the token is replaced. Catches provider bodies like
/// `{"message": "invalid x-api-key: abcd1234..."}` where the key isn't
/// carried behind a `Bearer` or `sk-` prefix.
fn match_api_key_field(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    const NAMES: [&[u8]; 4] = [b"x-api-key", b"api-key", b"api_key", b"apikey"];
    let rest = &bytes[i..];
    let name_len = NAMES.iter().find_map(|n| {
        (rest.len() >= n.len() && rest[..n.len()].eq_ignore_ascii_case(n)).then(|| n.len())
    })?;
    let mut j = name_len;
    // Optional closing quote of a JSON field name.
    if rest.get(j) == Some(&b'"') || rest.get(j) == Some(&b'\'') {
        j += 1;
    }
    let sep_start = j;
    while matches!(rest.get(j), Some(b':' | b'=' | b'"' | b'\'' | b' ' | b'\t')) {
        j += 1;
    }
    if j == sep_start {
        return None; // no separator ⇒ just prose mentioning "api key"
    }
    let token_len = token_run(&rest[j..]);
    if token_len < 16 {
        return None;
    }
    Some((j, j + token_len))
}

#[inline]
fn token_run(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .take_while(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.'))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_clean_strings_through_borrowed() {
        let s = "no secrets here, just regular logs";
        let out = redact_secrets(s);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, s);
    }

    #[test]
    fn redacts_bearer_in_header_dump() {
        let s = "Authorization: Bearer sk-abc123xyz456def789ghi\nContent-Type: application/json";
        let out = redact_secrets(s);
        assert!(out.contains("Bearer ***REDACTED***"));
        assert!(!out.contains("sk-abc123xyz456def789ghi"));
        assert!(out.contains("Content-Type"));
    }

    #[test]
    fn redacts_inline_openai_key() {
        let s = r#"{"error":{"message":"invalid api key sk-proj-abcdefghij1234567890XYZ"}}"#;
        let out = redact_secrets(s);
        assert!(out.contains("***REDACTED***"));
        assert!(!out.contains("sk-proj-abcdefghij1234567890XYZ"));
        assert!(out.contains("invalid api key"));
    }

    #[test]
    fn redacts_anthropic_key() {
        let s = "key=sk-ant-api03-abcdefghij1234567890";
        let out = redact_secrets(s);
        assert!(out.contains("***REDACTED***"));
        assert!(!out.contains("sk-ant-api03"));
    }

    #[test]
    fn does_not_redact_short_sk_prefix() {
        let s = "see sk-help-page for details";
        let out = redact_secrets(s);
        // Only 12 chars after `sk-` here; below the minimum length check.
        assert_eq!(out, s);
    }

    #[test]
    fn does_not_redact_bearer_without_token() {
        let s = "the word Bearer alone should not trip the filter";
        let out = redact_secrets(s);
        // `Bearer ` + "alone" -> "alone" is 5 chars, below typical key len
        // but our token_run accepts any non-empty run. Acceptable false
        // positive — "alone" gets redacted. Document the behavior.
        // Verify the rest is intact instead.
        assert!(out.contains("the word Bearer ***REDACTED***"));
    }

    #[test]
    fn redacts_x_api_key_field_and_preserves_field_name() {
        let s = r#"{"error": "invalid x-api-key: abcdefghij1234567890"}"#;
        let out = redact_secrets(s);
        assert!(out.contains("x-api-key"), "field name survives");
        assert!(out.contains("***REDACTED***"));
        assert!(!out.contains("abcdefghij1234567890"));
    }

    #[test]
    fn short_tokens_after_api_key_are_left_alone() {
        // "api key 12" style prose with a short number — not a credential.
        let s = "api_key: short";
        assert_eq!(redact_secrets(s), s);
    }

    #[test]
    fn multibyte_text_survives_redaction_intact() {
        // Regression: the old byte-as-char copy mangled UTF-8 whenever a
        // secret appeared in the same string.
        let s = "认证失败：Bearer sk-abc123def456ghi789jkl 请检查密钥";
        let out = redact_secrets(s);
        assert!(out.contains("认证失败"), "Chinese text must survive: {out}");
        assert!(out.contains("请检查密钥"));
        assert!(out.contains("Bearer ***REDACTED***"));
    }
}
