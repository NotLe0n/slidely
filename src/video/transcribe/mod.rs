//! Speech-to-text for a video's audio track, with whisper.cpp running on the GPU
//! (Vulkan, or Metal on macOS). Nothing in here renders UI.
//!
//! The audio is cut into parts, which are transcribed at the same time, and each part
//! in blocks while it's still being decoded, so the first segments are there after
//! seconds rather than once the whole lecture is through.

mod block;
mod cuts;
mod decoder;
mod language;
mod models;
mod part;
mod progress;
mod speech;

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context as _;

use decoder::AudioDecoder;
use language::Language;
use part::Part;
pub use progress::{Progress, Stage};

/// Whisper only takes 16 kHz mono audio.
const SAMPLE_RATE: u32 = 16_000;
/// The most parts the audio is cut into, however many threads the CPU has. Each part's
/// Whisper state takes GPU memory of its own, so more might not fit on the GPU.
const MAX_PARTS: usize = 8;
/// The most CPU threads a Whisper state uses. The GPU does the heavy lifting; the
/// threads only help the CPU-side parts.
const MAX_WHISPER_THREADS: usize = 8;

/// A stretch of speech and when it's spoken.
#[derive(Clone, Debug)]
pub struct Segment {
    pub start: Duration,
    pub end: Duration,
    pub text: String,
}

/// Transcribes the speech in the video at `video_path`, passing the segments of each
/// block to `on_segments` as they're done. The parts are transcribed at the same
/// time, so the blocks don't come in order. Blocks until done, so it should run on a
/// background thread. Returns early with an error once `progress` is cancelled.
pub fn transcribe(
    video_path: &Path,
    progress: Arc<Progress>,
    on_segments: impl FnMut(Vec<Segment>) + Send,
) -> anyhow::Result<Vec<Segment>> {
    // Fails early if there's no model, and otherwise loads the models while the audio
    // decodes, unless that's done already.
    models::find_whisper_model()?;
    models::preload_models();

    let first_decoder = AudioDecoder::open(video_path, &progress)?;
    let cuts = match first_decoder.duration() {
        Some(duration) => {
            progress
                .total
                .store(to_samples(duration), Ordering::Relaxed);
            cuts::part_cuts(video_path, duration, &progress)?
        }
        None => Vec::new(),
    };
    let starts = std::iter::once(Duration::ZERO).chain(cuts.iter().copied());
    let ends = cuts.iter().copied().map(Some).chain(std::iter::once(None));
    let parts: Vec<_> = starts.zip(ends).collect();

    let threads = thread_count(parts.len());
    let language = Language::default();
    let on_segments = Mutex::new(on_segments);
    // The error that stopped the transcription; the other parts are cancelled after it.
    let first_error = Mutex::new(None);
    let mut first_decoder = Some(first_decoder);
    let transcripts: Vec<Option<Vec<Segment>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = parts
            .iter()
            .enumerate()
            .map(|(ix, &(start, end))| {
                let decoder = first_decoder.take();
                let (progress, language, on_segments, first_error) =
                    (&progress, &language, &on_segments, &first_error);
                scope.spawn(move || {
                    let _guard = PartGuard {
                        progress,
                        first_error,
                        // Only the first part detects the language, so the others
                        // mustn't wait for it if the first stops without.
                        language: (ix == 0).then_some(language),
                    };
                    let decoder = match decoder {
                        Some(decoder) => Ok(decoder),
                        None => AudioDecoder::open(video_path, progress),
                    };
                    let part = Part {
                        ix,
                        start,
                        end,
                        threads,
                    };
                    let result = decoder.and_then(|decoder| {
                        part::transcribe_part(decoder, part, progress, language, on_segments)
                    });
                    result.map_err(|err| fail(first_error, progress, err)).ok()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().ok().flatten())
            .collect()
    });

    if let Some(err) = first_error
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
    {
        return Err(err);
    }
    let mut segments = Vec::new();
    for transcript in transcripts {
        segments.extend(transcript.context("transcription crashed")?);
    }
    Ok(segments)
}

/// Keeps `err` as the error that stopped the transcription, unless there's one
/// already, and cancels the other parts.
fn fail(first_error: &Mutex<Option<anyhow::Error>>, progress: &Progress, err: anyhow::Error) {
    first_error
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert(err);
    progress.cancel();
}

/// Cleans up after a part's thread when it ends. If it panics, the other parts are
/// cancelled right away rather than finishing work that's thrown away.
struct PartGuard<'a> {
    progress: &'a Progress,
    first_error: &'a Mutex<Option<anyhow::Error>>,
    /// The shared language, if this part is the one to detect it.
    language: Option<&'a Language>,
}

impl Drop for PartGuard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            fail(
                self.first_error,
                self.progress,
                anyhow::anyhow!("transcription crashed"),
            );
        }
        if let Some(language) = self.language {
            language.set(None);
        }
    }
}

/// The most parts the audio is cut into: one per two threads the CPU has, since each
/// part also runs the VAD and decodes on the CPU. A single Whisper state leaves much
/// of the GPU idle, as most of its time goes to generating one token after another,
/// so the parts run side by side. At most `MAX_PARTS`.
fn max_parts() -> usize {
    std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).clamp(1, MAX_PARTS))
}

/// The CPU threads each Whisper state uses when `parts` run at the same time: the
/// CPU's threads shared between them, so they don't fight over the cores.
fn thread_count(parts: usize) -> i32 {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    (threads / parts.max(1)).clamp(1, MAX_WHISPER_THREADS) as i32
}

/// The number of samples in `time` of audio.
fn to_samples(time: Duration) -> usize {
    (time.as_nanos() * SAMPLE_RATE as u128 / 1_000_000_000) as usize
}

/// How long `samples` of audio last.
fn to_time(samples: usize) -> Duration {
    Duration::from_secs_f64(samples as f64 / SAMPLE_RATE as f64)
}
