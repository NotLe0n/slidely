//! Deciding which language the whole video is transcribed in.

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context as _, bail};
use whisper_rs::WhisperState;

use super::progress::Progress;
use super::speech::Speech;

/// The spoken language, shared by the parts so the whole video is transcribed in the
/// one the first part detects, rather than each part guessing from its own start.
#[derive(Default)]
pub(super) struct Language {
    /// `None` until it's decided, then the language, or `None` if it couldn't be
    /// detected and each part has to detect its own.
    decided: Mutex<Option<Option<String>>>,
    changed: Condvar,
}

impl Language {
    /// Decides the language, unless it's decided already.
    pub(super) fn set(&self, language: Option<String>) {
        let mut decided = self.decided.lock().unwrap_or_else(PoisonError::into_inner);
        if decided.is_none() {
            *decided = Some(language);
            self.changed.notify_all();
        }
    }

    /// Waits until the language is decided, or `progress` is cancelled.
    pub(super) fn wait(&self, progress: &Progress) -> anyhow::Result<Option<String>> {
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

/// The language spoken at the start of `speech`, from Whisper's guess over its first
/// 30 s, the same pass it makes itself when the language is "auto".
pub(super) fn detect_language(
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
