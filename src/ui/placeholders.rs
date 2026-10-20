//! Elements shown in place of content that isn't there (yet): loading and empty
//! screens, and pages that haven't been rendered.

use std::path::Path;
use std::sync::Arc;

use gpui_kit::component::button::*;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Shown while a file loads; `on_cancel` discards the load.
pub fn loading_dialog(
    title: &'static str,
    path: &Path,
    cx: &App,
    on_cancel: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
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
                .child(format!("Loading {}…", file_name(path))),
        )
        .child(
            h_flex().justify_end().child(
                Button::new("cancel-loading")
                    .label("Cancel")
                    .on_click(move |_, _, cx| on_cancel(cx)),
            ),
        )
}

/// Shown when nothing is open, with a button that opens a file. `error` explains why
/// the last file couldn't be opened, if it couldn't.
pub fn empty_screen(
    message: impl Into<SharedString>,
    error: Option<&str>,
    button_id: &'static str,
    button_label: &'static str,
    cx: &App,
    on_open: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    v_flex()
        .items_center()
        .gap_2()
        .max_w(px(480.))
        .child(message.into())
        .when_some(error, |screen, error| {
            screen.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.to_string()),
            )
        })
        .child(
            Button::new(button_id)
                .label(button_label)
                .on_click(move |_, _, cx| on_open(cx)),
        )
}

/// The empty screen after `path` failed to open.
pub fn failed_screen(
    path: &Path,
    error: &str,
    button_id: &'static str,
    button_label: &'static str,
    cx: &App,
    on_open: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let message = format!("Couldn't open {}", file_name(path));
    empty_screen(message, Some(error), button_id, button_label, cx, on_open)
}

/// A rendered page filling its parent, or a blank page until it's rendered.
pub fn page_image(image: Option<Arc<RenderImage>>) -> AnyElement {
    match image {
        Some(image) => img(image).size_full().into_any_element(),
        None => div().size_full().bg(gpui_kit::white()).into_any_element(),
    }
}

/// The file's name, or the whole path if it has none.
pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}
