//! Decoding a video's audio track into samples Whisper takes, with GStreamer.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context as _, bail};
use gpui_video_player::Url;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;

use super::progress::Progress;
use super::{SAMPLE_RATE, to_samples};

/// How far decoding may run ahead of the transcription, bounding the memory used.
const DECODE_AHEAD: Duration = Duration::from_secs(360);

/// Decodes the audio track of a file into 16 kHz mono samples, as fast as GStreamer
/// can decode it, but at most `DECODE_AHEAD` ahead of what's been read.
pub(super) struct AudioDecoder {
    pipeline: gstreamer::Pipeline,
    sink: AppSink,
    /// Number of samples read so far.
    received: usize,
    /// Where in the audio the first sample read is, from its timestamp, if it has one.
    pub(super) start: Option<Duration>,
    /// Set once all of the audio is read.
    pub(super) finished: bool,
}

impl AudioDecoder {
    /// Opens the audio track of the file at `path`, ready to `play`.
    pub(super) fn open(path: &Path, progress: &Progress) -> anyhow::Result<Self> {
        gstreamer::init()?;
        let uri = Url::from_file_path(path).map_err(|()| anyhow::anyhow!("invalid path"))?;

        // Bounded by bytes, not `max-time`: after seeking into the middle of a file,
        // appsink sometimes counts its empty queue as full and blocks the decoding for
        // good.
        let sink_bin = gstreamer::parse::bin_from_description(
            &format!(
                "audioconvert ! audioresample ! \
                 audio/x-raw,format=F32LE,channels=1,rate={SAMPLE_RATE} ! \
                 appsink name=sink sync=false max-bytes={}",
                to_samples(DECODE_AHEAD) * size_of::<f32>()
            ),
            true,
        )?;
        let sink = sink_bin
            .by_name("sink")
            .and_then(|sink| sink.downcast::<AppSink>().ok())
            .context("audio sink missing")?;
        let sink_pad = sink_bin
            .static_pad("sink")
            .context("audio sink has no pad")?;

        let pipeline = gstreamer::Pipeline::new();
        let source = gstreamer::ElementFactory::make("uridecodebin")
            .property("uri", uri.as_str())
            .build()?;
        // Leaves the video undecoded. Decoding a lecture's video only to drop it takes
        // far longer than decoding its audio. The handler must return a
        // value, or glib panics on the streaming thread, so the result type is checked
        // here and anything unexpected falls back to plugging the element.
        let select_result = gstreamer::glib::Type::from_name("GstAutoplugSelectResult")
            .filter(|&ty| {
                gstreamer::glib::EnumClass::with_type(ty)
                    .is_some_and(|class| class.to_value(0).is_some() && class.to_value(1).is_some())
            })
            .context("GstAutoplugSelectResult missing")?;
        source.connect("autoplug-select", false, move |values| {
            let video_decoder = values
                .get(3)
                .and_then(|value| value.get::<gstreamer::ElementFactory>().ok())
                .is_some_and(|factory| {
                    let klass = factory.metadata("klass").unwrap_or_default();
                    klass.contains("Decoder") && klass.contains("Video")
                });
            // EXPOSE leaves the stream as it is; TRY plugs the element.
            gstreamer::glib::EnumClass::with_type(select_result)?.to_value(if video_decoder {
                1
            } else {
                0
            })
        });
        source.connect_pad_added({
            let pipeline = pipeline.downgrade();
            let sink_pad = sink_pad.clone();
            move |_, pad| {
                let raw_audio = pad
                    .current_caps()
                    .and_then(|caps| Some(caps.structure(0)?.name().starts_with("audio/x-raw")))
                    .unwrap_or(false);
                let linked = raw_audio && !sink_pad.is_linked() && pad.link(&sink_pad).is_ok();
                if !linked && let Some(pipeline) = pipeline.upgrade() {
                    discard(&pipeline, pad);
                }
            }
        });
        let no_more_pads = Arc::new(AtomicBool::new(false));
        source.connect_no_more_pads({
            let no_more_pads = no_more_pads.clone();
            move |_| no_more_pads.store(true, Ordering::Relaxed)
        });
        pipeline.add_many([&source, sink_bin.upcast_ref()])?;

        let decoder = Self {
            pipeline,
            sink,
            received: 0,
            start: None,
            finished: false,
        };
        // Waits until the first audio is there, which is when the length is known and
        // the pipeline can seek.
        decoder
            .pipeline
            .set_state(gstreamer::State::Paused)
            .context("failed to open the audio")?;
        loop {
            if progress.is_cancelled() {
                bail!("cancelled");
            }
            decoder.check_error()?;
            if no_more_pads.load(Ordering::Relaxed) && !sink_pad.is_linked() {
                bail!("the video has no audio");
            }
            match decoder
                .pipeline
                .state(gstreamer::ClockTime::from_mseconds(100))
                .0
            {
                Ok(gstreamer::StateChangeSuccess::Async) => {}
                Ok(_) => return Ok(decoder),
                Err(_) => {
                    decoder.check_error()?;
                    bail!("failed to open the audio");
                }
            }
        }
    }

    /// Length of the audio, if GStreamer knows it.
    pub(super) fn duration(&self) -> Option<Duration> {
        self.pipeline
            .query_duration::<gstreamer::ClockTime>()
            .map(|duration| Duration::from_nanos(duration.nseconds()))
    }

    /// Starts decoding at `start`, until `end` or else the end.
    pub(super) fn play(&mut self, start: Duration, end: Option<Duration>) -> anyhow::Result<()> {
        let clock_time =
            |time: Duration| gstreamer::ClockTime::from_nseconds(time.as_nanos() as u64);
        if !start.is_zero() || end.is_some() {
            self.pipeline
                .seek(
                    1.,
                    gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::ACCURATE,
                    gstreamer::SeekType::Set,
                    Some(clock_time(start)),
                    match end {
                        Some(_) => gstreamer::SeekType::Set,
                        None => gstreamer::SeekType::None,
                    },
                    end.map(clock_time),
                )
                .context("failed to seek in the audio")?;
        }
        self.pipeline.set_state(gstreamer::State::Playing)?;
        Ok(())
    }

    /// Appends samples to `out` until it holds `wanted` or all of the audio is read,
    /// counting them in `decoded`.
    pub(super) fn read(
        &mut self,
        out: &mut Vec<f32>,
        wanted: usize,
        progress: &Progress,
        decoded: &AtomicUsize,
    ) -> anyhow::Result<()> {
        while out.len() < wanted && !self.finished {
            if progress.is_cancelled() {
                bail!("cancelled");
            }
            self.check_error()?;

            match self
                .sink
                .try_pull_sample(gstreamer::ClockTime::from_mseconds(100))
            {
                Some(sample) => {
                    let buffer = sample.buffer().context("audio sample has no buffer")?;
                    if self.received == 0 {
                        self.start = stream_time(&sample, buffer);
                    }
                    let map = buffer.map_readable()?;
                    let len = out.len();
                    out.extend(
                        map.chunks_exact(4)
                            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    );
                    self.received += out.len() - len;
                }
                None => self.finished = self.sink.is_eos(),
            }
            decoded.store(self.received, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Fails if GStreamer reported an error.
    fn check_error(&self) -> anyhow::Result<()> {
        if let Some(message) = self
            .pipeline
            .bus()
            .and_then(|bus| bus.pop_filtered(&[gstreamer::MessageType::Error]))
            && let gstreamer::MessageView::Error(err) = message.view()
        {
            bail!("failed to decode audio: {}", err.error());
        }
        Ok(())
    }
}

impl Drop for AudioDecoder {
    fn drop(&mut self) {
        self.pipeline.set_state(gstreamer::State::Null).ok();
    }
}

/// Where in the media `buffer` of `sample` is, as stream time: its timestamp with
/// the segment's start and any edit list's offset taken into account.
fn stream_time(sample: &gstreamer::Sample, buffer: &gstreamer::BufferRef) -> Option<Duration> {
    let pts = buffer.pts()?;
    let segment = sample.segment()?.downcast_ref::<gstreamer::ClockTime>()?;
    let time = segment.to_stream_time(pts)?;
    Some(Duration::from_nanos(time.nseconds()))
}

/// Sends what comes out of `pad`, a stream other than the audio, to a sink that drops
/// it. Left unlinked, it would stop the pipeline.
fn discard(pipeline: &gstreamer::Pipeline, pad: &gstreamer::Pad) {
    let Ok(sink) = gstreamer::ElementFactory::make("fakesink")
        .property("sync", false)
        .property("async", false)
        .build()
    else {
        return;
    };
    if pipeline.add(&sink).is_ok()
        && sink.sync_state_with_parent().is_ok()
        && let Some(sink_pad) = sink.static_pad("sink")
    {
        pad.link(&sink_pad).ok();
    }
}
