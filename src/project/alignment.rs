//! Keeping the project's alignment of the videos to the slides up to date as text is
//! extracted and transcripts come in.

use std::time::Duration;

use gpui_kit::{App, Context, Entity, EntityId, Task};

use super::Project;
use crate::align::{self, Alignment};
use crate::pdf::{PageText, PdfState};
use crate::video::{Segment, VideoDocument};

/// Where each slide is discussed in the videos, and what that was found from.
#[derive(Default)]
pub(super) struct ProjectAlignment {
    alignment: Alignment,
    /// The videos the alignment's video indices refer to.
    videos: Vec<Entity<VideoDocument>>,
    /// What the alignment being computed, or else the current one, is from.
    inputs: Inputs,
    /// Dropping it discards the alignment being computed.
    task: Option<Task<()>>,
}

/// What's aligned, as far as it changes the result.
#[derive(Default, PartialEq)]
struct Inputs {
    /// Each loaded PDF, its page count, and whether its text is extracted.
    pdfs: Vec<(EntityId, usize, bool)>,
    /// Each video and how many segments of its transcript there are.
    videos: Vec<(EntityId, usize)>,
}

impl Project {
    /// Aligns the videos to the slides again in the background if a PDF or video was
    /// added, removed or moved, or text or transcript segments came in.
    pub(super) fn update_alignment(&mut self, cx: &mut Context<Self>) {
        let inputs = self.alignment_inputs(cx);
        if inputs == self.alignment.inputs {
            return;
        }
        self.alignment.inputs = inputs;

        let slides = self.slides(cx);
        let pdfs: Vec<Vec<Option<PageText>>> = slides
            .documents
            .iter()
            .map(|(pdf, range)| match pdf.read(cx).page_texts() {
                Some(texts) => texts.iter().cloned().map(Some).collect(),
                None => range.clone().map(|_| None).collect(),
            })
            .collect();
        let videos = self.videos.clone();
        let transcripts: Vec<Vec<Segment>> = videos
            .iter()
            .map(|video| video.read(cx).segments_so_far().to_vec())
            .collect();

        self.alignment.task = Some(cx.spawn(async move |this, app| {
            let alignment = app
                .background_executor()
                .spawn(async move { align::align(&pdfs, &transcripts) })
                .await;
            this.update(app, |this, cx| {
                this.alignment.alignment = alignment;
                this.alignment.videos = videos;
                this.alignment.task = None;
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn clear_alignment(&mut self) {
        self.alignment = ProjectAlignment::default();
    }

    fn alignment_inputs(&self, cx: &App) -> Inputs {
        let pdfs = self
            .pdfs
            .iter()
            .filter_map(|pdf| {
                let document = pdf.read(cx);
                let PdfState::Loaded(loaded) = document.state() else {
                    return None;
                };
                let page_count = loaded.page_sizes().len();
                Some((pdf.entity_id(), page_count, loaded.page_texts().is_some()))
            })
            .collect();
        let videos = self
            .videos
            .iter()
            .map(|video| (video.entity_id(), video.read(cx).segments_so_far().len()))
            .collect();
        Inputs { pdfs, videos }
    }

    /// Where slide `ix` is discussed the longest: in which video, and when that
    /// starts. Lecturers often mention a slide briefly, e.g. in an overview or when
    /// going back, before or after they go through it.
    pub fn slide_start(&self, ix: usize) -> Option<(Entity<VideoDocument>, Duration)> {
        let spans = self.alignment.alignment.spans.get(ix)?;
        // The first of equally long ones.
        let span = spans
            .iter()
            .rev()
            .max_by_key(|span| span.end - span.start)?;
        let video = self.alignment.videos.get(span.video)?;
        Some((video.clone(), span.start))
    }

    /// Whether slide `ix` is discussed in any of the videos, as far as they're
    /// transcribed.
    pub fn is_discussed(&self, ix: usize) -> bool {
        self.alignment
            .alignment
            .spans
            .get(ix)
            .is_some_and(|spans| !spans.is_empty())
    }

    /// The slide discussed in `video` at `time`, if any.
    pub fn slide_at(&self, video: &Entity<VideoDocument>, time: Duration) -> Option<usize> {
        let video = self.alignment.videos.iter().position(|v| v == video)?;
        self.alignment.alignment.slide_at(video, time)
    }

    /// The slide discussed where the video shown is, if any.
    pub fn current_slide(&self, cx: &App) -> Option<usize> {
        let video = self.active_video()?;
        self.slide_at(video, video.read(cx).position())
    }

    /// Shows the video where slide `ix` is discussed the longest, at that point,
    /// unless the video shown is at a point where it's discussed already.
    pub fn play_slide(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some((video, start)) = self.slide_start(ix) else {
            return;
        };
        if self.current_slide(cx) == Some(ix) {
            return;
        }
        if let Some(video_ix) = self.video_index(&video) {
            self.set_active_video(video_ix, cx);
        }
        video.update(cx, |video, cx| video.seek(start, cx));
    }
}
