//! Application actions and the menu bar.

use gpui_kit::component::GlobalState;
use gpui_kit::*;

use crate::{
    Settings,
    pdf::{PdfDocument, open_pdf},
    video::{PLAYBACK_SPEEDS, VideoDocument, open_video},
};

actions!(slidely, [Quit, OpenPdf, ClosePdf, OpenVideo, CloseVideo]);
actions!(slidely, [TogglePdfView, ToggleVideoView]);
// Handled by the views, since they hold the zoom level and the playback controls.
actions!(
    slidely,
    [ZoomIn, ZoomOut, ResetZoom, FitPage, RotatePageClockwise]
);
actions!(slidely, [TogglePlayback, ToggleMute]);

/// Sets the video's playback speed to the given factor.
#[derive(Action, Clone, PartialEq)]
#[action(namespace = slidely, no_json)]
pub struct SetPlaybackSpeed(pub f64);

/// Which documents are loaded; the menu items acting on one are disabled without it.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct LoadedDocuments {
    pub pdf: bool,
    pub video: bool,
}

/// Registers the app's actions and menus. `pdf_document` is where `OpenPdf` loads
/// into, `video_document` where `OpenVideo` does. The actions acting on the views are
/// registered by the root view.
pub fn init(
    pdf_document: Entity<PdfDocument>,
    video_document: Entity<VideoDocument>,
    cx: &mut App,
) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    let pdf = pdf_document.clone();
    cx.on_action(move |_: &OpenPdf, cx| open_pdf(pdf.clone(), cx));
    cx.on_action(move |_: &ClosePdf, cx| {
        pdf_document.update(cx, |document, cx| document.close(cx))
    });
    let video = video_document.clone();
    cx.on_action(move |_: &OpenVideo, cx| open_video(video.clone(), cx));
    cx.on_action(move |_: &CloseVideo, cx| {
        video_document.update(cx, |document, cx| document.close(cx))
    });
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

    set_menus(LoadedDocuments::default(), cx);
}

/// Sets the menu bar's menus, with the items acting on a document disabled unless it's
/// loaded. `AppMenuBar` has to be reloaded afterwards to show the change.
pub fn set_menus(loaded: LoadedDocuments, cx: &mut App) {
    let no_pdf = !loaded.pdf;
    let no_video = !loaded.video;
    let speed_items = PLAYBACK_SPEEDS.iter().map(|&speed| {
        MenuItem::action(format!("{speed}×"), SetPlaybackSpeed(speed)).disabled(no_video)
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
            MenuItem::action("Toggle Video View", ToggleVideoView),
            MenuItem::separator(),
            MenuItem::action("Zoom In", ZoomIn).disabled(no_pdf),
            MenuItem::action("Zoom Out", ZoomOut).disabled(no_pdf),
            MenuItem::action("Reset Zoom", ResetZoom).disabled(no_pdf),
            MenuItem::action("Fit Page", FitPage).disabled(no_pdf),
            MenuItem::action("Rotate Page Clockwise", RotatePageClockwise).disabled(no_pdf),
            MenuItem::separator(),
            MenuItem::action("Play/Pause", TogglePlayback).disabled(no_video),
            MenuItem::action("Mute/Unmute", ToggleMute).disabled(no_video),
            MenuItem::submenu(
                Menu::new("Playback Speed")
                    .items(speed_items)
                    .disabled(no_video),
            ),
        ]),
    ]);

    // `set_menus` only shows a native menu bar on macOS. On Windows/Linux the
    // menus have to be handed to GlobalState so `AppMenuBar` can render them.
    if let Some(menus) = cx.get_menus() {
        GlobalState::global_mut(cx).set_app_menus(menus);
    }
}
