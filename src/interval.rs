//! Running something regularly for as long as an entity is around.

use std::time::Duration;

use gpui_kit::{Context, Task};

/// Calls `f` every `interval` until the entity is dropped, or the task is.
pub fn spawn_interval<V: 'static>(
    interval: Duration,
    cx: &mut Context<V>,
    mut f: impl FnMut(&mut V, &mut Context<V>) + 'static,
) -> Task<()> {
    cx.spawn(async move |this, app| {
        loop {
            app.background_executor().timer(interval).await;
            if this.update(app, |this, cx| f(this, cx)).is_err() {
                return;
            }
        }
    })
}
