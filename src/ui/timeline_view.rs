use std::sync::Arc;

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::*;

use super::placeholders::{file_name, page_image};
use crate::pdf::{PdfDocument, PdfState};

/// Displayed height of each thumbnail.
const THUMBNAIL_HEIGHT: Pixels = px(64.);

pub enum TimelineEvent {
    /// The thumbnail of the page at this index was clicked.
    PageClicked(usize),
}

/// What the strip shows for one page.
struct TimelinePage {
    /// Width divided by height.
    aspect_ratio: f32,
    /// `None` until it's rendered.
    thumbnail: Option<Arc<RenderImage>>,
}

/// The pages of the open PDF as a strip of thumbnails.
pub struct TimelineView {
    document: Entity<PdfDocument>,
    _subscription: Subscription,
}

impl TimelineView {
    pub fn new(document: Entity<PdfDocument>, cx: &mut Context<Self>) -> Self {
        // Redraw as thumbnails arrive.
        let _subscription = cx.observe(&document, |_, _, cx| cx.notify());
        Self {
            document,
            _subscription,
        }
    }

    fn content(&self, cx: &mut Context<Self>) -> Div {
        match self.document.read(cx).state() {
            PdfState::Loaded(pdf) => {
                // Copied out so the document isn't borrowed while building the strip.
                let pages: Vec<_> = pdf
                    .page_sizes()
                    .iter()
                    .enumerate()
                    .map(|(ix, size)| TimelinePage {
                        aspect_ratio: size.width / size.height,
                        thumbnail: pdf.thumbnail(ix).cloned(),
                    })
                    .collect();
                self.timeline(pages, cx)
            }
            PdfState::Loading(loading) => h_flex()
                .items_center()
                .gap_2()
                .child(Spinner::new())
                .child(format!("Loading {}…", file_name(&loading.path))),
            PdfState::Failed { .. } | PdfState::None => {
                h_flex().items_center().child("No PDF open")
            }
        }
    }

    /// One thumbnail per page, scrolling horizontally; clicking one jumps to its
    /// page.
    fn timeline(&self, pages: Vec<TimelinePage>, cx: &mut Context<Self>) -> Div {
        let thumbnails = pages.into_iter().enumerate().map(|(ix, page)| {
            div()
                .id(("thumbnail", ix))
                .flex_shrink_0()
                .cursor_pointer()
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(TimelineEvent::PageClicked(ix))))
                .w(THUMBNAIL_HEIGHT * page.aspect_ratio)
                .h(THUMBNAIL_HEIGHT)
                .border_1()
                .border_color(cx.theme().border)
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
        div()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(self.content(cx))
    }
}
