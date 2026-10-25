use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::*;
// Not the component size (small, medium, ...) from the glob above.
use gpui_kit::Size;

use super::pdf_controls::{PdfControlsBar, PdfControlsBarEvent};
use super::placeholders::{empty_screen, file_name, page_image};
use crate::observe::ObservedDocuments;
use crate::pdf::{PX_PER_POINT, PdfDocument, PdfState};
use crate::project::{Project, Slides, add_pdfs};

pub const DEFAULT_ZOOM: f32 = 100.;
pub const MIN_ZOOM: f32 = 10.;
pub const MAX_ZOOM: f32 = 400.;
/// How far the zoom input's arrows and the Zoom In/Out actions change the zoom.
pub const ZOOM_STEP: f32 = 10.;

/// Vertical space around each page, split above and below it.
const PAGE_GAP: Pixels = px(16.);
/// Horizontal space on each side of the widest page.
const PAGE_MARGIN: Pixels = px(16.);
/// Zoom factor per scrolled line with Ctrl held. A wheel notch is usually 3 lines
/// (~12%); touchpads send fractions of a line, which zoom smoothly.
const ZOOM_PER_WHEEL_LINE: f32 = 1.04;
/// Converts pixel-based wheel deltas (touchpads) to lines.
const WHEEL_LINE_HEIGHT: Pixels = px(20.);

pub enum PdfViewEvent {
    /// The user scrolled to the page at this index of the project's slides, i.e. it's
    /// in the middle of the viewport now. Not emitted for `scroll_to_page`.
    PageChanged(usize),
}

/// The pages of all of the project's PDFs, one PDF after the other as if they were
/// one. Pages are in a virtual list so only the visible ones are laid out, and only
/// those (plus a few around them) are rendered by their documents.
pub struct PdfView {
    project: Entity<Project>,
    /// Redraw when one of the project's PDFs changes.
    pdfs: ObservedDocuments<PdfDocument>,
    controls: Entity<PdfControlsBar>,
    zoom_percent: f32,
    scroll_handle: VirtualListScrollHandle,
    /// The PDFs whose pages were shown last frame, in order. When that changes, the
    /// scroll offset is moved so the same page stays in view.
    shown_documents: Vec<Entity<PdfDocument>>,
    /// Where the top of the viewport was last frame.
    scroll_anchor: Option<ScrollAnchor>,
    /// The page in the middle of the viewport last frame.
    reported_page: Option<usize>,
    /// Set by `scroll_to_page` until the scroll is done, so the page it lands on
    /// isn't reported as the user scrolling there.
    scrolling_to_page: bool,
    /// The page `step_page` last went to, until the pages are scrolled elsewhere.
    /// Steps go on from it rather than from the page in the middle of the viewport,
    /// which is a later one when pages are short, or doesn't change at the end.
    stepped_to: Option<usize>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

/// A spot on a page, which stays put when pages before it are added or removed.
struct ScrollAnchor {
    document: Entity<PdfDocument>,
    /// The page's index in `document`.
    page: usize,
    /// How far down the page's item the spot is, from 0 to 1.
    fraction: f32,
}

impl PdfView {
    pub fn new(project: Entity<Project>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let controls = cx.new(|cx| PdfControlsBar::new(window, cx));

        let _subscriptions = vec![
            cx.observe(&project, |this, _, cx| {
                this.observe_documents(cx);
                cx.notify();
            }),
            cx.subscribe_in(&controls, window, Self::on_controls_event),
        ];

        let mut this = Self {
            project,
            pdfs: ObservedDocuments::new(),
            controls,
            zoom_percent: DEFAULT_ZOOM,
            scroll_handle: VirtualListScrollHandle::new(),
            shown_documents: Vec::new(),
            scroll_anchor: None,
            reported_page: None,
            scrolling_to_page: false,
            stepped_to: None,
            focus_handle: cx.focus_handle(),
            _subscriptions,
        };
        this.observe_documents(cx);
        this
    }

    /// Observes the project's current PDFs, after one was added or removed.
    fn observe_documents(&mut self, cx: &mut Context<Self>) {
        let pdfs = self.project.read(cx).pdfs().to_vec();
        self.pdfs.observe(&pdfs, cx, |_, cx| cx.notify());
    }

    fn slides(&self, cx: &App) -> Slides {
        self.project.read(cx).slides(cx)
    }

    /// Scrolls so page `ix` is at the top of the viewport.
    pub fn scroll_to_page(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.scroll_handle.scroll_to_item(ix, ScrollStrategy::Top);
        self.scrolling_to_page = true;
        self.stepped_to = None;
        cx.notify();
    }

    /// Scrolls `delta` pages forward or back from the current one. Unlike
    /// `scroll_to_page`, this emits `PageChanged`, since the user picked the page.
    pub fn step_page(&mut self, delta: isize, cx: &mut Context<Self>) {
        let page_count = self.slides(cx).sizes.len();
        let Some(current) = self.stepped_to.or_else(|| self.current_page(cx)) else {
            return;
        };
        let target = current
            .saturating_add_signed(delta)
            .min(page_count.saturating_sub(1));
        if target == current {
            return;
        }
        self.scroll_to_page(target, cx);
        self.stepped_to = Some(target);
        cx.emit(PdfViewEvent::PageChanged(target));
    }

    /// Emits `PageChanged` if the page in the middle of the viewport changed since
    /// last frame, other than by `scroll_to_page`. Called while the list is laid out,
    /// after it applied a scroll to a page.
    fn track_current_page(&mut self, cx: &mut Context<Self>) {
        let page = self.current_page(cx);
        let changed = page != self.reported_page;
        self.reported_page = page;
        if std::mem::take(&mut self.scrolling_to_page) {
            return;
        }
        if changed && let Some(page) = page {
            self.stepped_to = None;
            cx.emit(PdfViewEvent::PageChanged(page));
        }
    }

    pub fn zoom_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_around_center(self.zoom_percent + ZOOM_STEP, window, cx);
    }

    pub fn zoom_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_around_center(self.zoom_percent - ZOOM_STEP, window, cx);
    }

    pub fn reset_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_around_center(DEFAULT_ZOOM, window, cx);
    }

    /// Turns the page in the middle of the viewport 90° clockwise.
    pub fn rotate_page(&mut self, cx: &mut Context<Self>) {
        let Some(ix) = self.current_page(cx) else {
            return;
        };
        if let Some((document, page)) = self.slides(cx).locate(ix) {
            document.update(cx, |document, cx| document.rotate_page(page, cx));
        }
    }

    /// Zooms so the page in the middle of the viewport fits into it entirely, and
    /// scrolls that page to the top.
    pub fn fit_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.current_page(cx) else {
            return;
        };
        let page_sizes = self.slides(cx).sizes;
        self.zoom_percent = fit_zoom(page_sizes[ix], self.scroll_handle.bounds().size);
        let top = page_top(&page_sizes, self.display_scale(), ix);
        self.scroll_handle.set_offset(point(px(0.), -top));
        self.show_zoom(window, cx);
        cx.notify();
    }

    /// Zooms around the viewport's center and shows the new level in the controls.
    fn zoom_around_center(
        &mut self,
        zoom_percent: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let center = self.scroll_handle.bounds().size.center();
        self.zoom_to(zoom_percent, center, cx);
        self.show_zoom(window, cx);
    }

    /// Zooms so the document point at `anchor` (relative to the viewport's top left)
    /// stays where it is on screen.
    fn zoom_to(&mut self, zoom_percent: f32, anchor: Point<Pixels>, cx: &mut Context<Self>) {
        let old_scale = self.display_scale();
        self.zoom_percent = zoom_percent.clamp(MIN_ZOOM, MAX_ZOOM);
        let new_scale = self.display_scale();

        // The list clamps the offset to the new content size when it lays out next.
        let offset = zoom_anchored_offset(
            &self.slides(cx).sizes,
            old_scale,
            new_scale,
            self.scroll_handle.offset(),
            self.scroll_handle.bounds().size,
            anchor,
        );
        self.scroll_handle.set_offset(offset);
        cx.notify();
    }

    fn zoom_by_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Scrolling up (positive) zooms in.
        let lines = event.delta.pixel_delta(WHEEL_LINE_HEIGHT).y / WHEEL_LINE_HEIGHT;
        let zoom_percent =
            (self.zoom_percent * ZOOM_PER_WHEEL_LINE.powf(lines)).clamp(MIN_ZOOM, MAX_ZOOM);
        if zoom_percent == self.zoom_percent {
            return;
        }
        let viewport = self.scroll_handle.bounds();
        self.zoom_to(zoom_percent, event.position - viewport.origin, cx);
        self.show_zoom(window, cx);
    }

    /// Shows the current zoom level in the controls' input.
    fn show_zoom(&self, window: &mut Window, cx: &mut Context<Self>) {
        let zoom_percent = self.zoom_percent;
        self.controls.update(cx, |controls, cx| {
            controls.show_zoom(zoom_percent, window, cx)
        });
    }

    fn on_controls_event(
        &mut self,
        _: &Entity<PdfControlsBar>,
        event: &PdfControlsBarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // Not shown back in the input, which already has it and is being typed in.
            PdfControlsBarEvent::ZoomChanged(zoom_percent) => {
                let center = self.scroll_handle.bounds().size.center();
                self.zoom_to(*zoom_percent, center, cx);
            }
            // Hand focus back to the pages once the zoom is entered.
            PdfControlsBarEvent::ZoomConfirmed => window.focus(&self.focus_handle, cx),
            PdfControlsBarEvent::FitPage => self.fit_page(window, cx),
            PdfControlsBarEvent::RotatePage => self.rotate_page(cx),
        }
    }

    /// The index of the page in the middle of the viewport.
    pub fn current_page(&self, cx: &App) -> Option<usize> {
        let viewport = self.scroll_handle.bounds().size;
        let center_y = -self.scroll_handle.offset().y + viewport.height / 2.;
        page_at(&self.slides(cx).sizes, self.display_scale(), center_y)
    }

    /// Keeps the page at the top of the viewport there when a PDF before it finished
    /// loading, or was removed or moved, which shifts the pages after it. Its PDF's
    /// pages are kept in view if that was removed.
    fn keep_scroll_position(&mut self, slides: &Slides) {
        let scale = self.display_scale();
        let documents: Vec<_> = slides
            .documents
            .iter()
            .map(|(document, _)| document.clone())
            .collect();
        if documents != self.shown_documents
            && let Some(anchor) = &self.scroll_anchor
        {
            // The anchor's PDF, or else the first one after it that's still shown.
            let top = self
                .shown_documents
                .iter()
                .skip_while(|document| **document != anchor.document)
                .enumerate()
                .find_map(|(n, document)| {
                    let (_, pages) = slides
                        .documents
                        .iter()
                        .find(|(shown, pages)| shown == document && !pages.is_empty())?;
                    let (ix, fraction) = if n == 0 {
                        (
                            (pages.start + anchor.page).min(pages.end - 1),
                            anchor.fraction,
                        )
                    } else {
                        (pages.start, 0.)
                    };
                    Some(
                        page_top(&slides.sizes, scale, ix)
                            + item_height(slides.sizes[ix], scale) * fraction,
                    )
                });
            if let Some(top) = top {
                let offset = self.scroll_handle.offset();
                self.scroll_handle.set_offset(point(offset.x, -top));
            }
        }
        self.shown_documents = documents;

        let y = -self.scroll_handle.offset().y;
        self.scroll_anchor = page_at(&slides.sizes, scale, y).and_then(|ix| {
            let (document, page) = slides.locate(ix)?;
            let fraction =
                (y - page_top(&slides.sizes, scale, ix)) / item_height(slides.sizes[ix], scale);
            Some(ScrollAnchor {
                document: document.clone(),
                page,
                fraction: fraction.clamp(0., 1.),
            })
        });
    }

    /// Display size per PDF point.
    fn display_scale(&self) -> f32 {
        self.zoom_percent / 100. * PX_PER_POINT
    }

    /// Tells each document which of its pages are visible, given the visible pages
    /// of `slides`, so it renders those.
    fn report_visible(&mut self, slides: &Slides, visible: Range<usize>, cx: &mut Context<Self>) {
        let documents: Vec<_> = slides
            .documents
            .iter()
            .map(|(document, pages)| {
                let start = visible.start.clamp(pages.start, pages.end);
                let end = visible.end.clamp(pages.start, pages.end);
                (document.clone(), start - pages.start..end - pages.start)
            })
            .collect();
        // Reported every frame; documents ignore ranges that didn't change. Entities
        // shouldn't be updated while the window is drawing, so it's applied right after.
        cx.defer(move |cx| {
            for (document, visible) in documents {
                document.update(cx, |document, cx| document.set_visible_range(visible, cx));
            }
        });
    }

    /// Tells the documents what resolution to render pages at, like `report_visible`.
    fn report_render_scale(&mut self, render_scale: f32, cx: &mut Context<Self>) {
        let documents = self.project.read(cx).pdfs().to_vec();
        cx.defer(move |cx| {
            for document in documents {
                document.update(cx, |document, cx| {
                    document.set_render_scale(render_scale, cx)
                });
            }
        });
    }

    /// The scrolling pages with the controls below them. `page_sizes` are in PDF
    /// points.
    fn pages(&self, page_sizes: &[Size<Pixels>], cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| window.focus(&this.focus_handle, cx)),
                    )
                    .child(self.page_list(page_sizes, cx))
                    .child(Scrollbar::new(&self.scroll_handle))
                    .child(self.ctrl_wheel_zoom(cx)),
            )
            .child(self.controls.clone())
    }

    fn page_list(&self, page_sizes: &[Size<Pixels>], cx: &mut Context<Self>) -> impl IntoElement {
        let scale = self.display_scale();
        let page_sizes: Vec<_> = page_sizes
            .iter()
            .map(|page| size(page.width * scale, page.height * scale))
            .collect();
        let page_count = page_sizes.len();
        let content_width = page_sizes
            .iter()
            .map(|page| page.width)
            .fold(px(0.), Pixels::max)
            + PAGE_MARGIN * 2.;

        // One item per page, plus a zero-height spacer at the end that the list
        // measures for its scrollable width. Measuring calls the item callback too;
        // with the spacer that call is the only one whose range starts past the pages.
        let item_sizes = Rc::new(
            page_sizes
                .iter()
                .map(|page| size(content_width, page.height + PAGE_GAP))
                .chain([size(content_width, px(0.))])
                .collect::<Vec<_>>(),
        );

        v_virtual_list(
            cx.entity(),
            "pdf-pages",
            item_sizes,
            move |this, range, _, cx| {
                let slides = this.slides(cx);
                if range.start < page_count {
                    let visible = range.start..range.end.min(page_count);
                    this.report_visible(&slides, visible, cx);
                    this.track_current_page(cx);
                }

                range
                    .map(|ix| {
                        // At least as wide as the viewport so pages stay centered, but
                        // as wide as the widest page so the list scrolls horizontally.
                        let item = h_flex()
                            .w_full()
                            .min_w(content_width)
                            .justify_center()
                            .items_center();
                        let Some(page_size) = page_sizes.get(ix) else {
                            return item.h(px(0.));
                        };
                        let image = slides.locate(ix).and_then(|(document, page)| {
                            match document.read(cx).state() {
                                PdfState::Loaded(pdf) => pdf.page_image(page).cloned(),
                                _ => None,
                            }
                        });
                        item.h(page_size.height + PAGE_GAP).child(
                            div()
                                .flex_shrink_0()
                                .w(page_size.width)
                                .h(page_size.height)
                                .child(page_image(image)),
                        )
                    })
                    .collect()
            },
        )
        .with_item_to_measure_index(page_count)
        .track_scroll(&self.scroll_handle)
        .size_full()
    }

    /// An overlay that makes Ctrl+scroll zoom instead of scrolling. The list handles
    /// the wheel itself before any parent's `on_scroll_wheel` would run, so this
    /// listens in the capture phase, which runs first, and stops the event there.
    fn ctrl_wheel_zoom(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture
                        && event.modifiers.control
                        && bounds.contains(&event.position)
                    {
                        view.update(cx, |this, cx| this.zoom_by_wheel(event, window, cx))
                            .ok();
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}

impl PdfView {
    /// Shown while there are no pages: there are no PDFs, they're still loading, or
    /// they couldn't be opened.
    fn no_pages(&self, cx: &mut Context<Self>) -> AnyElement {
        let project = self.project.clone();
        let add_pdfs_screen = |message: String, error: Option<&str>, cx: &App| {
            empty_screen(message, error, "add-pdfs", "Add PDFs", cx, move |cx| {
                add_pdfs(project.clone(), cx)
            })
            .into_any_element()
        };

        let pdfs = self.project.read(cx).pdfs();
        if pdfs.is_empty() {
            return add_pdfs_screen("No PDFs in the project".into(), None, cx);
        }
        let states: Vec<_> = pdfs.iter().map(|pdf| pdf.read(cx).state()).collect();
        if states
            .iter()
            .any(|state| matches!(state, PdfState::Loading(_)))
        {
            return h_flex()
                .gap_3()
                .items_center()
                .child(Spinner::new())
                .child("Opening PDFs…")
                .into_any_element();
        }
        let failed = states.iter().find_map(|state| match state {
            PdfState::Failed { path, error, .. } => Some((file_name(path), error.clone())),
            _ => None,
        });
        match failed {
            Some((name, error)) => {
                add_pdfs_screen(format!("Couldn't open {name}"), Some(&error), cx)
            }
            None => add_pdfs_screen("No PDFs in the project".into(), None, cx),
        }
    }
}

impl EventEmitter<PdfViewEvent> for PdfView {}

impl Render for PdfView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render pages at the resolution they're displayed at. Doing this from `render`
        // also catches the window moving to a display with another scale factor.
        let render_scale = self.display_scale() * window.scale_factor();
        self.report_render_scale(render_scale, cx);

        let slides = self.slides(cx);
        self.keep_scroll_position(&slides);
        let content = if slides.is_empty() {
            self.no_pages(cx)
        } else {
            self.pages(&slides.sizes, cx).into_any_element()
        };

        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(content)
    }
}

/// Height of a page's list item at `scale`: the page and the gap around it.
fn item_height(page: Size<Pixels>, scale: f32) -> Pixels {
    page.height * scale + PAGE_GAP
}

/// The index of the page whose item contains the document position `y` at `scale`,
/// or the last page if `y` is past them all.
fn page_at(page_sizes: &[Size<Pixels>], scale: f32, y: Pixels) -> Option<usize> {
    let mut bottom = px(0.);
    for (ix, page) in page_sizes.iter().enumerate() {
        bottom += item_height(*page, scale);
        if y < bottom {
            return Some(ix);
        }
    }
    page_sizes.len().checked_sub(1)
}

/// The document position of the top of page `ix`'s item at `scale`.
fn page_top(page_sizes: &[Size<Pixels>], scale: f32, ix: usize) -> Pixels {
    page_sizes[..ix]
        .iter()
        .fold(px(0.), |top, page| top + item_height(*page, scale))
}

/// The zoom level at which a page of `page` points, with its gap and side margins,
/// fits into `viewport` entirely, within the allowed zoom range.
fn fit_zoom(page: Size<Pixels>, viewport: Size<Pixels>) -> f32 {
    let fit_scale = ((viewport.height - PAGE_GAP) / page.height)
        .min((viewport.width - PAGE_MARGIN * 2.) / page.width);
    (fit_scale / PX_PER_POINT * 100.).clamp(MIN_ZOOM, MAX_ZOOM)
}

/// The scroll offset at `new_scale` that keeps the document point at `anchor` (relative
/// to the viewport's top left) where it was at `old_scale`. Offsets are negative, as in
/// gpui's scroll handles.
fn zoom_anchored_offset(
    page_sizes: &[Size<Pixels>],
    old_scale: f32,
    new_scale: f32,
    offset: Point<Pixels>,
    viewport: Size<Pixels>,
    anchor: Point<Pixels>,
) -> Point<Pixels> {
    // Vertically, find the item under the anchor and how far into it the anchor is.
    // The gaps don't scale, so the whole offset can't simply be multiplied.
    let anchor_y = -offset.y + anchor.y;
    let mut old_top = px(0.);
    let mut new_top = px(0.);
    let mut new_anchor_y = None;
    for page in page_sizes {
        let old_height = item_height(*page, old_scale);
        let new_height = item_height(*page, new_scale);
        if anchor_y < old_top + old_height {
            let fraction = (anchor_y - old_top) / old_height;
            new_anchor_y = Some(new_top + new_height * fraction);
            break;
        }
        old_top += old_height;
        new_top += new_height;
    }
    let new_anchor_y = new_anchor_y.unwrap_or(new_top);

    // Horizontally, pages are centered in a row as wide as the viewport or the widest
    // page, so the distance from the row's center scales with the zoom.
    let widest = page_sizes
        .iter()
        .map(|page| page.width)
        .fold(px(0.), Pixels::max);
    let row_width = |scale: f32| (widest * scale + PAGE_MARGIN * 2.).max(viewport.width);
    let anchor_x = -offset.x + anchor.x;
    let new_anchor_x = row_width(new_scale) / 2.
        + (anchor_x - row_width(old_scale) / 2.) * (new_scale / old_scale);

    point(-(new_anchor_x - anchor.x), -(new_anchor_y - anchor.y))
}
