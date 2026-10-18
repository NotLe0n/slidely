use std::path::PathBuf;

use gpui_kit::component::button::*;
use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::*;

use super::document::document;
use super::pdf_view::{PdfView, PdfViewEvent};
use super::zoom_bar::{ZoomBar, ZoomBarEvent};
use crate::pdf::{PdfDocument, PdfState, close_pdf};

/// The window's root view: title bar with menus, the document, and the zoom bar.
pub struct SlidelyUi {
    menu_bar: Entity<AppMenuBar>,
    document: Entity<PdfDocument>,
    pdf_view: Entity<PdfView>,
    zoom_bar: Entity<ZoomBar>,
    /// The file the loading dialog is open for; it follows `PdfState::Loading`.
    loading_dialog: Option<PathBuf>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl SlidelyUi {
    pub fn new(document: Entity<PdfDocument>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let zoom_bar = cx.new(|cx| ZoomBar::new(window, cx));
        let zoom_percent = zoom_bar.read(cx).zoom_percent();
        let pdf_view = cx.new(|cx| PdfView::new(document.clone(), zoom_percent, cx));

        let _subscriptions = vec![
            cx.observe_in(&document, window, Self::on_document_changed),
            cx.subscribe_in(&zoom_bar, window, Self::on_zoom_bar_event),
            cx.subscribe_in(&pdf_view, window, Self::on_pdf_view_event),
        ];

        Self {
            menu_bar: AppMenuBar::new(cx),
            document,
            pdf_view,
            zoom_bar,
            loading_dialog: None,
            focus_handle: cx.focus_handle(),
            _subscriptions,
        }
    }

    fn on_document_changed(
        &mut self,
        document: Entity<PdfDocument>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let loading_path = match document.read(cx).state() {
            PdfState::Loading(loading) => Some(loading.path.clone()),
            _ => None,
        };
        if loading_path != self.loading_dialog {
            // Loading ended (loaded, failed or cancelled), or another file replaced
            // the one loading.
            if self.loading_dialog.take().is_some() {
                window.close_dialog(cx);
            }
            if let Some(path) = loading_path {
                open_loading_dialog(path.clone(), document, window, cx);
                self.loading_dialog = Some(path);
            }
        }
        cx.notify();
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

/// Shows a spinner while `path` loads. It can only be dismissed with Cancel, which
/// closes the document; `SlidelyUi` closes the dialog whenever loading ends, so that's
/// the one way it goes away.
fn open_loading_dialog(
    path: PathBuf,
    document: Entity<PdfDocument>,
    window: &mut Window,
    cx: &mut App,
) {
    let file_name: SharedString = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
        .into();

    window.open_dialog(cx, move |dialog, _, _| {
        let document = document.clone();
        dialog
            .title("Opening PDF")
            .close_button(false)
            .overlay_closable(false)
            .keyboard(false)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(Spinner::new())
                    .child(format!("Loading {file_name}…")),
            )
            .footer(
                h_flex().w_full().justify_end().child(
                    Button::new("cancel-loading")
                        .label("Cancel")
                        .on_click(move |_, _, cx| close_pdf(cx, document.clone())),
                ),
            )
    });
}
