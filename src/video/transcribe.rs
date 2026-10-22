//! Speech-to-text for a video's audio track, with whisper.cpp running on the GPU
//! (Vulkan). Nothing in here renders UI.
//!
//! The audio is transcribed in blocks while it's still being decoded, so the first
//! segments are there after seconds rather than once the whole lecture is through.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
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
/// The spoken language, e.g. "de" or "en". Detected from the first block if unset,
/// which costs an extra pass over its first 30 s.
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

/// Shared with the thread doing the transcription.
#[derive(Default)]
pub struct Progress {
    /// A `Stage`, as its index.
    stage: AtomicU8,
    /// How much of the audio is decoded, in percent.
    decoded: AtomicU8,
    /// How much of the audio is transcribed, in percent.
    transcribed: AtomicU8,
    /// Set to stop the transcription early.
    cancelled: AtomicBool,
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
            Stage::ExtractingAudio => Some(self.decoded.load(Ordering::Relaxed)),
            Stage::LoadingModel => None,
            Stage::Transcribing => Some(self.transcribed.load(Ordering::Relaxed)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn set_stage(&self, stage: Stage) {
        self.stage.store(stage as u8, Ordering::Relaxed);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// The Whisper model, kept loaded between transcriptions: loading it reads hundreds
/// of MB and sets up the GPU, which takes seconds.
static MODEL: Mutex<Option<(PathBuf, Arc<WhisperContext>)>> = Mutex::new(None);

/// The voice activity detection model, kept loaded like `MODEL`. `None` until it's
/// first needed; a `Vad` without a model if there isn't one.
static VAD: Mutex<Option<Vad>> = Mutex::new(None);

/// Starts loading the Whisper and VAD models in the background, so they load while
/// the first block of audio decodes.
fn preload_models() {
    std::thread::spawn(|| {
        if let Err(err) = with_vad(|_| Ok(())) {
            eprintln!("Failed to preload the VAD model: {err:#}");
        }
    });
    std::thread::spawn(|| {
        if let Err(err) = load_model() {
            eprintln!("Failed to preload the Whisper model: {err:#}");
        }
    });
}

/// Calls `f` with the VAD, loading it unless that's done already.
fn with_vad<R>(f: impl FnOnce(&mut Vad) -> anyhow::Result<R>) -> anyhow::Result<R> {
    let mut vad = VAD.lock().unwrap_or_else(PoisonError::into_inner);
    let vad = match &mut *vad {
        Some(vad) => vad,
        None => vad.insert(Vad::load()?),
    };
    f(vad)
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

fn thread_count() -> i32 {
    std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32
}

/// Transcribes the speech in the video at `video_path`, passing each block's segments
/// to `on_segments` as they're done. Blocks until done, so it should run on a
/// background thread. Returns early with an error once `progress` is cancelled.
pub fn transcribe(
    video_path: &Path,
    progress: Arc<Progress>,
    mut on_segments: impl FnMut(Vec<Segment>),
) -> anyhow::Result<Vec<Segment>> {
    // Fails early if there's no model, and otherwise loads the models while the audio
    // decodes, unless that's done already.
    find_whisper_model()?;
    preload_models();

    let mut decoder = AudioDecoder::start(video_path)?;
    let mut state: Option<WhisperState> = None;
    let mut language = std::env::var(LANGUAGE_ENV_VAR).ok();
    let mut segments = Vec::new();

    // Decoded audio not transcribed yet, and where it starts in the whole audio.
    let mut pending = Vec::new();
    let mut pending_start = 0;
    let mut block_samples = FIRST_BLOCK_SAMPLES;
    loop {
        decoder.read(&mut pending, block_samples, &progress)?;
        if pending.is_empty() {
            break;
        }

        // Stops at the start of the last stretch of speech, which may continue in the
        // audio not decoded yet; it's transcribed with the next block instead.
        let mut speech_ranges = with_vad(|vad| vad.speech(&pending))?;
        let cut = if !decoder.finished && speech_ranges.len() >= 2 {
            speech_ranges.pop().unwrap().start
        } else {
            pending.len()
        };
        let speech = Speech::join(&pending, pending_start, speech_ranges);

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
            let total = decoder.total_samples();
            let block = BlockProgress {
                progress: Arc::downgrade(&progress),
                start: pending_start,
                len: cut,
                total,
            };
            let previous_text = segments
                .last()
                .map(|segment: &Segment| segment.text.as_str());
            let new_segments =
                transcribe_block(state, &speech, language.as_deref(), previous_text, block)?;
            if language.is_none() {
                language =
                    whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_owned);
            }
            segments.extend(new_segments.iter().cloned());
            on_segments(new_segments);
        }

        pending.drain(..cut);
        pending_start += cut;
        block_samples = BLOCK_SAMPLES;
        progress.transcribed.store(
            percent(pending_start, decoder.total_samples()),
            Ordering::Relaxed,
        );
        if decoder.finished && pending.is_empty() {
            break;
        }
    }

    if decoder.received == 0 {
        bail!("the video has no audio");
    }
    Ok(segments)
}

fn percent(done: usize, total: usize) -> u8 {
    if total == 0 {
        return 0;
    }
    (done as u64 * 100 / total as u64).min(100) as u8
}

/// Where a block is in the whole audio, to turn Whisper's progress within the block
/// into progress overall.
struct BlockProgress {
    /// Only weakly referenced: whisper-rs never frees its callbacks.
    progress: Weak<Progress>,
    /// Index of the block's first sample in the whole audio.
    start: usize,
    len: usize,
    /// Number of samples in the whole audio.
    total: usize,
}

/// Transcribes one block's `speech`, continuing on from `previous_text`.
fn transcribe_block(
    state: &mut WhisperState,
    speech: &Speech,
    language: Option<&str>,
    previous_text: Option<&str>,
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
    // The GPU does the heavy lifting; the threads only help the CPU-side parts.
    params.set_n_threads(thread_count());

    // whisper-rs never frees the callbacks, so they only hold weak references.
    let progress = block.progress.clone();
    params.set_progress_callback_safe(move |block_percent: i32| {
        if let Some(progress) = block.progress.upgrade() {
            let done = block.start + block.len * block_percent.clamp(0, 100) as usize / 100;
            progress
                .transcribed
                .store(percent(done, block.total), Ordering::Relaxed);
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
            eprintln!(
                "No VAD model at {}; transcribing all of the audio",
                model_path.display()
            );
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
        Duration::from_secs_f64(original as f64 / SAMPLE_RATE as f64)
    }
}

/// Decodes the audio track of a file into 16 kHz mono samples, as fast as GStreamer
/// can decode it, but at most `DECODE_AHEAD` ahead of what's been read.
struct AudioDecoder {
    pipeline: gstreamer::Element,
    sink: AppSink,
    /// Number of samples read so far.
    received: usize,
    /// Length of the audio in samples, once GStreamer knows it.
    duration: Option<usize>,
    /// Set once all of the audio is read.
    finished: bool,
}

impl AudioDecoder {
    fn start(path: &Path) -> anyhow::Result<Self> {
        gstreamer::init()?;
        let uri = Url::from_file_path(path).map_err(|()| anyhow::anyhow!("invalid path"))?;

        let sink_bin = gstreamer::parse::bin_from_description(
            &format!(
                "audioconvert ! audioresample ! \
                 audio/x-raw,format=F32LE,channels=1,rate={SAMPLE_RATE} ! \
                 appsink name=sink sync=false max-time={}",
                DECODE_AHEAD.as_nanos()
            ),
            true,
        )?;
        let sink = sink_bin
            .by_name("sink")
            .and_then(|sink| sink.downcast::<AppSink>().ok())
            .context("audio sink missing")?;

        // playbin with only the audio flag set doesn't decode the video at all.
        let pipeline = gstreamer::ElementFactory::make("playbin")
            .property("uri", uri.as_str())
            .property("audio-sink", &sink_bin)
            .property_from_str("flags", "audio")
            .build()?;
        pipeline.set_state(gstreamer::State::Playing)?;

        Ok(Self {
            pipeline,
            sink,
            received: 0,
            duration: None,
            finished: false,
        })
    }

    /// Length of the audio in samples, or the samples read so far until that's known.
    fn total_samples(&self) -> usize {
        self.duration.unwrap_or(self.received).max(self.received)
    }

    /// Appends samples to `out` until it holds `wanted` or all of the audio is read.
    fn read(
        &mut self,
        out: &mut Vec<f32>,
        wanted: usize,
        progress: &Progress,
    ) -> anyhow::Result<()> {
        while out.len() < wanted && !self.finished {
            if progress.is_cancelled() {
                bail!("cancelled");
            }
            if let Some(message) = self
                .pipeline
                .bus()
                .and_then(|bus| bus.pop_filtered(&[gstreamer::MessageType::Error]))
                && let gstreamer::MessageView::Error(err) = message.view()
            {
                bail!("failed to decode audio: {}", err.error());
            }

            match self
                .sink
                .try_pull_sample(gstreamer::ClockTime::from_mseconds(100))
            {
                Some(sample) => {
                    let buffer = sample.buffer().context("audio sample has no buffer")?;
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

            if self.duration.is_none() {
                self.duration =
                    self.pipeline
                        .query_duration::<gstreamer::ClockTime>()
                        .map(|duration| {
                            (duration.nseconds() as u128 * SAMPLE_RATE as u128 / 1_000_000_000)
                                as usize
                        });
            }
            let decoded = if self.finished {
                100
            } else {
                percent(self.received, self.total_samples())
            };
            progress.decoded.store(decoded, Ordering::Relaxed);
        }
        Ok(())
    }
}

impl Drop for AudioDecoder {
    fn drop(&mut self) {
        self.pipeline.set_state(gstreamer::State::Null).ok();
    }
}
