//! Application actions and the menu bar.

use gpui_kit::component::GlobalState;
use gpui_kit::*;

use crate::{
    Settings,
    pdf::{PdfDocument, close_pdf, open_pdf},
};

actions!(slidely, [Quit, OpenPdf, ClosePdf]);
actions!(slidely, [TogglePdfView, ToggleVideoView]);

/// Registers the app's actions and menus. `document` is where `OpenPdf` loads into.
pub fn init(document: Entity<PdfDocument>, cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    let doc = document.clone();
    cx.on_action(move |_: &OpenPdf, cx| open_pdf(cx, doc.clone()));
    cx.on_action(move |_: &ClosePdf, cx| close_pdf(cx, document.clone()));
    cx.on_action(|_: &TogglePdfView, cx| {
        cx.update_global::<Settings, _>(|value, _| {
            value.pdf_view_visible = !value.pdf_view_visible
        });
    });
    cx.on_action(|_: &ToggleVideoView, cx| {
        cx.update_global::<Settings, _>(|value, _| {
            value.video_view_visible = !value.video_view_visible
        });
    });

    cx.set_menus([
        Menu::new("File").items([
            MenuItem::action("Open PDF", OpenPdf),
            MenuItem::action("Close PDF", ClosePdf),
            MenuItem::separator(),
            MenuItem::action("Quit", Quit),
        ]),
        Menu::new("View").items([
            MenuItem::action("Toggle PDF View", TogglePdfView),
            MenuItem::action("Toggle video View", ToggleVideoView),
        ]),
    ]);

    // `set_menus` only shows a native menu bar on macOS. On Windows/Linux the
    // menus have to be handed to GlobalState so `AppMenuBar` can render them.
    if let Some(menus) = cx.get_menus() {
        GlobalState::global_mut(cx).set_app_menus(menus);
    }
}
