use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use indicatif::{ProgressBar, ProgressStyle};
use photo_pick_core::group::{StageAParams, StageBParams};
use photo_pick_core::ingest::{PhotoSource, ThumbnailSpec};
use photo_pick_core::models::ExecutionProvider;
use photo_pick_core::pipeline::{LinkMode, NoopProgress, Pipeline, PipelineConfig, ProgressSink, Stage};
use photo_pick_core::scoring::TechWeights;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "photo-pick",
    version,
    about = "Intelligent photo culling for burst shots",
    long_about = "Scan a folder of photos, cluster near-duplicate bursts, score each \
        photo for technical quality + composition + face quality, and emit the top \
        picks (hardlinked or copied into an output folder). Outputs an HTML report \
        you can open in any browser plus a machine-readable JSON.\n\n\
        Pipeline: Stage A bursts (time + CLIP) → top-K1 by technical score → \
        Stage B composition groups (CLIP) → top-K2 by scene-aware final score."
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Scan a directory, cluster near-duplicate bursts, and emit selections.
    Scan(ScanArgs),
}

#[derive(Parser, Debug)]
#[command(
    long_about = "Run the full pipeline against ROOT and write picks to --output. \
        Sensible defaults (K1=3, K2=1, hardlink) match a typical event/portrait \
        shoot.\n\n\
        Tuning cheatsheet:\n  \
        • Want stricter culling     → lower --k1 (e.g. 1) and/or --k2 (1)\n  \
        • Bursts splitting too eager → raise --time-k (e.g. 5.0) or --max-dt\n  \
        • Different shots merging   → raise --stage-b-threshold (e.g. 0.96)\n  \
        • No ML available           → --no-clip (falls back to pHash via --hash-dist)\n  \
        • No portraits              → --no-face speeds extraction ~15%\n\n\
        Example: photo-pick scan ./shoot --output ./picked --k1 2 --k2 1 --link hardlink"
)]
struct ScanArgs {
    /// Directory to scan recursively. Supports JPEG, HEIC, and common RAW formats
    /// (NEF, ARW, CR2/CR3, RAF, ORF, DNG). EXIF capture time is required for the
    /// time-window component of Stage A clustering.
    root: PathBuf,

    /// Where picked / rejected folders, the JSON + HTML report, and the feature
    /// cache are written. Created if missing. Files inside a previous run's
    /// output (other than `.cache.db`) are NOT cleaned automatically.
    #[arg(short, long, default_value = "./picked")]
    output: PathBuf,

    /// Per-burst keep count. Each Stage A burst contributes its top-K1 photos by
    /// *technical* score (sharpness/exposure/WB/noise) to Stage B. Lower = harsher
    /// culling. Default 3 keeps a safety margin in case Stage B reshuffles.
    #[arg(long, default_value_t = 3)]
    k1: usize,

    /// Per-composition keep count. Each Stage B composition group emits its top-K2
    /// photos by *final* score (scene-aware blend of tech/aesthetic/composition/face).
    /// Omit (or pass 0) to enable auto mode: every group keeps ≥1 photo, plus any
    /// additional photos whose scores are within ~5 % of the best (capped at 5/group).
    /// Useful when shoot quality varies — clear winners stay singletons, near-ties
    /// keep both.
    #[arg(long)]
    k2: Option<usize>,

    /// How to materialize picks into <output>/picked:\n  \
    /// hardlink — zero disk cost when source + output are on the same filesystem\n  \
    /// copy     — safest; works across filesystems but uses real disk\n  \
    /// symlink  — smallest, but breaks if you later move the source folder
    #[arg(long, value_enum, default_value_t = LinkModeArg::Hardlink)]
    link: LinkModeArg,

    /// Stage A time-window scaling factor: Δt = time_k · median_dt, where median_dt
    /// is the median time gap between adjacent photos in the shoot. Larger →
    /// looser bursts. 3× is conservative; high-speed shooters can lower to 1.5–2.
    #[arg(long, default_value_t = 3.0)]
    time_k: f32,

    /// Floor for the adaptive burst time window in seconds. Prevents Δt from
    /// collapsing to zero on shoots dominated by tiny gaps (20+ fps bursts).
    #[arg(long, default_value_t = 0.3)]
    min_dt: f32,

    /// Cap for the adaptive burst time window in seconds. Prevents two photos
    /// minutes apart from joining the same burst even when CLIP says they look
    /// identical (e.g. the same locked-off scene revisited later).
    #[arg(long, default_value_t = 30.0)]
    max_dt: f32,

    /// Stage B CLIP cosine threshold for "same composition" merging. Higher →
    /// stricter (only near-identical framings group together). Default 0.93
    /// is balanced; 0.96+ tends to keep every alternate framing separate.
    #[arg(long, default_value_t = 0.93)]
    stage_b_threshold: f32,

    /// Skip loading CLIP entirely. Disables Stage B composition grouping and
    /// falls back to pHash for the Stage A duplicate check (see --hash-dist).
    /// Useful on machines without an onnxruntime build.
    #[arg(long)]
    no_clip: bool,

    /// Skip face detection (YuNet). All photos treated as landscape; face_bonus
    /// disabled; scene auto-detect collapses to landscape weights. Shaves ~15ms
    /// per photo on CPU.
    #[arg(long)]
    no_face: bool,

    /// pHash fallback: max Hamming distance (0–32) for two photos to merge into
    /// the same burst when CLIP is disabled. 0 = identical bytes; 6 = lenient
    /// default. Ignored when CLIP is enabled.
    #[arg(long, default_value_t = 6)]
    hash_dist: u32,

    /// Stage A CLIP threshold — tighter than Stage B because real bursts are
    /// nearly indistinguishable. 0.95 ≈ "looks identical". Lower → looser bursts.
    #[arg(long, default_value_t = 0.95)]
    stage_a_clip_threshold: f32,

    /// Rayon worker count. Default: physical CPU cores. Lower if extraction is
    /// starving other work on the machine.
    #[arg(short, long)]
    jobs: Option<usize>,

    /// Don't write the picked/ or rejected/ folders; still produces reports.
    /// Use to preview which photos would be picked before committing.
    #[arg(long)]
    dry_run: bool,

    /// JSON report path (default: <output>/report.json). Always written unless
    /// --dry-run AND this flag is not given.
    #[arg(long)]
    report: Option<PathBuf>,

    /// HTML report path (default: <output>/report.html). Self-contained:
    /// thumbnails are base64-embedded so the file is portable.
    #[arg(long)]
    html_report: Option<PathBuf>,

    /// Don't generate the HTML report (JSON-only output).
    #[arg(long)]
    no_html: bool,

    /// SQLite cache for extracted features (default: <output>/.cache.db).
    /// Subsequent runs against the same source skip extraction for unchanged
    /// files (keyed by sha256 + size). Safe to delete; will be re-created.
    #[arg(long)]
    cache_db: Option<PathBuf>,

    /// Skip the feature cache entirely. Every photo is re-extracted from scratch
    /// — useful when debugging the extractor or after upgrading models.
    #[arg(long)]
    no_cache: bool,

    /// Increase log verbosity: -v = debug, -vv = trace. Default is info.
    /// Honors RUST_LOG / env-filter syntax if set.
    #[arg(short, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Errors only. Suppresses progress bars and informational logs.
    #[arg(long)]
    quiet: bool,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum LinkModeArg {
    Copy,
    Hardlink,
    Symlink,
}

impl From<LinkModeArg> for LinkMode {
    fn from(v: LinkModeArg) -> Self {
        match v {
            LinkModeArg::Copy => LinkMode::Copy,
            LinkModeArg::Hardlink => LinkMode::Hardlink,
            LinkModeArg::Symlink => LinkMode::Symlink,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Scan(args) => run_scan(args),
    }
}

fn run_scan(args: ScanArgs) -> Result<()> {
    init_tracing(args.verbose, args.quiet);

    if let Some(jobs) = args.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build_global()
            .context("failed to configure rayon thread pool")?;
    }

    let report_path = args.report.clone().or_else(|| {
        if args.dry_run {
            None
        } else {
            Some(args.output.join("report.json"))
        }
    });
    let html_report_path = if args.no_html {
        None
    } else {
        args.html_report.clone().or_else(|| {
            if args.dry_run {
                None
            } else {
                Some(args.output.join("report.html"))
            }
        })
    };

    let cache_path = if args.no_cache {
        None
    } else {
        Some(args.cache_db.clone().unwrap_or_else(|| args.output.join(".cache.db")))
    };

    let cfg = PipelineConfig {
        source: PhotoSource::Directory(args.root.clone()),
        output: args.output.clone(),
        report_path,
        html_report_path,
        cache_path,
        stage_a: StageAParams {
            k_time: args.time_k,
            min_dt: Duration::from_secs_f32(args.min_dt),
            max_dt: Duration::from_secs_f32(args.max_dt),
            max_hash_dist: args.hash_dist,
            clip_threshold: args.stage_a_clip_threshold,
        },
        stage_b: StageBParams {
            similarity_threshold: args.stage_b_threshold,
            chain_margin: StageBParams::default().chain_margin,
        },
        k1: args.k1,
        k2: match args.k2 {
            // `--k2 0` is shorthand for "auto" (matches the Option=None default).
            Some(0) | None => None,
            Some(k) => Some(k),
        },
        tech_weights: TechWeights::default(),
        link_mode: args.link.into(),
        thumbnail: ThumbnailSpec::default(),
        dry_run: args.dry_run,
        enable_clip: !args.no_clip,
        enable_face: !args.no_face,
        materialize_picks: true,
        adaptive_thresholds: true,
        thumb_cache_dir: Some(args.output.join(".thumbs")),
        execution_provider: ExecutionProvider::Cpu,
    };

    let pipeline = Pipeline::new(cfg);
    let output = if args.quiet {
        pipeline.run(&NoopProgress)?
    } else {
        let sink = IndicatifProgress::new();
        let result = pipeline.run(&sink)?;
        sink.finish_all();
        result
    };
    let report = output.report;

    let verb = if args.dry_run { "would place" } else { "placed" };
    let stage_b_note = if report.stage_b_group_count > 0 {
        format!(" → {} composition groups", report.stage_b_group_count)
    } else {
        String::new()
    };
    let cache_note = if report.cached_count > 0 {
        format!(" (cache hit {}/{})", report.cached_count, report.photo_count)
    } else {
        String::new()
    };
    println!(
        "Done in {:.2}s — {} photos{} in {} bursts{}, {} kept / {} rejected, {} in {}",
        report.elapsed.as_secs_f64(),
        report.photo_count,
        cache_note,
        report.stage_a_group_count,
        stage_b_note,
        report.picked_count,
        report.rejected_count,
        verb,
        args.output.display(),
    );
    Ok(())
}

fn init_tracing(verbose: u8, quiet: bool) {
    use tracing_subscriber::{fmt, EnvFilter};
    let default_level = if quiet {
        "error"
    } else {
        match verbose {
            0 => "info",
            1 => "debug",
            _ => "trace",
        }
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    fmt().with_env_filter(filter).with_target(false).init();
}

struct IndicatifProgress {
    features: Mutex<Option<ProgressBar>>,
    write: Mutex<Option<ProgressBar>>,
}

impl IndicatifProgress {
    fn new() -> Self {
        Self {
            features: Mutex::new(None),
            write: Mutex::new(None),
        }
    }
    fn finish_all(&self) {
        for slot in [&self.features, &self.write] {
            if let Some(pb) = slot.lock().unwrap().take() {
                pb.finish_and_clear();
            }
        }
    }
}

impl ProgressSink for IndicatifProgress {
    fn on_stage(&self, stage: Stage, total: u64) {
        match stage {
            Stage::Scan => eprintln!("scanning..."),
            Stage::Features => {
                let pb = ProgressBar::new(total);
                pb.set_style(
                    ProgressStyle::with_template("features [{bar:30}] {pos}/{len} {eta}")
                        .unwrap()
                        .progress_chars("=>-"),
                );
                *self.features.lock().unwrap() = Some(pb);
            }
            Stage::Cluster => eprintln!("stage A (time + hash) clustering..."),
            Stage::Score => eprintln!("scoring + selecting top-K1..."),
            Stage::StageB => eprintln!("stage B (CLIP composition) clustering..."),
            Stage::FinalSelect => eprintln!("final scene-aware K2 selection..."),
            Stage::Write => {
                let pb = ProgressBar::new(total);
                pb.set_style(
                    ProgressStyle::with_template("output   [{bar:30}] {pos}/{len}")
                        .unwrap()
                        .progress_chars("=>-"),
                );
                *self.write.lock().unwrap() = Some(pb);
            }
        }
    }

    fn on_tick(&self, stage: Stage, done: u64) {
        let slot = match stage {
            Stage::Features => &self.features,
            Stage::Write => &self.write,
            _ => return,
        };
        if let Some(pb) = slot.lock().unwrap().as_ref() {
            pb.set_position(done);
        }
    }

    fn on_finish(&self, stage: Stage) {
        let slot = match stage {
            Stage::Features => &self.features,
            Stage::Write => &self.write,
            _ => return,
        };
        if let Some(pb) = slot.lock().unwrap().take() {
            pb.finish_and_clear();
        }
    }
}
