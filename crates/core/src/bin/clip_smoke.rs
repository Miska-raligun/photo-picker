//! Smoke test: load CLIP, embed three fixtures, print pairwise cosine similarity.
//!
//! Run: `cargo run -p photo-pick-core --release --bin clip_smoke -- <jpeg1> <jpeg2> <jpeg3>`

use photo_pick_core::ingest::{decode_thumbnail, ThumbnailSpec};
use photo_pick_core::models::{ClipEncoder, ExecutionProvider, CLIP_EMBED_DIM};
use std::env;
use std::path::PathBuf;

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum() // already L2-normalized
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<PathBuf> = env::args().skip(1).map(PathBuf::from).collect();
    if paths.len() < 2 {
        eprintln!("usage: clip_smoke <img1> <img2> [<img3> ...]");
        std::process::exit(2);
    }

    let mut encoder = ClipEncoder::load(ExecutionProvider::Cpu)?;
    let mut embeds: Vec<(PathBuf, Vec<f32>)> = Vec::new();
    for p in &paths {
        let thumb = decode_thumbnail(p, ThumbnailSpec::default())?;
        let emb = encoder.embed(&thumb)?;
        assert_eq!(emb.len(), CLIP_EMBED_DIM);
        println!("embedded {} (dim={})", p.display(), emb.len());
        embeds.push((p.clone(), emb));
    }

    println!("\ncosine similarity:");
    for i in 0..embeds.len() {
        for j in (i + 1)..embeds.len() {
            let s = cosine(&embeds[i].1, &embeds[j].1);
            println!(
                "  {} <-> {}  =  {:.4}",
                embeds[i].0.file_name().unwrap().to_string_lossy(),
                embeds[j].0.file_name().unwrap().to_string_lossy(),
                s
            );
        }
    }
    Ok(())
}
