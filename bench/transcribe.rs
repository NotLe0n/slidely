//! Measures how long transcribing a video takes, from opening it to the last segment.
//!
//! Run it on a video of your own, from the repository root so the models in `models/`
//! are found:
//!
//! ```sh
//! cargo bench --bench transcribe -- path/to/lecture.mp4
//! ```
//!
//! The video can also be given with `SLIDELY_BENCH_VIDEO`, and the number of runs with
//! `SLIDELY_BENCH_RUNS` (1 by default). The models stay loaded between runs, as they do
//! in the app, so the first run also measures loading them and the later ones don't.

#[allow(dead_code, unused_imports)]
#[path = "../src/video/transcribe/mod.rs"]
mod transcribe;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context as _;

use transcribe::{Progress, Stage};

/// Picks the video to transcribe if it isn't given as an argument.
const VIDEO_ENV_VAR: &str = "SLIDELY_BENCH_VIDEO";
/// How many times the video is transcribed.
const RUNS_ENV_VAR: &str = "SLIDELY_BENCH_RUNS";
/// How often the stage is checked. Short next to the seconds each stage takes, so the
/// times measured are off by at most this much.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// When each stage of one run started, and how long it took, from its start.
#[derive(Default)]
struct Timings {
    loading_model: Option<Duration>,
    transcribing: Option<Duration>,
    first_segments: Option<Duration>,
    total: Duration,
    /// Where the last segment ends: how much of the video was transcribed, near enough.
    audio: Duration,
    segments: usize,
}

fn main() -> anyhow::Result<()> {
    // `cargo bench` passes `--bench` along, so flags are skipped.
    let video = std::env::args()
        .skip(1)
        .find(|arg| !arg.starts_with("--"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os(VIDEO_ENV_VAR).map(PathBuf::from))
        .with_context(|| format!("no video given; pass it after `--` or set {VIDEO_ENV_VAR}"))?;
    let video = video
        .canonicalize()
        .with_context(|| format!("no video at {}", video.display()))?;
    let runs: usize = match std::env::var(RUNS_ENV_VAR) {
        Ok(runs) => runs
            .parse()
            .with_context(|| format!("invalid {RUNS_ENV_VAR}"))?,
        Err(_) => 1,
    };

    println!("Transcribing {}", video.display());
    let mut totals = Vec::new();
    for run in 1..=runs {
        let timings = run_once(&video)?;
        print_timings(run, &timings);
        totals.push(timings.total);
    }
    if runs > 1 {
        let mean = totals.iter().sum::<Duration>() / runs as u32;
        let min = totals.iter().min().copied().unwrap_or_default();
        println!("\n{runs} runs: mean {mean:.2?}, fastest {min:.2?}");
    }
    Ok(())
}

/// Transcribes `video` once, timing each stage on a thread that watches its progress.
fn run_once(video: &std::path::Path) -> anyhow::Result<Timings> {
    let progress = Arc::new(Progress::default());
    let done = AtomicBool::new(false);
    let start = Instant::now();
    let mut first_segments = None;

    let (result, (loading_model, transcribing)) = std::thread::scope(|scope| {
        let watcher = scope.spawn(|| {
            let (mut loading_model, mut transcribing) = (None, None);
            while !done.load(Ordering::Relaxed) {
                match progress.stage() {
                    Stage::ExtractingAudio => {}
                    Stage::LoadingModel => {
                        loading_model.get_or_insert_with(|| start.elapsed());
                    }
                    Stage::Transcribing => {
                        // Stages can be skipped when the model is loaded already.
                        let now = start.elapsed();
                        loading_model.get_or_insert(now);
                        transcribing.get_or_insert(now);
                    }
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            (loading_model, transcribing)
        });
        let result = transcribe::transcribe(video, progress.clone(), |_| {
            first_segments.get_or_insert_with(|| start.elapsed());
        });
        done.store(true, Ordering::Relaxed);
        let stages = watcher.join().expect("the stage watcher panicked");
        (result, stages)
    });
    let total = start.elapsed();
    let segments = result.context("transcription failed")?;

    Ok(Timings {
        loading_model,
        transcribing,
        first_segments,
        total,
        audio: segments
            .iter()
            .map(|segment| segment.end)
            .max()
            .unwrap_or_default(),
        segments: segments.len(),
    })
}

fn print_timings(run: usize, timings: &Timings) {
    let at = |time: Option<Duration>| time.map_or_else(|| "-".to_owned(), |t| format!("{t:.2?}"));
    println!("\nRun {run}");
    println!("  model loading from  {}", at(timings.loading_model));
    println!("  transcribing from   {}", at(timings.transcribing));
    println!("  first segments at   {}", at(timings.first_segments));
    println!("  done after          {:.2?}", timings.total);
    println!(
        "  {} segments, {:.0?} of audio, {:.1}x real time",
        timings.segments,
        timings.audio,
        timings.audio.as_secs_f64() / timings.total.as_secs_f64().max(f64::EPSILON),
    );
}
