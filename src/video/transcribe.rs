//! Speech-to-text for a video's audio track, with whisper.cpp running on the GPU
//! (Vulkan, or Metal on macOS). Nothing in here renders UI.
//!
//! The audio is cut into parts, which are transcribed at the same time, and each part
//! in blocks while it's still being decoded, so the first segments are there after
//! seconds rather than once the whole lecture is through.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Once, PoisonError, Weak};
use std::time::Duration;

use anyhow::{Context as _, bail};
use gpui_video_player::Url;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    WhisperVadContext, WhisperVadContextParams, WhisperVadParams,
};

/// Overrides where the Whisper model is loaded from.
const MODEL_ENV_VAR: &str = "SLIDELY_WHISPER_MODEL";
/// Where the Whisper model is looked for, relative to the working directory, unless
/// `MODEL_ENV_VAR` is set. The first one that exists is used: small is several times
/// faster than large-v3-turbo and plenty to match the speech to the slides.
const DEFAULT_MODEL_PATHS: &[&str] = &[
    "models/ggml-small-q5_1.bin",
    "models/ggml-small.bin",
    "models/ggml-large-v3-turbo-q5_0.bin",
];
/// Overrides where the voice activity detection model is loaded from.
const VAD_MODEL_ENV_VAR: &str = "SLIDELY_VAD_MODEL";
/// Where the Silero voice activity detection model is looked for, like
/// `DEFAULT_MODEL_PATHS`. Without it the silent parts are transcribed too, which is slower.
const DEFAULT_VAD_MODEL_PATH: &str = "models/ggml-silero-v5.1.2.bin";
/// The spoken language, e.g. "de" or "en". Detected from the first speech of the
/// video if unset, which costs an extra pass over its first 30 s.
const LANGUAGE_ENV_VAR: &str = "SLIDELY_WHISPER_LANGUAGE";
/// Whisper only takes 16 kHz mono audio.
const SAMPLE_RATE: u32 = 16_000;
/// Silence put between the stretches of speech, so Whisper doesn't run them together.
/// The same as whisper.cpp uses.
const SPEECH_GAP_SAMPLES: usize = SAMPLE_RATE as usize / 10;
/// Audio transcribed in the first block. Short, so the first segments come quickly.
const FIRST_BLOCK_SAMPLES: usize = 60 * SAMPLE_RATE as usize;
/// Audio transcribed in each later block. Every block boundary loses Whisper some
/// context, so they're longer.
const BLOCK_SAMPLES: usize = 180 * SAMPLE_RATE as usize;
/// How far decoding may run ahead of the transcription, bounding the memory used.
const DECODE_AHEAD: Duration = Duration::from_secs(360);
/// Parts are at least this long, so short videos aren't cut up for little gain: each
/// part needs a Whisper state of its own.
const MIN_PART: Duration = Duration::from_secs(5 * 60);
/// The most parts the audio is cut into, however many threads the CPU has. Each part's
/// Whisper state takes GPU memory of its own, and eight is as many as were measured
/// (see `max_parts`); more might not fit on the GPU.
const MAX_PARTS: usize = 8;
/// The most CPU threads a Whisper state uses. The GPU does the heavy lifting; the
/// threads only help the CPU-side parts.
const MAX_WHISPER_THREADS: usize = 8;
/// The audio is cut into parts in the longest pause at most this far from where it
/// would be cut evenly, so no words are cut in half.
const CUT_SEARCH: Duration = Duration::from_secs(15);

/// A stretch of speech and when it's spoken.
#[derive(Clone, Debug)]
pub struct Segment {
    pub start: Duration,
    pub end: Duration,
    pub text: String,
}

/// What the transcription is busy with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Decoding the first block of audio.
    ExtractingAudio,
    LoadingModel,
    Transcribing,
}

/// Shared with the threads doing the transcription.
pub struct Progress {
    /// A `Stage`, as its index.
    stage: AtomicU8,
    /// How far each part is; as many as the audio may be cut into.
    parts: Box<[PartProgress]>,
    /// Length of the audio in samples, once it's known.
    total: AtomicUsize,
    /// Set to stop the transcription early.
    cancelled: AtomicBool,
}

/// How far one part of the audio is, in samples from its start.
#[derive(Default)]
struct PartProgress {
    /// Samples decoded, shown while the first block decodes.
    decoded: AtomicUsize,
    /// Samples transcribed, including Whisper's progress within the current block.
    transcribed: AtomicUsize,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            stage: AtomicU8::default(),
            parts: (0..max_parts()).map(|_| PartProgress::default()).collect(),
            total: AtomicUsize::default(),
            cancelled: AtomicBool::default(),
        }
    }
}

impl Progress {
    pub fn stage(&self) -> Stage {
        match self.stage.load(Ordering::Relaxed) {
            0 => Stage::ExtractingAudio,
            1 => Stage::LoadingModel,
            _ => Stage::Transcribing,
        }
    }

    /// Progress of the current stage in percent, if it has any.
    pub fn percent(&self) -> Option<u8> {
        match self.stage() {
            Stage::ExtractingAudio => Some(self.percent_of(|part| &part.decoded)),
            Stage::LoadingModel => None,
            Stage::Transcribing => Some(self.percent_of(|part| &part.transcribed)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// How much of the audio all parts together are through, by `counter`. Taken as
    /// all of it until the length is known.
    fn percent_of(&self, counter: impl Fn(&PartProgress) -> &AtomicUsize) -> u8 {
        let done = self
            .parts
            .iter()
            .map(|part| counter(part).load(Ordering::Relaxed))
            .sum();
        percent(done, self.total.load(Ordering::Relaxed).max(done))
    }

    /// Moves on to `stage`, unless another part is further along already.
    fn set_stage(&self, stage: Stage) {
        self.stage.fetch_max(stage as u8, Ordering::Relaxed);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// The Whisper model, kept loaded between transcriptions: loading it reads hundreds
/// of MB and sets up the GPU, which takes seconds. The parts share it, each with a
/// state of its own.
static MODEL: Mutex<Option<(PathBuf, Arc<WhisperContext>)>> = Mutex::new(None);

/// Voice activity detection models not in use, kept loaded like `MODEL`. Each part
/// needs one of its own, since they run at the same time.
static VADS: Mutex<Vec<Vad>> = Mutex::new(Vec::new());

/// Starts loading the Whisper and VAD models in the background, so they load while
/// the first block of audio decodes.
fn preload_models() {
    std::thread::spawn(|| match take_vad() {
        Ok(vad) => return_vad(vad),
        Err(err) => eprintln!("Failed to preload the VAD model: {err:#}"),
    });
    std::thread::spawn(|| {
        if let Err(err) = load_model() {
            eprintln!("Failed to preload the Whisper model: {err:#}");
        }
    });
}

/// A loaded VAD, loading another one if they're all in use.
fn take_vad() -> anyhow::Result<Vad> {
    let vad = VADS.lock().unwrap_or_else(PoisonError::into_inner).pop();
    match vad {
        Some(vad) => Ok(vad),
        None => Vad::load(),
    }
}

/// Keeps `vad` loaded for the next part that needs one.
fn return_vad(vad: Vad) {
    VADS.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(vad);
}

/// The loaded Whisper model, loading it unless that's done already. Waits for a load
/// running on another thread instead of loading it twice.
fn load_model() -> anyhow::Result<Arc<WhisperContext>> {
    let model_path = find_whisper_model()?;
    let mut model = MODEL.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((path, context)) = &*model
        && *path == model_path
    {
        return Ok(context.clone());
    }
    // Frees the GPU memory of a different model before loading this one.
    *model = None;

    let context_params = WhisperContextParameters {
        use_gpu: true,
        flash_attn: true,
        ..Default::default()
    };
    let model_path_str = model_path
        .to_str()
        .context("model path isn't valid UTF-8")?;
    let context = Arc::new(
        WhisperContext::new_with_params(model_path_str, context_params)
            .context("failed to load the Whisper model")?,
    );
    *model = Some((model_path, context.clone()));
    Ok(context)
}

/// The model at `MODEL_ENV_VAR`, or else the first of `DEFAULT_MODEL_PATHS` there is.
fn find_whisper_model() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os(MODEL_ENV_VAR) {
        let path = PathBuf::from(path);
        if !path.is_file() {
            bail!(
                "Whisper model not found at {} ({MODEL_ENV_VAR}).",
                path.display()
            );
        }
        return Ok(path);
    }
    DEFAULT_MODEL_PATHS
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .with_context(|| {
            format!(
                "Whisper model not found at any of {}. Download a ggml model (e.g. from \
                 huggingface.co/ggerganov/whisper.cpp) there, or set {MODEL_ENV_VAR}.",
                DEFAULT_MODEL_PATHS.join(", ")
            )
        })
}

/// The most parts the audio is cut into: one per two threads the CPU has, since each
/// part also runs the VAD and decodes on the CPU. A single Whisper state leaves much
/// of the GPU idle, as most of its time goes to generating one token after another,
/// so the parts run side by side: on an RTX 4070 with 16 threads, a 90 min lecture
/// took 150 s in one part, 48 s in four and 38 s in eight, as accurately. At most
/// `MAX_PARTS`.
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
    find_whisper_model()?;
    preload_models();

    let first_decoder = AudioDecoder::open(video_path, &progress)?;
    let cuts = match first_decoder.duration() {
        Some(duration) => {
            progress
                .total
                .store(to_samples(duration), Ordering::Relaxed);
            part_cuts(video_path, duration, &progress)?
        }
        None => Vec::new(),
    };
    let starts = std::iter::once(Duration::ZERO).chain(cuts.iter().copied());
    let ends = cuts.iter().copied().map(Some).chain(std::iter::once(None));
    let parts: Vec<_> = starts.zip(ends).collect();

    let threads = thread_count(parts.len());
    let language = Language::new();
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
                        transcribe_part(decoder, part, progress, language, on_segments)
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

/// The spoken language, shared by the parts so the whole video is transcribed in the
/// one the first part detects, rather than each part guessing from its own start.
struct Language {
    /// `None` until it's decided, then the language, or `None` if it couldn't be
    /// detected and each part has to detect its own.
    decided: Mutex<Option<Option<String>>>,
    changed: Condvar,
}

impl Language {
    /// Decided already if `LANGUAGE_ENV_VAR` is set.
    fn new() -> Self {
        Self {
            decided: Mutex::new(std::env::var(LANGUAGE_ENV_VAR).ok().map(Some)),
            changed: Condvar::new(),
        }
    }

    /// Decides the language, unless it's decided already.
    fn set(&self, language: Option<String>) {
        let mut decided = self.decided.lock().unwrap_or_else(PoisonError::into_inner);
        if decided.is_none() {
            *decided = Some(language);
            self.changed.notify_all();
        }
    }

    /// The language once it's decided, if it's decided yet.
    fn get(&self) -> Option<Option<String>> {
        self.decided
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Waits until the language is decided, or `progress` is cancelled.
    fn wait(&self, progress: &Progress) -> anyhow::Result<Option<String>> {
        let mut decided = self.decided.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(language) = &*decided {
                return Ok(language.clone());
            }
            if progress.is_cancelled() {
                bail!("cancelled");
            }
            decided = self
                .changed
                .wait_timeout(decided, Duration::from_millis(100))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// Where to cut the audio of `path`, `duration` long, into parts: in the longest pause
/// near each even split. The cuts are looked for at the same time, each with a
/// decoder of its own.
fn part_cuts(
    path: &Path,
    duration: Duration,
    progress: &Progress,
) -> anyhow::Result<Vec<Duration>> {
    let count =
        ((duration.as_secs_f64() / MIN_PART.as_secs_f64()) as usize).clamp(1, progress.parts.len());
    std::thread::scope(|scope| {
        let handles: Vec<_> = (1..count)
            .map(|ix| {
                let time = duration * ix as u32 / count as u32;
                scope.spawn(move || {
                    let mut vad = take_vad()?;
                    let cut = pause_near(path, time, &mut vad, progress);
                    return_vad(vad);
                    cut
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| bail!("looking for a pause crashed"))
            })
            .collect()
    })
}

/// The middle of the longest pause within `CUT_SEARCH` of `time`, counting the
/// silence at the edges of that window, or `time` itself if there's no audio there.
fn pause_near(
    path: &Path,
    time: Duration,
    vad: &mut Vad,
    progress: &Progress,
) -> anyhow::Result<Duration> {
    let from = time.saturating_sub(CUT_SEARCH);
    let mut decoder = AudioDecoder::open(path, progress)?;
    decoder.play(from, Some(time + CUT_SEARCH))?;
    let mut audio = Vec::new();
    decoder.read(&mut audio, usize::MAX, progress, &AtomicUsize::new(0))?;
    if audio.is_empty() {
        return Ok(time);
    }
    let speech = vad.speech(&audio)?;
    // The pauses are between the ends of one stretch of speech and the start of the
    // next, with the window's edges as the ends of the speech outside it.
    let starts = speech.iter().map(|range| range.start).chain([audio.len()]);
    let ends = std::iter::once(0).chain(speech.iter().map(|range| range.end));
    let longest = ends
        .zip(starts)
        .max_by_key(|&(end, start)| start.saturating_sub(end))
        .unwrap();
    let audio_start = decoder.start.unwrap_or(from);
    Ok(audio_start + to_time((longest.0 + longest.1) / 2))
}

/// Which part of the audio to transcribe.
#[derive(Clone, Copy)]
struct Part {
    /// Index of the part in `Progress::parts`.
    ix: usize,
    start: Duration,
    /// Where the part ends, or `None` for the last part.
    end: Option<Duration>,
    /// CPU threads for its Whisper state.
    threads: i32,
}

/// Transcribes `part` of the audio of `decoder`, in the shared `language`.
fn transcribe_part(
    mut decoder: AudioDecoder,
    part: Part,
    progress: &Arc<Progress>,
    shared_language: &Language,
    on_segments: &Mutex<impl FnMut(Vec<Segment>)>,
) -> anyhow::Result<Vec<Segment>> {
    decoder.play(part.start, part.end)?;
    let mut vad = take_vad()?;
    let mut state: Option<WhisperState> = None;
    // `None` until this part knows which language to transcribe in.
    let mut language: Option<Option<String>> = None;
    let mut segments = Vec::new();
    let part_progress = &progress.parts[part.ix];

    // Decoded audio not transcribed yet, and where it starts in the part.
    let mut pending = Vec::new();
    let mut pending_start = 0;
    // Where the part's audio starts in the whole audio, known once its first samples
    // are decoded. Taken from their timestamp rather than `part.start`, as the audio
    // may not start at 0 or the seek land exactly.
    let mut part_start = None;
    let mut block_samples = FIRST_BLOCK_SAMPLES;
    loop {
        decoder.read(
            &mut pending,
            block_samples,
            progress,
            &part_progress.decoded,
        )?;
        if pending.is_empty() {
            break;
        }
        let part_start =
            *part_start.get_or_insert_with(|| to_samples(decoder.start.unwrap_or(part.start)));

        // Stops at the start of the last stretch of speech, which may continue in the
        // audio not decoded yet; it's transcribed with the next block instead.
        let mut speech_ranges = vad.speech(&pending)?;
        let cut = if !decoder.finished && speech_ranges.len() >= 2 {
            speech_ranges.pop().unwrap().start
        } else {
            pending.len()
        };
        let speech = Speech::join(&pending, part_start + pending_start, speech_ranges);

        if !speech.samples.is_empty() {
            if state.is_none() {
                progress.set_stage(Stage::LoadingModel);
                state = Some(load_model()?.create_state()?);
                if progress.is_cancelled() {
                    bail!("cancelled");
                }
                progress.set_stage(Stage::Transcribing);
            }
            let state = state.as_mut().unwrap();
            if language.is_none() {
                language = Some(if part.ix == 0 {
                    let detected = match shared_language.get() {
                        Some(language) => language,
                        None => detect_language(state, &speech, part.threads)?,
                    };
                    shared_language.set(detected.clone());
                    detected
                } else {
                    shared_language.wait(progress)?
                });
            }
            let language = language.as_mut().unwrap();
            let block = BlockProgress {
                progress: Arc::downgrade(progress),
                part: part.ix,
                done: pending_start,
                len: cut,
            };
            let previous_text = segments
                .last()
                .map(|segment: &Segment| segment.text.as_str());
            let new_segments = transcribe_block(
                state,
                &speech,
                language.as_deref(),
                previous_text,
                part.threads,
                block,
            )?;
            if language.is_none() {
                // Only when the first part couldn't tell: this part keeps to the
                // language Whisper detected in its own first block.
                *language =
                    whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_owned);
            }
            segments.extend(new_segments.iter().cloned());
            (on_segments.lock().unwrap_or_else(PoisonError::into_inner))(new_segments);
        }

        pending.drain(..cut);
        pending_start += cut;
        block_samples = BLOCK_SAMPLES;
        part_progress
            .transcribed
            .store(pending_start, Ordering::Relaxed);
        if decoder.finished && pending.is_empty() {
            break;
        }
    }

    return_vad(vad);
    Ok(segments)
}

/// The language spoken at the start of `speech`, from Whisper's guess over its first
/// 30 s, the same pass it makes itself when the language is "auto".
fn detect_language(
    state: &mut WhisperState,
    speech: &Speech,
    threads: i32,
) -> anyhow::Result<Option<String>> {
    let threads = threads as usize;
    state
        .pcm_to_mel(&speech.samples, threads)
        .context("failed to prepare the audio for language detection")?;
    let (id, _) = state
        .lang_detect(0, threads)
        .context("failed to detect the language")?;
    Ok(whisper_rs::get_lang_str(id).map(str::to_owned))
}

fn percent(done: usize, total: usize) -> u8 {
    if total == 0 {
        return 0;
    }
    (done as u64 * 100 / total as u64).min(100) as u8
}

/// Where a block is in its part, to turn Whisper's progress within the block into
/// the part's progress.
struct BlockProgress {
    /// Only weakly referenced: whisper-rs never frees its callbacks.
    progress: Weak<Progress>,
    /// Index of the part in `Progress::parts`.
    part: usize,
    /// Samples of the part transcribed before the block.
    done: usize,
    len: usize,
}

/// Transcribes one block's `speech` with `threads` CPU threads, continuing on from
/// `previous_text`.
fn transcribe_block(
    state: &mut WhisperState,
    speech: &Speech,
    language: Option<&str>,
    previous_text: Option<&str>,
    threads: i32,
    block: BlockProgress,
) -> anyhow::Result<Vec<Segment>> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some(language.unwrap_or("auto")));
    if let Some(text) = previous_text {
        // Carries the context over the block boundary, like Whisper does between its
        // own 30 s windows.
        params.set_initial_prompt(text);
    }
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_n_threads(threads);

    // whisper-rs never frees the callbacks, so they only hold weak references.
    let progress = block.progress.clone();
    params.set_progress_callback_safe(move |block_percent: i32| {
        if let Some(progress) = block.progress.upgrade() {
            let done = block.done + block.len * block_percent.clamp(0, 100) as usize / 100;
            progress.parts[block.part]
                .transcribed
                .store(done, Ordering::Relaxed);
        }
    });
    // whisper-rs boxes the closure as `Box<dyn FnMut() -> bool>` but calls it back as the
    // closure's own type, so the closure has to be that box already for the two to match.
    let abort: Box<dyn FnMut() -> bool> = Box::new({
        let progress = progress.clone();
        move || {
            progress
                .upgrade()
                .is_none_or(|progress| progress.is_cancelled())
        }
    });
    params.set_abort_callback_safe(abort);

    state
        .full(params, &speech.samples)
        .context("transcription failed")?;
    if progress
        .upgrade()
        .is_none_or(|progress| progress.is_cancelled())
    {
        bail!("cancelled");
    }

    Ok(state
        .as_iter()
        .map(|segment| Segment {
            start: speech.original_time(segment.start_timestamp()),
            end: speech.original_time(segment.end_timestamp()),
            text: segment
                .to_str_lossy()
                .map(|text| text.trim().to_string())
                .unwrap_or_default(),
        })
        .filter(|segment| !segment.text.is_empty())
        .collect())
}

/// Finds the speech in audio with the Silero VAD model, if it's there.
struct Vad(Option<WhisperVadContext>);

impl Vad {
    fn load() -> anyhow::Result<Self> {
        let model_path = std::env::var_os(VAD_MODEL_ENV_VAR)
            .map_or_else(|| PathBuf::from(DEFAULT_VAD_MODEL_PATH), PathBuf::from);
        let Some(model_path_str) = model_path.to_str().filter(|_| model_path.is_file()) else {
            // Every part loads one, but once is enough to say so.
            static WARNING: Once = Once::new();
            WARNING.call_once(|| {
                eprintln!(
                    "No VAD model at {}; transcribing all of the audio",
                    model_path.display()
                )
            });
            return Ok(Self(None));
        };

        // The model is tiny, so the CPU is fast enough and the GPU stays free. It's run
        // once per 32 ms of audio, and on such small work more threads spend longer
        // syncing than they save: one thread is about 3x as fast as eight.
        let mut context_params = WhisperVadContextParams::new();
        context_params.set_n_threads(1);
        let context = WhisperVadContext::new(model_path_str, context_params)
            .context("failed to load the VAD model")?;
        Ok(Self(Some(context)))
    }

    /// The stretches of speech in `audio`, as sample ranges in order. Without the
    /// model, all of the audio is taken to be speech.
    fn speech(&mut self, audio: &[f32]) -> anyhow::Result<Vec<Range<usize>>> {
        let Some(context) = &mut self.0 else {
            return Ok(std::iter::once(0..audio.len()).collect());
        };
        let segments = context
            .segments_from_samples(WhisperVadParams::default(), audio)
            .context("voice activity detection failed")?;
        // The segments' timestamps are in centiseconds.
        let to_sample =
            |cs: f32| ((cs.max(0.) * SAMPLE_RATE as f32 / 100.) as usize).min(audio.len());
        Ok(segments
            .map(|segment| to_sample(segment.start)..to_sample(segment.end))
            .filter(|range| !range.is_empty())
            .collect())
    }
}

/// The stretches of speech in some audio, joined together. Whisper processes the
/// audio in 30 s windows however much speech they contain, so transcribing just the
/// speech skips the work for the silence in between.
///
/// whisper.cpp does this itself when VAD is enabled, but only in `whisper_full`, not in
/// the `whisper_full_with_state` that whisper-rs calls.
struct Speech {
    samples: Vec<f32>,
    /// Where each stretch of speech is in `samples` and in the whole audio, in order.
    chunks: Vec<SpeechChunk>,
}

struct SpeechChunk {
    /// Index of the chunk's first sample in `Speech::samples`.
    start: usize,
    /// Index of the chunk's first sample in the whole audio.
    original_start: usize,
    len: usize,
}

impl Speech {
    /// Joins the `ranges` of `audio`, which starts at sample `offset` of the whole audio.
    fn join(audio: &[f32], offset: usize, ranges: Vec<Range<usize>>) -> Self {
        let mut speech = Speech {
            samples: Vec::new(),
            chunks: Vec::new(),
        };
        for range in ranges {
            if !speech.samples.is_empty() {
                speech
                    .samples
                    .resize(speech.samples.len() + SPEECH_GAP_SAMPLES, 0.);
            }
            speech.chunks.push(SpeechChunk {
                start: speech.samples.len(),
                original_start: offset + range.start,
                len: range.len(),
            });
            speech.samples.extend_from_slice(&audio[range]);
        }
        speech
    }

    /// Maps a Whisper timestamp in `samples` (in centiseconds) to the time in the
    /// whole audio. Times in a gap between chunks map to the end of the chunk before.
    fn original_time(&self, centis: i64) -> Duration {
        let sample = centis.max(0) as usize * SAMPLE_RATE as usize / 100;
        let ix = self.chunks.partition_point(|chunk| chunk.start <= sample);
        let original = match ix.checked_sub(1).map(|ix| &self.chunks[ix]) {
            Some(chunk) => chunk.original_start + (sample - chunk.start).min(chunk.len),
            None => 0,
        };
        to_time(original)
    }
}

/// Decodes the audio track of a file into 16 kHz mono samples, as fast as GStreamer
/// can decode it, but at most `DECODE_AHEAD` ahead of what's been read.
struct AudioDecoder {
    pipeline: gstreamer::Pipeline,
    sink: AppSink,
    /// Number of samples read so far.
    received: usize,
    /// Where in the audio the first sample read is, from its timestamp, if it has one.
    start: Option<Duration>,
    /// Set once all of the audio is read.
    finished: bool,
}

impl AudioDecoder {
    /// Opens the audio track of the file at `path`, ready to `play`.
    fn open(path: &Path, progress: &Progress) -> anyhow::Result<Self> {
        gstreamer::init()?;
        let uri = Url::from_file_path(path).map_err(|()| anyhow::anyhow!("invalid path"))?;

        // Bounded by bytes, not `max-time`: after seeking into the middle of a file,
        // appsink sometimes counts its empty queue as full and blocks the decoding for
        // good, which stalled three of four transcriptions in eight parts.
        let sink_bin = gstreamer::parse::bin_from_description(
            &format!(
                "audioconvert ! audioresample ! \
                 audio/x-raw,format=F32LE,channels=1,rate={SAMPLE_RATE} ! \
                 appsink name=sink sync=false max-bytes={}",
                to_samples(DECODE_AHEAD) * size_of::<f32>()
            ),
            true,
        )?;
        let sink = sink_bin
            .by_name("sink")
            .and_then(|sink| sink.downcast::<AppSink>().ok())
            .context("audio sink missing")?;
        let sink_pad = sink_bin
            .static_pad("sink")
            .context("audio sink has no pad")?;

        let pipeline = gstreamer::Pipeline::new();
        let source = gstreamer::ElementFactory::make("uridecodebin")
            .property("uri", uri.as_str())
            .build()?;
        // Leaves the video undecoded. Decoding a lecture's video only to drop it takes
        // about ten times as long as decoding its audio. The handler must return a
        // value, or glib panics on the streaming thread, so the result type is checked
        // here and anything unexpected falls back to plugging the element.
        let select_result = gstreamer::glib::Type::from_name("GstAutoplugSelectResult")
            .filter(|&ty| {
                gstreamer::glib::EnumClass::with_type(ty)
                    .is_some_and(|class| class.to_value(0).is_some() && class.to_value(1).is_some())
            })
            .context("GstAutoplugSelectResult missing")?;
        source.connect("autoplug-select", false, move |values| {
            let video_decoder = values
                .get(3)
                .and_then(|value| value.get::<gstreamer::ElementFactory>().ok())
                .is_some_and(|factory| {
                    let klass = factory.metadata("klass").unwrap_or_default();
                    klass.contains("Decoder") && klass.contains("Video")
                });
            // EXPOSE leaves the stream as it is; TRY plugs the element.
            gstreamer::glib::EnumClass::with_type(select_result)?.to_value(if video_decoder {
                1
            } else {
                0
            })
        });
        source.connect_pad_added({
            let pipeline = pipeline.downgrade();
            let sink_pad = sink_pad.clone();
            move |_, pad| {
                let raw_audio = pad
                    .current_caps()
                    .and_then(|caps| Some(caps.structure(0)?.name().starts_with("audio/x-raw")))
                    .unwrap_or(false);
                let linked = raw_audio && !sink_pad.is_linked() && pad.link(&sink_pad).is_ok();
                if !linked && let Some(pipeline) = pipeline.upgrade() {
                    discard(&pipeline, pad);
                }
            }
        });
        let no_more_pads = Arc::new(AtomicBool::new(false));
        source.connect_no_more_pads({
            let no_more_pads = no_more_pads.clone();
            move |_| no_more_pads.store(true, Ordering::Relaxed)
        });
        pipeline.add_many([&source, sink_bin.upcast_ref()])?;

        let decoder = Self {
            pipeline,
            sink,
            received: 0,
            start: None,
            finished: false,
        };
        // Waits until the first audio is there, which is when the length is known and
        // the pipeline can seek.
        decoder
            .pipeline
            .set_state(gstreamer::State::Paused)
            .context("failed to open the audio")?;
        loop {
            if progress.is_cancelled() {
                bail!("cancelled");
            }
            decoder.check_error()?;
            if no_more_pads.load(Ordering::Relaxed) && !sink_pad.is_linked() {
                bail!("the video has no audio");
            }
            match decoder
                .pipeline
                .state(gstreamer::ClockTime::from_mseconds(100))
                .0
            {
                Ok(gstreamer::StateChangeSuccess::Async) => {}
                Ok(_) => return Ok(decoder),
                Err(_) => {
                    decoder.check_error()?;
                    bail!("failed to open the audio");
                }
            }
        }
    }

    /// Length of the audio, if GStreamer knows it.
    fn duration(&self) -> Option<Duration> {
        self.pipeline
            .query_duration::<gstreamer::ClockTime>()
            .map(|duration| Duration::from_nanos(duration.nseconds()))
    }

    /// Starts decoding at `start`, until `end` or else the end.
    fn play(&mut self, start: Duration, end: Option<Duration>) -> anyhow::Result<()> {
        let clock_time =
            |time: Duration| gstreamer::ClockTime::from_nseconds(time.as_nanos() as u64);
        if !start.is_zero() || end.is_some() {
            self.pipeline
                .seek(
                    1.,
                    gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::ACCURATE,
                    gstreamer::SeekType::Set,
                    Some(clock_time(start)),
                    match end {
                        Some(_) => gstreamer::SeekType::Set,
                        None => gstreamer::SeekType::None,
                    },
                    end.map(clock_time),
                )
                .context("failed to seek in the audio")?;
        }
        self.pipeline.set_state(gstreamer::State::Playing)?;
        Ok(())
    }

    /// Appends samples to `out` until it holds `wanted` or all of the audio is read,
    /// counting them in `decoded`.
    fn read(
        &mut self,
        out: &mut Vec<f32>,
        wanted: usize,
        progress: &Progress,
        decoded: &AtomicUsize,
    ) -> anyhow::Result<()> {
        while out.len() < wanted && !self.finished {
            if progress.is_cancelled() {
                bail!("cancelled");
            }
            self.check_error()?;

            match self
                .sink
                .try_pull_sample(gstreamer::ClockTime::from_mseconds(100))
            {
                Some(sample) => {
                    let buffer = sample.buffer().context("audio sample has no buffer")?;
                    if self.received == 0 {
                        self.start = stream_time(&sample, buffer);
                    }
                    let map = buffer.map_readable()?;
                    let len = out.len();
                    out.extend(
                        map.chunks_exact(4)
                            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    );
                    self.received += out.len() - len;
                }
                None => self.finished = self.sink.is_eos(),
            }
            decoded.store(self.received, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Fails if GStreamer reported an error.
    fn check_error(&self) -> anyhow::Result<()> {
        if let Some(message) = self
            .pipeline
            .bus()
            .and_then(|bus| bus.pop_filtered(&[gstreamer::MessageType::Error]))
            && let gstreamer::MessageView::Error(err) = message.view()
        {
            bail!("failed to decode audio: {}", err.error());
        }
        Ok(())
    }
}

impl Drop for AudioDecoder {
    fn drop(&mut self) {
        self.pipeline.set_state(gstreamer::State::Null).ok();
    }
}

/// Where in the media `buffer` of `sample` is, as stream time: its timestamp with
/// the segment's start and any edit list's offset taken into account.
fn stream_time(sample: &gstreamer::Sample, buffer: &gstreamer::BufferRef) -> Option<Duration> {
    let pts = buffer.pts()?;
    let segment = sample.segment()?.downcast_ref::<gstreamer::ClockTime>()?;
    let time = segment.to_stream_time(pts)?;
    Some(Duration::from_nanos(time.nseconds()))
}

/// Sends what comes out of `pad`, a stream other than the audio, to a sink that drops
/// it. Left unlinked, it would stop the pipeline.
fn discard(pipeline: &gstreamer::Pipeline, pad: &gstreamer::Pad) {
    let Ok(sink) = gstreamer::ElementFactory::make("fakesink")
        .property("sync", false)
        .property("async", false)
        .build()
    else {
        return;
    };
    if pipeline.add(&sink).is_ok()
        && sink.sync_state_with_parent().is_ok()
        && let Some(sink_pad) = sink.static_pad("sink")
    {
        pad.link(&sink_pad).ok();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// Transcribes the video at `$SLIDELY_TRANSCRIBE_VIDEO` and prints how long that
    /// took, to measure changes on a real lecture; the transcript is written as JSON to
    /// `$SLIDELY_TRANSCRIPT_OUT` if that's set:
    /// `SLIDELY_TRANSCRIBE_VIDEO=path/to/video cargo test transcribe_video -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn transcribe_video() {
        let path = PathBuf::from(std::env::var("SLIDELY_TRANSCRIBE_VIDEO").unwrap());
        let started = Instant::now();
        let segments = transcribe(&path, Arc::default(), |_| {}).unwrap();
        println!(
            "transcribed into {} segments in {:?}",
            segments.len(),
            started.elapsed()
        );

        if let Some(out) = std::env::var_os("SLIDELY_TRANSCRIPT_OUT") {
            let json: Vec<_> = segments
                .iter()
                .map(|segment| {
                    serde_json::json!({
                        "start": segment.start.as_secs_f64(),
                        "end": segment.end.as_secs_f64(),
                        "text": segment.text,
                    })
                })
                .collect();
            std::fs::write(out, serde_json::to_string(&json).unwrap()).unwrap();
        }
    }
}
