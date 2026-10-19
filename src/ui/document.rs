use std::path::Path;

use gpui_kit::component::button::*;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::*;

use super::pdf_view::PdfView;
use crate::pdf::{PdfDocument, PdfState, close_pdf, open_pdf};

pub fn document(
    document: &Entity<PdfDocument>,
    pdf_view: &Entity<PdfView>,
    cx: &App,
) -> AnyElement {
    match document.read(cx).state() {
        PdfState::Loaded(_) => pdf_view.clone().into_any_element(),
        PdfState::Loading(loading) => {
            let document = document.clone();
            loading_dialog("Opening PDF", &loading.path, cx, move |cx| {
                close_pdf(cx, document.clone())
            })
            .into_any_element()
        }
        PdfState::None => {
            let document = document.clone();
            empty_screen("No PDF open", "open-pdf", "Open PDF", move |cx| {
                open_pdf(cx, document.clone())
            })
            .into_any_element()
        }
    }
}

/// Shown while a file loads; `on_cancel` discards the load.
pub fn loading_dialog(
    title: &'static str,
    path: &Path,
    cx: &App,
    on_cancel: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let file_name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );

    v_flex()
        .w(px(360.))
        .gap_4()
        .p_4()
        .bg(cx.theme().background)
        .border_1()
        .border_color(cx.theme().border)
        .rounded(cx.theme().radius_lg)
        .child(div().font_semibold().child(title))
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(Spinner::new())
                .child(format!("Loading {file_name}…")),
        )
        .child(
            h_flex().justify_end().child(
                Button::new("cancel-loading")
                    .label("Cancel")
                    .on_click(move |_, _, cx| on_cancel(cx)),
            ),
        )
}

/// Shown when nothing is open, with a button that opens a file.
pub fn empty_screen(
    message: &'static str,
    button_id: &'static str,
    button_label: &'static str,
    on_open: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    v_flex().items_center().child(message).child(
        Button::new(button_id)
            .label(button_label)
            .on_click(move |_, _, cx| on_open(cx)),
    )
}
