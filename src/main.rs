mod menus;
mod pdf;
mod ui;

use gpui_kit::component::{Theme, TitleBar};
use gpui_kit::*;

use pdf::PdfDocument;
use ui::SlidelyUi;

struct Settings {
    pdf_view_visible: bool,
    video_view_visible: bool,
}

impl Global for Settings {}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::sync_system_appearance(None, cx);
            cx.set_global(Settings {
                pdf_view_visible: true,
                video_view_visible: true,
            });

            let document = cx.new(|_| PdfDocument::new());
            menus::init(document.clone(), cx);

            let restore_bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            let window_opts = WindowOptions {
                window_bounds: Some(WindowBounds::Maximized(restore_bounds)),
                ..TitleBar::window_options()
            };

            gpui_kit::open_window(window_opts, cx, |window, cx| {
                window
                    .observe_window_appearance(|window, cx| {
                        Theme::sync_system_appearance(Some(window), cx)
                    })
                    .detach();

                cx.new(|cx| SlidelyUi::new(document, window, cx))
            })
            .expect("Failed to open window");
        });
}
