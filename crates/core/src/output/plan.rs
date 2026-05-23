use crate::ingest::{PhotoId, PhotoRef};
use crate::scoring::{CompositionPick, SelectedGroup};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

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

/// Build the output plan. If `composition_picks` is non-empty (Stage B ran),
/// final picks come from its kept set; otherwise fall back to Stage A K1 picks.
pub fn plan_output(
    photos: &HashMap<PhotoId, PhotoRef>,
    stage_a_picks: &[SelectedGroup],
    composition_picks: &[CompositionPick],
) -> OutputPlan {
    let mut picked: Vec<(PhotoId, PathBuf)> = Vec::new();
    let mut picked_set: HashSet<PhotoId> = HashSet::new();

    if !composition_picks.is_empty() {
        for cp in composition_picks {
            let short = &cp.group.id.0.simple().to_string()[..8];
            let dir = PathBuf::from("picked").join(format!("comp_{short}"));
            for (pid, _) in &cp.kept {
                if let Some(p) = photos.get(pid) {
                    if let Some(name) = p.path.file_name() {
                        picked.push((*pid, dir.join(name)));
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
                        picked.push((pid, dir.join(name)));
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
                    rejected.push((*pid, rejected_dir.join(name)));
                }
            }
        }
    }

    OutputPlan { picked, rejected }
}
