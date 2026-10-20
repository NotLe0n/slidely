use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::*;
use gpui_kit::*;

use super::pdf_controls::{
    DEFAULT_ZOOM, MAX_ZOOM, MIN_ZOOM, PdfControlsBar, PdfControlsBarEvent, ZOOM_STEP,
};
use crate::pdf::{LoadedPdf, PdfDocument, PdfState};

/// Vertical space around each page, split above and below it.
const PAGE_GAP: Pixels = px(16.);
/// Horizontal space on each side of the widest page.
const PAGE_MARGIN: Pixels = px(16.);
/// Zoom factor per scrolled line with Ctrl held. A wheel notch is usually 3 lines
/// (~12%); touchpads send fractions of a line, which zoom smoothly.
const ZOOM_PER_WHEEL_LINE: f32 = 1.04;
/// Converts pixel-based wheel deltas (touchpads) to lines.
const WHEEL_LINE_HEIGHT: Pixels = px(20.);

/// The pages of the loaded document, in a virtual list so only the visible ones are
/// laid out, and only those (plus a few around them) are rendered by the document.
pub struct PdfView {
    document: Entity<PdfDocument>,
    zoom_bar: Entity<PdfControlsBar>,
    zoom_percent: f32,
    scroll_handle: VirtualListScrollHandle,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl PdfView {
    pub fn new(document: Entity<PdfDocument>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let zoom_bar = cx.new(|cx| PdfControlsBar::new(window, cx));

        let _subscriptions = vec![
            cx.observe(&document, |_, _, cx| cx.notify()),
            cx.subscribe_in(&zoom_bar, window, Self::on_zoom_bar_event),
        ];

        Self {
            document,
            zoom_bar,
            zoom_percent: DEFAULT_ZOOM,
            scroll_handle: VirtualListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            _subscriptions,
        }
    }

    /// Scrolls so page `ix` is at the top of the viewport.
    pub fn scroll_to_page(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.scroll_handle.scroll_to_item(ix, ScrollStrategy::Top);
        cx.notify();
    }

    /// Zooms around the viewport's center.
    pub fn set_zoom_percent(&mut self, zoom_percent: f32, cx: &mut Context<Self>) {
        let center = self.scroll_handle.bounds().size.center();
        self.zoom_to(zoom_percent, center, cx);
    }

    /// Zooms in one step of the zoom input.
    pub fn zoom_in(&mut self, cx: &mut Context<Self>) {
        self.set_zoom_percent((self.zoom_percent + ZOOM_STEP).min(MAX_ZOOM), cx);
    }

    /// Zooms out one step of the zoom input.
    pub fn zoom_out(&mut self, cx: &mut Context<Self>) {
        self.set_zoom_percent((self.zoom_percent - ZOOM_STEP).max(MIN_ZOOM), cx);
    }

    pub fn reset_zoom(&mut self, cx: &mut Context<Self>) {
        self.set_zoom_percent(DEFAULT_ZOOM, cx);
    }

    /// Zooms so the document point at `anchor` (relative to the viewport's top left)
    /// stays where it is on screen.
    fn zoom_to(&mut self, zoom_percent: f32, anchor: Point<Pixels>, cx: &mut Context<Self>) {
        let old_scale = self.display_scale();
        self.zoom_percent = zoom_percent;
        let new_scale = self.display_scale();

        // The list clamps the offset to the new content size when it lays out next.
        if let PdfState::Loaded(pdf) = self.document.read(cx).state() {
            let offset = zoom_anchored_offset(
                pdf.page_sizes(),
                old_scale,
                new_scale,
                self.scroll_handle.offset(),
                self.scroll_handle.bounds().size,
                anchor,
            );
            self.scroll_handle.set_offset(offset);
        }
        cx.notify();
    }

    fn zoom_by_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        // Scrolling up (positive) zooms in.
        let lines = event.delta.pixel_delta(WHEEL_LINE_HEIGHT).y / WHEEL_LINE_HEIGHT;
        let zoom_percent =
            (self.zoom_percent * ZOOM_PER_WHEEL_LINE.powf(lines)).clamp(MIN_ZOOM, MAX_ZOOM);
        if zoom_percent == self.zoom_percent {
            return;
        }
        let viewport = self.scroll_handle.bounds();
        self.zoom_to(zoom_percent, event.position - viewport.origin, cx);
    }

    fn on_zoom_bar_event(
        &mut self,
        _zoom_bar: &Entity<PdfControlsBar>,
        event: &PdfControlsBarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PdfControlsBarEvent::ZoomChanged(new_zoom_percent) => {
                self.set_zoom_percent(*new_zoom_percent, cx)
            }
            PdfControlsBarEvent::ZoomConfirmed => window.focus(&self.focus_handle, cx),
            PdfControlsBarEvent::FitPage => self.fit_page(cx),
            PdfControlsBarEvent::RotatePage => self.rotate_page(cx),
        }
    }

    /// The index of the page in the middle of the viewport.
    fn current_page(&self, cx: &App) -> Option<usize> {
        let PdfState::Loaded(pdf) = self.document.read(cx).state() else {
            return None;
        };
        let viewport = self.scroll_handle.bounds().size;
        let center_y = -self.scroll_handle.offset().y + viewport.height / 2.;
        page_at(pdf.page_sizes(), self.display_scale(), center_y)
    }

    /// Turns the page in the middle of the viewport 90° clockwise.
    pub fn rotate_page(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.current_page(cx) {
            self.document
                .update(cx, |document, cx| document.rotate_page(ix, cx));
        }
    }

    /// Zooms so the page in the middle of the viewport fits into it entirely, and
    /// scrolls that page to the top.
    pub fn fit_page(&mut self, cx: &mut Context<Self>) {
        let Some(ix) = self.current_page(cx) else {
            return;
        };
        let PdfState::Loaded(pdf) = self.document.read(cx).state() else {
            return;
        };

        self.zoom_percent = self.get_fitted_page_zoom(ix, pdf);

        let page_top = pdf.page_sizes()[..ix]
            .iter()
            .map(|page| page.height * self.display_scale() + PAGE_GAP)
            .fold(px(0.), |top, height| top + height);

        self.scroll_handle.set_offset(point(px(0.), -page_top));
        cx.notify();
    }

    fn get_fitted_page_zoom(&mut self, page_idx: usize, pdf: &LoadedPdf) -> f32 {
        let page_sizes = pdf.page_sizes();
        let viewport = self.scroll_handle.bounds().size;

        // The page's gap and side margins have to fit as well.
        let page = page_sizes[page_idx];
        let fit_scale = ((viewport.height - PAGE_GAP) / page.height)
            .min((viewport.width - PAGE_MARGIN * 2.) / page.width);
        return (fit_scale / (96. / 72.) * 100.).clamp(MIN_ZOOM, MAX_ZOOM);
    }

    /// Display size per PDF point: zoom, and points (1/72 in) to gpui pixels (1/96 in).
    fn display_scale(&self) -> f32 {
        self.zoom_percent / 100. * (96. / 72.)
    }

    /// Tells the document what to render. Both values are reported every frame; the
    /// document ignores ones that didn't change. Entities shouldn't be updated while
    /// the window is drawing, so it's applied right after.
    fn report_to_document(
        &mut self,
        visible_range: Option<Range<usize>>,
        render_scale: Option<f32>,
        cx: &mut Context<Self>,
    ) {
        let document = self.document.clone();
        cx.defer(move |cx| {
            document.update(cx, |document, cx| {
                if let Some(render_scale) = render_scale {
                    document.set_render_scale(render_scale, cx);
                }
                if let Some(visible_range) = visible_range {
                    document.set_visible_range(visible_range, cx);
                }
            })
        });
    }
}

impl Render for PdfView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render pages at the resolution they're displayed at. Doing this from `render`
        // also catches the window moving to a display with another scale factor.
        let scale = self.display_scale();
        self.report_to_document(None, Some(scale * window.scale_factor()), cx);

        // Show zoom levels set elsewhere (Ctrl+scroll, fit page, the menu) in the zoom
        // bar. Values it sent itself already match, so this doesn't fight typing.
        let zoom_percent = self.zoom_percent;
        if self.zoom_bar.read(cx).zoom_percent() != zoom_percent {
            self.zoom_bar.update(cx, |zoom_bar, cx| {
                zoom_bar.set_zoom_percent(zoom_percent, window, cx)
            });
        }

        let PdfState::Loaded(pdf) = self.document.read(cx).state() else {
            return div().into_any_element();
        };

        let page_sizes: Vec<_> = pdf
            .page_sizes()
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

        let list = v_virtual_list(
            cx.entity(),
            "pdf-pages",
            item_sizes,
            move |this, range, _, cx| {
                if range.start < page_count {
                    let visible = range.start..range.end.min(page_count);
                    this.report_to_document(Some(visible), None, cx);
                }

                let PdfState::Loaded(pdf) = this.document.read(cx).state() else {
                    return Vec::new();
                };
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
                        let page = match pdf.page_image(ix) {
                            Some(image) => img(image.clone()).size_full().into_any_element(),
                            // Not rendered yet.
                            None => div().size_full().bg(gpui_kit::white()).into_any_element(),
                        };
                        item.h(page_size.height + PAGE_GAP).child(
                            div()
                                .flex_shrink_0()
                                .w(page_size.width)
                                .h(page_size.height)
                                .child(page),
                        )
                    })
                    .collect()
            },
        )
        .with_item_to_measure_index(page_count)
        .track_scroll(&self.scroll_handle)
        .size_full();

        // Ctrl+scroll zooms instead of scrolling. The list handles the wheel itself
        // before any parent's `on_scroll_wheel` would run, so this listens in the
        // capture phase, which runs first, and stops the event there.
        let view = cx.entity().downgrade();
        let ctrl_wheel_zoom = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture
                        && event.modifiers.control
                        && bounds.contains(&event.position)
                    {
                        view.update(cx, |this, cx| this.zoom_by_wheel(event, cx))
                            .ok();
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .inset_0();

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
                    .child(list)
                    .child(Scrollbar::new(&self.scroll_handle))
                    .child(ctrl_wheel_zoom),
            )
            .child(self.zoom_bar.clone())
            .into_any_element()
    }
}

/// The index of the page whose item (page plus gap) contains the document position `y`
/// at `scale`, or the last page if `y` is past them all.
fn page_at(page_sizes: &[gpui_kit::Size<Pixels>], scale: f32, y: Pixels) -> Option<usize> {
    let mut bottom = px(0.);
    for (ix, page) in page_sizes.iter().enumerate() {
        bottom += page.height * scale + PAGE_GAP;
        if y < bottom {
            return Some(ix);
        }
    }
    page_sizes.len().checked_sub(1)
}

/// The scroll offset at `new_scale` that keeps the document point at `anchor` (relative
/// to the viewport's top left) where it was at `old_scale`. Offsets are negative, as in
/// gpui's scroll handles.
fn zoom_anchored_offset(
    page_sizes: &[gpui_kit::Size<Pixels>],
    old_scale: f32,
    new_scale: f32,
    offset: Point<Pixels>,
    viewport: gpui_kit::Size<Pixels>,
    anchor: Point<Pixels>,
) -> Point<Pixels> {
    // Vertically, find the item under the anchor and how far into it the anchor is.
    // The gaps don't scale, so the whole offset can't simply be multiplied.
    let anchor_y = -offset.y + anchor.y;
    let mut old_top = px(0.);
    let mut new_top = px(0.);
    let mut new_anchor_y = None;
    for page in page_sizes {
        let old_height = page.height * old_scale + PAGE_GAP;
        let new_height = page.height * new_scale + PAGE_GAP;
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
