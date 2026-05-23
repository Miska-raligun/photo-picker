use crate::error::{Error, Result};
use crate::ingest::{PhotoId, PhotoRef};
use crate::scoring::{SelectedGroup, TechScore};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
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
    pub group_count: usize,
    pub picked_count: usize,
    pub rejected_count: usize,
    pub groups: Vec<JsonGroup<'a>>,
}

#[derive(Debug, Serialize)]
pub struct JsonGroup<'a> {
    pub id: String,
    pub representative: String,
    pub photos: Vec<JsonPhoto<'a>>,
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

pub fn write_json_report(
    path: &Path,
    root: &Path,
    elapsed: Duration,
    photos: &HashMap<PhotoId, PhotoRef>,
    selections: &[SelectedGroup],
) -> Result<()> {
    let mut picked_count = 0usize;
    let mut rejected_count = 0usize;
    let mut photo_count = 0usize;

    let json_groups: Vec<JsonGroup> = selections
        .iter()
        .map(|sel| {
            photo_count += sel.group.photo_ids.len();
            picked_count += sel.kept.len();
            rejected_count += sel.rejected.len();

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
            // Photos with no tech score (extraction failed) — list as unscored.
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

            JsonGroup {
                id: sel.group.id.0.to_string(),
                representative: sel.group.representative.0.to_string(),
                photos: photos_json,
            }
        })
        .collect();

    let report = JsonReport {
        version: env!("CARGO_PKG_VERSION"),
        root: root.to_path_buf(),
        generated_at: Utc::now(),
        elapsed_secs: elapsed.as_secs_f64(),
        photo_count,
        group_count: selections.len(),
        picked_count,
        rejected_count,
        groups: json_groups,
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::Io { path: parent.to_path_buf(), source: e })?;
    }
    let f = fs::File::create(path).map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    serde_json::to_writer_pretty(f, &report)
        .map_err(|e| Error::Io { path: path.to_path_buf(), source: std::io::Error::other(e) })?;
    Ok(())
}
