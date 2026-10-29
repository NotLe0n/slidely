//! Finding the speech in audio with voice activity detection, and joining it so the
//! silence in between isn't transcribed.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Once;
use std::time::Duration;

use anyhow::Context as _;
use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

use super::{SAMPLE_RATE, to_time};

/// Overrides where the voice activity detection model is loaded from.
const VAD_MODEL_ENV_VAR: &str = "SLIDELY_VAD_MODEL";
/// Where the Silero voice activity detection model is looked for, relative to the
/// working directory, unless `VAD_MODEL_ENV_VAR` is set. Without it the silent parts
/// are transcribed too, which is slower.
const DEFAULT_VAD_MODEL_PATH: &str = "models/ggml-silero-v5.1.2.bin";
/// Silence put between the stretches of speech, so Whisper doesn't run them together.
/// The same as whisper.cpp uses.
const SPEECH_GAP_SAMPLES: usize = SAMPLE_RATE as usize / 10;

/// Finds the speech in audio with the Silero VAD model, if it's there.
pub(super) struct Vad(Option<WhisperVadContext>);

impl Vad {
    pub(super) fn load() -> anyhow::Result<Self> {
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
        // syncing than they save.
        let mut context_params = WhisperVadContextParams::new();
        context_params.set_n_threads(1);
        let context = WhisperVadContext::new(model_path_str, context_params)
            .context("failed to load the VAD model")?;
        Ok(Self(Some(context)))
    }

    /// The stretches of speech in `audio`, as sample ranges in order. Without the
    /// model, all of the audio is taken to be speech.
    pub(super) fn speech(&mut self, audio: &[f32]) -> anyhow::Result<Vec<Range<usize>>> {
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
pub(super) struct Speech {
    pub(super) samples: Vec<f32>,
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
    pub(super) fn join(audio: &[f32], offset: usize, ranges: Vec<Range<usize>>) -> Self {
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
    pub(super) fn original_time(&self, centis: i64) -> Duration {
        let sample = centis.max(0) as usize * SAMPLE_RATE as usize / 100;
        let ix = self.chunks.partition_point(|chunk| chunk.start <= sample);
        let original = match ix.checked_sub(1).map(|ix| &self.chunks[ix]) {
            Some(chunk) => chunk.original_start + (sample - chunk.start).min(chunk.len),
            None => 0,
        };
        to_time(original)
    }
}
