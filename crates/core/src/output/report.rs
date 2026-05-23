use crate::error::{Error, Result};
use crate::group::Group;
use crate::ingest::{PhotoId, PhotoRef};
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
    pub groups: Vec<JsonGroup<'a>>,
}

#[derive(Debug, Serialize)]
pub struct JsonGroup<'a> {
    pub id: String,
    pub representative: String,
    pub photos: Vec<JsonPhoto<'a>>,
}

#[derive(Debug, Serialize)]
pub struct JsonPhoto<'a> {
    pub id: String,
    pub path: &'a Path,
    pub captured_at: Option<DateTime<Utc>>,
}

pub fn write_json_report(
    path: &Path,
    root: &Path,
    elapsed: Duration,
    photos: &HashMap<PhotoId, PhotoRef>,
    groups: &[Group],
) -> Result<()> {
    let json_groups: Vec<JsonGroup> = groups
        .iter()
        .map(|g| JsonGroup {
            id: g.id.0.to_string(),
            representative: g.representative.0.to_string(),
            photos: g
                .photo_ids
                .iter()
                .filter_map(|pid| {
                    photos.get(pid).map(|p| JsonPhoto {
                        id: pid.0.to_string(),
                        path: &p.path,
                        captured_at: p.captured_at,
                    })
                })
                .collect(),
        })
        .collect();

    let report = JsonReport {
        version: env!("CARGO_PKG_VERSION"),
        root: root.to_path_buf(),
        generated_at: Utc::now(),
        elapsed_secs: elapsed.as_secs_f64(),
        photo_count: photos.len(),
        group_count: groups.len(),
        groups: json_groups,
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::Io { path: parent.to_path_buf(), source: e })?;
    }
    let f = fs::File::create(path).map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    serde_json::to_writer_pretty(f, &report)
        .map_err(|e| Error::Io { path: path.to_path_buf(), source: std::io::Error::new(std::io::ErrorKind::Other, e) })?;
    Ok(())
}
