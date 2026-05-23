use crate::error::{Error, Result};
use crate::group::CompositionGroup;
use crate::ingest::{PhotoId, PhotoRef};
use crate::scoring::{CompositionPick, FinalScore, SelectedGroup, TechScore};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct JsonReport<'a> {
    pub version: &'static str,
    pub root: PathBuf,
    pub generated_at: DateTime<Utc>,
    pub elapsed_secs: f64,
    pub photo_count: usize,
    pub stage_a_group_count: usize,
    pub stage_b_group_count: usize,
    pub picked_count: usize,
    pub rejected_count: usize,
    pub stage_a_groups: Vec<JsonStageAGroup<'a>>,
    pub composition_groups: Vec<JsonCompositionGroup<'a>>,
}

#[derive(Debug, Serialize)]
pub struct JsonStageAGroup<'a> {
    pub id: String,
    pub representative: String,
    pub photos: Vec<JsonPhoto<'a>>,
}

#[derive(Debug, Serialize)]
pub struct JsonCompositionGroup<'a> {
    pub id: String,
    pub member_ids: Vec<String>,
    pub picks: Vec<JsonPick<'a>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict { Kept, Rejected, Unscored }

#[derive(Debug, Serialize)]
pub struct JsonPhoto<'a> {
    pub id: String,
    pub path: &'a Path,
    pub captured_at: Option<DateTime<Utc>>,
    pub iso: Option<u32>,
    pub verdict: Verdict,
    pub rank_in_group: Option<usize>,
    pub tech: Option<TechScore>,
}

#[derive(Debug, Serialize)]
pub struct JsonPick<'a> {
    pub id: String,
    pub path: &'a Path,
    pub verdict: Verdict,
    pub rank: usize,
    pub final_score: FinalScore,
}

pub fn write_json_report(
    path: &Path,
    root: &Path,
    elapsed: Duration,
    photos: &HashMap<PhotoId, PhotoRef>,
    selections: &[SelectedGroup],
    stage_b_groups: &[CompositionGroup],
    composition_picks: &[CompositionPick],
    final_picked_ids: &HashSet<PhotoId>,
) -> Result<()> {
    let mut picked_count = 0usize;
    let mut rejected_count = 0usize;
    let mut photo_count = 0usize;

    let stage_a_json: Vec<JsonStageAGroup> = selections
        .iter()
        .map(|sel| {
            photo_count += sel.group.photo_ids.len();
            let mut photos_json: Vec<JsonPhoto> = Vec::new();
            for (rank, (pid, tech)) in sel.kept.iter().enumerate() {
                if let Some(p) = photos.get(pid) {
                    photos_json.push(JsonPhoto {
                        id: pid.0.to_string(),
                        path: &p.path,
                        captured_at: p.captured_at,
                        iso: p.iso,
                        verdict: Verdict::Kept,
                        rank_in_group: Some(rank + 1),
                        tech: Some(*tech),
                    });
                }
            }
            let base = sel.kept.len();
            for (idx, (pid, tech)) in sel.rejected.iter().enumerate() {
                if let Some(p) = photos.get(pid) {
                    photos_json.push(JsonPhoto {
                        id: pid.0.to_string(),
                        path: &p.path,
                        captured_at: p.captured_at,
                        iso: p.iso,
                        verdict: Verdict::Rejected,
                        rank_in_group: Some(base + idx + 1),
                        tech: Some(*tech),
                    });
                }
            }
            if sel.kept.is_empty() && sel.rejected.is_empty() {
                for pid in &sel.group.photo_ids {
                    if let Some(p) = photos.get(pid) {
                        photos_json.push(JsonPhoto {
                            id: pid.0.to_string(),
                            path: &p.path,
                            captured_at: p.captured_at,
                            iso: p.iso,
                            verdict: Verdict::Unscored,
                            rank_in_group: None,
                            tech: None,
                        });
                    }
                }
            }
            JsonStageAGroup {
                id: sel.group.id.0.to_string(),
                representative: sel.group.representative.0.to_string(),
                photos: photos_json,
            }
        })
        .collect();

    let composition_json: Vec<JsonCompositionGroup> = composition_picks
        .iter()
        .map(|cp| {
            let mut picks: Vec<JsonPick> = Vec::new();
            for (rank, (pid, fs)) in cp.kept.iter().enumerate() {
                if let Some(p) = photos.get(pid) {
                    picks.push(JsonPick {
                        id: pid.0.to_string(),
                        path: &p.path,
                        verdict: Verdict::Kept,
                        rank: rank + 1,
                        final_score: *fs,
                    });
                }
            }
            let base = cp.kept.len();
            for (idx, (pid, fs)) in cp.rejected.iter().enumerate() {
                if let Some(p) = photos.get(pid) {
                    picks.push(JsonPick {
                        id: pid.0.to_string(),
                        path: &p.path,
                        verdict: Verdict::Rejected,
                        rank: base + idx + 1,
                        final_score: *fs,
                    });
                }
            }
            JsonCompositionGroup {
                id: cp.group.id.0.to_string(),
                member_ids: cp.group.photo_ids.iter().map(|p| p.0.to_string()).collect(),
                picks,
            }
        })
        .collect();

    // Photos in `final_picked_ids` are kept; everything else from the Stage A
    // pool is rejected. Count from the plan, not from the per-group lists.
    for sel in selections {
        for pid in &sel.group.photo_ids {
            if final_picked_ids.contains(pid) {
                picked_count += 1;
            } else {
                rejected_count += 1;
            }
        }
    }
    // If there was no scoring at all (CLIP off + no tech), the loop above may
    // not match user intent. Override with simple count when composition is
    // empty.
    if composition_picks.is_empty() {
        picked_count = selections
            .iter()
            .map(|s| {
                if s.kept.is_empty() && s.rejected.is_empty() {
                    s.group.photo_ids.len()
                } else {
                    s.kept.len()
                }
            })
            .sum();
        rejected_count = selections.iter().map(|s| s.rejected.len()).sum();
    }

    let report = JsonReport {
        version: env!("CARGO_PKG_VERSION"),
        root: root.to_path_buf(),
        generated_at: Utc::now(),
        elapsed_secs: elapsed.as_secs_f64(),
        photo_count,
        stage_a_group_count: selections.len(),
        stage_b_group_count: stage_b_groups.len(),
        picked_count,
        rejected_count,
        stage_a_groups: stage_a_json,
        composition_groups: composition_json,
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::Io { path: parent.to_path_buf(), source: e })?;
    }
    let f = fs::File::create(path).map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    serde_json::to_writer_pretty(f, &report)
        .map_err(|e| Error::Io { path: path.to_path_buf(), source: std::io::Error::other(e) })?;
    Ok(())
}
