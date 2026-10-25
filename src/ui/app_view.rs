use std::rc::Rc;

use gpui_kit::component::button::*;
use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::*;
use gpui_kit::*;

use super::follow::SlideFollower;
use super::pdf_view::PdfView;
use super::placeholders::{empty_screen, file_name};
use super::timeline_view::{TimelineEvent, TimelineView};
use super::video_view::{VideoView, VideoViewEvent};
use crate::Settings;
use crate::menus::{
    self, FitPage, MenuState, NextSlide, PreviousSlide, ResetZoom, RotatePageClockwise,
    SetPlaybackSpeed, ToggleMute, TogglePlayback, ZoomIn, ZoomOut,
};
use crate::observe::ObservedDocuments;
use crate::pdf::PdfDocument;
use crate::project::{Project, ProjectEvent, add_videos, open_project};
use crate::video::VideoDocument;

/// The window's root view: title bar with menus, and the PDF and video panels and the
/// timeline once a project is open.
pub struct SlidelyUi {
    menu_bar: Entity<AppMenuBar>,
    project: Entity<Project>,
    /// What the menus were last built for.
    menus_built_for: MenuState,
    pdf_view: Entity<PdfView>,
    /// One per video in the project, in the same order. Each video keeps its own
    /// view, so its controls stay as they were when another one is shown.
    video_panels: Vec<VideoPanel>,
    timeline_view: Entity<TimelineView>,
    /// The menus depend on whether the PDFs are loaded.
    pdfs: ObservedDocuments<PdfDocument>,
    /// Keeps the slides and the video in step.
    _follower: Entity<SlideFollower>,
    _subscriptions: Vec<Subscription>,
}

/// The view of one of the project's videos.
struct VideoPanel {
    document: Entity<VideoDocument>,
    view: Entity<VideoView>,
    _subscriptions: [Subscription; 2],
}

impl SlidelyUi {
    pub fn new(project: Entity<Project>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pdf_view = cx.new(|cx| PdfView::new(project.clone(), window, cx));
        let timeline_view = cx.new(|cx| TimelineView::new(project.clone(), cx));
        let _follower = cx.new(|cx| SlideFollower::new(project.clone(), pdf_view.clone(), cx));

        let _subscriptions = vec![
            cx.observe(&project, |this, _, cx| {
                this.observe_documents(cx);
                this.sync_video_panels(cx);
                this.update_menus(cx);
                cx.notify();
            }),
            cx.subscribe_in(&project, window, |_, _, event, window, cx| match event {
                ProjectEvent::Saved(path) => window.push_notification(
                    Notification::success(format!("Saved {}", file_name(path))),
                    cx,
                ),
                ProjectEvent::Error(error) => {
                    window.push_notification(Notification::error(error.clone()), cx)
                }
            }),
            cx.subscribe(&timeline_view, |this, _, event, cx| match event {
                TimelineEvent::PageClicked(ix) => {
                    this.pdf_view
                        .update(cx, |pdf_view, cx| pdf_view.scroll_to_page(*ix, cx));
                    this.project
                        .update(cx, |project, cx| project.play_slide(*ix, cx));
                }
            }),
            cx.observe_global::<Settings>(|_, cx| cx.notify()),
        ];

        register_view_actions(cx.entity().downgrade(), window.window_handle(), cx);

        Self {
            menu_bar: AppMenuBar::new(cx),
            project,
            // What `menus::init` built them for.
            menus_built_for: MenuState::default(),
            pdf_view,
            video_panels: Vec::new(),
            timeline_view,
            pdfs: ObservedDocuments::new(),
            _follower,
            _subscriptions,
        }
    }

    /// Observes the project's current PDFs, after one was added or removed.
    fn observe_documents(&mut self, cx: &mut Context<Self>) {
        let pdfs = self.project.read(cx).pdfs().to_vec();
        self.pdfs
            .observe(&pdfs, cx, |this, cx| this.update_menus(cx));
    }

    /// Creates the views of videos added to the project and drops those of removed
    /// ones.
    fn sync_video_panels(&mut self, cx: &mut Context<Self>) {
        let documents = self.project.read(cx).videos().to_vec();
        let mut old_panels = std::mem::take(&mut self.video_panels);
        self.video_panels = documents
            .into_iter()
            .map(|document| {
                match old_panels
                    .iter()
                    .position(|panel| panel.document == document)
                {
                    Some(ix) => old_panels.swap_remove(ix),
                    None => self.new_video_panel(document, cx),
                }
            })
            .collect();
    }

    fn new_video_panel(
        &mut self,
        document: Entity<VideoDocument>,
        cx: &mut Context<Self>,
    ) -> VideoPanel {
        let view = cx.new(|cx| VideoView::new(document.clone(), cx));
        let _subscriptions = [
            // The menus depend on whether the video shown is loaded.
            cx.observe(&document, |this, _, cx| this.update_menus(cx)),
            cx.subscribe(&view, {
                let document = document.clone();
                move |this, _, event, cx| match event {
                    VideoViewEvent::Remove => this.project.update(cx, |project, cx| {
                        if let Some(ix) = project.video_index(&document) {
                            project.remove_video(ix, cx);
                        }
                    }),
                }
            }),
        ];
        VideoPanel {
            document,
            view,
            _subscriptions,
        }
    }

    /// The view of the video that's shown.
    fn active_video_view(&self, cx: &App) -> Option<Entity<VideoView>> {
        let ix = self.project.read(cx).active_video_index()?;
        Some(self.video_panels.get(ix)?.view.clone())
    }

    /// Tabs for the project's videos above the one that's shown, or a prompt to add
    /// some.
    fn video_area(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.active_video_view(cx) else {
            let project = self.project.clone();
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(empty_screen(
                    "No videos in the project",
                    None,
                    "add-videos",
                    "Add Videos",
                    cx,
                    move |cx| add_videos(project.clone(), cx),
                ))
                .into_any_element();
        };

        let project = self.project.read(cx);
        let tabs: Vec<_> = project
            .videos()
            .iter()
            .map(|video| {
                let name = video.read(cx).path().map(file_name).unwrap_or_default();
                Tab::new().label(name)
            })
            .collect();
        let tab_bar = TabBar::new("video-tabs")
            .selected_index(project.active_video_index().unwrap_or_default())
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.project
                    .update(cx, |project, cx| project.set_active_video(*ix, cx))
            }))
            .children(tabs);

        v_flex()
            .size_full()
            .child(tab_bar)
            .child(div().flex_1().min_h_0().child(view))
            .into_any_element()
    }

    /// Rebuilds the menus if a project, PDF or video was opened or closed, so their
    /// items are enabled only while there's something to act on. Rebuilding closes an
    /// open menu, so it isn't done on every change to the documents.
    fn update_menus(&mut self, cx: &mut Context<Self>) {
        let state = MenuState::of(&self.project, cx);
        if state == self.menus_built_for {
            return;
        }
        self.menus_built_for = state;
        menus::set_menus(state, cx);
        self.menu_bar.update(cx, |menu_bar, cx| menu_bar.reload(cx));
    }

    /// Shown until a project is created or opened.
    fn no_project_screen(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self.project.clone();
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .child("No project open")
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("new-project")
                            .label("New Project")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.project
                                    .update(cx, |project, cx| project.new_project(cx))
                            })),
                    )
                    .child(
                        Button::new("open-project")
                            .label("Open Project")
                            .on_click(move |_, _, cx| open_project(project.clone(), cx)),
                    ),
            )
    }
}

/// Handles the menu actions that act on the views. They're registered on the app
/// rather than on an element, since an element only gets actions while it or one of
/// its children has focus, and initially nothing has.
fn register_view_actions(this: WeakEntity<SlidelyUi>, window: AnyWindowHandle, cx: &mut App) {
    let pdf_view = |ui: &SlidelyUi, _: &App| Some(ui.pdf_view.clone());
    let video_view = |ui: &SlidelyUi, cx: &App| ui.active_video_view(cx);

    on_view_action(
        &this,
        window,
        pdf_view,
        cx,
        |view, _: &PreviousSlide, _, cx| view.step_page(-1, cx),
    );
    on_view_action(&this, window, pdf_view, cx, |view, _: &NextSlide, _, cx| {
        view.step_page(1, cx)
    });
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
/// is dispatched, if it picks one.
fn on_view_action<A: Action + Clone, V: 'static>(
    this: &WeakEntity<SlidelyUi>,
    window: AnyWindowHandle,
    view: fn(&SlidelyUi, &App) -> Option<Entity<V>>,
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
                    let Some(view) = view(this.read(cx), cx) else {
                        return;
                    };
                    view.update(cx, |view, cx| handler(view, &action, window, cx));
                })
                .ok();
        });
    });
}

impl Render for SlidelyUi {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(TitleBar::new().child(self.menu_bar.clone()));
        if !self.project.read(cx).is_open() {
            return root.child(self.no_project_screen(cx));
        }

        let settings = cx.global::<Settings>();
        let (pdf_view_visible, video_view_visible) =
            (settings.pdf_view_visible, settings.video_view_visible);
        root.child(
            h_resizable("main-view")
                .child(
                    resizable_panel()
                        .visible(pdf_view_visible)
                        .child(self.pdf_view.clone()),
                )
                .child(
                    resizable_panel()
                        .visible(video_view_visible)
                        .child(self.video_area(cx)),
                ),
        )
        .child(self.timeline_view.clone())
    }
}
