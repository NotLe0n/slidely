use std::sync::Arc;

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::placeholders::page_image;
use crate::observe::ObservedDocuments;
use crate::pdf::{PdfDocument, PdfState};
use crate::project::Project;

/// Displayed height of each thumbnail.
const THUMBNAIL_HEIGHT: Pixels = px(64.);

pub enum TimelineEvent {
    /// The thumbnail of the page at this index of all of the project's slides was
    /// clicked.
    PageClicked(usize),
}

/// What the strip shows for one page.
struct TimelinePage {
    /// Width divided by height.
    aspect_ratio: f32,
    /// `None` until it's rendered.
    thumbnail: Option<Arc<RenderImage>>,
    /// Whether it's discussed in the videos, as far as they're transcribed.
    discussed: bool,
    /// Whether it's discussed at the playback position of the video shown.
    current: bool,
}

/// The pages of the project's PDF that's shown as a strip of thumbnails.
pub struct TimelineView {
    project: Entity<Project>,
    /// Redraw as thumbnails arrive.
    pdfs: ObservedDocuments<PdfDocument>,
    _subscription: Subscription,
}

impl TimelineView {
    pub fn new(project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let _subscription = cx.observe(&project, |this, _, cx| {
            this.observe_documents(cx);
            cx.notify();
        });
        let mut this = Self {
            project,
            pdfs: ObservedDocuments::new(),
            _subscription,
        };
        this.observe_documents(cx);
        this
    }

    /// Observes the project's current PDFs, after one was added or removed.
    fn observe_documents(&mut self, cx: &mut Context<Self>) {
        let pdfs = self.project.read(cx).pdfs().to_vec();
        self.pdfs.observe(&pdfs, cx, |_, cx| cx.notify());
    }

    /// The index among all slides of the first page shown, and the pages. Copied out
    /// so the documents aren't borrowed while building the strip.
    fn pages(&self, cx: &App) -> (usize, Vec<TimelinePage>) {
        let project = self.project.read(cx);
        let slides = project.shown_slides(cx);
        let current = project.current_slide(cx);
        let Some((document, _)) = slides.documents.first() else {
            return (slides.first, Vec::new());
        };
        let PdfState::Loaded(pdf) = document.read(cx).state() else {
            return (slides.first, Vec::new());
        };
        let pages = slides
            .sizes
            .iter()
            .enumerate()
            .map(|(page, size)| {
                let slide = slides.first + page;
                TimelinePage {
                    aspect_ratio: size.width / size.height,
                    thumbnail: pdf.thumbnail(page).cloned(),
                    discussed: project.is_discussed(slide),
                    current: current == Some(slide),
                }
            })
            .collect();
        (slides.first, pages)
    }

    /// One thumbnail per page, scrolling horizontally; clicking one jumps to its
    /// page, and to where it's discussed in the videos. Once any page is found in
    /// them, those that aren't are dimmed.
    fn timeline(&self, first: usize, pages: Vec<TimelinePage>, cx: &mut Context<Self>) -> Div {
        let any_discussed = pages.iter().any(|page| page.discussed);
        let thumbnails = pages.into_iter().enumerate().map(|(page_ix, page)| {
            let ix = first + page_ix;
            div()
                .id(("thumbnail", ix))
                .flex_shrink_0()
                .cursor_pointer()
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(TimelineEvent::PageClicked(ix))))
                .w(THUMBNAIL_HEIGHT * page.aspect_ratio)
                .h(THUMBNAIL_HEIGHT)
                .border_2()
                .border_color(cx.theme().border)
                .when(page.current, |thumbnail| {
                    thumbnail.border_2().border_color(cx.theme().red)
                })
                .when(any_discussed && !page.discussed, |thumbnail| {
                    thumbnail.opacity(0.4)
                })
                .rounded(cx.theme().radius)
                .overflow_hidden()
                .child(page_image(page.thumbnail))
        });

        v_flex().w_full().child(
            h_flex()
                .id("timeline")
                .w_full()
                .gap_1()
                .pb_3()
                .overflow_x_scrollbar()
                .children(thumbnails),
        )
    }
}

impl EventEmitter<TimelineEvent> for TimelineView {}

impl Render for TimelineView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (first, pages) = self.pages(cx);
        let content = if pages.is_empty() {
            h_flex().items_center().child("No slides")
        } else {
            self.timeline(first, pages, cx)
        };
        div()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(content)
    }
}
