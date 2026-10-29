//! A video's transcript: queueing and running its transcription, and looking up
//! what's said when.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use gpui_kit::{Context, Task};

use super::transcribe::{self, Progress, Segment, Stage};
use super::{VideoDocument, VideoState};

pub enum TranscriptState {
    None,
    /// Waiting for the transcriptions of other videos to finish
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
