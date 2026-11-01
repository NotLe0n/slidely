use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_video_player::{Video, video as video_element};

use super::super::placeholders::{failed_screen, loading_dialog};
use super::video_controls::{VideoControlsBar, VideoControlsBarEvent};
use crate::video::{RunningTranscription, Stage, VideoDocument, VideoState};

pub enum VideoViewEvent {
    /// The user asked to remove the video from the project: they cancelled loading
    /// it, or it couldn't be opened.
    Remove,
}

/// One of the project's videos.
pub struct VideoView {
    document: Entity<VideoDocument>,
    /// Size of the area the video is fitted into, measured during the last paint.
    /// The video element needs an explicit size; it can't fill its parent itself.
    viewport: Rc<Cell<gpui_kit::Size<Pixels>>>,
    controls: Entity<VideoControlsBar>,
    _subscriptions: Vec<Subscription>,
}

impl VideoView {
    pub fn new(document: Entity<VideoDocument>, cx: &mut Context<Self>) -> Self {
        let controls = cx.new(|cx| VideoControlsBar::new(document.clone(), cx));
        let _subscriptions = vec![
            cx.observe(&document, |_, _, cx| cx.notify()),
            cx.subscribe(&controls, Self::on_controls_event),
        ];
        Self {
            document,
            viewport: Rc::new(Cell::new(gpui_kit::Size::default())),
            controls,
            _subscriptions,
        }
    }

    fn video(&self, cx: &App) -> Option<Video> {
        match self.document.read(cx).state() {
            VideoState::Loaded(loaded) => Some(loaded.video.clone()),
            _ => None,
        }
    }

    fn on_controls_event(
        &mut self,
        _: Entity<VideoControlsBar>,
        event: &VideoControlsBarEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            VideoControlsBarEvent::Seek(time) => {
                self.document
                    .update(cx, |document, cx| document.seek(*time, cx));
            }
            VideoControlsBarEvent::VolumeChanged(volume) => {
                if let Some(video) = self.video(cx) {
                    video.set_volume(*volume);
                    video.set_muted(false);
                    cx.notify();
                }
            }
            VideoControlsBarEvent::TogglePlayback => self.toggle_playback(cx),
            VideoControlsBarEvent::ToggleMute => self.toggle_mute(cx),
            VideoControlsBarEvent::SetSpeed(speed) => self.set_speed(*speed, cx),
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
            eprintln!("Failed to set playback speed: {err}");
        }
        cx.notify();
    }

    fn player(
        &mut self,
        video: Video,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // While playing, the video element requests a frame each paint, which
        // renders this view again and so keeps the progress bar moving.
        // Asking the pipeline where it is isn't free, so it's done once per frame.
        let position = self.document.read(cx).position();
        self.controls.update(cx, |controls, cx| {
            controls.show_position(position, window, cx)
        });
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
            .child(self.caption(position, cx))
            .child(self.controls.clone())
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
