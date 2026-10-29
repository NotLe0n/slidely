//! How far a transcription is, shared between the threads doing it and the UI.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use super::max_parts;

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
    pub(super) parts: Box<[PartProgress]>,
    /// Length of the audio in samples, once it's known.
    pub(super) total: AtomicUsize,
    /// Set to stop the transcription early.
    cancelled: AtomicBool,
}

/// How far one part of the audio is, in samples from its start.
#[derive(Default)]
pub(super) struct PartProgress {
    /// Samples decoded, shown while the first block decodes.
    pub(super) decoded: AtomicUsize,
    /// Samples transcribed, including Whisper's progress within the current block.
    pub(super) transcribed: AtomicUsize,
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
    pub(super) fn set_stage(&self, stage: Stage) {
        self.stage.fetch_max(stage as u8, Ordering::Relaxed);
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

fn percent(done: usize, total: usize) -> u8 {
    if total == 0 {
        return 0;
    }
    (done as u64 * 100 / total as u64).min(100) as u8
}
