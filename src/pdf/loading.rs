use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::StreamExt as _;
use gpui_kit::{App, AppContext as _, Context, Entity, RenderImage, Task};
use rfd::AsyncFileDialog;

use super::PdfData;
use super::render::{RenderedPage, render_pages};

/// How long the render scale has to stay unchanged before pages are re-rendered,
/// so stepping through zoom levels doesn't start a render for each step.
const RERENDER_DELAY: Duration = Duration::from_millis(150);
/// Pages rendered ahead on each side of the visible ones, so scrolling a little
/// doesn't show blank pages.
const PRERENDER_PAGES: usize = 2;
/// Rendered pages further than this from the visible ones are released.
const KEEP_PAGES: usize = 6;

/// The open document, shared between the UI and the `OpenPdf` action.
pub struct PdfDocument {
    state: PdfState,
    /// Pixels per PDF point pages should be rendered at; set by the UI from the zoom
    /// level and the display's scale factor.
    render_scale: f32,
}

pub enum PdfState {
    Loaded(LoadedPdf),
    Loading(LoadingPdf),
    None,
}

pub struct LoadingPdf {
    pub path: PathBuf,
    /// Dropping it discards the load.
    _task: Task<()>,
}

pub struct LoadedPdf {
    pub data: PdfData,
    /// Rendered image of each page, if it's near the visible ones.
    pages: Vec<Option<RenderedPage>>,
    /// Pages currently on screen, as reported by the UI.
    visible: Range<usize>,
    render_job: Option<RenderJob>,
}

/// Background rendering. Dropping it cancels the work: the task is dropped, and the
/// flag makes the background threads skip pages they haven't started yet.
struct RenderJob {
    render_scale: f32,
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for RenderJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl LoadedPdf {
    fn new(data: PdfData) -> Self {
        Self {
            pages: data.page_sizes.iter().map(|_| None).collect(),
            data,
            visible: 0..0,
            render_job: None,
        }
    }

    /// The most recently rendered image of page `ix`. It may be from a previous zoom
    /// level while the page is being re-rendered.
    pub fn page_image(&self, ix: usize) -> Option<&Arc<RenderImage>> {
        Some(&self.pages.get(ix)?.as_ref()?.image)
    }

    /// The visible pages extended by `margin` on each side.
    fn around_visible(&self, margin: usize) -> Range<usize> {
        self.visible.start.saturating_sub(margin)..(self.visible.end + margin).min(self.pages.len())
    }
}

impl PdfDocument {
    pub fn new() -> Self {
        Self {
            state: PdfState::None,
            // 100% zoom on a 1x display; the UI sets the real value before first paint.
            render_scale: 96. / 72.,
        }
    }

    pub fn state(&self) -> &PdfState {
        &self.state
    }

    /// Closes the document, cancelling any load or rendering in progress.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.set_state(PdfState::None, cx);
    }

    /// Sets the resolution pages should be rendered at, in pixels per PDF point.
    /// Visible pages are re-rendered shortly after the scale stops changing.
    pub fn set_render_scale(&mut self, render_scale: f32, cx: &mut Context<Self>) {
        if same_scale(self.render_scale, render_scale) {
            return;
        }
        self.render_scale = render_scale;
        if let PdfState::Loaded(loaded) = &mut self.state
            && loaded
                .render_job
                .as_ref()
                .is_some_and(|job| !same_scale(job.render_scale, render_scale))
        {
            // Rendering at the old scale is wasted work now.
            loaded.render_job = None;
        }
        self.schedule_render(Some(RERENDER_DELAY), cx);
    }

    /// Tells the document which pages are on screen, so those (and their neighbors)
    /// get rendered and pages far away are released.
    pub fn set_visible_range(&mut self, visible: Range<usize>, cx: &mut Context<Self>) {
        let PdfState::Loaded(loaded) = &mut self.state else {
            return;
        };
        if loaded.visible == visible {
            return;
        }
        loaded.visible = visible;

        let keep = loaded.around_visible(KEEP_PAGES);
        for (ix, page) in loaded.pages.iter_mut().enumerate() {
            if !keep.contains(&ix)
                && let Some(page) = page.take()
            {
                cx.drop_image(page.image, None);
            }
        }
        self.schedule_render(None, cx);
    }

    fn set_state(&mut self, state: PdfState, cx: &mut Context<Self>) {
        // Free the old document's GPU textures; replacing the state drops (and so
        // cancels) a running load or render.
        if let PdfState::Loaded(loaded) = &mut self.state {
            for page in loaded.pages.iter_mut().filter_map(Option::take) {
                cx.drop_image(page.image, None);
            }
        }
        self.state = state;
        cx.notify();
    }

    fn start_loading(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let task = cx.spawn({
            let path = path.clone();
            async move |this, app| {
                let result = app
                    .background_executor()
                    .spawn(async move { PdfData::load(path) })
                    .await;

                let new_state = match result {
                    Ok(data) => PdfState::Loaded(LoadedPdf::new(data)),
                    Err(err) => {
                        eprintln!("Failed to open PDF: {err}");
                        PdfState::None
                    }
                };
                // Rendering starts once the UI reports which pages are visible.
                this.update(app, |this, cx| this.set_state(new_state, cx))
                    .ok();
            }
        });

        self.set_state(PdfState::Loading(LoadingPdf { path, _task: task }), cx);
    }

    /// Starts rendering the pages near the visible ones that don't have an image at
    /// the current scale yet, unless a render is already running; that one calls this
    /// again when it finishes, to catch up with changes made in the meantime.
    fn schedule_render(&mut self, delay: Option<Duration>, cx: &mut Context<Self>) {
        let render_scale = self.render_scale;
        let PdfState::Loaded(loaded) = &mut self.state else {
            return;
        };
        if loaded.render_job.is_some() {
            return;
        }

        let mut missing: Vec<usize> = loaded
            .around_visible(PRERENDER_PAGES)
            .filter(|&ix| {
                !loaded.pages[ix]
                    .as_ref()
                    .is_some_and(|page| same_scale(page.render_scale, render_scale))
            })
            .collect();
        if missing.is_empty() {
            return;
        }
        // Visible pages first.
        let visible = loaded.visible.clone();
        missing.sort_by_key(|&ix| !visible.contains(&ix));

        let pdf = loaded.data.pdf.clone();
        let cancelled = Arc::new(AtomicBool::new(false));

        let task = cx.spawn({
            let cancelled = cancelled.clone();
            async move |this, app| {
                if let Some(delay) = delay {
                    app.background_executor().timer(delay).await;
                }

                let (page_tx, mut page_rx) = futures::channel::mpsc::unbounded();
                let render = app.background_executor().spawn(async move {
                    render_pages(&pdf, &missing, render_scale, &cancelled, |ix, page| {
                        page_tx.unbounded_send((ix, page)).ok();
                    })
                });

                // Show each page as soon as it's done. The channel closes when
                // `render` finishes and drops the sender.
                while let Some((ix, page)) = page_rx.next().await {
                    let updated = this.update(app, |this, cx| this.insert_page(ix, page, cx));
                    if updated.is_err() {
                        return;
                    }
                }
                render.await;

                this.update(app, |this, cx| {
                    if let PdfState::Loaded(loaded) = &mut this.state {
                        loaded.render_job = None;
                    }
                    this.schedule_render(None, cx);
                })
                .ok();
            }
        });

        loaded.render_job = Some(RenderJob {
            render_scale,
            cancelled,
            _task: task,
        });
    }

    fn insert_page(&mut self, ix: usize, page: RenderedPage, cx: &mut Context<Self>) {
        // Replacing or closing the document drops the render job, so a page that
        // arrives here belongs to the loaded document.
        let PdfState::Loaded(loaded) = &mut self.state else {
            return;
        };
        if !loaded.around_visible(KEEP_PAGES).contains(&ix) {
            // Scrolled away while it was rendering.
            cx.drop_image(page.image, None);
            return;
        }
        if let Some(old) = loaded.pages[ix].replace(page) {
            cx.drop_image(old.image, None);
        }
        cx.notify();
    }
}

fn same_scale(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.001
}

/// Asks the user for a PDF file and starts loading it into `document`.
pub fn open_pdf(cx: &mut App, document: Entity<PdfDocument>) {
    cx.spawn(async move |app| {
        let selected_file = AsyncFileDialog::new()
            .add_filter("pdf", &["pdf"])
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

pub fn close_pdf(cx: &mut App, document: Entity<PdfDocument>) {
    cx.update_entity(&document, |document, cx| document.close(cx));
}
