use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    base::{h_flex, v_flex},
    component::{ActiveTheme, scroll::ScrollableElement as _, spinner::Spinner},
    *,
};

use crate::video::{VideoDocument, VideoState};

/// Displayed height of each thumbnail.
const THUMBNAIL_HEIGHT: Pixels = px(64.);

pub struct TimelineView {
    document: Entity<VideoDocument>,
    _subscription: Subscription,
}

impl TimelineView {
    pub fn new(document: Entity<VideoDocument>, cx: &mut Context<Self>) -> Self {
        // Redraw as thumbnails arrive.
        let _subscription = cx.observe(&document, |_, _, cx| cx.notify());
        Self {
            document,
            _subscription,
        }
    }

    fn content(&self, cx: &mut Context<Self>) -> Div {
        match self.document.read(cx).state() {
            VideoState::Loaded(loaded) => {
                let timeline = Timeline {
                    span: loaded.timeline.span,
                    thumbnails: loaded.timeline.thumbnails.clone(),
                    aspect_ratio: loaded.video.aspect_ratio(),
                    position: loaded.video.position(),
                };
                self.timeline(timeline, cx)
            }
            VideoState::Loading(loading_video) => h_flex()
                .items_center()
                .child(Spinner::new())
                .child(format!("Loading {}", loading_video.path.display())),
            VideoState::None => h_flex().items_center().child("No video to display"),
        }
    }

    /// One thumbnail per span of the video, scrolling horizontally. Clicking one jumps
    /// there; the one being played is highlighted.
    fn timeline(&self, timeline: Timeline, cx: &mut Context<Self>) -> Div {
        let thumbnail_width = THUMBNAIL_HEIGHT * timeline.aspect_ratio;
        let current = (!timeline.span.is_zero())
            .then(|| timeline.position.div_duration_f64(timeline.span) as usize);

        let thumbnails = timeline
            .thumbnails
            .into_iter()
            .enumerate()
            .map(|(ix, image)| {
                let start = timeline.span * ix as u32;
                let border_color = if current == Some(ix) {
                    cx.theme().primary
                } else {
                    gpui_kit::transparent_black()
                };
                let frame = match image {
                    Some(image) => img(image).size_full().into_any_element(),
                    // Not decoded yet.
                    None => div().size_full().bg(cx.theme().muted).into_any_element(),
                };

                v_flex()
                    .id(("thumbnail", ix))
                    .flex_shrink_0()
                    .gap_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.document
                            .update(cx, |document, cx| document.seek(start, cx))
                    }))
                    .child(
                        div()
                            .w(thumbnail_width)
                            .h(THUMBNAIL_HEIGHT)
                            .border_2()
                            .border_color(border_color)
                            .rounded(cx.theme().radius)
                            .overflow_hidden()
                            .child(frame),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format_time(start)),
                    )
            });

        div().w_full().child(
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

/// What the timeline shows of the loaded video, copied out so the document isn't
/// borrowed while rendering.
struct Timeline {
    span: Duration,
    thumbnails: Vec<Option<Arc<RenderImage>>>,
    aspect_ratio: f32,
    position: Duration,
}

/// `m:ss`, or `h:mm:ss` from an hour on.
fn format_time(time: Duration) -> String {
    let secs = time.as_secs();
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

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
