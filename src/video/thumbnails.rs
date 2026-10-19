use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::RenderImage;
use gpui_video_player::Url;
use gpui_video_player::gst;
use gpui_video_player::gst::prelude::*;

/// How long to wait for the pipeline to settle after starting or seeking.
const STATE_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(10);

/// Decodes a frame at each of `times` from the video at `uri`, scaled to `height`
/// pixels high (keeping `aspect_ratio`), calling `on_thumbnail` as each one finishes.
/// Uses its own pipeline, so it doesn't disturb playback. Blocking; run it on a
/// background thread. Stops early when `cancelled` is set.
pub fn render_thumbnails(
    uri: &Url,
    times: &[Duration],
    height: u32,
    aspect_ratio: f32,
    cancelled: &AtomicBool,
    on_thumbnail: impl Fn(usize, Arc<RenderImage>),
) -> anyhow::Result<()> {
    gst::init()?;

    let fakesink = || gst::ElementFactory::make("fakesink").build();
    let pipeline = gst::ElementFactory::make("playbin")
        .property("uri", uri.as_str())
        .property("video-sink", fakesink()?)
        .property("audio-sink", fakesink()?)
        .build()?;

    // Make sure the pipeline is torn down however this returns.
    struct StopOnDrop(gst::Element);
    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            let _ = self.0.set_state(gst::State::Null);
        }
    }
    let pipeline = StopOnDrop(pipeline);
    let pipeline = &pipeline.0;

    pipeline.set_state(gst::State::Paused)?;
    pipeline.state(STATE_TIMEOUT).0?;

    // Even width keeps the converters from padding rows.
    let width = ((height as f32 * aspect_ratio).round() as i32 / 2 * 2).max(2);
    let caps = gst::Caps::builder("video/x-raw")
        .field("format", "BGRA")
        .field("width", width)
        .field("height", height as i32)
        .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
        .build();

    for (ix, &time) in times.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        // Accurate seeks decode up to the exact frame instead of stopping at the
        // keyframe before it, so neighboring thumbnails don't repeat.
        pipeline.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::ClockTime::from_nseconds(time.as_nanos() as u64),
        )?;
        pipeline.state(STATE_TIMEOUT).0?;

        let sample = pipeline.emit_by_name::<Option<gst::Sample>>("convert-sample", &[&caps]);
        let Some(image) = sample.and_then(|sample| to_render_image(&sample)) else {
            continue;
        };
        on_thumbnail(ix, image);
    }
    Ok(())
}

fn to_render_image(sample: &gst::Sample) -> Option<Arc<RenderImage>> {
    let structure = sample.caps()?.structure(0)?;
    let width = structure.get::<i32>("width").ok()? as u32;
    let height = structure.get::<i32>("height").ok()? as u32;
    let map = sample.buffer()?.map_readable().ok()?;

    let buffer = image::RgbaImage::from_raw(width, height, map.as_slice().to_vec())?;
    Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}
