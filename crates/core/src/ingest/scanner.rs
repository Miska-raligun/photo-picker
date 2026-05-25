use super::{exif::extract_exif_info, ImageFormat, PhotoId, PhotoRef, RawKind};
use crate::error::{Error, Result};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// What the pipeline should ingest: either a directory walk or an explicit
/// caller-provided file list.
#[derive(Debug, Clone)]
pub enum PhotoSource {
    Directory(std::path::PathBuf),
    Files(Vec<std::path::PathBuf>),
}

impl PhotoSource {
    /// Best-effort "where did these photos come from" for logs / reports.
    pub fn root_hint(&self) -> std::path::PathBuf {
        match self {
            Self::Directory(p) => p.clone(),
            Self::Files(fs) => fs
                .first()
                .and_then(|f| f.parent())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from(".")),
        }
    }
}

pub trait Scanner: Send + Sync {
    fn scan(&self, root: &Path) -> Result<Vec<PhotoRef>>;
}

pub struct FsScanner {
    pub follow_symlinks: bool,
}

impl Default for FsScanner {
    fn default() -> Self {
        Self { follow_symlinks: false }
    }
}

impl Scanner for FsScanner {
    fn scan(&self, root: &Path) -> Result<Vec<PhotoRef>> {
        // Walk the tree sequentially (cheap directory listing), collecting
        // supported files. The expensive per-file work — full-content SHA-256
        // plus EXIF parsing — is then run in parallel, since on large libraries
        // it dominates and is embarrassingly parallel.
        let walker = WalkDir::new(root)
            .follow_links(self.follow_symlinks)
            .into_iter()
            .filter_entry(|e| !is_hidden(e.path()));

        let mut candidates: Vec<(PathBuf, ImageFormat)> = Vec::new();
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!(error = %err, "skipping unreadable entry");
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let Some(format) = classify(path) else { continue };
            candidates.push((path.to_path_buf(), format));
        }

        // `par_iter().collect()` preserves input (walk) order.
        let out: Vec<PhotoRef> = candidates
            .into_par_iter()
            .filter_map(|(path, format)| match build_photo_ref(path.clone(), format) {
                Ok(p) => Some(p),
                Err(err) => {
                    tracing::warn!(path = %path.display(), %err, "skipping unreadable photo");
                    None
                }
            })
            .collect();

        if out.is_empty() {
            return Err(Error::EmptyScan { root: root.to_path_buf() });
        }
        Ok(out)
    }
}

/// Scan an explicit caller-provided list of photo file paths. Skips entries
/// that don't classify as a supported format or fail to open.
pub fn scan_files(paths: &[std::path::PathBuf]) -> Result<Vec<PhotoRef>> {
    let mut out = Vec::new();
    for path in paths {
        let Some(format) = classify(path) else {
            tracing::warn!(path = %path.display(), "skipping (unsupported extension)");
            continue;
        };
        match build_photo_ref(path.clone(), format) {
            Ok(p) => out.push(p),
            Err(err) => tracing::warn!(path = %path.display(), %err, "skipping unreadable photo"),
        }
    }
    if out.is_empty() {
        return Err(Error::EmptyScan {
            root: paths
                .first()
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from(".")),
        });
    }
    Ok(out)
}

/// Public helper so callers (like the browse endpoint) can ask "is this a
/// photo extension we'd ingest?" without duplicating the table.
pub fn classify_extension(path: &Path) -> Option<super::ImageFormat> {
    classify(path)
}

fn classify(path: &Path) -> Option<ImageFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "jpe" => Some(ImageFormat::Jpeg),
        "cr2" => Some(ImageFormat::Raw(RawKind::Cr2)),
        "cr3" => Some(ImageFormat::Raw(RawKind::Cr3)),
        "nef" => Some(ImageFormat::Raw(RawKind::Nef)),
        "arw" => Some(ImageFormat::Raw(RawKind::Arw)),
        "dng" => Some(ImageFormat::Raw(RawKind::Dng)),
        "pef" => Some(ImageFormat::Raw(RawKind::Pef)),
        "orf" => Some(ImageFormat::Raw(RawKind::Orf)),
        "raf" => Some(ImageFormat::Raw(RawKind::Raf)),
        _ => None,
    }
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
}

fn build_photo_ref(path: PathBuf, format: ImageFormat) -> Result<PhotoRef> {
    let meta = fs::metadata(&path).map_err(|e| Error::Io { path: path.clone(), source: e })?;
    let file_size = meta.len();
    let sha256_short = hash_prefix(&path)?;
    let exif_info = extract_exif_info(&path).unwrap_or_default();

    Ok(PhotoRef {
        id: PhotoId::new(),
        path,
        format,
        captured_at: exif_info.captured_at,
        file_size,
        sha256_short,
        burst_id: exif_info.burst_id,
        drive_mode: exif_info.drive_mode,
        iso: exif_info.iso,
        exposure_bias_ev: exif_info.exposure_bias_ev,
    })
}

fn hash_prefix(path: &Path) -> Result<[u8; 16]> {
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn scans_supported_files_skips_hidden_and_unsupported() {
        let dir = tempdir().unwrap();
        // tempdir's own name starts with ".tmp", which `is_hidden` would filter;
        // scan a normal-named subdirectory instead (mirrors a real user folder).
        let root = dir.path().join("photos");
        fs::create_dir(&root).unwrap();
        let root = root.as_path();
        fs::write(root.join("a.jpg"), b"not-a-real-jpeg-but-fine-for-hashing").unwrap();
        fs::write(root.join("b.JPEG"), b"another").unwrap();
        fs::write(root.join("c.nef"), b"raw-bytes").unwrap();
        fs::write(root.join("notes.txt"), b"ignore me").unwrap();
        fs::write(root.join(".hidden.jpg"), b"hidden").unwrap();

        let scanner = FsScanner::default();
        let mut refs = scanner.scan(root).unwrap();

        assert_eq!(refs.len(), 3, "3 supported, non-hidden files");
        refs.sort_by(|a, b| a.path.cmp(&b.path));
        let names: Vec<_> = refs
            .iter()
            .filter_map(|r| r.path.file_name().and_then(|n| n.to_str()))
            .collect();
        assert_eq!(names, ["a.jpg", "b.JPEG", "c.nef"]);
        // Distinct content must yield distinct content hashes.
        assert_ne!(refs[0].sha256_short, refs[1].sha256_short);
    }

    #[test]
    fn empty_directory_errors() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("photos");
        fs::create_dir(&root).unwrap();
        let err = FsScanner::default().scan(&root);
        assert!(matches!(err, Err(Error::EmptyScan { .. })));
    }
}
