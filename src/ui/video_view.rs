use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_video_player::{Video, video as video_element};

use super::document::{empty_screen, loading_dialog};
use crate::video::{VideoDocument, VideoState, close_video, open_video};

/// The open video, or a prompt to open one.
pub struct VideoView {
    document: Entity<VideoDocument>,
    /// Size of the area the video is fitted into, measured during the last paint.
    /// The video element needs an explicit size; it can't fill its parent itself.
    viewport: Rc<Cell<gpui_kit::Size<Pixels>>>,
    _subscription: Subscription,
}

impl VideoView {
    pub fn new(document: Entity<VideoDocument>, cx: &mut Context<Self>) -> Self {
        let _subscription = cx.observe(&document, |_, _, cx| cx.notify());
        Self {
            document,
            viewport: Rc::new(Cell::new(gpui_kit::Size::default())),
            _subscription,
        }
    }

    fn player(&self, path: String, video: Video, cx: &mut Context<Self>) -> impl IntoElement {
        let paused = video.paused();
        let viewport = self.viewport.get();

        // Records the area's size each frame; if it changed (panel or window
        // resized), render again so the video is laid out at the new size.
        let measure = canvas(
            {
                let measured = self.viewport.clone();
                move |bounds, window, _| {
                    if measured.replace(bounds.size) != bounds.size {
                        window.refresh();
                    }
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();

        let toggle_playback = Button::new("toggle-playback")
            .label(if paused { "Play" } else { "Pause" })
            .on_click(cx.listener(|this, _, _, cx| {
                if let VideoState::Loaded(loaded) = this.document.read(cx).state() {
                    loaded.video.set_paused(!loaded.video.paused());
                }
                cx.notify();
            }));

        v_flex()
            .size_full()
            .child(div().px_4().py_1().text_sm().child(path))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(measure)
                    // Positioned absolutely so the video never affects the area's size.
                    // It's drawn aspect-fit inside the size it's given.
                    .when(!viewport.is_zero(), |area| {
                        area.child(
                            div().absolute().inset_0().child(
                                video_element(video)
                                    .id("video")
                                    .size(viewport.width, viewport.height),
                            ),
                        )
                    }),
            )
            .child(
                h_flex()
                    .justify_center()
                    .gap_2()
                    .p_2()
                    .child(toggle_playback)
                    .child(Button::new("close-video").label("Close").on_click({
                        let document = self.document.clone();
                        move |_, _, cx| close_video(cx, document.clone())
                    })),
            )
    }
}

impl Render for VideoView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.document.read(cx).state() {
            VideoState::Loaded(loaded) => {
                let path = loaded.path.display().to_string();
                let video = loaded.video.clone();
                self.player(path, video, cx).into_any_element()
            }
            VideoState::Loading(loading) => {
                let document = self.document.clone();
                loading_dialog("Opening video", &loading.path, cx, move |cx| {
                    close_video(cx, document.clone())
                })
                .into_any_element()
            }
            VideoState::None => {
                let document = self.document.clone();
                empty_screen("No video open", "open-video", "Open Video", move |cx| {
                    open_video(cx, document.clone())
                })
                .into_any_element()
            }
        };

        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(content)
    }
}
