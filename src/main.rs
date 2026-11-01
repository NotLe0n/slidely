mod align;
mod assets;
mod interval;
mod menus;
mod observe;
mod pdf;
mod project;
mod ui;
mod video;

use gpui_kit::component::{Theme, TitleBar};
use gpui_kit::*;

use project::{Project, confirm_discard};
use ui::SlidelyUi;

#[derive(Clone, Copy)]
struct Settings {
    pdf_view_visible: bool,
    video_view_visible: bool,
    video_follows_slides: bool,
    slides_follow_video: bool,
    volume: f64,
    muted: bool,
    playback_speed: f64,
}

impl Global for Settings {}

impl Settings {
    /// Flips the setting `field` picks.
    fn toggle(cx: &mut App, field: fn(&mut Settings) -> &mut bool) {
        cx.update_global::<Settings, _>(|settings, _| {
            let value = field(settings);
            *value = !*value;
        });
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::sync_system_appearance(None, cx);
            cx.set_global(Settings {
                pdf_view_visible: true,
                video_view_visible: true,
                video_follows_slides: false,
                slides_follow_video: false,
                volume: 1.,
                muted: false,
                playback_speed: 1.,
            });

            let project = cx.new(|_| Project::new());
            menus::init(project.clone(), cx);

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
                let this = project.clone();
                window.on_window_should_close(cx, move |_, cx| {
                    if !this.read(cx).has_unsaved_changes(cx) {
                        return true;
                    }
                    // Asked once this window isn't being updated anymore, since the
                    // prompt is shown in it.
                    let project = this.clone();
                    cx.defer(move |cx| confirm_discard(project, cx, |cx| cx.quit()));
                    false
                });

                cx.new(|cx| SlidelyUi::new(project, window, cx))
            })
            .expect("Failed to open window");
        });
}
