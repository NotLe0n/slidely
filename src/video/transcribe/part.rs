//! Transcribing one part of the audio, block by block while it's still being decoded.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::bail;
use whisper_rs::WhisperState;

use super::block::{BlockProgress, transcribe_block};
use super::decoder::AudioDecoder;
use super::language::{Language, detect_language};
use super::models::{load_model, return_vad, take_vad};
use super::progress::{Progress, Stage};
use super::speech::Speech;
use super::{SAMPLE_RATE, Segment, to_samples};

/// Audio transcribed in the first block. Short, so the first segments come quickly.
const FIRST_BLOCK_SAMPLES: usize = 60 * SAMPLE_RATE as usize;
/// Audio transcribed in each later block. Every block boundary loses Whisper some
/// context, so they're longer.
const BLOCK_SAMPLES: usize = 180 * SAMPLE_RATE as usize;

/// Which part of the audio to transcribe.
#[derive(Clone, Copy)]
pub(super) struct Part {
    /// Index of the part in `Progress::parts`.
    pub(super) ix: usize,
    pub(super) start: Duration,
    /// Where the part ends, or `None` for the last part.
    pub(super) end: Option<Duration>,
    /// CPU threads for its Whisper state.
    pub(super) threads: i32,
}

/// Transcribes `part` of the audio of `decoder`, in the shared `language`.
pub(super) fn transcribe_part(
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
                    let detected = detect_language(state, &speech, part.threads)?;
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
