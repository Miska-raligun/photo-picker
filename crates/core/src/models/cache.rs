use crate::error::{Error, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Static description of an ONNX model: where to fetch it from and the SHA-256
/// of the expected file. Hashes are pinned so we never silently use a model
/// HuggingFace may have updated under the same path.
#[derive(Debug, Clone, Copy)]
pub struct ModelDescriptor {
    /// Short human-readable name for logs / UI ("clip-vit-b32-vision").
    pub name: &'static str,
    /// Local filename inside the cache directory.
    pub filename: &'static str,
    /// Full URL to download from (HuggingFace `resolve/main/...` recommended).
    pub url: &'static str,
    /// Hex-encoded SHA-256 of the file at `url`.
    pub sha256_hex: &'static str,
    /// Approximate file size in bytes (for progress UI; not enforced).
    pub size_bytes: u64,
}

/// Return the cache directory for ONNX models. Created if missing.
///
/// Honors `PHOTO_PICK_MODELS_DIR` first: release bundles ship the models
/// alongside the binary and point this at them, so a packaged copy runs fully
/// offline (the SHA-256 check in `ensure_model` still verifies them, and falls
/// back to downloading only if a bundled file is missing/corrupt). Otherwise
/// uses the per-user XDG cache.
pub fn cache_dir() -> Result<PathBuf> {
    let dir = if let Some(custom) = std::env::var_os("PHOTO_PICK_MODELS_DIR") {
        PathBuf::from(custom)
    } else {
        let base = dirs::cache_dir()
            .ok_or_else(|| Error::Config("no XDG cache dir available on this system".into()))?;
        base.join("photo-pick").join("models")
    };
    fs::create_dir_all(&dir).map_err(|e| Error::Io { path: dir.clone(), source: e })?;
    Ok(dir)
}

/// Ensure the model is present in the local cache and matches its declared
/// SHA-256, downloading if necessary. Returns the absolute path to the file.
pub fn ensure_model(desc: &ModelDescriptor) -> Result<PathBuf> {
    let dir = cache_dir()?;
    let path = dir.join(desc.filename);

    if path.exists() {
        if verify_sha256(&path, desc.sha256_hex)? {
            tracing::debug!(model = desc.name, path = %path.display(), "cache hit");
            return Ok(path);
        }
        tracing::warn!(
            model = desc.name,
            "cached file failed checksum; redownloading"
        );
        fs::remove_file(&path).map_err(|e| Error::Io { path: path.clone(), source: e })?;
    }

    tracing::info!(
        model = desc.name,
        size_mb = desc.size_bytes / 1_048_576,
        "downloading from {}",
        desc.url
    );
    download(desc.url, &path)?;

    if !verify_sha256(&path, desc.sha256_hex)? {
        fs::remove_file(&path).ok();
        return Err(Error::Config(format!(
            "{}: SHA-256 mismatch after download (file may be corrupted or descriptor outdated)",
            desc.name
        )));
    }
    Ok(path)
}

fn download(url: &str, dest: &Path) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(900)))
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| Error::Config(format!("download {url} failed: {e}")))?;

    let tmp = dest.with_extension("part");
    let mut out = fs::File::create(&tmp).map_err(|e| Error::Io { path: tmp.clone(), source: e })?;
    let mut reader = resp.body_mut().as_reader();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| Error::Io { path: tmp.clone(), source: e })?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| Error::Io { path: tmp.clone(), source: e })?;
    }
    out.flush().map_err(|e| Error::Io { path: tmp.clone(), source: e })?;
    drop(out);

    fs::rename(&tmp, dest).map_err(|e| Error::Io { path: dest.to_path_buf(), source: e })?;
    Ok(())
}

fn verify_sha256(path: &Path, expected_hex: &str) -> Result<bool> {
    let mut f = fs::File::open(path).map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = hex::encode(hasher.finalize());
    Ok(actual.eq_ignore_ascii_case(expected_hex))
}
