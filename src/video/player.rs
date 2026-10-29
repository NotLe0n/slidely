//! Playing a video with libmpv: mpv decodes it, plays its sound and keeps the two in
//! sync, and draws its frames into memory, from where the UI shows them. Nothing in
//! here renders UI.

use std::ffi::{CStr, c_int, c_void};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context as _, bail};
use futures::channel::mpsc;
use libmpv2::events::{Event, PropertyData};
use libmpv2::{Format, Mpv};
use libmpv2_sys as sys;

/// How long opening a file may take before it's given up on.
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

/// A frame mpv rendered, waiting to be shown.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// BGRA, as `gpui::RenderImage` takes it.
    pub pixels: Vec<u8>,
}

/// A video playing in mpv. Clones share the same playback, which stops once the last
/// is dropped.
#[derive(Clone)]
pub struct Player {
    inner: Arc<Inner>,
}

struct Inner {
    mpv: Arc<Mpv>,
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

/// State shared between the player, its threads and the UI.
struct Shared {
    /// What mpv last said about the playback, or what it was last told.
    state: Mutex<State>,
    /// The newest frame, until the UI takes it.
    frame: Mutex<Option<Frame>>,
    render: Mutex<RenderRequest>,
    /// Wakes the render thread when `render` changed.
    render_changed: Condvar,
    /// Tells the UI that something changed; it's only a wakeup, so it's never queued
    /// more than once.
    changed: Mutex<mpsc::Sender<()>>,
    stop: AtomicBool,
}

#[derive(Default)]
struct State {
    position: Duration,
    duration: Duration,
    paused: bool,
    eof: bool,
    /// From 0 to 1.
    volume: f64,
    muted: bool,
    speed: f64,
    /// The video's size as it's displayed, with its aspect ratio applied.
    video_size: Option<(u32, u32)>,
}

/// What the render thread should do.
#[derive(Default)]
struct RenderRequest {
    /// mpv has a new frame.
    frame_ready: bool,
    /// The largest size frames may be rendered at, in device pixels; set by the UI.
    max_size: Option<(u32, u32)>,
    /// The size the last frame was rendered at, so it's rendered again when that
    /// changes.
    rendered_size: Option<(u32, u32)>,
}

impl Player {
    /// Opens the video at `path`, paused on its first frame. Blocks until it's open or
    /// fails, so it should run on a background thread. `changed` is woken whenever the
    /// playback changes or a new frame is ready.
    pub fn open(path: &Path, changed: mpsc::Sender<()>) -> anyhow::Result<Self> {
        // libmpv refuses to start unless numbers are formatted the C way.
        #[cfg(unix)]
        unsafe {
            libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
        }
        let mpv = Mpv::with_initializer(|init| {
            // Frames go to the render context, rather than a window of mpv's own.
            init.set_option("vo", "libmpv")?;
            // Stays on the last frame at the end, rather than unloading the file.
            init.set_option("keep-open", "yes")?;
            init.set_option("pause", true)?;
            // Decodes on the GPU where that's known to work, copying the frames back
            // for the software renderer.
            init.set_option("hwdec", "auto-copy-safe")?;
            init.set_option("audio-client-name", "slidely")?;
            init.set_option("ytdl", false)?;
            Ok(())
        })
        .map_err(|err| mpv_error(err).context("failed to start mpv"))?;
        let mpv = Arc::new(mpv);

        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                paused: true,
                volume: 1.,
                speed: 1.,
                ..State::default()
            }),
            frame: Mutex::new(None),
            render: Mutex::new(RenderRequest::default()),
            render_changed: Condvar::new(),
            changed: Mutex::new(changed),
            stop: AtomicBool::new(false),
        });

        // The render context has to exist before the file is loaded, or mpv plays it
        // without video.
        let render = RenderContext::new(&mpv)?;
        render.set_update_callback(&shared);
        let render_thread = std::thread::Builder::new()
            .name("mpv render".into())
            .spawn({
                let shared = shared.clone();
                move || render_loop(render, &shared)
            })?;
        let mut inner = Inner {
            mpv: mpv.clone(),
            shared: shared.clone(),
            threads: vec![render_thread],
        };

        for (ix, (name, format)) in OBSERVED.iter().enumerate() {
            mpv.observe_property(name, *format, ix as u64)
                .map_err(|err| mpv_error(err).context(format!("failed to observe {name}")))?;
        }
        let path = path.to_str().context("the path isn't valid UTF-8")?;
        mpv.command("loadfile", &[path])
            .map_err(|err| mpv_error(err).context("failed to open the video"))?;
        wait_until_loaded(&mpv, &shared).context("failed to open the video")?;

        inner.threads.push(
            std::thread::Builder::new()
                .name("mpv events".into())
                .spawn(move || event_loop(&mpv, &shared))?,
        );
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        lock(&self.inner.shared.state)
    }

    pub fn position(&self) -> Duration {
        self.state().position
    }

    /// Length of the video, or zero until it's known.
    pub fn duration(&self) -> Duration {
        self.state().duration
    }

    pub fn paused(&self) -> bool {
        self.state().paused
    }

    pub fn set_paused(&self, paused: bool) {
        self.state().paused = paused;
        self.set("pause", paused);
    }

    /// Whether playback reached the end.
    pub fn eos(&self) -> bool {
        self.state().eof
    }

    /// Jumps to `time`, to exactly that frame.
    pub fn seek(&self, time: Duration) -> anyhow::Result<()> {
        let mut state = self.state();
        state.position = time;
        state.eof = false;
        drop(state);
        self.inner
            .mpv
            .command(
                "seek",
                &[&format!("{:.6}", time.as_secs_f64()), "absolute+exact"],
            )
            .map_err(mpv_error)
    }

    /// From 0 to 1.
    pub fn volume(&self) -> f64 {
        self.state().volume
    }

    pub fn set_volume(&self, volume: f64) {
        self.state().volume = volume;
        self.set("volume", volume * 100.);
    }

    pub fn muted(&self) -> bool {
        self.state().muted
    }

    pub fn set_muted(&self, muted: bool) {
        self.state().muted = muted;
        self.set("mute", muted);
    }

    pub fn speed(&self) -> f64 {
        self.state().speed
    }

    /// Plays at `speed` times the normal speed. mpv keeps the pitch of the speech.
    pub fn set_speed(&self, speed: f64) -> anyhow::Result<()> {
        self.inner
            .mpv
            .set_property("speed", speed)
            .map_err(mpv_error)?;
        self.state().speed = speed;
        Ok(())
    }

    /// Takes the newest frame, if one was rendered since the last call.
    pub fn take_frame(&self) -> Option<Frame> {
        lock(&self.inner.shared.frame).take()
    }

    /// Renders frames at most `width` × `height` device pixels, the size they're shown
    /// at. Smaller frames are cheaper to render, and mpv scales them down better than
    /// the GPU would.
    pub fn set_max_size(&self, width: u32, height: u32) {
        let shared = &self.inner.shared;
        let mut render = lock(&shared.render);
        if render.max_size != Some((width, height)) {
            render.max_size = Some((width, height));
            shared.render_changed.notify_all();
        }
    }

    fn set<T: libmpv2::SetData>(&self, name: &str, value: T) {
        if let Err(err) = self.inner.mpv.set_property(name, value) {
            eprintln!("Failed to set {name}: {:#}", mpv_error(err));
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.wake_render();
        unsafe { sys::mpv_wakeup(self.mpv.ctx.as_ptr()) };
        // The render context is freed on its thread, and has to be before mpv is.
        for thread in self.threads.drain(..) {
            thread.join().ok();
        }
    }
}

/// The properties the event thread keeps `State` up to date with, in the order of
/// their ids.
const OBSERVED: &[(&str, Format)] = &[
    ("time-pos", Format::Double),
    ("duration", Format::Double),
    ("pause", Format::Flag),
    ("eof-reached", Format::Flag),
    ("volume", Format::Double),
    ("mute", Format::Flag),
    ("speed", Format::Double),
    ("dwidth", Format::Int64),
    ("dheight", Format::Int64),
];

/// Waits for mpv to load the file it was told to, or to fail.
fn wait_until_loaded(mpv: &Mpv, shared: &Shared) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + OPEN_TIMEOUT;
    while std::time::Instant::now() < deadline {
        match mpv.wait_event(0.1) {
            Some(Ok(Event::FileLoaded)) => return Ok(()),
            Some(Ok(Event::EndFile(_))) => bail!("mpv can't play the file"),
            Some(Ok(Event::PropertyChange { name, change, .. })) => {
                apply(&mut lock(&shared.state), name, change);
            }
            Some(Err(err)) => return Err(mpv_error(err)),
            _ => {}
        }
    }
    bail!("opening the video timed out")
}

/// Keeps `State` up to date with mpv until the player is dropped.
fn event_loop(mpv: &Mpv, shared: &Shared) {
    while !shared.stop.load(Ordering::Relaxed) {
        match mpv.wait_event(-1.) {
            Some(Ok(Event::PropertyChange { name, change, .. })) => {
                let shown = apply(&mut lock(&shared.state), name, change);
                if name == "dwidth" || name == "dheight" {
                    // Frames can only be rendered once their size is known.
                    shared.wake_render();
                }
                if shown {
                    shared.notify();
                }
            }
            Some(Ok(Event::Shutdown)) => return,
            Some(Err(err)) => eprintln!("mpv: {:#}", mpv_error(err)),
            _ => {}
        }
    }
}

/// Applies a change to an observed property, and says whether the UI shows it.
fn apply(state: &mut State, name: &str, change: PropertyData) -> bool {
    let seconds = |value: f64| Duration::from_secs_f64(value.max(0.));
    match (name, change) {
        ("time-pos", PropertyData::Double(value)) => {
            let position = seconds(value);
            // The UI only shows whole seconds, so it's only told when those change.
            let changed = position.as_secs() != state.position.as_secs();
            state.position = position;
            return changed;
        }
        ("duration", PropertyData::Double(value)) => state.duration = seconds(value),
        ("pause", PropertyData::Flag(value)) => state.paused = value,
        ("eof-reached", PropertyData::Flag(value)) => state.eof = value,
        ("volume", PropertyData::Double(value)) => state.volume = value / 100.,
        ("mute", PropertyData::Flag(value)) => state.muted = value,
        ("speed", PropertyData::Double(value)) => state.speed = value,
        ("dwidth", PropertyData::Int64(value)) => {
            let height = state.video_size.map_or(0, |size| size.1);
            state.video_size = Some((value.max(0) as u32, height));
        }
        ("dheight", PropertyData::Int64(value)) => {
            let width = state.video_size.map_or(0, |size| size.0);
            state.video_size = Some((width, value.max(0) as u32));
        }
        _ => return false,
    }
    true
}

impl Shared {
    fn wake_render(&self) {
        // Taken so the render thread can't miss the wakeup between checking what to
        // do and waiting.
        let _render = lock(&self.render);
        self.render_changed.notify_all();
    }

    fn notify(&self) {
        // Fails when a wakeup is already pending, which is just as good.
        lock(&self.changed).try_send(()).ok();
    }
}

/// Renders a frame whenever mpv has one, or the size it's shown at changes, until the
/// player is dropped.
fn render_loop(context: RenderContext, shared: &Shared) {
    let mut buffer = FrameBuffer::default();
    loop {
        let size = {
            let mut render = lock(&shared.render);
            loop {
                if shared.stop.load(Ordering::Relaxed) {
                    drop(render);
                    // Frees the render context before mpv is destroyed.
                    drop(context);
                    return;
                }
                // Waits with a new frame until its size is known. Once one is shown,
                // it's rendered again when the size changes.
                let size = frame_size(&lock(&shared.state), render.max_size);
                let resized = render.rendered_size.is_some() && size != render.rendered_size;
                if let Some(size) = size
                    && (render.frame_ready || resized)
                {
                    render.frame_ready = false;
                    render.rendered_size = Some(size);
                    break size;
                }
                render = shared
                    .render_changed
                    .wait(render)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };

        // Has to be called after every update callback, whether or not it renders.
        let has_frame = context.update();
        let (width, height) = size;
        if !has_frame && lock(&shared.frame).is_some() {
            // Not shown yet, so it isn't rendered again at the new size.
            continue;
        }
        match context.render(&mut buffer, width, height) {
            Ok(frame) => {
                *lock(&shared.frame) = Some(frame);
                shared.notify();
            }
            Err(err) => eprintln!("Failed to render video: {err:#}"),
        }
    }
}

/// The size to render frames at: the video's, scaled down to fit into `max`, or
/// `None` while it isn't known.
fn frame_size(state: &State, max: Option<(u32, u32)>) -> Option<(u32, u32)> {
    let (width, height) = state.video_size.filter(|&(w, h)| w > 0 && h > 0)?;
    let (max_width, max_height) = max.unwrap_or((width, height));
    let scale = (max_width as f64 / width as f64)
        .min(max_height as f64 / height as f64)
        .min(1.);
    let scaled = |n: u32| ((n as f64 * scale).round() as u32).max(1);
    Some((scaled(width), scaled(height)))
}

/// mpv's software renderer.
struct RenderContext(*mut sys::mpv_render_context);

// mpv's render API may be used from any thread, though only one at a time, which
// the render thread owning it ensures.
unsafe impl Send for RenderContext {}

impl RenderContext {
    fn new(mpv: &Mpv) -> anyhow::Result<Self> {
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                data: sys::MPV_RENDER_API_TYPE_SW.as_ptr() as *mut c_void,
            },
            END_PARAM,
        ];
        let mut context = ptr::null_mut();
        let result = unsafe {
            sys::mpv_render_context_create(&mut context, mpv.ctx.as_ptr(), params.as_mut_ptr())
        };
        check(result).context("failed to set up video rendering")?;
        Ok(Self(context))
    }

    /// Has mpv set `frame_ready` and wake the render thread when there's a new frame.
    fn set_update_callback(&self, shared: &Arc<Shared>) {
        unsafe extern "C" fn on_update(shared: *mut c_void) {
            let shared = unsafe { &*(shared as *const Shared) };
            let mut render = lock(&shared.render);
            render.frame_ready = true;
            shared.render_changed.notify_all();
        }
        // The render thread holds a reference to `shared` until it frees the context,
        // after which mpv no longer calls back.
        unsafe {
            sys::mpv_render_context_set_update_callback(
                self.0,
                Some(on_update),
                Arc::as_ptr(shared) as *mut c_void,
            );
        }
    }

    /// Says whether there's a new frame to render.
    fn update(&self) -> bool {
        let flags = unsafe { sys::mpv_render_context_update(self.0) };
        flags & sys::mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME as u64 != 0
    }

    /// Renders the current frame at `width` × `height`, using `buffer` for mpv to draw
    /// into.
    fn render(&self, buffer: &mut FrameBuffer, width: u32, height: u32) -> anyhow::Result<Frame> {
        // mpv draws fastest when each row starts at a multiple of 64 bytes.
        let stride = (width as usize * 4).next_multiple_of(64);
        let data = buffer.get(stride * height as usize);
        let mut size: [c_int; 2] = [width as c_int, height as c_int];
        let mut stride_param = stride;
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_SIZE,
                data: size.as_mut_ptr().cast(),
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_FORMAT,
                data: SW_FORMAT.as_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_STRIDE,
                data: (&mut stride_param as *mut usize).cast(),
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_POINTER,
                data: data.as_mut_ptr().cast(),
            },
            END_PARAM,
        ];
        check(unsafe { sys::mpv_render_context_render(self.0, params.as_mut_ptr()) })?;

        // Packs the rows together, and makes the pixels opaque: the fourth byte of
        // "bgr0" is garbage.
        let row = width as usize * 4;
        let mut pixels = Vec::with_capacity(row * height as usize);
        for line in data.chunks_exact(stride) {
            pixels.extend_from_slice(&line[..row]);
        }
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel[3] = 0xff;
        }
        Ok(Frame {
            width,
            height,
            pixels,
        })
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        unsafe { sys::mpv_render_context_free(self.0) };
    }
}

/// The pixel format mpv renders to: BGRA, with the alpha left unset.
const SW_FORMAT: &CStr = c"bgr0";

const END_PARAM: sys::mpv_render_param = sys::mpv_render_param {
    type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
    data: ptr::null_mut(),
};

/// Memory for mpv to render into, aligned for its SIMD code.
#[derive(Default)]
struct FrameBuffer(Vec<Aligned>);

#[derive(Clone, Copy)]
#[repr(C, align(64))]
struct Aligned([u8; 64]);

impl FrameBuffer {
    /// At least `len` bytes, starting at a multiple of 64.
    fn get(&mut self, len: usize) -> &mut [u8] {
        let blocks = len.div_ceil(64);
        if self.0.len() < blocks {
            self.0.resize(blocks, Aligned([0; 64]));
        }
        unsafe { std::slice::from_raw_parts_mut(self.0.as_mut_ptr().cast(), len) }
    }
}

/// `err` with mpv's description of it.
fn mpv_error(err: libmpv2::Error) -> anyhow::Error {
    match err {
        libmpv2::Error::Raw(code) => anyhow::anyhow!("{}", sys::mpv_error_str(code)),
        err => anyhow::anyhow!("{err}"),
    }
}

fn check(result: c_int) -> anyhow::Result<()> {
    if result < 0 {
        bail!("{}", sys::mpv_error_str(result));
    }
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
