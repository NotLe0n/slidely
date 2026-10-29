//! Controlling where a video plays.

use std::time::Duration;

use gpui_kit::Context;

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

    /// The playback position, or where it's seeking to.
    pub fn position(&self) -> Duration {
        match &self.state {
            VideoState::Loaded(loaded) => loaded.video.position(),
            _ => Duration::ZERO,
        }
    }

    /// Jumps playback to `time`.
    pub fn seek(&mut self, time: Duration, cx: &mut Context<Self>) {
        let VideoState::Loaded(loaded) = &self.state else {
            return;
        };
        if let Err(err) = loaded.video.seek(time) {
            eprintln!("Failed to seek video: {err:#}");
            return;
        }
        cx.notify();
    }
}
