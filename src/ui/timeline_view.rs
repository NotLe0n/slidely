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
    /// The thumbnail of the page at this index of the project's slides was clicked.
    PageClicked(usize),
}

/// What the strip shows for one page.
struct TimelinePage {
    /// Width divided by height.
    aspect_ratio: f32,
    /// `None` until it's rendered.
    thumbnail: Option<Arc<RenderImage>>,
    /// The first page of a PDF other than the first one.
    starts_pdf: bool,
    /// Whether it's discussed in the videos, as far as they're transcribed.
    discussed: bool,
    /// Whether it's discussed at the playback position of the video shown.
    current: bool,
}

/// The pages of the project's PDFs as one strip of thumbnails.
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

    /// Copied out so the documents aren't borrowed while building the strip.
    fn pages(&self, cx: &App) -> Vec<TimelinePage> {
        let project = self.project.read(cx);
        let slides = project.slides(cx);
        let current = project.current_slide(cx);
        let mut pages = Vec::with_capacity(slides.len());
        for (ix, (document, range)) in slides.documents.iter().enumerate() {
            let PdfState::Loaded(pdf) = document.read(cx).state() else {
                continue;
            };
            pages.extend(range.clone().enumerate().map(|(page, slide)| {
                let size = slides.sizes[slide];
                TimelinePage {
                    aspect_ratio: size.width / size.height,
                    thumbnail: pdf.thumbnail(page).cloned(),
                    starts_pdf: page == 0 && ix > 0,
                    discussed: project.is_discussed(slide),
                    current: current == Some(slide),
                }
            }));
        }
        pages
    }

    /// One thumbnail per page, scrolling horizontally; clicking one jumps to its
    /// page, and to where it's discussed in the videos. Once any page is found in
    /// them, those that aren't are dimmed.
    fn timeline(&self, pages: Vec<TimelinePage>, cx: &mut Context<Self>) -> Div {
        let any_discussed = pages.iter().any(|page| page.discussed);
        let thumbnails = pages.into_iter().enumerate().map(|(ix, page)| {
            let thumbnail = div()
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
                .child(page_image(page.thumbnail));
            // A line between the PDFs.
            h_flex()
                .flex_shrink_0()
                .gap_1()
                .when(page.starts_pdf, |item| {
                    item.child(
                        div()
                            .mx_1()
                            .w(px(2.))
                            .h(THUMBNAIL_HEIGHT)
                            .bg(cx.theme().border),
                    )
                })
                .child(thumbnail)
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
        let pages = self.pages(cx);
        let content = if pages.is_empty() {
            h_flex().items_center().child("No slides")
        } else {
            self.timeline(pages, cx)
        };
        div()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(content)
    }
}
