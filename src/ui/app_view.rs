use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::*;
use gpui_kit::*;

use super::document::document;
use super::pdf_view::{PdfView, PdfViewEvent};
use super::zoom_bar::{ZoomBar, ZoomBarEvent};
use crate::pdf::PdfDocument;

/// The window's root view: title bar with menus, the document, and the zoom bar.
pub struct SlidelyUi {
    menu_bar: Entity<AppMenuBar>,
    document: Entity<PdfDocument>,
    pdf_view: Entity<PdfView>,
    zoom_bar: Entity<ZoomBar>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl SlidelyUi {
    pub fn new(document: Entity<PdfDocument>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let zoom_bar = cx.new(|cx| ZoomBar::new(window, cx));
        let zoom_percent = zoom_bar.read(cx).zoom_percent();
        let pdf_view = cx.new(|cx| PdfView::new(document.clone(), zoom_percent, cx));

        let _subscriptions = vec![
            cx.observe(&document, |_, _, cx| cx.notify()),
            cx.subscribe_in(&zoom_bar, window, Self::on_zoom_bar_event),
            cx.subscribe_in(&pdf_view, window, Self::on_pdf_view_event),
        ];

        Self {
            menu_bar: AppMenuBar::new(cx),
            document,
            pdf_view,
            zoom_bar,
            focus_handle: cx.focus_handle(),
            _subscriptions,
        }
    }

    fn on_zoom_bar_event(
        &mut self,
        zoom_bar: &Entity<ZoomBar>,
        event: &ZoomBarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ZoomBarEvent::Changed => {
                let zoom_percent = zoom_bar.read(cx).zoom_percent();
                self.pdf_view.update(cx, |pdf_view, cx| {
                    pdf_view.set_zoom_percent(zoom_percent, cx)
                });
            }
            // Hand focus back to the pages once the zoom is entered.
            ZoomBarEvent::Confirmed => window.focus(&self.focus_handle, cx),
        }
    }

    fn on_pdf_view_event(
        &mut self,
        _: &Entity<PdfView>,
        event: &PdfViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // Ctrl+scroll; keep the zoom bar showing the current level.
            PdfViewEvent::ZoomChanged(zoom_percent) => {
                self.zoom_bar.update(cx, |zoom_bar, cx| {
                    zoom_bar.set_zoom_percent(*zoom_percent, window, cx)
                });
            }
        }
    }
}

impl Render for SlidelyUi {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(TitleBar::new().child(self.menu_bar.clone()))
            .child(
                v_flex()
                    .gap_2()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .min_h_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| window.focus(&this.focus_handle, cx)),
                    )
                    .child(document(&self.document, &self.pdf_view, cx)),
            )
            .child(self.zoom_bar.clone())
    }
}
