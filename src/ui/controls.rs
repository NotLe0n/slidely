//! Controls shared by the views.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::*;

use crate::Settings;

/// A button showing whether the setting `field` picks is on, which flips it. The view
/// showing it has to observe `Settings` to show changes.
pub fn setting_toggle(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    field: fn(&mut Settings) -> &mut bool,
    cx: &App,
) -> Button {
    // Read from a copy, since borrowing the global mutably notifies its observers.
    let mut settings = *cx.global::<Settings>();
    let on = *field(&mut settings);
    Button::new(id)
        .ghost()
        .icon(icon)
        .selected(on)
        .toggled(on)
        .tooltip(tooltip)
        .on_click(move |_, _, cx| Settings::toggle(cx, field))
}
