//! Self-contained HTML report with base64-encoded thumbnails (M3.8).
//!
//! The report is one file: no relative-path references, no JS bundles. Open it
//! anywhere and you see the same thing the pipeline produced.

use super::ThumbDiskCache;
use crate::error::{Error, Result};
use crate::ingest::{decode_thumbnail_for, PhotoId, PhotoRef, ThumbnailSpec};
use crate::scoring::{CompositionPick, FinalScore, Scene, SelectedGroup, TechScore};
use base64::Engine;
use chrono::Utc;
use image::{codecs::jpeg::JpegEncoder, ImageEncoder};
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

const THUMB_LONG_EDGE: u32 = 256;
const THUMB_JPEG_QUALITY: u8 = 70;

pub fn write_html_report(
    path: &Path,
    root: &Path,
    elapsed: Duration,
    photos: &HashMap<PhotoId, PhotoRef>,
    stage_a_picks: &[SelectedGroup],
    composition_picks: &[CompositionPick],
    thumb_cache: Option<&ThumbDiskCache>,
) -> Result<()> {
    // Build thumbnail data URLs in parallel — JPEG decoding is the bottleneck.
    let thumbs = build_thumbnail_map(photos, thumb_cache);

    let mut html = String::with_capacity(64 * 1024);
    html.push_str(&format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head>\n\
         <meta charset=\"utf-8\">\n\
         <title>photo-pick report — {}</title>\n\
         <style>{}</style>\n\
         </head><body>\n",
        escape_html(&root.display().to_string()),
        EMBEDDED_CSS,
    ));

    write_header(&mut html, root, photos.len(), stage_a_picks, composition_picks, elapsed);

    if !composition_picks.is_empty() {
        html.push_str("<h2>Final picks by composition group</h2>\n");
        for cp in composition_picks {
            write_composition_section(&mut html, cp, photos, &thumbs);
        }
    }

    html.push_str("<h2>Stage A bursts (technical-score filter)</h2>\n");
    for sa in stage_a_picks {
        write_stage_a_section(&mut html, sa, photos, &thumbs);
    }

    html.push_str("</body></html>\n");

    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| Error::Io { path: p.to_path_buf(), source: e })?;
    }
    fs::write(path, html).map_err(|e| Error::Io { path: path.to_path_buf(), source: e })?;
    Ok(())
}

fn build_thumbnail_map(
    photos: &HashMap<PhotoId, PhotoRef>,
    cache: Option<&ThumbDiskCache>,
) -> HashMap<PhotoId, String> {
    let entries: Vec<(PhotoId, String)> = photos
        .par_iter()
        .map(|(pid, p)| {
            // Prefer the disk cache: the pipeline populated it during feature
            // extraction with the already-decoded thumbnail, so we skip the
            // RAW byte-scan + decode on every photo.
            let bytes = cache
                .and_then(|c| c.read(&p.sha256_short))
                .or_else(|| thumbnail_jpeg_bytes(p));
            let url = match bytes {
                Some(b) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&b);
                    format!("data:image/jpeg;base64,{b64}")
                }
                None => String::from("data:,"),
            };
            (*pid, url)
        })
        .collect();
    entries.into_iter().collect()
}

fn thumbnail_jpeg_bytes(p: &PhotoRef) -> Option<Vec<u8>> {
    let spec = ThumbnailSpec { long_edge: THUMB_LONG_EDGE };
    let img = decode_thumbnail_for(p, spec).ok()?;
    let rgb = img.to_rgb8();
    let mut buf: Vec<u8> = Vec::with_capacity(20_000);
    let encoder = JpegEncoder::new_with_quality(Cursor::new(&mut buf), THUMB_JPEG_QUALITY);
    encoder
        .write_image(&rgb, rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(buf)
}

fn write_header(
    html: &mut String,
    root: &Path,
    photo_count: usize,
    stage_a: &[SelectedGroup],
    comp: &[CompositionPick],
    elapsed: Duration,
) {
    let picked_total: usize = if comp.is_empty() {
        stage_a.iter().map(|s| {
            if s.kept.is_empty() && s.rejected.is_empty() { s.group.photo_ids.len() } else { s.kept.len() }
        }).sum()
    } else {
        comp.iter().map(|c| c.kept.len()).sum()
    };
    html.push_str(&format!(
        "<header>\n\
         <h1>photo-pick</h1>\n\
         <p class=\"meta\"><strong>{}</strong> · generated {}</p>\n\
         <div class=\"summary\">\n\
         <span><strong>{}</strong> photos</span> · \
         <span><strong>{}</strong> bursts</span> · \
         <span><strong>{}</strong> composition groups</span> · \
         <span><strong>{}</strong> kept</span> · \
         <span>{:.2}s</span>\n\
         </div></header>\n",
        escape_html(&root.display().to_string()),
        Utc::now().format("%Y-%m-%d %H:%M UTC"),
        photo_count,
        stage_a.len(),
        comp.len(),
        picked_total,
        elapsed.as_secs_f64(),
    ));
}

fn write_composition_section(
    html: &mut String,
    cp: &CompositionPick,
    photos: &HashMap<PhotoId, PhotoRef>,
    thumbs: &HashMap<PhotoId, String>,
) {
    let short = &cp.group.id.0.simple().to_string()[..8];
    let scene_label = cp
        .kept
        .first()
        .or_else(|| cp.rejected.first())
        .map(|(_, fs)| scene_to_label(fs.scene))
        .unwrap_or("—");

    html.push_str(&format!(
        "<section class=\"group\">\n\
         <h3>composition <code>{short}</code> · scene: {scene_label} · {} kept, {} rejected</h3>\n\
         <div class=\"grid\">\n",
        cp.kept.len(),
        cp.rejected.len(),
    ));

    for (rank, (pid, fs)) in cp.kept.iter().chain(cp.rejected.iter()).enumerate() {
        let is_kept = rank < cp.kept.len();
        let display_rank = rank + 1;
        write_photo_card_final(html, photos, thumbs, pid, *fs, display_rank, is_kept);
    }
    html.push_str("</div></section>\n");
}

fn write_stage_a_section(
    html: &mut String,
    sa: &SelectedGroup,
    photos: &HashMap<PhotoId, PhotoRef>,
    thumbs: &HashMap<PhotoId, String>,
) {
    let short = &sa.group.id.0.simple().to_string()[..8];
    html.push_str(&format!(
        "<section class=\"group dim\">\n\
         <h3>burst <code>{short}</code> · {} kept, {} rejected (Stage A)</h3>\n\
         <div class=\"grid\">\n",
        sa.kept.len(),
        sa.rejected.len(),
    ));
    for (rank, (pid, ts)) in sa.kept.iter().chain(sa.rejected.iter()).enumerate() {
        let is_kept = rank < sa.kept.len();
        write_photo_card_tech(html, photos, thumbs, pid, *ts, rank + 1, is_kept);
    }
    // Unscored singleton.
    if sa.kept.is_empty() && sa.rejected.is_empty() {
        for (i, pid) in sa.group.photo_ids.iter().enumerate() {
            write_photo_card_unscored(html, photos, thumbs, pid, i + 1);
        }
    }
    html.push_str("</div></section>\n");
}

fn write_photo_card_final(
    html: &mut String,
    photos: &HashMap<PhotoId, PhotoRef>,
    thumbs: &HashMap<PhotoId, String>,
    pid: &PhotoId,
    fs: FinalScore,
    rank: usize,
    is_kept: bool,
) {
    let name = file_name_of(photos, pid).unwrap_or_else(|| pid.0.to_string());
    let thumb = thumbs.get(pid).map(|s| s.as_str()).unwrap_or("data:,");
    let klass = if is_kept { "card kept" } else { "card rejected" };
    let badge = if is_kept { "<span class=\"badge kept\">kept</span>" } else { "<span class=\"badge rejected\">rejected</span>" };
    html.push_str(&format!(
        "<div class=\"{klass}\">\n\
         <img src=\"{thumb}\" alt=\"{name}\">\n\
         <div class=\"row\"><span class=\"rank\">#{rank}</span>{badge}\
           <span class=\"score\">final {:.3}</span></div>\n\
         <div class=\"name\">{name}</div>\n\
         <div class=\"breakdown\">\
           tech {:.2} · aes {:.2} · comp {:.2} · face {:.2}\
         </div>\n\
         </div>\n",
        fs.value,
        fs.tech,
        fs.aesthetic,
        fs.composition,
        fs.face_bonus,
    ));
}

fn write_photo_card_tech(
    html: &mut String,
    photos: &HashMap<PhotoId, PhotoRef>,
    thumbs: &HashMap<PhotoId, String>,
    pid: &PhotoId,
    ts: TechScore,
    rank: usize,
    is_kept: bool,
) {
    let name = file_name_of(photos, pid).unwrap_or_else(|| pid.0.to_string());
    let thumb = thumbs.get(pid).map(|s| s.as_str()).unwrap_or("data:,");
    let klass = if is_kept { "card kept" } else { "card rejected" };
    let badge = if is_kept { "<span class=\"badge kept\">kept</span>" } else { "<span class=\"badge rejected\">rejected</span>" };
    html.push_str(&format!(
        "<div class=\"{klass}\">\n\
         <img src=\"{thumb}\" alt=\"{name}\">\n\
         <div class=\"row\"><span class=\"rank\">#{rank}</span>{badge}\
           <span class=\"score\">tech {:.3}</span></div>\n\
         <div class=\"name\">{name}</div>\n\
         <div class=\"breakdown\">\
           exp {:.2} · wb {:.2} · sharp {:.2} · noise {:.2}\
         </div>\n\
         </div>\n",
        ts.tech, ts.exposure, ts.wb, ts.sharpness, ts.noise,
    ));
}

fn write_photo_card_unscored(
    html: &mut String,
    photos: &HashMap<PhotoId, PhotoRef>,
    thumbs: &HashMap<PhotoId, String>,
    pid: &PhotoId,
    rank: usize,
) {
    let name = file_name_of(photos, pid).unwrap_or_else(|| pid.0.to_string());
    let thumb = thumbs.get(pid).map(|s| s.as_str()).unwrap_or("data:,");
    html.push_str(&format!(
        "<div class=\"card unscored\">\n\
         <img src=\"{thumb}\" alt=\"{name}\">\n\
         <div class=\"row\"><span class=\"rank\">#{rank}</span>\
           <span class=\"badge unscored\">unscored</span></div>\n\
         <div class=\"name\">{name}</div>\n\
         </div>\n",
    ));
}

fn file_name_of(photos: &HashMap<PhotoId, PhotoRef>, pid: &PhotoId) -> Option<String> {
    photos
        .get(pid)?
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| escape_html(s))
}

fn scene_to_label(s: Scene) -> &'static str {
    match s {
        Scene::Portrait => "portrait",
        Scene::Landscape => "landscape",
        Scene::Mixed => "mixed",
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const EMBEDDED_CSS: &str = r#"
:root {
  --bg: #fafafa; --fg: #1a1a1a; --muted: #666; --border: #e0e0e0;
  --kept-bg: #e8f5e9; --kept-border: #4caf50;
  --rejected-bg: #fafafa; --rejected-fg: #999;
  --unscored-bg: #fff3e0; --unscored-border: #ffb74d;
}
* { box-sizing: border-box; }
body {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
  margin: 0; padding: 2rem; background: var(--bg); color: var(--fg); line-height: 1.4;
}
header h1 { margin: 0 0 0.5rem; font-size: 1.4rem; }
header .meta { margin: 0 0 0.5rem; color: var(--muted); font-size: 0.85rem; }
header .summary {
  background: white; border: 1px solid var(--border); border-radius: 6px;
  padding: 0.75rem 1rem; margin-bottom: 2rem; font-size: 0.9rem;
}
header .summary span { margin-right: 0.5rem; }
h2 { margin-top: 2rem; font-size: 1.2rem; border-bottom: 1px solid var(--border); padding-bottom: 0.3rem; }
section.group {
  background: white; border: 1px solid var(--border); border-radius: 6px;
  padding: 1rem; margin-bottom: 1.5rem;
}
section.group.dim { opacity: 0.85; }
section.group h3 { margin: 0 0 0.75rem; font-size: 0.95rem; font-weight: 600; }
section.group h3 code { background: #eee; padding: 1px 5px; border-radius: 3px; font-size: 0.85em; }
.grid {
  display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
  gap: 0.75rem;
}
.card {
  background: var(--rejected-bg); border: 1px solid var(--border); border-radius: 4px;
  padding: 0.5rem; transition: transform 0.1s;
}
.card.kept { background: var(--kept-bg); border-color: var(--kept-border); border-width: 2px; }
.card.rejected { color: var(--rejected-fg); }
.card.rejected img { opacity: 0.7; }
.card.unscored { background: var(--unscored-bg); border-color: var(--unscored-border); }
.card img {
  width: 100%; aspect-ratio: 4/3; object-fit: cover; border-radius: 3px;
  background: #ddd;
}
.card .row {
  display: flex; align-items: center; gap: 0.4rem;
  margin-top: 0.4rem; font-size: 0.8rem;
}
.card .rank { font-weight: 600; color: var(--muted); }
.card .score { margin-left: auto; font-variant-numeric: tabular-nums; }
.card .name {
  font-size: 0.75rem; color: var(--muted);
  margin-top: 0.25rem; word-break: break-all;
}
.card .breakdown {
  font-size: 0.7rem; color: var(--muted); margin-top: 0.25rem;
  font-variant-numeric: tabular-nums;
}
.badge {
  display: inline-block; padding: 1px 6px; border-radius: 3px;
  font-size: 0.7rem; font-weight: 600; text-transform: uppercase;
}
.badge.kept { background: var(--kept-border); color: white; }
.badge.rejected { background: #bbb; color: white; }
.badge.unscored { background: var(--unscored-border); color: white; }
"#;
