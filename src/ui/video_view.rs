use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::*;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::controls::setting_toggle;
use super::placeholders::{failed_screen, loading_dialog};
use crate::Settings;
use crate::video::{
    PLAYBACK_SPEEDS, Player, RunningTranscription, Stage, VideoDocument, VideoState,
};

pub enum VideoViewEvent {
    /// The user asked to remove the video from the project: they cancelled loading
    /// it, or it couldn't be opened.
    Remove,
}

/// One of the project's videos.
pub struct VideoView {
    document: Entity<VideoDocument>,
    /// The frame shown. Each one takes a texture of its own until it's dropped.
    frame: Option<Arc<RenderImage>>,
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
            // Shows whether the slides follow the video.
            cx.observe_global::<Settings>(|_, cx| cx.notify()),
            cx.on_release(|this: &mut Self, cx| {
                if let Some(frame) = this.frame.take() {
                    cx.drop_image(frame, None);
                }
            }),
        ];
        Self {
            document,
            frame: None,
            progress,
            seeking: false,
            volume,
            _subscriptions,
        }
    }

    fn video(&self, cx: &App) -> Option<Player> {
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
    pub fn toggle_playback(&mut self, cx: &mut Context<Self>) {
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

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if let Some(video) = self.video(cx) {
            video.set_muted(!video.muted());
            cx.notify();
        }
    }

    /// Sets the playback speed as a factor of the normal speed.
    pub fn set_speed(&mut self, speed: f64, cx: &mut Context<Self>) {
        if let Some(video) = self.video(cx)
            && let Err(err) = video.set_speed(speed)
        {
            eprintln!("Failed to set playback speed: {err:#}");
        }
        cx.notify();
    }

    /// Moves the sliders to match the video. A slider is only updated if its value
    /// changed, since updating it renders this view again.
    fn sync_sliders(
        &mut self,
        video: &Player,
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

        let volume = video.volume() as f32;
        if self.volume.read(cx).value().start() != volume {
            self.volume
                .update(cx, |slider, cx| slider.set_value(volume, window, cx));
        }
    }

    /// Shows the newest frame of `video`, if there's one it hasn't shown yet.
    fn update_frame(&mut self, video: &Player, window: &mut Window, cx: &mut Context<Self>) {
        let Some(frame) = video.take_frame() else {
            return;
        };
        let Some(buffer) = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels)
        else {
            return;
        };
        let image = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
        if let Some(old) = self.frame.replace(image) {
            cx.drop_image(old, Some(window));
        }
    }

    fn player(
        &mut self,
        video: Player,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // The player renders this view again for every new frame, and whenever the
        // position passes a second.
        self.update_frame(&video, window, cx);
        let position = self.document.read(cx).position();
        self.sync_sliders(&video, position, window, cx);

        // Tells the player the size the video is shown at, in device pixels, so its
        // frames are rendered at that size.
        let measure = canvas(
            {
                let video = video.clone();
                move |bounds, window, _| {
                    let size = bounds.size.scale(window.scale_factor());
                    video.set_max_size(size.width.ceil().0 as u32, size.height.ceil().0 as u32);
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
                    .when_some(self.frame.clone(), |area, frame| {
                        area.child(
                            img(ImageSource::Render(frame))
                                .absolute()
                                .inset_0()
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        )
                    }),
            )
            .child(self.caption(position, cx))
            .child(self.progress_bar(&video, position, cx))
            .child(self.controls(&video, cx))
    }

    /// The transcript segment being spoken, once the video is transcribed.
    fn caption(&self, position: Duration, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self
            .document
            .read(cx)
            .transcript()
            .and_then(|transcript| transcript.segment_at(position))
            .map(|segment| segment.text.clone());
        div().when_some(text, |caption, text| {
            caption.px_4().pt_2().w_full().text_center().child(text)
        })
    }

    /// The seek slider, between the elapsed and the total time.
    fn progress_bar(
        &self,
        video: &Player,
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
    fn controls(&self, video: &Player, cx: &mut Context<Self>) -> impl IntoElement {
        let volume = video.volume();
        h_flex()
            .gap_2()
            .px_4()
            .py_2()
            .child(self.playback_button(video, cx))
            .child(self.mute_button(video, cx))
            .child(div().w(px(100.)).child(Slider::new(&self.volume)))
            .child(format!("{}%", (volume * 100.0).round()))
            .child(div().flex_1())
            .child(self.slides_follow_button(cx))
            .child(self.speed_menu(video, cx))
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

    fn playback_button(&self, video: &Player, cx: &mut Context<Self>) -> Button {
        let playing = !video.paused() && !video.eos();
        Button::new("toggle-playback")
            .ghost()
            .icon(if playing {
                IconName::Pause
            } else {
                IconName::Play
            })
            .tooltip(if playing { "Pause" } else { "Play" })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_playback(cx)))
    }

    fn mute_button(&self, video: &Player, cx: &mut Context<Self>) -> Button {
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
            .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx)))
    }

    /// A button showing the playback speed, with a menu to change it.
    fn speed_menu(&self, video: &Player, cx: &mut Context<Self>) -> impl IntoElement {
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
                                view.update(cx, |this, cx| this.set_speed(option, cx)).ok();
                            }),
                    );
                }
                menu
            })
    }
}

impl EventEmitter<VideoViewEvent> for VideoView {}

impl Render for VideoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let remove = move |cx: &mut App| {
            view.update(cx, |_, cx| cx.emit(VideoViewEvent::Remove))
                .ok();
        };
        let content = match self.document.read(cx).state() {
            VideoState::Loaded(loaded) => {
                let video = loaded.video.clone();
                self.player(video, window, cx).into_any_element()
            }
            VideoState::Loading(loading) => {
                loading_dialog("Opening video", &loading.path, cx, remove).into_any_element()
            }
            VideoState::Failed { path, error, .. } => {
                failed_screen(path, error, "remove-video", "Remove Video", cx, remove)
                    .into_any_element()
            }
            // Only briefly, while the video is being removed.
            VideoState::None => div().into_any_element(),
        };

        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(content)
    }
}

/// What a running transcription is doing, e.g. "Transcribing 45%".
pub fn progress_label(running: &RunningTranscription) -> String {
    match (running.stage(), running.percent()) {
        (Stage::ExtractingAudio, percent) => {
            format!("Extracting audio {}%", percent.unwrap_or(0))
        }
        (Stage::LoadingModel, _) => "Loading model".into(),
        (Stage::Transcribing, percent) => format!("Transcribing {}%", percent.unwrap_or(0)),
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
