//! Loading video files for playback. Nothing in here renders UI.

mod thumbnails;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::StreamExt as _;
use gpui_kit::{App, AppContext as _, Context, Entity, RenderImage, Task};
use gpui_video_player::{Url, Video};
use rfd::AsyncFileDialog;

use thumbnails::render_thumbnails;

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "webm", "mov", "avi", "m4v", "ogv"];
/// Video time covered by each timeline thumbnail, unless that would exceed
/// `MAX_THUMBNAILS`; longer videos get proportionally longer spans.
const THUMBNAIL_SPAN: Duration = Duration::from_secs(2);
const MAX_THUMBNAILS: usize = 200;
/// Height thumbnails are decoded at, in pixels. Twice their displayed height, so
/// they stay sharp on high-DPI displays.
const THUMBNAIL_HEIGHT_PX: u32 = 128;

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
    pub path: PathBuf,
    /// Playback handle; dropping the last clone stops the pipeline.
    pub video: Video,
    pub timeline: Timeline,
}

/// Evenly spaced frames across the video, filled in as they're decoded.
pub struct Timeline {
    /// Video time each thumbnail covers; thumbnail `ix` shows the middle of
    /// `span * ix .. span * (ix + 1)`.
    pub span: Duration,
    pub thumbnails: Vec<Option<Arc<RenderImage>>>,
    _job: Option<ThumbnailJob>,
}

impl Timeline {
    fn new(duration: Duration) -> Self {
        let count = if duration.is_zero() {
            0
        } else {
            (duration.div_duration_f64(THUMBNAIL_SPAN).ceil() as usize).clamp(1, MAX_THUMBNAILS)
        };
        Self {
            span: if count == 0 {
                Duration::ZERO
            } else {
                duration / count as u32
            },
            thumbnails: vec![None; count],
            _job: None,
        }
    }

    fn times(&self) -> Vec<Duration> {
        (0..self.thumbnails.len())
            .map(|ix| self.span * ix as u32 + self.span / 2)
            .collect()
    }
}

/// Background thumbnail decoding. Dropping it cancels the work: the task is dropped,
/// and the flag makes the background thread stop after the current thumbnail.
struct ThumbnailJob {
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for ThumbnailJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
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
        // Free the old video's thumbnail textures; replacing the state drops (and so
        // cancels) a running load or thumbnail job.
        if let VideoState::Loaded(loaded) = &mut self.state {
            for image in loaded
                .timeline
                .thumbnails
                .iter_mut()
                .filter_map(Option::take)
            {
                cx.drop_image(image, None);
            }
        }
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
                    Ok(video) => VideoState::Loaded(LoadedVideo {
                        path,
                        timeline: Timeline::new(video.duration()),
                        video,
                    }),
                    Err(err) => {
                        eprintln!("Failed to open video: {err}");
                        VideoState::None
                    }
                };
                this.update(app, |this, cx| {
                    this.set_state(new_state, cx);
                    this.start_thumbnails(cx);
                })
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

    /// Starts decoding the timeline's thumbnails in the background; each one shows up
    /// as soon as it's done.
    fn start_thumbnails(&mut self, cx: &mut Context<Self>) {
        let VideoState::Loaded(loaded) = &mut self.state else {
            return;
        };
        let times = loaded.timeline.times();
        if times.is_empty() {
            return;
        }
        let Ok(uri) = Url::from_file_path(&loaded.path) else {
            return;
        };
        let aspect_ratio = loaded.video.aspect_ratio();
        let cancelled = Arc::new(AtomicBool::new(false));

        let task = cx.spawn({
            let cancelled = cancelled.clone();
            async move |this, app| {
                let (thumbnail_tx, mut thumbnail_rx) = futures::channel::mpsc::unbounded();
                let render = app.background_executor().spawn(async move {
                    render_thumbnails(
                        &uri,
                        &times,
                        THUMBNAIL_HEIGHT_PX,
                        aspect_ratio,
                        &cancelled,
                        |ix, image| {
                            thumbnail_tx.unbounded_send((ix, image)).ok();
                        },
                    )
                });

                // The channel closes when `render` finishes and drops the sender.
                while let Some((ix, image)) = thumbnail_rx.next().await {
                    let updated = this.update(app, |this, cx| this.insert_thumbnail(ix, image, cx));
                    if updated.is_err() {
                        return;
                    }
                }
                if let Err(err) = render.await {
                    eprintln!("Failed to render video thumbnails: {err}");
                }
            }
        });

        loaded.timeline._job = Some(ThumbnailJob {
            cancelled,
            _task: task,
        });
    }

    fn insert_thumbnail(&mut self, ix: usize, image: Arc<RenderImage>, cx: &mut Context<Self>) {
        // Replacing or closing the video drops the job, so a thumbnail that arrives
        // here belongs to the loaded video.
        let VideoState::Loaded(loaded) = &mut self.state else {
            cx.drop_image(image, None);
            return;
        };
        if let Some(old) = loaded.timeline.thumbnails[ix].replace(image) {
            cx.drop_image(old, None);
        }
        cx.notify();
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
