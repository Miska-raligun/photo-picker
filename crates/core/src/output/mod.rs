mod report;

pub use report::write_json_report;

use crate::error::{Error, Result};
use crate::ingest::{PhotoId, PhotoRef};
use crate::pipeline::LinkMode;
use crate::scoring::SelectedGroup;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Materialize picked + rejected photos under `output_root` using the requested
/// link mode.
///
/// Layout:
///   `output_root/picked/group_<short_id>/<filename>`
///   `output_root/rejected/group_<short_id>/<filename>`
///
/// Returns `(picked_paths, rejected_paths)`.
pub fn materialize(
    output_root: &Path,
    photos: &HashMap<PhotoId, PhotoRef>,
    selections: &[SelectedGroup],
    link_mode: LinkMode,
) -> Result<(Vec<(PhotoId, PathBuf)>, Vec<(PhotoId, PathBuf)>)> {
    fs::create_dir_all(output_root)
        .map_err(|e| Error::Io { path: output_root.to_path_buf(), source: e })?;

    let mut picked_jobs: Vec<(PhotoId, PathBuf, PathBuf)> = Vec::new();
    let mut rejected_jobs: Vec<(PhotoId, PathBuf, PathBuf)> = Vec::new();

    for sel in selections {
        let group_dir_name = format!("group_{}", &sel.group.id.0.simple().to_string()[..8]);
        let picked_dir = output_root.join("picked").join(&group_dir_name);
        let rejected_dir = output_root.join("rejected").join(&group_dir_name);

        for (pid, _) in &sel.kept {
            if let Some(dest) = dest_for(photos, pid, &picked_dir) {
                picked_jobs.push((*pid, photos[pid].path.clone(), dest));
            }
        }
        for (pid, _) in &sel.rejected {
            if let Some(dest) = dest_for(photos, pid, &rejected_dir) {
                rejected_jobs.push((*pid, photos[pid].path.clone(), dest));
            }
        }

        // Singleton fallback: no kept/rejected list — keep all photos as picked
        // (covers groups where scoring failed for every member).
        if sel.kept.is_empty() && sel.rejected.is_empty() {
            for pid in &sel.group.photo_ids {
                if let Some(dest) = dest_for(photos, pid, &picked_dir) {
                    picked_jobs.push((*pid, photos[pid].path.clone(), dest));
                }
            }
        }
    }

    let mut dirs: HashSet<PathBuf> = HashSet::new();
    for (_, _, dest) in picked_jobs.iter().chain(rejected_jobs.iter()) {
        if let Some(parent) = dest.parent() {
            dirs.insert(parent.to_path_buf());
        }
    }
    for dir in &dirs {
        fs::create_dir_all(dir).map_err(|e| Error::Io { path: dir.clone(), source: e })?;
    }

    let run_jobs = |jobs: &[(PhotoId, PathBuf, PathBuf)]| -> Result<Vec<(PhotoId, PathBuf)>> {
        jobs.par_iter()
            .map(|(pid, src, dest)| {
                place(src, dest, link_mode)?;
                Ok((*pid, dest.clone()))
            })
            .collect()
    };

    let picked = run_jobs(&picked_jobs)?;
    let rejected = run_jobs(&rejected_jobs)?;
    Ok((picked, rejected))
}

fn dest_for(photos: &HashMap<PhotoId, PhotoRef>, pid: &PhotoId, dir: &Path) -> Option<PathBuf> {
    let p = photos.get(pid)?;
    let name = p.path.file_name()?;
    Some(dir.join(name))
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
