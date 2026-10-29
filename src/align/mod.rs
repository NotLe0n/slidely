//! Matching what's said in the lecture videos to the slides, so each slide can be
//! looked up in the videos. Nothing in here renders UI.
//!
//! The transcript is cut into overlapping windows, each one is compared with the text
//! of every slide, and then the most likely walk through the slides is found with the
//! Viterbi algorithm: lecturers mostly stay on a slide or go on to the next one, so a
//! single window that happens to match another slide better doesn't count for much.

mod index;
mod viterbi;

use std::time::Duration;

use crate::pdf::PageText;
use crate::video::Segment;
use index::SlideIndex;
use viterbi::viterbi;

/// The transcript is compared with the slides in slots of this length; it's how
/// precisely a slide's start is found.
const SLOT: Duration = Duration::from_secs(10);
/// Speech before and after each slot that's compared along with it, since one slot
/// alone has few words. More blurs where one slide ends and the next begins.
const SLOT_CONTEXT: Duration = Duration::from_secs(5);
/// How much the words in the context count, relative to those in the slot, so the
/// slot's own words decide where one slide ends and the next begins.
const CONTEXT_WEIGHT: f32 = 0.35;
/// Each slot a video spends on a slide an earlier video went through already. Lectures
/// go on where the last one stopped, so talk at the start of one, e.g. announcements,
/// that sounds like earlier slides isn't taken for going back to them.
const REVISIT_COST: f32 = 0.5;
/// Slides up to this many before the one the previous video ended on, which the next
/// one often goes over again, aren't charged `REVISIT_COST`.
const RESUME_SLIDES: usize = 3;

/// A stretch of one video in which a slide is discussed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlideSpan {
    /// Index of the video among those aligned.
    pub video: usize,
    pub start: Duration,
    pub end: Duration,
}

/// Where in the videos each slide is discussed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Alignment {
    /// By slide; in order of the videos and time. Empty for slides that aren't
    /// discussed.
    pub spans: Vec<Vec<SlideSpan>>,
}

impl Alignment {
    /// The slide discussed at `time` in `video`, if any.
    pub fn slide_at(&self, video: usize, time: Duration) -> Option<usize> {
        self.spans.iter().position(|spans| {
            spans
                .iter()
                .any(|span| span.video == video && span.start <= time && time < span.end)
        })
    }
}

/// Matches the transcripts of the videos, in the order they're watched in, to the
/// slides. `pdfs` has the text of every page of each PDF, in order; `None` for pages
/// whose text isn't extracted, which only the order of the slides places. The slides
/// are numbered across the PDFs.
pub fn align(pdfs: &[Vec<Option<PageText>>], videos: &[Vec<Segment>]) -> Alignment {
    let slide_count = pdfs.iter().map(Vec::len).sum();
    let mut alignment = Alignment {
        spans: vec![Vec::new(); slide_count],
    };
    if slide_count == 0 {
        return alignment;
    }
    let index = SlideIndex::new(pdfs);
    // Each video continues where the previous one left off.
    let mut last_slide: usize = 0;
    // Slides earlier videos went through.
    let mut covered = vec![false; slide_count];
    for (video, segments) in videos.iter().enumerate() {
        let Some((slots, video_end)) = slots(segments) else {
            continue;
        };
        let mut emissions = index.emissions(segments, &slots);
        let resumed = last_slide.saturating_sub(RESUME_SLIDES)..=last_slide;
        for slot in &mut emissions {
            for (slide, emission) in slot.iter_mut().enumerate() {
                if covered[slide] && !resumed.contains(&slide) {
                    *emission -= REVISIT_COST;
                }
            }
        }
        let path = viterbi(&emissions, slide_count, last_slide);
        last_slide = *path.last().unwrap_or(&last_slide);
        for &slide in &path {
            covered[slide] = true;
        }
        for (slide, start, end) in path_spans(&path, &slots, segments, video_end) {
            alignment.spans[slide].push(SlideSpan { video, start, end });
        }
    }
    alignment
}

/// Each run of slots on the same slide in `path` as that slide, and when the run
/// starts and ends in the video.
fn path_spans(
    path: &[usize],
    slots: &[Slot],
    segments: &[Segment],
    video_end: Duration,
) -> Vec<(usize, Duration, Duration)> {
    let mut spans = Vec::new();
    let mut run_start = 0;
    for ix in 1..=path.len() {
        if ix < path.len() && path[ix] == path[run_start] {
            continue;
        }
        let start = speech_start(segments, slots[run_start].start);
        let end = match slots.get(ix) {
            Some(next) => speech_start(segments, next.start),
            None => video_end,
        };
        if start < end {
            spans.push((path[run_start], start, end));
        }
        run_start = ix;
    }
    spans
}

/// A stretch of a video's speech the slides are compared with.
struct Slot {
    start: Duration,
    /// The segments spoken in the slot and the context around it, by index, with how
    /// much each counts.
    segments: Vec<(usize, f32)>,
}

/// The video's speech in consecutive slots of `SLOT`, from its start until the last
/// segment ends, and when that is; `None` if there's no speech.
fn slots(segments: &[Segment]) -> Option<(Vec<Slot>, Duration)> {
    let end = segments.iter().map(|segment| segment.end).max()?;
    let count = end.as_millis().div_ceil(SLOT.as_millis()) as u32;
    let slots: Vec<_> = (0..count)
        .map(|ix| {
            let start = SLOT * ix;
            let from = start.saturating_sub(SLOT_CONTEXT);
            let to = start + SLOT + SLOT_CONTEXT;
            let segments = segments
                .iter()
                .enumerate()
                .filter(|(_, segment)| segment.start < to && from < segment.end)
                .map(|(ix, segment)| {
                    let in_slot = segment.start < start + SLOT && start < segment.end;
                    (ix, if in_slot { 1. } else { CONTEXT_WEIGHT })
                })
                .collect();
            Slot { start, segments }
        })
        .collect();
    (!slots.is_empty()).then_some((slots, end))
}

/// When the sentence being spoken at `time` began, so jumping there doesn't start
/// mid-sentence; `time` itself between sentences.
fn speech_start(segments: &[Segment], time: Duration) -> Duration {
    segments
        .iter()
        .find(|segment| segment.start <= time && time < segment.end)
        .map_or(time, |segment| segment.start)
}
