use std::sync::Arc;

use gpui_kit::{
    base::{h_flex, v_flex},
    component::{ActiveTheme, scroll::ScrollableElement as _, spinner::Spinner},
    *,
};

use crate::pdf::{PdfDocument, PdfState};

/// Displayed height of each thumbnail.
const THUMBNAIL_HEIGHT: Pixels = px(64.);

pub enum TimelineEvent {
    /// The thumbnail of the page at this index was clicked.
    PageClicked(usize),
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
                    .data
                    .page_sizes
                    .iter()
                    .enumerate()
                    .map(|(ix, size)| (size.width / size.height, pdf.thumbnail(ix).cloned()))
                    .collect();
                self.timeline(pages, cx)
            }
            PdfState::Loading(loading) => h_flex()
                .items_center()
                .gap_2()
                .child(Spinner::new())
                .child(format!("Loading {}", loading.path.display())),
            PdfState::None => h_flex().items_center().child("No PDF open"),
        }
    }

    /// One thumbnail per page, scrolling horizontally; clicking one jumps to its
    /// page. `pages` holds each page's aspect ratio and its thumbnail, if rendered yet.
    fn timeline(&self, pages: Vec<(f32, Option<Arc<RenderImage>>)>, cx: &mut Context<Self>) -> Div {
        let thumbnails = pages
            .into_iter()
            .enumerate()
            .map(|(ix, (aspect_ratio, image))| {
                let page = match image {
                    Some(image) => img(image).size_full().into_any_element(),
                    // Not rendered yet.
                    None => div().size_full().bg(gpui_kit::white()).into_any_element(),
                };
                div()
                    .id(("thumbnail", ix))
                    .flex_shrink_0()
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(TimelineEvent::PageClicked(ix))),
                    )
                    .w(THUMBNAIL_HEIGHT * aspect_ratio)
                    .h(THUMBNAIL_HEIGHT)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .overflow_hidden()
                    .child(page)
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
