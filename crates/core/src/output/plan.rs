use crate::ingest::{PhotoId, PhotoRef};
use crate::scoring::{CompositionPick, SelectedGroup};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// A flat materialization plan: where each photo should land relative to the
/// output root.
///
/// Layout:
/// - `picked/comp_<short_id>/<filename>` when Stage B ran
/// - `picked/group_<short_id>/<filename>` when only Stage A ran (no CLIP)
/// - `rejected/<filename>` for everything else
#[derive(Debug, Clone)]
pub struct OutputPlan {
    pub picked: Vec<(PhotoId, PathBuf)>,
    pub rejected: Vec<(PhotoId, PathBuf)>,
}

impl OutputPlan {
    pub fn entries(&self) -> impl Iterator<Item = (&PhotoId, &PathBuf)> {
        self.picked.iter().map(|(p, d)| (p, d)).chain(self.rejected.iter().map(|(p, d)| (p, d)))
    }
}

/// Pick a destination path under `dir` for `name` that doesn't collide with one
/// already assigned. Recursive scans pull files from many subfolders, and DSLR
/// cards reset their counters, so duplicate basenames (`IMG_0001.jpg`) are
/// common — without disambiguation two photos would map to the same path and
/// one would silently overwrite the other during materialization. On collision
/// we insert the content-hash prefix (stable across runs), then a counter for
/// the rare identical-hash case.
fn unique_dest(dir: &Path, name: &OsStr, sha: &[u8; 16], used: &mut HashSet<PathBuf>) -> PathBuf {
    let first = dir.join(name);
    if used.insert(first.clone()) {
        return first;
    }
    let np = Path::new(name);
    let stem = np.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = np.extension().and_then(|s| s.to_str());
    let tag = hex::encode(&sha[..4]);
    let mut attempt = 0u32;
    loop {
        let fname = match (ext, attempt) {
            (Some(e), 0) => format!("{stem}_{tag}.{e}"),
            (None, 0) => format!("{stem}_{tag}"),
            (Some(e), n) => format!("{stem}_{tag}_{n}.{e}"),
            (None, n) => format!("{stem}_{tag}_{n}"),
        };
        let cand = dir.join(fname);
        if used.insert(cand.clone()) {
            return cand;
        }
        attempt += 1;
    }
}

/// Build the output plan. If `composition_picks` is non-empty (Stage B ran),
/// final picks come from its kept set; otherwise fall back to Stage A K1 picks.
pub fn plan_output(
    photos: &HashMap<PhotoId, PhotoRef>,
    stage_a_picks: &[SelectedGroup],
    composition_picks: &[CompositionPick],
) -> OutputPlan {
    let mut picked: Vec<(PhotoId, PathBuf)> = Vec::new();
    let mut picked_set: HashSet<PhotoId> = HashSet::new();
    // Destination paths already claimed, so duplicate basenames don't collide.
    let mut used: HashSet<PathBuf> = HashSet::new();

    if !composition_picks.is_empty() {
        for cp in composition_picks {
            let short = &cp.group.id.0.simple().to_string()[..8];
            let dir = PathBuf::from("picked").join(format!("comp_{short}"));
            for (pid, _) in &cp.kept {
                if let Some(p) = photos.get(pid) {
                    if let Some(name) = p.path.file_name() {
                        picked.push((*pid, unique_dest(&dir, name, &p.sha256_short, &mut used)));
                        picked_set.insert(*pid);
                    }
                }
            }
        }
    } else {
        for sg in stage_a_picks {
            let short = &sg.group.id.0.simple().to_string()[..8];
            let dir = PathBuf::from("picked").join(format!("group_{short}"));
            // Use kept if scored; otherwise the whole singleton group (no score available).
            let ids: Vec<PhotoId> = if sg.kept.is_empty() && sg.rejected.is_empty() {
                sg.group.photo_ids.clone()
            } else {
                sg.kept.iter().map(|(p, _)| *p).collect()
            };
            for pid in ids {
                if let Some(p) = photos.get(&pid) {
                    if let Some(name) = p.path.file_name() {
                        picked.push((pid, unique_dest(&dir, name, &p.sha256_short, &mut used)));
                        picked_set.insert(pid);
                    }
                }
            }
        }
    }

    // Everything else from the Stage A pool is rejected.
    let mut rejected: Vec<(PhotoId, PathBuf)> = Vec::new();
    let rejected_dir = PathBuf::from("rejected");
    for sg in stage_a_picks {
        for pid in &sg.group.photo_ids {
            if picked_set.contains(pid) {
                continue;
            }
            if let Some(p) = photos.get(pid) {
                if let Some(name) = p.path.file_name() {
                    rejected.push((*pid, unique_dest(&rejected_dir, name, &p.sha256_short, &mut used)));
                }
            }
        }
    }

    OutputPlan { picked, rejected }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group::{Group, GroupId};
    use crate::ingest::{ImageFormat, PhotoId, PhotoRef};

    fn photo(id: PhotoId, path: &str, sha: u8) -> PhotoRef {
        PhotoRef {
            id,
            path: path.into(),
            format: ImageFormat::Jpeg,
            captured_at: None,
            file_size: 0,
            sha256_short: [sha; 16],
            burst_id: None,
            drive_mode: None,
            iso: None,
            exposure_bias_ev: None,
        }
    }

    #[test]
    fn duplicate_basenames_get_distinct_dests() {
        // Two rejected photos with the same filename from different folders.
        let a = PhotoId::new();
        let b = PhotoId::new();
        let photos: HashMap<_, _> = [
            (a, photo(a, "/card1/IMG_0001.jpg", 0x11)),
            (b, photo(b, "/card2/IMG_0001.jpg", 0x22)),
        ]
        .into();
        // Both in one Stage A group, neither picked (kept empty but rejected non-empty
        // would mark them scored; use a group with both as members and no kept).
        let sg = SelectedGroup {
            group: Group { id: GroupId::new(), photo_ids: vec![a, b], representative: a },
            kept: vec![],
            rejected: vec![],
        };
        // kept+rejected both empty => singleton-style: both photo_ids become "picked".
        let plan = plan_output(&photos, &[sg], &[]);
        let dests: HashSet<&PathBuf> = plan.picked.iter().map(|(_, d)| d).collect();
        assert_eq!(dests.len(), plan.picked.len(), "every dest must be unique");
        assert_eq!(plan.picked.len(), 2);
    }
}
