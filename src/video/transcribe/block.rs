//! Running Whisper over one block of speech.

use std::sync::Weak;
use std::sync::atomic::Ordering;

use anyhow::{Context as _, bail};
use whisper_rs::{FullParams, SamplingStrategy, WhisperState};

use super::Segment;
use super::progress::Progress;
use super::speech::Speech;

/// Where a block is in its part, to turn Whisper's progress within the block into
/// the part's progress.
pub(super) struct BlockProgress {
    /// Only weakly referenced: whisper-rs never frees its callbacks.
    pub(super) progress: Weak<Progress>,
    /// Index of the part in `Progress::parts`.
    pub(super) part: usize,
    /// Samples of the part transcribed before the block.
    pub(super) done: usize,
    pub(super) len: usize,
}

/// Transcribes one block's `speech` with `threads` CPU threads, continuing on from
/// `previous_text`.
pub(super) fn transcribe_block(
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
