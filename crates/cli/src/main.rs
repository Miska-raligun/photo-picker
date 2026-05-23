use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use indicatif::{ProgressBar, ProgressStyle};
use photo_pick_core::group::{StageAParams, StageBParams};
use photo_pick_core::ingest::ThumbnailSpec;
use photo_pick_core::models::ExecutionProvider;
use photo_pick_core::pipeline::{LinkMode, NoopProgress, Pipeline, PipelineConfig, ProgressSink, Stage};
use photo_pick_core::scoring::TechWeights;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "photo-pick", version, about = "Intelligent photo culling for burst shots")]
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
struct ScanArgs {
    /// Directory to scan (recursive).
    root: PathBuf,

    /// Output directory for selected files.
    #[arg(short, long, default_value = "./picked")]
    output: PathBuf,

    /// Stage A per-group top-K (M1: not enforced, reserved for M2).
    #[arg(long, default_value_t = 3)]
    k1: usize,

    /// Stage B per-group top-K (M3+).
    #[arg(long, default_value_t = 1)]
    k2: usize,

    /// How to materialize selected files into the output directory.
    #[arg(long, value_enum, default_value_t = LinkModeArg::Hardlink)]
    link: LinkModeArg,

    /// Stage A time-window scaling factor: Δt = time_k · median_dt.
    #[arg(long, default_value_t = 3.0)]
    time_k: f32,

    /// Stage A minimum time-window (seconds).
    #[arg(long, default_value_t = 0.3)]
    min_dt: f32,

    /// Stage A maximum time-window (seconds). Caps adaptive widening for sparse shoots.
    #[arg(long, default_value_t = 30.0)]
    max_dt: f32,

    /// Stage B CLIP cosine-similarity threshold for "same composition" merge.
    #[arg(long, default_value_t = 0.93)]
    stage_b_threshold: f32,

    /// Disable CLIP loading (skip Stage B; pipeline behaves like M2).
    #[arg(long)]
    no_clip: bool,

    /// Disable face detection (skip YuNet; scenes default to landscape).
    #[arg(long)]
    no_face: bool,

    /// Stage A maximum pHash Hamming distance.
    #[arg(long, default_value_t = 6)]
    hash_dist: u32,

    /// Parallel worker count (default: physical CPUs).
    #[arg(short, long)]
    jobs: Option<usize>,

    /// Skip writing files; only produce the report.
    #[arg(long)]
    dry_run: bool,

    /// Write a JSON report to this path (default: <output>/report.json).
    #[arg(long)]
    report: Option<PathBuf>,

    /// Write a self-contained HTML report to this path (default: <output>/report.html).
    #[arg(long)]
    html_report: Option<PathBuf>,

    /// Skip the HTML report (otherwise generated alongside the JSON one).
    #[arg(long)]
    no_html: bool,

    /// Feature cache path (default: <output>/.cache.db). Re-runs against the
    /// same directory reuse cached features and skip extraction.
    #[arg(long)]
    cache_db: Option<PathBuf>,

    /// Disable the feature cache entirely (always re-extract).
    #[arg(long)]
    no_cache: bool,

    /// Increase log verbosity (-v, -vv).
    #[arg(short, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Suppress all output except errors.
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
        root: args.root.clone(),
        output: args.output.clone(),
        report_path,
        html_report_path,
        cache_path,
        stage_a: StageAParams {
            k_time: args.time_k,
            min_dt: Duration::from_secs_f32(args.min_dt),
            max_dt: Duration::from_secs_f32(args.max_dt),
            max_hash_dist: args.hash_dist,
        },
        stage_b: StageBParams {
            similarity_threshold: args.stage_b_threshold,
        },
        k1: args.k1,
        k2: args.k2,
        tech_weights: TechWeights::default(),
        link_mode: args.link.into(),
        thumbnail: ThumbnailSpec::default(),
        dry_run: args.dry_run,
        enable_clip: !args.no_clip,
        enable_face: !args.no_face,
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
