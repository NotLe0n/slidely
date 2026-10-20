use std::rc::Rc;

use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::*;
use gpui_kit::*;

use super::pdf_view::PdfView;
use super::timeline_view::{TimelineEvent, TimelineView};
use super::video_view::VideoView;
use crate::Settings;
use crate::menus::{
    self, FitPage, LoadedDocuments, ResetZoom, RotatePageClockwise, SetPlaybackSpeed, ToggleMute,
    TogglePlayback, ZoomIn, ZoomOut,
};
use crate::pdf::PdfDocument;
use crate::video::VideoDocument;

/// The window's root view: title bar with menus, the PDF and video panels, and the
/// timeline.
pub struct SlidelyUi {
    menu_bar: Entity<AppMenuBar>,
    pdf_document: Entity<PdfDocument>,
    video_document: Entity<VideoDocument>,
    /// What the menus were last built for.
    menus_built_for: LoadedDocuments,
    pdf_view: Entity<PdfView>,
    video_view: Entity<VideoView>,
    timeline_view: Entity<TimelineView>,
    _subscriptions: Vec<Subscription>,
}

impl SlidelyUi {
    pub fn new(
        pdf_document: Entity<PdfDocument>,
        video_document: Entity<VideoDocument>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pdf_view = cx.new(|cx| PdfView::new(pdf_document.clone(), window, cx));
        let video_view = cx.new(|cx| VideoView::new(video_document.clone(), cx));
        let timeline_view = cx.new(|cx| TimelineView::new(pdf_document.clone(), cx));

        let _subscriptions = vec![
            cx.observe(&pdf_document, |this, _, cx| this.update_menus(cx)),
            cx.observe(&video_document, |this, _, cx| this.update_menus(cx)),
            cx.subscribe(&timeline_view, |this, _, event, cx| match event {
                TimelineEvent::PageClicked(ix) => this
                    .pdf_view
                    .update(cx, |pdf_view, cx| pdf_view.scroll_to_page(*ix, cx)),
            }),
            cx.observe_global::<Settings>(|_, cx| cx.notify()),
        ];

        register_view_actions(cx.entity().downgrade(), window.window_handle(), cx);

        Self {
            menu_bar: AppMenuBar::new(cx),
            pdf_document,
            video_document,
            // What `menus::init` built them for.
            menus_built_for: LoadedDocuments::default(),
            pdf_view,
            video_view,
            timeline_view,
            _subscriptions,
        }
    }

    /// Rebuilds the menus if a PDF or video was loaded or closed, so their items are
    /// enabled only while there's something to act on. Rebuilding closes an open
    /// menu, so it isn't done on every change to the documents.
    fn update_menus(&mut self, cx: &mut Context<Self>) {
        let loaded = LoadedDocuments {
            pdf: self.pdf_document.read(cx).is_loaded(),
            video: self.video_document.read(cx).is_loaded(),
        };
        if loaded == self.menus_built_for {
            return;
        }
        self.menus_built_for = loaded;
        menus::set_menus(loaded, cx);
        self.menu_bar.update(cx, |menu_bar, cx| menu_bar.reload(cx));
    }
}

/// Handles the menu actions that act on the views. They're registered on the app
/// rather than on an element, since an element only gets actions while it or one of
/// its children has focus, and initially nothing has.
fn register_view_actions(this: WeakEntity<SlidelyUi>, window: AnyWindowHandle, cx: &mut App) {
    let pdf_view = |ui: &SlidelyUi| ui.pdf_view.clone();
    let video_view = |ui: &SlidelyUi| ui.video_view.clone();

    on_view_action(
        &this,
        window,
        pdf_view,
        cx,
        |view, _: &ZoomIn, window, cx| view.zoom_in(window, cx),
    );
    on_view_action(
        &this,
        window,
        pdf_view,
        cx,
        |view, _: &ZoomOut, window, cx| view.zoom_out(window, cx),
    );
    on_view_action(
        &this,
        window,
        pdf_view,
        cx,
        |view, _: &ResetZoom, window, cx| view.reset_zoom(window, cx),
    );
    on_view_action(
        &this,
        window,
        pdf_view,
        cx,
        |view, _: &FitPage, window, cx| view.fit_page(window, cx),
    );
    on_view_action(
        &this,
        window,
        pdf_view,
        cx,
        |view, _: &RotatePageClockwise, _, cx| view.rotate_page(cx),
    );
    on_view_action(
        &this,
        window,
        video_view,
        cx,
        |view, _: &TogglePlayback, _, cx| view.toggle_playback(cx),
    );
    on_view_action(
        &this,
        window,
        video_view,
        cx,
        |view, _: &ToggleMute, _, cx| view.toggle_mute(cx),
    );
    on_view_action(
        &this,
        window,
        video_view,
        cx,
        |view, action: &SetPlaybackSpeed, _, cx| view.set_speed(action.0, cx),
    );
}

/// Calls `handler` with the view `view` picks from the root view whenever action `A`
/// is dispatched.
fn on_view_action<A: Action + Clone, V: 'static>(
    this: &WeakEntity<SlidelyUi>,
    window: AnyWindowHandle,
    view: fn(&SlidelyUi) -> Entity<V>,
    cx: &mut App,
    handler: impl Fn(&mut V, &A, &mut Window, &mut Context<V>) + 'static,
) {
    let this = this.clone();
    let handler = Rc::new(handler);
    cx.on_action(move |action: &A, cx| {
        let (this, handler, action) = (this.clone(), handler.clone(), action.clone());
        // Actions are dispatched while the window is being updated, so it can only be
        // updated again once that's done.
        cx.defer(move |cx| {
            window
                .update(cx, |_, window, cx| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    let view = view(this.read(cx));
                    view.update(cx, |view, cx| handler(view, &action, window, cx));
                })
                .ok();
        });
    });
}

impl Render for SlidelyUi {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = cx.global::<Settings>();

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(TitleBar::new().child(self.menu_bar.clone()))
            .child(
                h_resizable("main-view")
                    .child(
                        resizable_panel()
                            .visible(settings.pdf_view_visible)
                            .child(self.pdf_view.clone()),
                    )
                    .child(
                        resizable_panel()
                            .visible(settings.video_view_visible)
                            .child(self.video_view.clone()),
                    ),
            )
            .child(self.timeline_view.clone())
    }
}
