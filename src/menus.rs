//! Application actions and the menu bar.

use gpui_kit::component::GlobalState;
use gpui_kit::*;

use crate::{
    Settings,
    pdf::{PdfDocument, close_pdf, open_pdf},
    video::{VideoDocument, close_video, open_video},
};

actions!(slidely, [Quit, OpenPdf, ClosePdf, OpenVideo, CloseVideo]);
actions!(slidely, [TogglePdfView, ToggleVideoView]);

/// Registers the app's actions and menus. `document` is where `OpenPdf` loads into,
/// `video_document` where `OpenVideo` does.
pub fn init(document: Entity<PdfDocument>, video_document: Entity<VideoDocument>, cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    let doc = document.clone();
    cx.on_action(move |_: &OpenPdf, cx| open_pdf(cx, doc.clone()));
    cx.on_action(move |_: &ClosePdf, cx| close_pdf(cx, document.clone()));
    let video = video_document.clone();
    cx.on_action(move |_: &OpenVideo, cx| open_video(cx, video.clone()));
    cx.on_action(move |_: &CloseVideo, cx| close_video(cx, video_document.clone()));
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
            MenuItem::action("Open Video", OpenVideo),
            MenuItem::action("Close Video", CloseVideo),
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
