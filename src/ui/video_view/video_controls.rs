//! The seek slider and the playback buttons below the videos.

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
use crate::observe::ObservedDocuments;
use crate::project::Project;
use crate::video::{PLAYBACK_SPEEDS, VideoDocument, VideoState};

/// The controls of the video that's shown. Seeking and playing act on that video;
/// the volume and speed are settings, which every video follows, so they stay the
/// same when another one is shown.
pub struct VideoControlsBar {
    project: Entity<Project>,
    /// Redraw when the video shown starts or stops playing, or finishes loading.
    videos: ObservedDocuments<VideoDocument>,
    /// Playback position as a fraction of the video's duration.
    progress: Entity<SliderState>,
    /// Whether the progress slider is being dragged; meanwhile it isn't moved to
    /// the playback position.
    seeking: bool,
    volume: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl VideoControlsBar {
    pub fn new(project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let progress = cx.new(|_| SliderState::new().min(0.).max(1.).step(0.0001));
        let volume = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.01)
                .default_value(1.)
        });
        let _subscriptions = vec![
            cx.observe(&project, |this, _, cx| {
                this.observe_videos(cx);
                cx.notify();
            }),
            cx.subscribe(&progress, Self::on_progress_event),
            cx.subscribe(&volume, Self::on_volume_event),
            // Shows the volume, the speed and whether the slides follow the video.
            cx.observe_global::<Settings>(|_, cx| cx.notify()),
        ];
        let mut this = Self {
            project,
            videos: ObservedDocuments::new(),
            progress,
            seeking: false,
            volume,
            _subscriptions,
        };
        this.observe_videos(cx);
        this
    }

    /// Observes the project's current videos, after one was added or removed.
    fn observe_videos(&mut self, cx: &mut Context<Self>) {
        let videos = self.project.read(cx).videos().to_vec();
        self.videos.observe(&videos, cx, |_, cx| cx.notify());
    }

    /// The video shown, if it's loaded.
    fn document(&self, cx: &App) -> Option<(Entity<VideoDocument>, Video)> {
        let document = self.project.read(cx).active_video()?;
        match document.read(cx).state() {
            VideoState::Loaded(loaded) => Some((document.clone(), loaded.video.clone())),
            _ => None,
        }
    }

    fn on_progress_event(
        &mut self,
        _: Entity<SliderState>,
        event: &SliderEvent,
        cx: &mut Context<Self>,
    ) {
        let Some((document, video)) = self.document(cx) else {
            return;
        };
        match event {
            SliderEvent::Change(value) => {
                self.seeking = true;
                let time = video.duration().mul_f64(value.start().clamp(0., 1.) as f64);
                document.update(cx, |document, cx| document.seek(time, cx));
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
        let volume = value.start() as f64;
        cx.update_global::<Settings, _>(|settings, _| {
            settings.volume = volume;
            settings.muted = false;
        });
    }

    /// Moves the sliders to match the video's position and the volume. A slider is
    /// only updated if its value changed, since updating it renders this view again.
    fn sync_sliders(
        &mut self,
        video: &Video,
        position: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

        let volume = cx.global::<Settings>().volume as f32;
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
    fn buttons(
        &self,
        document: Entity<VideoDocument>,
        video: &Video,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hide_video_disabled = !cx.global::<Settings>().pdf_view_visible;

        h_flex()
            .gap_2()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(self.playback_button(document, video))
            .child(self.mute_button(cx))
            .child(self.volume_control(cx))
            .child(div().flex_1())
            .child(self.slides_follow_button(cx))
            .child(self.speed_menu(cx))
            .child(
                Button::new("hide-video")
                    .ghost()
                    .icon(IconName::PanelRight)
                    .tooltip("Hide video view")
                    .disabled(hide_video_disabled)
                    .on_click(|_, _, cx| Settings::toggle(cx, |s| &mut s.video_view_visible)),
            )
    }

    fn playback_button(&self, document: Entity<VideoDocument>, video: &Video) -> Button {
        let playing = !video.paused() && !video.eos();
        Button::new("toggle-playback")
            .ghost()
            .icon(if playing {
                IconName::Pause
            } else {
                IconName::Play
            })
            .tooltip(if playing { "Pause" } else { "Play" })
            .on_click(move |_, _, cx| {
                document.update(cx, |document, cx| document.toggle_playback(cx))
            })
    }

    fn mute_button(&self, cx: &mut Context<Self>) -> Button {
        let settings = cx.global::<Settings>();
        let muted = settings.muted || settings.volume == 0.;
        Button::new("toggle-mute")
            .ghost()
            .icon(if settings.muted {
                IconName::VolumeX
            } else {
                match settings.volume {
                    0.75..=1.0 => IconName::Volume2,
                    0.25..0.75 => IconName::Volume1,
                    0.0..0.25 => IconName::Volume,
                    _ => IconName::VolumeOff,
                }
            })
            .tooltip(if muted { "Unmute" } else { "Mute" })
            .on_click(|_, _, cx| Settings::toggle(cx, |settings| &mut settings.muted))
    }

    /// The volume slider, with the volume in percent next to it like the times next
    /// to the progress slider.
    fn volume_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let volume = cx.global::<Settings>().volume;
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
    fn speed_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let speed = cx.global::<Settings>().playback_speed;
        Button::new("speed")
            .ghost()
            .icon(IconName::Gauge)
            .label(format!("{speed}×"))
            .tooltip("Playback speed")
            .dropdown_menu(move |mut menu, _, _| {
                for &option in PLAYBACK_SPEEDS {
                    menu = menu.item(
                        PopupMenuItem::new(format!("{option}×"))
                            .checked(option == speed)
                            .on_click(move |_, _, cx| {
                                cx.update_global::<Settings, _>(|settings, _| {
                                    settings.playback_speed = option
                                })
                            }),
                    );
                }
                menu
            })
    }
}

impl Render for VideoControlsBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((document, video)) = self.document(cx) else {
            return v_flex();
        };
        // While playing, the video element requests a frame each paint, which renders
        // this view again and so keeps the progress bar moving.
        let position = document.read(cx).position();
        self.sync_sliders(&video, position, window, cx);
        v_flex()
            .w_full()
            .gap_2()
            .child(self.progress_bar(&video, position, cx))
            .child(self.buttons(document, &video, cx))
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
