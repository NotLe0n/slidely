//! Decoding a video's audio track into samples Whisper takes, with FFmpeg.

use std::path::Path;
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context as _, bail};
use ffmpeg::ffi;
use ffmpeg::software::resampling;
use ffmpeg::{ChannelLayout, codec, format, frame, media};
use ffmpeg_next as ffmpeg;

use super::progress::Progress;
use super::{SAMPLE_RATE, to_samples, to_time};

/// Decodes the audio track of a file into 16 kHz mono samples, as they're read.
pub(super) struct AudioDecoder {
    input: format::context::Input,
    /// Index of the audio stream in `input`.
    stream: usize,
    /// Time base of the stream's timestamps, in seconds.
    time_base: f64,
    /// Timestamp in the stream where the file starts, in seconds. Timestamps are
    /// counted from it, as the player does, so the transcript lines up with playback.
    file_start: f64,
    decoder: codec::decoder::Audio,
    /// Converts the decoder's frames, made again if their format changes.
    resampler: Option<Resampler>,
    /// Where decoding starts and ends, in samples; set by `play`.
    from: usize,
    until: Option<usize>,
    /// Where in the audio the next sample converted is, in samples, once the first
    /// frame after `play` is decoded.
    position: Option<usize>,
    /// Converted samples not read yet.
    buffer: Vec<f32>,
    /// Set once the decoder gave up its last frame, or decoding reached `until`.
    drained: bool,
    /// Number of samples read so far.
    received: usize,
    /// Where in the audio the first sample read is, if it's known.
    pub(super) start: Option<Duration>,
    /// Set once all of the audio is read.
    pub(super) finished: bool,
}

struct Resampler {
    context: resampling::Context,
    /// What the frames it converts are like.
    input: FrameFormat,
}

/// The sample format, channels and rate of a frame. The channels are kept as their
/// count and mask, since a `ChannelLayout` may point into the frame.
#[derive(PartialEq)]
struct FrameFormat {
    format: format::Sample,
    channels: u16,
    mask: u64,
    rate: u32,
}

impl AudioDecoder {
    /// Opens the audio track of the file at `path`, ready to `play`.
    pub(super) fn open(path: &Path) -> anyhow::Result<Self> {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            ffmpeg::init().ok();
            // Damaged packets are skipped, so FFmpeg's complaints about them are noise.
            ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Fatal);
        });

        let mut input = format::input(path).context("failed to open the video")?;
        let stream = input
            .streams()
            .best(media::Type::Audio)
            .context("the video has no audio")?;
        let index = stream.index();
        let time_base = f64::from(stream.time_base());
        let decoder = codec::context::Context::from_parameters(stream.parameters())?
            .decoder()
            .audio()
            .context("failed to open the audio decoder")?;
        // Leaves the other streams unread. Demuxing a lecture's video only to drop
        // it takes far longer than decoding its audio.
        unsafe {
            let context = input.as_mut_ptr();
            for ix in 0..(*context).nb_streams as usize {
                if ix != index {
                    (**(*context).streams.add(ix)).discard = ffi::AVDiscard::AVDISCARD_ALL;
                }
            }
        }
        let file_start = match unsafe { (*input.as_ptr()).start_time } {
            ffi::AV_NOPTS_VALUE => 0.,
            start => start as f64 / f64::from(ffi::AV_TIME_BASE),
        };

        Ok(Self {
            input,
            stream: index,
            time_base,
            file_start,
            decoder,
            resampler: None,
            from: 0,
            until: None,
            position: None,
            buffer: Vec::new(),
            drained: false,
            received: 0,
            start: None,
            finished: false,
        })
    }

    /// Length of the audio, if the file says.
    pub(super) fn duration(&self) -> Option<Duration> {
        let duration = self.input.duration();
        (duration > 0).then(|| Duration::from_micros(duration as u64))
    }

    /// Starts decoding at `start`, until `end` or else the end.
    pub(super) fn play(&mut self, start: Duration, end: Option<Duration>) -> anyhow::Result<()> {
        if !start.is_zero() {
            // Lands on the last keyframe before `start`; what's before it is dropped
            // once it's decoded.
            let target = (start.as_secs_f64() + self.file_start) * f64::from(ffi::AV_TIME_BASE);
            self.input
                .seek(target as i64, ..target as i64)
                .context("failed to seek in the audio")?;
            self.decoder.flush();
        }
        self.from = to_samples(start);
        self.until = end.map(to_samples);
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
            if self.buffer.is_empty() {
                if self.drained {
                    self.finished = true;
                } else {
                    self.decode()?;
                }
                continue;
            }
            let take = self.buffer.len().min(wanted - out.len());
            out.extend(self.buffer.drain(..take));
            self.received += take;
            decoded.store(self.received, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Decodes the next frame into `buffer`, or sets `drained` at the end.
    fn decode(&mut self) -> anyhow::Result<()> {
        let mut frame = frame::Audio::empty();
        loop {
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => return self.convert(&frame),
                Err(ffmpeg::Error::Eof) => {
                    self.drained = true;
                    return Ok(());
                }
                Err(ffmpeg::Error::Other {
                    errno: ffmpeg::error::EAGAIN,
                }) => self.send_packet()?,
                Err(err) => return Err(err).context("failed to decode audio"),
            }
        }
    }

    /// Passes the stream's next packet to the decoder, or tells it the stream ended.
    fn send_packet(&mut self) -> anyhow::Result<()> {
        let mut packet = codec::packet::Packet::empty();
        loop {
            match packet.read(&mut self.input) {
                Ok(()) if packet.stream() != self.stream => {}
                Ok(()) => {
                    return match self.decoder.send_packet(&packet) {
                        // A damaged packet only loses a moment of audio.
                        Ok(()) | Err(ffmpeg::Error::InvalidData) => Ok(()),
                        Err(err) => Err(err).context("failed to decode audio"),
                    };
                }
                Err(ffmpeg::Error::Eof) => {
                    self.decoder.send_eof().ok();
                    return Ok(());
                }
                Err(ffmpeg::Error::InvalidData) => {}
                Err(err) => return Err(err).context("failed to read the audio"),
            }
        }
    }

    /// Converts `frame` to 16 kHz mono and adds what's between `from` and `until` to
    /// `buffer`.
    fn convert(&mut self, frame: &frame::Audio) -> anyhow::Result<()> {
        let position = *self.position.get_or_insert_with(|| {
            let time = frame.timestamp().unwrap_or(0) as f64 * self.time_base - self.file_start;
            (time.max(0.) * f64::from(SAMPLE_RATE)) as usize
        });
        let resampler = self.resampler(frame)?;
        let mut samples = resampler.run(frame)?;

        let skip = self.from.saturating_sub(position).min(samples.len());
        samples.drain(..skip);
        let first = position + skip;
        if let Some(until) = self.until
            && first + samples.len() >= until
        {
            samples.truncate(until.saturating_sub(first));
            self.drained = true;
        }
        self.position = Some(first + samples.len());
        if !samples.is_empty() && self.start.is_none() {
            self.start = Some(to_time(first));
        }
        self.buffer.extend(samples);
        Ok(())
    }

    /// The resampler for frames like `frame`.
    fn resampler(&mut self, frame: &frame::Audio) -> anyhow::Result<&mut Resampler> {
        let mut layout = frame.channel_layout();
        if layout.is_empty() {
            layout = ChannelLayout::default(i32::from(frame.channels()));
        }
        let input = FrameFormat {
            format: frame.format(),
            channels: frame.channels(),
            mask: layout.bits(),
            rate: frame.rate(),
        };
        if self
            .resampler
            .as_ref()
            .is_none_or(|resampler| resampler.input != input)
        {
            let context = resampling::Context::get(
                input.format,
                layout,
                input.rate,
                format::Sample::F32(format::sample::Type::Packed),
                ChannelLayout::MONO,
                SAMPLE_RATE,
            )
            .context("failed to convert the audio")?;
            self.resampler = Some(Resampler { context, input });
        }
        Ok(self.resampler.as_mut().unwrap())
    }
}

impl Resampler {
    /// Converts `frame`, returning the samples that are ready.
    fn run(&mut self, frame: &frame::Audio) -> anyhow::Result<Vec<f32>> {
        unsafe {
            let context = self.context.as_mut_ptr();
            let capacity = ffi::swr_get_out_samples(context, frame.samples() as i32).max(0);
            let mut samples = vec![0f32; capacity as usize];
            let out = [samples.as_mut_ptr().cast::<u8>()];
            let converted = ffi::swr_convert(
                context,
                out.as_ptr(),
                capacity,
                (*frame.as_ptr()).extended_data as *const *const u8,
                frame.samples() as i32,
            );
            if converted < 0 {
                bail!("failed to convert the audio");
            }
            samples.truncate(converted as usize);
            Ok(samples)
        }
    }
}
