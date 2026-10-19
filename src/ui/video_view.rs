use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::*;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_video_player::{Video, video as video_element};

use super::document::{empty_screen, loading_dialog};
use crate::video::{VideoDocument, VideoState, close_video, open_video};

/// Playback speeds offered in the speed menu.
const SPEEDS: &[f64] = &[0.25, 0.5, 0.75, 1., 1.25, 1.5, 2.];

/// The open video, or a prompt to open one.
pub struct VideoView {
    document: Entity<VideoDocument>,
    /// Size of the area the video is fitted into, measured during the last paint.
    /// The video element needs an explicit size; it can't fill its parent itself.
    viewport: Rc<Cell<gpui_kit::Size<Pixels>>>,
    /// Playback position as a fraction of the video's duration.
    progress: Entity<SliderState>,
    /// Whether the progress slider is being dragged; meanwhile it isn't moved to
    /// the playback position.
    seeking: bool,
    volume: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl VideoView {
    pub fn new(document: Entity<VideoDocument>, cx: &mut Context<Self>) -> Self {
        let progress = cx.new(|_| SliderState::new().min(0.).max(1.).step(0.0001));
        let volume = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.01)
                .default_value(1.)
        });
        let _subscriptions = vec![
            cx.observe(&document, |_, _, cx| cx.notify()),
            cx.subscribe(&progress, Self::on_progress_event),
            cx.subscribe(&volume, Self::on_volume_event),
        ];
        Self {
            document,
            viewport: Rc::new(Cell::new(gpui_kit::Size::default())),
            progress,
            seeking: false,
            volume,
            _subscriptions,
        }
    }

    fn video(&self, cx: &App) -> Option<Video> {
        match self.document.read(cx).state() {
            VideoState::Loaded(loaded) => Some(loaded.video.clone()),
            _ => None,
        }
    }

    fn on_progress_event(
        &mut self,
        _: Entity<SliderState>,
        event: &SliderEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(video) = self.video(cx) else {
            return;
        };
        match event {
            SliderEvent::Change(value) => {
                self.seeking = true;
                let time = video.duration().mul_f64(value.start().clamp(0., 1.) as f64);
                self.document
                    .update(cx, |document, cx| document.seek(time, cx));
            }
            SliderEvent::Release(_) => self.seeking = false,
        }
    }

    fn on_volume_event(
        &mut self,
        _: Entity<SliderState>,
        event: &SliderEvent,
        cx: &mut Context<Self>,
    ) {
        let (SliderEvent::Change(value) | SliderEvent::Release(value)) = event;
        if let Some(video) = self.video(cx) {
            video.set_volume(value.start() as f64);
            video.set_muted(false);
            cx.notify();
        }
    }

    /// Plays or pauses the video; at the end, plays it again from the start.
    fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        let Some(video) = self.video(cx) else {
            return;
        };
        if video.eos() {
            self.document
                .update(cx, |document, cx| document.seek(Duration::ZERO, cx));
            video.set_paused(false);
        } else {
            video.set_paused(!video.paused());
        }
        cx.notify();
    }

    /// Moves the sliders to match the video. A slider is only updated if its value
    /// changed, since updating it renders this view again.
    fn sync_sliders(&mut self, video: &Video, window: &mut Window, cx: &mut Context<Self>) {
        if !self.seeking {
            let duration = video.duration();
            let progress = if duration.is_zero() {
                0.
            } else {
                video.position().div_duration_f64(duration).clamp(0., 1.) as f32
            };
            if self.progress.read(cx).value().start() != progress {
                self.progress
                    .update(cx, |slider, cx| slider.set_value(progress, window, cx));
            }
        }

        let volume = video.volume() as f32;
        if self.volume.read(cx).value().start() != volume {
            self.volume
                .update(cx, |slider, cx| slider.set_value(volume, window, cx));
        }
    }

    fn player(
        &mut self,
        video: Video,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // While playing, the video element requests a frame each paint, which
        // renders this view again and so keeps the progress bar moving.
        self.sync_sliders(&video, window, cx);
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

        v_flex()
            .size_full()
            .child(
                div()
                    .id("video-area")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_playback(cx)))
                    .child(measure)
                    // Positioned absolutely so the video never affects the area's size.
                    // It's drawn aspect-fit inside the size it's given.
                    .when(!viewport.is_zero(), |area| {
                        area.child(
                            div().absolute().inset_0().child(
                                video_element(video.clone())
                                    .id("video")
                                    .size(viewport.width, viewport.height),
                            ),
                        )
                    }),
            )
            .child(self.progress_bar(&video, cx))
            .child(self.controls(&video, cx))
    }

    /// The seek slider, between the elapsed and the total time.
    fn progress_bar(&self, video: &Video, cx: &mut Context<Self>) -> impl IntoElement {
        let time_label = |time| {
            div()
                .flex_none()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(format_time(time))
        };

        h_flex()
            .gap_3()
            .px_4()
            .pt_2()
            .child(time_label(video.position()))
            .child(
                div()
                    .flex_1()
                    .child(Slider::new(&self.progress).cursor_pointer()),
            )
            .child(time_label(video.duration()))
    }

    fn controls(&self, video: &Video, cx: &mut Context<Self>) -> impl IntoElement {
        let playing = !video.paused() && !video.eos();
        let muted = video.muted() || video.volume() == 0.;
        let speed = video.speed();
        let volume = video.volume();

        let toggle_playback = Button::new("toggle-playback")
            .ghost()
            .icon(if playing {
                IconName::Pause
            } else {
                IconName::Play
            })
            .tooltip(if playing { "Pause" } else { "Play" })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_playback(cx)));

        let toggle_mute = Button::new("toggle-mute")
            .ghost()
            .icon(if video.muted() {
                IconName::VolumeX
            } else {
                match volume {
                    0.75..=1.0 => IconName::Volume2,
                    0.25..0.75 => IconName::Volume1,
                    0.0..0.25 => IconName::Volume,
                    _ => IconName::VolumeOff,
                }
            })
            .tooltip(if muted { "Unmute" } else { "Mute" })
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(video) = this.video(cx) {
                    video.set_muted(!video.muted());
                    cx.notify();
                }
            }));

        let view = cx.entity().downgrade();
        let speed_menu = Button::new("speed")
            .ghost()
            .icon(IconName::Gauge)
            .label(format!("{speed}×"))
            .tooltip("Playback speed")
            .dropdown_menu(move |mut menu, _, _| {
                for &option in SPEEDS {
                    let view = view.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("{option}×"))
                            .checked(option == speed)
                            .on_click(move |_, _, cx| {
                                view.update(cx, |this, cx| {
                                    if let Some(video) = this.video(cx)
                                        && let Err(err) = video.set_speed(option)
                                    {
                                        eprintln!("Failed to set playback speed: {err}");
                                    }
                                    cx.notify();
                                })
                                .ok();
                            }),
                    );
                }
                menu
            });

        h_flex()
            .gap_2()
            .px_4()
            .py_2()
            .child(toggle_playback)
            .child(toggle_mute)
            .child(div().w(px(100.)).child(Slider::new(&self.volume)))
            .child(format!("{}%", (volume * 100.0).round()))
            .child(div().flex_1())
            .child(speed_menu)
    }
}

impl Render for VideoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.document.read(cx).state() {
            VideoState::Loaded(loaded) => {
                let video = loaded.video.clone();
                self.player(video, window, cx).into_any_element()
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
