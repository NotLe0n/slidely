//! The seek slider and the playback buttons below a video.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::*;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::*;
use gpui_kit::*;
use gpui_video_player::Video;

use super::super::controls::setting_toggle;
use crate::Settings;
use crate::video::{PLAYBACK_SPEEDS, VideoDocument, VideoState};

pub enum VideoControlsBarEvent {
    /// The user dragged the progress slider to this point of the video.
    Seek(Duration),
    /// The user set the volume, from 0 to 1.
    VolumeChanged(f64),
    /// The user pressed the play/pause button.
    TogglePlayback,
    /// The user pressed the mute button.
    ToggleMute,
    /// The user picked this playback speed.
    SetSpeed(f64),
}

/// The seek slider and the buttons below the video. It only shows the video's state;
/// `VideoView` owns it and changes the video.
pub struct VideoControlsBar {
    document: Entity<VideoDocument>,
    /// Where the video is, as `show_position` last said.
    position: Duration,
    /// Playback position as a fraction of the video's duration.
    progress: Entity<SliderState>,
    /// Whether the progress slider is being dragged; meanwhile it isn't moved to
    /// the playback position.
    seeking: bool,
    volume: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<VideoControlsBarEvent> for VideoControlsBar {}

impl VideoControlsBar {
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
            cx.subscribe(&progress, Self::on_progress_event),
            cx.subscribe(&volume, Self::on_volume_event),
            // Shows whether the slides follow the video.
            cx.observe_global::<Settings>(|_, cx| cx.notify()),
        ];
        Self {
            document,
            position: Duration::ZERO,
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
                cx.emit(VideoControlsBarEvent::Seek(time));
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
        cx.emit(VideoControlsBarEvent::VolumeChanged(value.start() as f64));
    }

    /// Shows the playback position, and moves the sliders to match the video. A
    /// slider is only updated if its value changed, since updating it renders this
    /// view again.
    pub fn show_position(
        &mut self,
        position: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.position = position;
        let Some(video) = self.video(cx) else {
            return;
        };
        if !self.seeking {
            let duration = video.duration();
            let progress = if duration.is_zero() {
                0.
            } else {
                position.div_duration_f64(duration).clamp(0., 1.) as f32
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

    /// The seek slider, between the elapsed and the total time.
    fn progress_bar(
        &self,
        video: &Video,
        position: Duration,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
            .child(time_label(position))
            .child(
                div()
                    .flex_1()
                    .child(Slider::new(&self.progress).cursor_pointer()),
            )
            .child(time_label(video.duration()))
    }

    /// Play/pause, volume, following in the slides and speed.
    fn buttons(&self, video: &Video, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .px_4()
            .py_2()
            .child(self.playback_button(video, cx))
            .child(self.mute_button(video, cx))
            .child(self.volume_control(video, cx))
            .child(div().flex_1())
            .child(self.slides_follow_button(cx))
            .child(self.speed_menu(video, cx))
    }

    fn playback_button(&self, video: &Video, cx: &mut Context<Self>) -> Button {
        let playing = !video.paused() && !video.eos();
        Button::new("toggle-playback")
            .ghost()
            .icon(if playing {
                IconName::Pause
            } else {
                IconName::Play
            })
            .tooltip(if playing { "Pause" } else { "Play" })
            .on_click(cx.listener(|_, _, _, cx| cx.emit(VideoControlsBarEvent::TogglePlayback)))
    }

    fn mute_button(&self, video: &Video, cx: &mut Context<Self>) -> Button {
        let muted = video.muted() || video.volume() == 0.;
        Button::new("toggle-mute")
            .ghost()
            .icon(if video.muted() {
                IconName::VolumeX
            } else {
                match video.volume() {
                    0.75..=1.0 => IconName::Volume2,
                    0.25..0.75 => IconName::Volume1,
                    0.0..0.25 => IconName::Volume,
                    _ => IconName::VolumeOff,
                }
            })
            .tooltip(if muted { "Unmute" } else { "Mute" })
            .on_click(cx.listener(|_, _, _, cx| cx.emit(VideoControlsBarEvent::ToggleMute)))
    }

    /// The volume slider, with the volume in percent next to it like the times next
    /// to the progress slider.
    fn volume_control(&self, video: &Video, cx: &mut Context<Self>) -> impl IntoElement {
        let volume = video.volume();
        h_flex()
            .gap_3()
            .child(div().w(px(100.)).child(Slider::new(&self.volume)))
            .child(
                div()
                    .flex_none()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{}%", (volume * 100.0).round())),
            )
    }

    /// Toggles whether seeking scrolls the slides to the one discussed there.
    fn slides_follow_button(&self, cx: &mut Context<Self>) -> Button {
        setting_toggle(
            "slides-follow-video",
            IconName::Presentation,
            "Follow the video in the slides",
            |settings| &mut settings.slides_follow_video,
            cx,
        )
    }

    /// A button showing the playback speed, with a menu to change it.
    fn speed_menu(&self, video: &Video, cx: &mut Context<Self>) -> impl IntoElement {
        let speed = video.speed();
        let view = cx.entity().downgrade();
        Button::new("speed")
            .ghost()
            .icon(IconName::Gauge)
            .label(format!("{speed}×"))
            .tooltip("Playback speed")
            .dropdown_menu(move |mut menu, _, _| {
                for &option in PLAYBACK_SPEEDS {
                    let view = view.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("{option}×"))
                            .checked(option == speed)
                            .on_click(move |_, _, cx| {
                                view.update(cx, |_, cx| {
                                    cx.emit(VideoControlsBarEvent::SetSpeed(option))
                                })
                                .ok();
                            }),
                    );
                }
                menu
            })
    }
}

impl Render for VideoControlsBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(video) = self.video(cx) else {
            return v_flex();
        };
        v_flex()
            .w_full()
            .child(self.progress_bar(&video, self.position, cx))
            .child(self.buttons(&video, cx))
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
