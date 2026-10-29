//! Loading video files for playback. Nothing in here renders UI.

mod playback;
mod transcribe;
mod transcript;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{Context, Task};
use gpui_video_player::{Url, Video};

use playback::open_video;
pub use transcribe::{Segment, Stage};
pub use transcript::{RunningTranscription, TranscriptState};

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
