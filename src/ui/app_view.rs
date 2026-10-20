use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::*;
use gpui_kit::*;

use super::document::document;
use super::pdf_view::PdfView;
use super::video_view::VideoView;
use crate::Settings;
use crate::menus::{
    self, FitPage, ResetZoom, RotatePageClockwise, SetPlaybackSpeed, ToggleMute, TogglePlayback,
    ZoomIn, ZoomOut,
};
use crate::pdf::{PdfDocument, PdfState};
use crate::ui::timeline_view::{TimelineEvent, TimelineView};
use crate::video::{VideoDocument, VideoState};

/// The window's root view: title bar with menus, the document, and the zoom bar.
pub struct SlidelyUi {
    menu_bar: Entity<AppMenuBar>,
    document: Entity<PdfDocument>,
    video_document: Entity<VideoDocument>,
    /// Whether a PDF and a video were loaded when the menus were last built.
    menus_built_for: (bool, bool),
    pdf_view: Entity<PdfView>,
    video_view: Entity<VideoView>,
    timeline_view: Entity<TimelineView>,
    _subscriptions: Vec<Subscription>,
}

impl SlidelyUi {
    pub fn new(
        document: Entity<PdfDocument>,
        video_document: Entity<VideoDocument>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pdf_view = cx.new(|cx| PdfView::new(document.clone(), window, cx));
        let video_view = cx.new(|cx| VideoView::new(video_document.clone(), cx));
        let timeline_view = cx.new(|cx| TimelineView::new(document.clone(), cx));

        let _subscriptions = vec![
            cx.observe(&document, |this, _, cx| {
                this.update_menus(cx);
                cx.notify()
            }),
            cx.observe(&video_document, |this, _, cx| this.update_menus(cx)),
            cx.subscribe(&timeline_view, |this, _, event, cx| match event {
                TimelineEvent::PageClicked(ix) => this
                    .pdf_view
                    .update(cx, |pdf_view, cx| pdf_view.scroll_to_page(*ix, cx)),
            }),
            cx.observe_global::<Settings>(|_this, cx| {
                cx.notify();
            }),
        ];

        register_view_actions(cx.entity().downgrade(), cx);

        Self {
            menu_bar: AppMenuBar::new(cx),
            document,
            video_document,
            // What `menus::init` built them for.
            menus_built_for: (false, false),
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
        let loaded = (
            matches!(self.document.read(cx).state(), PdfState::Loaded(_)),
            matches!(self.video_document.read(cx).state(), VideoState::Loaded(_)),
        );
        if loaded == self.menus_built_for {
            return;
        }
        self.menus_built_for = loaded;
        menus::set_menus(loaded.0, loaded.1, cx);
        self.menu_bar.update(cx, |menu_bar, cx| menu_bar.reload(cx));
    }
}

/// Handles the menu actions that act on the views. They're registered on the app
/// rather than on an element, since an element only gets actions while it or one of
/// its children has focus, and initially nothing has.
fn register_view_actions(this: WeakEntity<SlidelyUi>, cx: &mut App) {
    fn on_pdf_view<A: Action>(
        this: &WeakEntity<SlidelyUi>,
        cx: &mut App,
        f: impl Fn(&mut PdfView, &A, &mut Context<PdfView>) + 'static,
    ) {
        let this = this.clone();
        cx.on_action(move |action: &A, cx| {
            if let Some(this) = this.upgrade() {
                let pdf_view = this.read(cx).pdf_view.clone();
                pdf_view.update(cx, |pdf_view, cx| f(pdf_view, action, cx));
            }
        });
    }
    fn on_video_view<A: Action>(
        this: &WeakEntity<SlidelyUi>,
        cx: &mut App,
        f: impl Fn(&mut VideoView, &A, &mut Context<VideoView>) + 'static,
    ) {
        let this = this.clone();
        cx.on_action(move |action: &A, cx| {
            if let Some(this) = this.upgrade() {
                let video_view = this.read(cx).video_view.clone();
                video_view.update(cx, |video_view, cx| f(video_view, action, cx));
            }
        });
    }

    on_pdf_view(&this, cx, |view, _: &ZoomIn, cx| view.zoom_in(cx));
    on_pdf_view(&this, cx, |view, _: &ZoomOut, cx| view.zoom_out(cx));
    on_pdf_view(&this, cx, |view, _: &ResetZoom, cx| view.reset_zoom(cx));
    on_pdf_view(&this, cx, |view, _: &FitPage, cx| view.fit_page(cx));
    on_pdf_view(&this, cx, |view, _: &RotatePageClockwise, cx| {
        view.rotate_page(cx)
    });
    on_video_view(&this, cx, |view, _: &TogglePlayback, cx| {
        view.toggle_playback(cx)
    });
    on_video_view(&this, cx, |view, _: &ToggleMute, cx| view.toggle_mute(cx));
    on_video_view(&this, cx, |view, action: &SetPlaybackSpeed, cx| {
        view.set_speed(action.0, cx)
    });
}

impl Render for SlidelyUi {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pdf_view_visible = cx.global::<Settings>().pdf_view_visible;
        let video_view_visible = cx.global::<Settings>().video_view_visible;

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(TitleBar::new().child(self.menu_bar.clone()))
            .child(
                h_resizable("main-view")
                    .child(
                        resizable_panel().visible(pdf_view_visible).child(
                            div()
                                .v_flex()
                                .gap_2()
                                .flex_1()
                                .items_center()
                                .justify_center()
                                .min_h_0()
                                .child(document(&self.document, &self.pdf_view, cx)),
                        ),
                    )
                    .child(
                        resizable_panel()
                            .visible(video_view_visible)
                            .child(self.video_view.clone().into_any_element()),
                    ),
            )
            .child(self.timeline_view.clone())
    }
}
