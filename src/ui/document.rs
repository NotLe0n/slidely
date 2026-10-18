use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::*;

use super::pdf_view::PdfView;
use crate::pdf::{PdfDocument, PdfState, open_pdf};

pub fn document(
    document: &Entity<PdfDocument>,
    pdf_view: &Entity<PdfView>,
    cx: &App,
) -> AnyElement {
    match document.read(cx).state() {
        PdfState::Loaded(_) => pdf_view.clone().into_any_element(),
        // `SlidelyUi` shows a loading dialog on top.
        PdfState::Loading(_) => div().into_any_element(),
        PdfState::None => empty_screen(document.clone()).into_any_element(),
    }
}

fn empty_screen(document: Entity<PdfDocument>) -> impl IntoElement {
    v_flex().items_center().child("No PDF open").child(
        Button::new("open-pdf")
            .label("Open PDF")
            .on_click(move |_, _, cx| open_pdf(cx, document.clone())),
    )
}
