//! Loading video files for playback. Nothing in here renders UI.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::{App, AppContext as _, Context, Entity, Task};
use gpui_video_player::{Url, Video};
use rfd::AsyncFileDialog;

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "webm", "mov", "avi", "m4v", "ogv"];

/// The open video, shared between the UI and the `OpenVideo` action.
pub struct VideoDocument {
    state: VideoState,
}

pub enum VideoState {
    Loaded(LoadedVideo),
    Loading(LoadingVideo),
    None,
}

pub struct LoadingVideo {
    pub path: PathBuf,
    /// Dropping it discards the load.
    _task: Task<()>,
}

pub struct LoadedVideo {
    /// Playback handle; dropping the last clone stops the pipeline.
    pub video: Video,
}

impl VideoDocument {
    pub fn new() -> Self {
        Self {
            state: VideoState::None,
        }
    }

    pub fn state(&self) -> &VideoState {
        &self.state
    }

    /// Closes the video, cancelling any load in progress.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.set_state(VideoState::None, cx);
    }

    fn set_state(&mut self, state: VideoState, cx: &mut Context<Self>) {
        // Replacing the state drops (and so cancels) a running load.
        self.state = state;
        cx.notify();
    }

    fn start_loading(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let task = cx.spawn({
            let path = path.clone();
            async move |this, app| {
                // Building the pipeline waits for GStreamer to preroll the file, so
                // it's done off the main thread.
                let result = app
                    .background_executor()
                    .spawn({
                        let path = path.clone();
                        async move {
                            let uri = Url::from_file_path(&path)
                                .map_err(|()| anyhow::anyhow!("invalid path"))?;
                            let video = Video::new(&uri)?;
                            // Start on the first frame; playback is started from the UI.
                            video.set_paused(true);
                            anyhow::Ok(video)
                        }
                    })
                    .await;

                let new_state = match result {
                    Ok(video) => VideoState::Loaded(LoadedVideo { video }),
                    Err(err) => {
                        eprintln!("Failed to open video: {err}");
                        VideoState::None
                    }
                };
                this.update(app, |this, cx| this.set_state(new_state, cx))
                    .ok();
            }
        });

        self.set_state(VideoState::Loading(LoadingVideo { path, _task: task }), cx);
    }
}

impl VideoDocument {
    /// Jumps playback to `time`.
    pub fn seek(&mut self, time: Duration, cx: &mut Context<Self>) {
        let VideoState::Loaded(loaded) = &self.state else {
            return;
        };
        if let Err(err) = loaded.video.seek(time, true) {
            eprintln!("Failed to seek video: {err}");
            return;
        }
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

/// Asks the user for a video file and starts loading it into `document`.
pub fn open_video(cx: &mut App, document: Entity<VideoDocument>) {
    cx.spawn(async move |app| {
        let selected_file = AsyncFileDialog::new()
            .add_filter("video", VIDEO_EXTENSIONS)
            .set_directory("/")
            .pick_file()
            .await;

        let Some(file) = selected_file else {
            return;
        };

        let path = file.path().to_path_buf();
        app.update_entity(&document, |document, cx| document.start_loading(path, cx));
    })
    .detach();
}

pub fn close_video(cx: &mut App, document: Entity<VideoDocument>) {
    cx.update_entity(&document, |document, cx| document.close(cx));
}
