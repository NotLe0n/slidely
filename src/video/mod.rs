//! Loading video files for playback. Nothing in here renders UI.

mod transcribe;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use gpui_kit::{Context, Task};
use gpui_video_player::{Url, Video};
use gstreamer::ClockTime;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;

use transcribe::Progress;
pub use transcribe::{Segment, Stage};

pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "webm", "mov", "avi", "m4v", "ogv"];
/// Playback speeds offered in the speed menus.
pub const PLAYBACK_SPEEDS: &[f64] = &[0.75, 1., 1.25, 1.5, 2., 4.];

/// One of the project's videos.
pub struct VideoDocument {
    state: VideoState,
}

pub enum VideoState {
    Loaded(LoadedVideo),
    Loading(LoadingVideo),
    /// The last file couldn't be opened.
    Failed {
        path: PathBuf,
        error: String,
        /// The transcript given to `load`, kept so it's saved and used when it's
        /// retried.
        transcript: Option<Vec<Segment>>,
    },
    None,
}

pub struct LoadingVideo {
    pub path: PathBuf,
    /// The transcript given to `load`, kept here so it's saved while the video loads.
    /// Without one, it's queued for transcription once it's loaded.
    transcript: Option<Vec<Segment>>,
    /// Dropping it discards the load.
    _task: Task<()>,
}

pub struct LoadedVideo {
    pub path: PathBuf,
    /// Playback handle; dropping the last clone stops the pipeline.
    pub video: Video,
    pub transcript: TranscriptState,
    /// Where playback was last known to be, or is seeking to; see `position`.
    last_position: Cell<Duration>,
}

pub enum TranscriptState {
    None,
    /// Waiting for the transcriptions of other videos to finish; only one runs at a
    /// time. The project starts it.
    Queued,
    Running(RunningTranscription),
    Done(Vec<Segment>),
    Failed(String),
}

pub struct RunningTranscription {
    progress: Arc<Progress>,
    /// The segments transcribed so far, in order.
    segments: Vec<Segment>,
    /// Dropping them discards the transcription.
    _tasks: [Task<()>; 2],
}

impl RunningTranscription {
    pub fn stage(&self) -> Stage {
        self.progress.stage()
    }

    /// Progress of the current stage in percent, if it has any.
    pub fn percent(&self) -> Option<u8> {
        self.progress.percent()
    }
}

impl Drop for RunningTranscription {
    fn drop(&mut self) {
        // The transcription runs on its own thread, which dropping the task doesn't stop.
        self.progress.cancel();
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

    pub fn is_loaded(&self) -> bool {
        matches!(self.state, VideoState::Loaded(_))
    }

    /// The transcription's state, once the video is loaded.
    pub fn transcript(&self) -> Option<&TranscriptState> {
        match &self.state {
            VideoState::Loaded(loaded) => Some(&loaded.transcript),
            _ => None,
        }
    }

    pub fn is_transcribing(&self) -> bool {
        matches!(self.transcript(), Some(TranscriptState::Running(_)))
    }

    /// Whether its transcription is waiting to be started.
    pub fn is_queued(&self) -> bool {
        matches!(self.transcript(), Some(TranscriptState::Queued))
    }

    /// Whether it's queued for transcription, or will be once it's loaded.
    pub fn awaits_transcription(&self) -> bool {
        match &self.state {
            VideoState::Loading(loading) => loading.transcript.is_none(),
            _ => self.is_queued(),
        }
    }

    /// Stops playback.
    pub fn pause(&mut self, cx: &mut Context<Self>) {
        if let VideoState::Loaded(loaded) = &self.state
            && !loaded.video.paused()
        {
            loaded.video.set_paused(true);
            cx.notify();
        }
    }

    /// The video file that's open, loading, or failed to open.
    pub fn path(&self) -> Option<&Path> {
        match &self.state {
            VideoState::Loaded(loaded) => Some(&loaded.path),
            VideoState::Loading(loading) => Some(&loading.path),
            VideoState::Failed { path, .. } => Some(path),
            VideoState::None => None,
        }
    }

    /// The transcript, once the transcription is done, or the one given to `load` if
    /// it's still loading or couldn't be opened.
    pub fn finished_transcript(&self) -> Option<&[Segment]> {
        match &self.state {
            VideoState::Loaded(LoadedVideo {
                transcript: TranscriptState::Done(segments),
                ..
            }) => Some(segments),
            VideoState::Loaded(_) | VideoState::None => None,
            VideoState::Loading(loading) => loading.transcript.as_deref(),
            VideoState::Failed { transcript, .. } => transcript.as_deref(),
        }
    }

    /// The transcript so far, including one that's still running.
    pub fn segments_so_far(&self) -> &[Segment] {
        match self.transcript() {
            Some(transcript) => transcript.segments(),
            None => self.finished_transcript().unwrap_or_default(),
        }
    }

    /// Loads the file again if it couldn't be opened, with the transcript it was given.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.state, VideoState::Failed { .. }) {
            return;
        }
        if let VideoState::Failed {
            path, transcript, ..
        } = std::mem::replace(&mut self.state, VideoState::None)
        {
            self.load(path, transcript, cx);
        }
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

    /// Starts loading the video at `path`, replacing the open one. Its speech is
    /// queued for transcription once it's loaded, unless its `transcript` is given.
    pub fn load(
        &mut self,
        path: PathBuf,
        transcript: Option<Vec<Segment>>,
        cx: &mut Context<Self>,
    ) {
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
                            let video = open_video(&uri)?;
                            // Start on the first frame; playback is started from the UI.
                            video.set_paused(true);
                            anyhow::Ok(video)
                        }
                    })
                    .await;

                this.update(app, |this, cx| {
                    let transcript = match &mut this.state {
                        VideoState::Loading(loading) => loading.transcript.take(),
                        _ => None,
                    };
                    let new_state = match result {
                        Ok(video) => VideoState::Loaded(LoadedVideo {
                            path,
                            video,
                            last_position: Cell::new(Duration::ZERO),
                            transcript: transcript
                                .map_or(TranscriptState::Queued, TranscriptState::Done),
                        }),
                        Err(err) => VideoState::Failed {
                            path,
                            error: format!("{err:#}"),
                            transcript,
                        },
                    };
                    this.set_state(new_state, cx);
                })
                .ok();
            }
        });

        self.set_state(
            VideoState::Loading(LoadingVideo {
                path,
                transcript,
                _task: task,
            }),
            cx,
        );
    }
}

impl VideoDocument {
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

impl VideoDocument {
    /// Queues the video for transcription if it isn't transcribed, e.g. after it was
    /// stopped or failed. A finished transcript is kept.
    pub fn queue_transcription(&mut self, cx: &mut Context<Self>) {
        if let VideoState::Loaded(loaded) = &self.state
            && matches!(
                loaded.transcript,
                TranscriptState::None | TranscriptState::Failed(_)
            )
        {
            self.set_transcript(TranscriptState::Queued, cx);
        }
    }

    /// Starts transcribing the video's speech if it's queued. The project calls this
    /// when it's the video's turn.
    pub fn start_transcription(&mut self, cx: &mut Context<Self>) {
        let VideoState::Loaded(loaded) = &mut self.state else {
            return;
        };
        if !matches!(loaded.transcript, TranscriptState::Queued) {
            return;
        }

        let progress = Arc::new(Progress::default());
        let (segment_sender, mut segment_receiver) = futures::channel::mpsc::unbounded();
        let (result_sender, result_receiver) = futures::channel::oneshot::channel();
        // It runs for minutes, so it gets its own thread rather than blocking one of
        // the executor's.
        std::thread::spawn({
            let path = loaded.path.clone();
            let progress = progress.clone();
            move || {
                let result = transcribe::transcribe(&path, progress, |segments| {
                    segment_sender.unbounded_send(segments).ok();
                });
                result_sender.send(result).ok()
            }
        });

        let result_task = cx.spawn(async move |this, app| {
            // Ends once the transcription is done, which drops the sender.
            while let Some(segments) = segment_receiver.next().await {
                let added = this.update(app, |this, cx| this.add_segments(segments, cx));
                if added.is_err() {
                    return;
                }
            }
            let transcript = match result_receiver.await {
                Ok(Ok(segments)) => TranscriptState::Done(segments),
                Ok(Err(err)) => TranscriptState::Failed(format!("{err:#}")),
                Err(_) => TranscriptState::Failed("transcription crashed".into()),
            };
            this.update(app, |this, cx| this.set_transcript(transcript, cx))
                .ok();
        });
        // Redraws regularly so the progress shown stays current.
        let progress_task = cx.spawn(async move |this, app| {
            loop {
                app.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if this.update(app, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        });

        self.set_transcript(
            TranscriptState::Running(RunningTranscription {
                progress,
                segments: Vec::new(),
                _tasks: [result_task, progress_task],
            }),
            cx,
        );
    }

    /// Stops a running transcription, or takes the video out of the queue.
    pub fn cancel_transcription(&mut self, cx: &mut Context<Self>) {
        if let VideoState::Loaded(loaded) = &self.state
            && matches!(
                loaded.transcript,
                TranscriptState::Queued | TranscriptState::Running(_)
            )
        {
            self.set_transcript(TranscriptState::None, cx);
        }
    }

    /// Shows the segments of a running transcription as they come in. Parts of the
    /// video are transcribed at the same time, so they're put in place by time.
    fn add_segments(&mut self, segments: Vec<Segment>, cx: &mut Context<Self>) {
        if let VideoState::Loaded(loaded) = &mut self.state
            && let TranscriptState::Running(running) = &mut loaded.transcript
            && let Some(first) = segments.first()
        {
            let ix = running
                .segments
                .partition_point(|segment| segment.start <= first.start);
            running.segments.splice(ix..ix, segments);
            cx.notify();
        }
    }

    fn set_transcript(&mut self, transcript: TranscriptState, cx: &mut Context<Self>) {
        if let VideoState::Loaded(loaded) = &mut self.state {
            loaded.transcript = transcript;
            cx.notify();
        }
    }
}

impl TranscriptState {
    /// The segments transcribed so far, in order.
    pub fn segments(&self) -> &[Segment] {
        match self {
            TranscriptState::Running(running) => &running.segments,
            TranscriptState::Done(segments) => segments,
            TranscriptState::None | TranscriptState::Queued | TranscriptState::Failed(_) => &[],
        }
    }

    /// The segment being spoken at `time`, if any.
    pub fn segment_at(&self, time: Duration) -> Option<&Segment> {
        let segments = self.segments();
        let ix = segments.partition_point(|segment| segment.start <= time);
        segments[..ix].last().filter(|segment| time < segment.end)
    }
}

/// Opens the video at `uri` for playback. It's the pipeline `Video::new` builds, with
/// `scaletempo` added so the speech keeps its pitch at other playback speeds.
fn open_video(uri: &Url) -> anyhow::Result<Video> {
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
