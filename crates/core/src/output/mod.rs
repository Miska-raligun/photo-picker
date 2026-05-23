mod report;

pub use report::{write_json_report, JsonReport};

use crate::error::{Error, Result};
use crate::group::Group;
use crate::ingest::{PhotoId, PhotoRef};
use crate::pipeline::LinkMode;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Materialize the chosen photos under `output_root` using the requested link mode.
///
/// Layout: `output_root/group_<short_id>/<original_filename>`.
/// Returns a flat list of (PhotoId, destination path).
pub fn materialize(
    output_root: &Path,
    photos: &HashMap<PhotoId, PhotoRef>,
    groups: &[Group],
    link_mode: LinkMode,
) -> Result<Vec<(PhotoId, PathBuf)>> {
    fs::create_dir_all(output_root)
        .map_err(|e| Error::Io { path: output_root.to_path_buf(), source: e })?;

    let jobs: Vec<(PhotoId, PathBuf, PathBuf)> = groups
        .iter()
        .flat_map(|g| {
            let group_dir = output_root.join(format!("group_{}", &g.id.0.simple().to_string()[..8]));
            g.photo_ids.iter().filter_map(move |pid| {
                let p = photos.get(pid)?;
                let file_name = p.path.file_name()?.to_owned();
                let dest = group_dir.join(file_name);
                Some((*pid, p.path.clone(), dest))
            })
        })
        .collect();

    // Pre-create group directories sequentially (cheap, avoids races).
    let mut dirs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for (_, _, dest) in &jobs {
        if let Some(parent) = dest.parent() {
            dirs.insert(parent.to_path_buf());
        }
    }
    for dir in &dirs {
        fs::create_dir_all(dir).map_err(|e| Error::Io { path: dir.clone(), source: e })?;
    }

    let results: Vec<Result<(PhotoId, PathBuf)>> = jobs
        .par_iter()
        .map(|(pid, src, dest)| {
            place(src, dest, link_mode)?;
            Ok((*pid, dest.clone()))
        })
        .collect();

    results.into_iter().collect()
}

fn place(src: &Path, dest: &Path, mode: LinkMode) -> Result<()> {
    if dest.exists() {
        fs::remove_file(dest).map_err(|e| Error::Io { path: dest.to_path_buf(), source: e })?;
    }
    let res = match mode {
        LinkMode::Copy => fs::copy(src, dest).map(|_| ()),
        LinkMode::Hardlink => fs::hard_link(src, dest),
        LinkMode::Symlink => symlink(src, dest),
    };

    // Hard-link fails across filesystems; fall back to copy so the user still gets the file.
    match res {
        Ok(()) => Ok(()),
        Err(e) if mode == LinkMode::Hardlink => {
            tracing::warn!(
                src = %src.display(), dest = %dest.display(),
                "hardlink failed ({e}); falling back to copy"
            );
            fs::copy(src, dest)
                .map(|_| ())
                .map_err(|e| Error::Io { path: dest.to_path_buf(), source: e })
        }
        Err(e) => Err(Error::Io { path: dest.to_path_buf(), source: e }),
    }
}

#[cfg(unix)]
fn symlink(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dest)
}

#[cfg(windows)]
fn symlink(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(src, dest)
}
