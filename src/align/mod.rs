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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::TextLine;

    fn page(lines: &[&str]) -> PageText {
        PageText {
            lines: lines
                .iter()
                .enumerate()
                .map(|(ix, text)| TextLine {
                    text: text.to_string(),
                    font_size: if ix == 0 { 24. } else { 12. },
                })
                .collect(),
        }
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// One segment every 5 s from `start`, each saying `text`, for `duration` s.
    fn speech(start: u64, duration: u64, text: &str) -> Vec<Segment> {
        (0..duration / 5)
            .map(|ix| Segment {
                start: secs(start + ix * 5),
                end: secs(start + ix * 5 + 5),
                text: text.to_string(),
            })
            .collect()
    }

    fn pages() -> Vec<PageText> {
        vec![
            page(&[
                "Vector spaces",
                "A vector space over a field",
                "Lecture 3 - Linear Algebra",
            ]),
            page(&[
                "Linear maps",
                "Kernel and image of a homomorphism",
                "Lecture 3 - Linear Algebra",
            ]),
            page(&[
                "Eigenvalues",
                "Characteristic polynomial determinant",
                "Lecture 3 - Linear Algebra",
            ]),
            page(&[
                "Diagonalization",
                "Similar matrices basis of eigenvectors",
                "Lecture 3 - Linear Algebra",
            ]),
        ]
    }

    fn first_start(alignment: &Alignment, slide: usize) -> Option<Duration> {
        alignment.spans[slide].first().map(|span| span.start)
    }

    #[test]
    fn follows_the_slides_in_order() {
        let pages = pages();
        let slides = vec![pages.into_iter().map(Some).collect::<Vec<_>>()];
        let segments = [
            speech(0, 60, "so a vector space over some field"),
            speech(60, 60, "the kernel and the image of this linear map"),
            speech(120, 60, "the characteristic polynomial gives eigenvalues"),
            speech(180, 60, "diagonalization with a basis of eigenvectors"),
        ]
        .concat();
        let alignment = align(&slides, &[segments]);
        assert_eq!(first_start(&alignment, 0), Some(secs(0)));
        assert_eq!(first_start(&alignment, 1), Some(secs(60)));
        assert_eq!(first_start(&alignment, 2), Some(secs(120)));
        assert_eq!(first_start(&alignment, 3), Some(secs(180)));
    }

    #[test]
    fn goes_back_when_it_lasts() {
        let pages = pages();
        let slides = vec![pages.into_iter().map(Some).collect::<Vec<_>>()];
        let segments = [
            speech(0, 60, "a vector space over a field"),
            speech(60, 60, "kernel and image of a linear map"),
            speech(120, 60, "remember the vector space over a field"),
            speech(180, 60, "kernel and image again, the linear map"),
        ]
        .concat();
        let alignment = align(&slides, &[segments]);
        let starts: Vec<_> = alignment.spans[0].iter().map(|span| span.start).collect();
        assert_eq!(starts, [secs(0), secs(120)]);
    }

    #[test]
    fn ignores_footers() {
        let pages = pages();
        let slides = vec![pages.into_iter().map(Some).collect::<Vec<_>>()];
        // Mentions the footer on every slide but is about the third one.
        let segments = speech(0, 60, "lecture linear algebra: characteristic polynomial");
        let alignment = align(&slides, &[segments]);
        assert_eq!(alignment.slide_at(0, secs(30)), Some(2));
    }

    #[test]
    fn places_slides_without_text_between_their_neighbours() {
        let mut pages = pages();
        pages.insert(1, PageText::default());
        let slides = vec![pages.into_iter().map(Some).collect::<Vec<_>>()];
        let segments = [
            speech(0, 60, "a vector space over a field"),
            speech(60, 60, "kernel and image of a linear map"),
        ]
        .concat();
        let alignment = align(&slides, &[segments]);
        let image = alignment.spans[1].first().expect("image slide placed");
        assert!(image.start > secs(0) && image.end <= secs(70));
    }

    #[test]
    fn continues_in_the_next_video() {
        let pages = pages();
        let slides = vec![pages.into_iter().map(Some).collect::<Vec<_>>()];
        let first = [
            speech(0, 60, "a vector space over a field"),
            speech(60, 60, "kernel and image of a linear map"),
        ]
        .concat();
        let second = [
            speech(0, 60, "the characteristic polynomial gives eigenvalues"),
            speech(60, 60, "diagonalization with a basis of eigenvectors"),
        ]
        .concat();
        let alignment = align(&slides, &[first, second]);
        assert_eq!(alignment.spans[2].first().map(|span| span.video), Some(1));
        assert_eq!(
            alignment.spans[3]
                .first()
                .map(|span| (span.video, span.start)),
            Some((1, secs(60)))
        );
    }

    #[test]
    fn does_not_go_back_to_slides_of_earlier_videos() {
        let mut pages = pages();
        pages.push(page(&[
            "Jordan normal form",
            "Generalized eigenspaces nilpotent",
        ]));
        pages.push(page(&[
            "Inner products",
            "Orthogonality Gram Schmidt process",
        ]));
        let slides = vec![pages.into_iter().map(Some).collect::<Vec<_>>()];
        let first = [
            speech(0, 60, "a vector space over a field"),
            speech(60, 60, "kernel and image of a linear map"),
            speech(120, 60, "the characteristic polynomial gives eigenvalues"),
            speech(180, 60, "diagonalization with a basis of eigenvectors"),
            speech(240, 60, "jordan normal form with generalized eigenspaces"),
        ]
        .concat();
        // Starts with an announcement that sounds like the first slide.
        let second = [
            speech(0, 60, "announcement about the field trip, vector of dates"),
            speech(60, 120, "inner products, orthogonality and gram schmidt"),
        ]
        .concat();
        let alignment = align(&slides, &[first, second]);
        assert!(alignment.spans[0].iter().all(|span| span.video == 0));
        assert_eq!(alignment.spans[5].first().map(|span| span.video), Some(1));
    }
}
