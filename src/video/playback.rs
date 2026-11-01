//! Opening a video for playback, and controlling where it plays.

use std::time::Duration;

use gpui_kit::Context;
use gpui_video_player::{Url, Video};
use gstreamer::ClockTime;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;

use super::{VideoDocument, VideoState};

impl VideoDocument {
    /// Stops playback.
    pub fn pause(&mut self, cx: &mut Context<Self>) {
        if let VideoState::Loaded(loaded) = &self.state
            && !loaded.video.paused()
        {
            loaded.video.set_paused(true);
            cx.notify();
        }
    }

    /// Plays or pauses the video; at the end, plays it again from the start.
    pub fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        let VideoState::Loaded(loaded) = &self.state else {
            return;
        };
        let video = loaded.video.clone();
        if video.eos() {
            self.seek(Duration::ZERO, cx);
            video.set_paused(false);
        } else {
            video.set_paused(!video.paused());
        }
        cx.notify();
    }

    /// The playback position. While a seek is in progress, GStreamer can't tell,
    /// and `Video::position` says 0; this says where it's seeking to instead.
    pub fn position(&self) -> Duration {
        let VideoState::Loaded(loaded) = &self.state else {
            return Duration::ZERO;
        };
        if let Some(position) = loaded.video.pipeline().query_position::<ClockTime>() {
            loaded
                .last_position
                .set(Duration::from_nanos(position.nseconds()));
        }
        loaded.last_position.get()
    }

    /// Jumps playback to `time`.
    pub fn seek(&mut self, time: Duration, cx: &mut Context<Self>) {
        let VideoState::Loaded(loaded) = &self.state else {
            return;
        };
        if let Err(err) = loaded.video.seek(time, true) {
            eprintln!("Failed to seek video: {err}");
            return;
        }
        loaded.last_position.set(time);
        cx.notify();

        // The player only redraws itself while playing. When paused, wait for the
        // frame at `time` to arrive and redraw once it's there.
        let video = loaded.video.clone();
        if video.paused() {
            cx.spawn(async move |this, app| {
                for _ in 0..60 {
                    app.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                    if video.take_frame_ready() {
                        this.update(app, |_, cx| cx.notify()).ok();
                        return;
                    }
                }
            })
            .detach();
        }
    }
}

/// Opens the video at `uri` for playback. It's the pipeline `Video::new` builds, with
/// `scaletempo` added so the speech keeps its pitch at other playback speeds.
pub(super) fn open_video(uri: &Url) -> anyhow::Result<Video> {
    gstreamer::init()?;
    let pipeline = format!(
        "playbin uri=\"{uri}\" audio-filter=scaletempo video-sink=\"videoscale ! videoconvert ! \
         appsink name=gpui_video drop=true max-buffers=200 enable-last-sample=false \
         caps=video/x-raw,format=NV12,pixel-aspect-ratio=1/1\""
    );
    let pipeline = gstreamer::parse::launch(&pipeline)?
        .downcast::<gstreamer::Pipeline>()
        .map_err(|_| anyhow::anyhow!("playbin isn't a pipeline"))?;
    let video_sink = pipeline
        .property::<gstreamer::Element>("video-sink")
        .downcast::<gstreamer::Bin>()
        .ok()
        .and_then(|bin| bin.by_name("gpui_video"))
        .and_then(|sink| sink.downcast::<AppSink>().ok())
        .ok_or_else(|| anyhow::anyhow!("the video sink is missing"))?;
    Ok(Video::from_gst_pipeline(pipeline, video_sink, None)?)
}
