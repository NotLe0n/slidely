use gpui_kit::component::button::*;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::*;

use super::pdf_view::PdfView;
use crate::pdf::{LoadingPdf, PdfDocument, PdfState, close_pdf, open_pdf};

pub fn document(
    document: &Entity<PdfDocument>,
    pdf_view: &Entity<PdfView>,
    cx: &App,
) -> AnyElement {
    match document.read(cx).state() {
        PdfState::Loaded(_) => pdf_view.clone().into_any_element(),
        PdfState::Loading(loading) => {
            loading_dialog(loading, document.clone(), cx).into_any_element()
        }
        PdfState::None => empty_screen(document.clone()).into_any_element(),
    }
}

fn loading_dialog(
    loading: &LoadingPdf,
    document: Entity<PdfDocument>,
    cx: &App,
) -> impl IntoElement {
    let file_name = loading.path.file_name().map_or_else(
        || loading.path.display().to_string(),
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
        .child(div().font_semibold().child("Opening PDF"))
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
                    .on_click(move |_, _, cx| close_pdf(cx, document.clone())),
            ),
        )
}

fn empty_screen(document: Entity<PdfDocument>) -> impl IntoElement {
    v_flex().items_center().child("No PDF open").child(
        Button::new("open-pdf")
            .label("Open PDF")
            .on_click(move |_, _, cx| open_pdf(cx, document.clone())),
    )
}
