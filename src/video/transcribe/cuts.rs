//! Where to cut the audio into the parts that are transcribed at the same time.

use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use anyhow::bail;

use super::decoder::AudioDecoder;
use super::models::{return_vad, take_vad};
use super::progress::Progress;
use super::speech::Vad;
use super::to_time;

/// Parts are at least this long, so short videos aren't cut up for little gain: each
/// part needs a Whisper state of its own.
const MIN_PART: Duration = Duration::from_secs(5 * 60);
/// The audio is cut into parts in the longest pause at most this far from where it
/// would be cut evenly, so no words are cut in half.
const CUT_SEARCH: Duration = Duration::from_secs(15);

/// Where to cut the audio of `path`, `duration` long, into parts: in the longest pause
/// near each even split. The cuts are looked for at the same time, each with a
/// decoder of its own.
pub(super) fn part_cuts(
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
    let mut decoder = AudioDecoder::open(path)?;
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
