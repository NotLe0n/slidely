use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::*;
use gpui_kit::*;

use super::document::document;
use super::pdf_view::PdfView;
use super::video_view::VideoView;
use crate::Settings;
use crate::pdf::PdfDocument;
use crate::ui::timeline_view::TimelineView;
use crate::video::VideoDocument;

/// The window's root view: title bar with menus, the document, and the zoom bar.
pub struct SlidelyUi {
    menu_bar: Entity<AppMenuBar>,
    document: Entity<PdfDocument>,
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
        let timeline_view = cx.new(|cx| TimelineView::new(video_document, cx));

        let _subscriptions = vec![
            cx.observe(&document, |_, _, cx| cx.notify()),
            cx.observe_global::<Settings>(|_this, cx| {
                cx.notify();
            }),
        ];

        Self {
            menu_bar: AppMenuBar::new(cx),
            document,
            pdf_view,
            video_view,
            timeline_view,
            _subscriptions,
        }
    }
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
