//! Optional TOML config file → environment-variable bridge.
//!
//! Every server knob has always been an env var (`PHOTO_PICK_*`). That stays
//! the source of truth — this module just lets users keep those settings in
//! one file instead of a launcher script. Precedence: real env var (explicit
//! wins) > config file > built-in default.
//!
//! File location, first hit wins:
//! 1. `$PHOTO_PICK_CONFIG` (explicit path; missing file is an error worth a
//!    warn since the user asked for it)
//! 2. `./photo-pick.toml` (next to where you launched the server)
//! 3. `<OS config dir>/photo-pick/config.toml`
//!
//! Format: flat keys, snake_case, matching the env var name minus the
//! `PHOTO_PICK_` prefix (case-insensitive):
//!
//! ```toml
//! bind = "127.0.0.1:7777"
//! browse_roots = "/home/me/Photos:/mnt/nas"
//! scan_concurrency = 2
//! thumb_cache_mb = 256
//! max_runs = 50
//! cache_max_rows = 200000
//! thumb_disk_max_mb = 1024
//! ```

use std::path::PathBuf;

/// Env vars the bridge is allowed to populate. An allowlist (rather than
/// blindly setting `PHOTO_PICK_<KEY>` for any key) keeps a typo'd key a
/// visible warning instead of a silently ignored setting.
const KNOWN_KEYS: &[&str] = &[
    "BIND",
    "TOKEN",
    "BROWSE_ROOTS",
    "CORS_ANY",
    "SCAN_CONCURRENCY",
    "IMAGE_DECODE_CONCURRENCY",
    "THUMB_CACHE_MB",
    "THUMB_DISK_MAX_MB",
    "MAX_RUNS",
    "CACHE_MAX_ROWS",
    "MODELS_DIR",
    "INFERENCE_POOL_SIZE",
    "LEGACY_COSINE",
];

fn candidate_paths() -> Vec<(PathBuf, bool)> {
    let mut out: Vec<(PathBuf, bool)> = Vec::new();
    if let Some(explicit) = std::env::var_os("PHOTO_PICK_CONFIG") {
        // (path, explicitly_requested)
        out.push((PathBuf::from(explicit), true));
        // An explicit path is exclusive — don't silently fall back to a
        // different file than the one the user named.
        return out;
    }
    out.push((PathBuf::from("photo-pick.toml"), false));
    if let Some(dir) = dirs::config_dir() {
        out.push((dir.join("photo-pick").join("config.toml"), false));
    }
    out
}

/// Load the first config file found and export its keys as `PHOTO_PICK_*`
/// env vars — skipping any var the user already set explicitly. Call once,
/// at the very top of `main`, before anything reads the env.
pub fn apply_config_file() {
    for (path, explicit) in candidate_paths() {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if explicit {
                    eprintln!(
                        "photo-pick: PHOTO_PICK_CONFIG points at {} but it doesn't exist",
                        path.display()
                    );
                }
                continue;
            }
            Err(e) => {
                eprintln!("photo-pick: config {} unreadable: {e}", path.display());
                continue;
            }
        };
        let table: toml::Table = match text.parse() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("photo-pick: config {} parse error: {e}", path.display());
                return; // A malformed file the user wrote deserves a stop-and-look, not a fallback.
            }
        };
        let mut applied = 0usize;
        for (key, value) in &table {
            let upper = key.to_ascii_uppercase();
            if !KNOWN_KEYS.contains(&upper.as_str()) {
                eprintln!(
                    "photo-pick: config {}: unknown key `{key}` (expected one of: {})",
                    path.display(),
                    KNOWN_KEYS.join(", ").to_lowercase()
                );
                continue;
            }
            let env_name = format!("PHOTO_PICK_{upper}");
            if std::env::var_os(&env_name).is_some() {
                continue; // explicit env var wins
            }
            // Accept strings/ints/floats/bools; render non-strings the way a
            // shell would have. `true`/`false` for the *_ANY style flags map
            // to set/unset because those vars are presence-checked.
            let rendered = match value {
                toml::Value::String(s) => s.clone(),
                toml::Value::Boolean(false) => continue, // presence-checked flags: false = leave unset
                toml::Value::Boolean(true) => "1".into(),
                other => other.to_string(),
            };
            std::env::set_var(&env_name, &rendered);
            applied += 1;
        }
        if applied > 0 {
            eprintln!(
                "photo-pick: applied {applied} setting(s) from {}",
                path.display()
            );
        }
        return; // first found file wins
    }
}
