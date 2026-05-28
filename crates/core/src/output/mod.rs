mod html;
mod plan;
mod report;
mod thumb_cache;

pub use html::write_html_report;
pub use plan::{plan_output, OutputPlan};
pub use report::write_json_report;
pub use thumb_cache::{ThumbDiskCache, DEFAULT_THUMB_LONG_EDGE, DEFAULT_THUMB_QUALITY};

use crate::error::{Error, Result};
use crate::ingest::{PhotoId, PhotoRef};
use crate::pipeline::LinkMode;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Write the planned picked + rejected files under `output_root` using the
/// requested link mode.
pub fn materialize(
    output_root: &Path,
    photos: &HashMap<PhotoId, PhotoRef>,
    plan: &OutputPlan,
    link_mode: LinkMode,
) -> Result<(usize, usize)> {
    fs::create_dir_all(output_root)
        .map_err(|e| Error::Io { path: output_root.to_path_buf(), source: e })?;

    let jobs: Vec<(PathBuf, PathBuf)> = plan
        .entries()
        .filter_map(|(pid, rel_dest)| {
            let src = photos.get(pid)?.path.clone();
            Some((src, output_root.join(rel_dest)))
        })
        .collect();

    let mut dirs: HashSet<PathBuf> = HashSet::new();
    for (_, dest) in &jobs {
        if let Some(p) = dest.parent() {
            dirs.insert(p.to_path_buf());
        }
    }
    for d in &dirs {
        fs::create_dir_all(d).map_err(|e| Error::Io { path: d.clone(), source: e })?;
    }

    jobs.par_iter()
        .try_for_each(|(src, dest)| place_file(src, dest, link_mode))?;

    Ok((plan.picked.len(), plan.rejected.len()))
}

/// Place a single source file at `dest` using the requested link mode,
/// overwriting any existing file. Hardlink failures (e.g. cross-device)
/// fall back to a plain copy. Shared by the pipeline's `materialize` Write
/// stage and the deferred per-run export endpoint.
pub fn place_file(src: &Path, dest: &Path, mode: LinkMode) -> Result<()> {
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
