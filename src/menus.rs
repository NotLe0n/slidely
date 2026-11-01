//! Application actions and the menu bar.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::GlobalState;
use gpui_kit::*;

use crate::{
    Settings,
    project::{Project, confirm_discard, open_project, save_project},
    ui::open_manage_project,
    video::PLAYBACK_SPEEDS,
};

actions!(
    slidely,
    [Quit, NewProject, OpenProject, SaveProject, CloseProject]
);
actions!(slidely, [ManageProject, TogglePdfView, ToggleVideoView]);
actions!(slidely, [ToggleVideoFollowsSlides, ToggleSlidesFollowVideo]);
// Handled by the views, since they hold the zoom level and the playback controls.
actions!(
    slidely,
    [ZoomIn, ZoomOut, ResetZoom, FitPage, RotatePageClockwise]
);
actions!(slidely, [PreviousSlide, NextSlide]);
actions!(slidely, [TogglePlayback, ToggleMute]);

/// Sets the video's playback speed to the given factor.
#[derive(Action, Clone, PartialEq)]
#[action(namespace = slidely, no_json)]
pub struct SetPlaybackSpeed(pub f64);

/// What the menu items act on; the items are disabled when there's nothing to act on.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct MenuState {
    pub project_open: bool,
    /// Pages of a PDF are shown.
    pub pdf_loaded: bool,
    /// The video that's shown is loaded.
    pub video_loaded: bool,
}

impl MenuState {
    pub fn of(project: &Entity<Project>, cx: &App) -> Self {
        let project = project.read(cx);
        Self {
            project_open: project.is_open(),
            pdf_loaded: project.pdfs().iter().any(|pdf| pdf.read(cx).is_loaded()),
            video_loaded: project
                .active_video()
                .is_some_and(|video| video.read(cx).is_loaded()),
        }
    }
}

/// Registers the app's actions and menus, which act on `project`. The actions acting
/// on the views are registered by the root view.
pub fn init(project: Entity<Project>, cx: &mut App) {
    // The actions replacing the open project ask to save its changes first.
    let this = project.clone();
    cx.on_action(move |_: &Quit, cx| confirm_discard(this.clone(), cx, |cx| cx.quit()));

    let this = project.clone();
    cx.on_action(move |_: &NewProject, cx| {
        let project = this.clone();
        confirm_discard(this.clone(), cx, move |cx| {
            project.update(cx, |project, cx| project.new_project(cx))
        });
    });
    let this = project.clone();
    cx.on_action(move |_: &OpenProject, cx| {
        let project = this.clone();
        confirm_discard(this.clone(), cx, move |cx| open_project(project, cx));
    });
    let this = project.clone();
    cx.on_action(move |_: &SaveProject, cx| save_project(this.clone(), cx));

    // The Manage Project window's handle, so it's only opened once.
    let manage_window: Rc<RefCell<Option<AnyWindowHandle>>> = Rc::new(RefCell::new(None));
    let this = project.clone();
    let window = manage_window.clone();
    cx.on_action(move |_: &CloseProject, cx| {
        let project = this.clone();
        let window = window.clone();
        confirm_discard(this.clone(), cx, move |cx| {
            project.update(cx, |project, cx| project.close_project(cx));
            // There's no project left to manage.
            if let Some(handle) = window.borrow_mut().take() {
                handle
                    .update(cx, |_, window, _| window.remove_window())
                    .ok();
            }
        });
    });
    let this = project.clone();
    cx.on_action(move |_: &ManageProject, cx| {
        if this.read(cx).is_open() {
            open_manage_project(this.clone(), &mut manage_window.borrow_mut(), cx);
        }
    });
    cx.on_action(|_: &TogglePdfView, cx| {
        Settings::toggle(cx, |settings| &mut settings.pdf_view_visible)
    });
    cx.on_action(|_: &ToggleVideoView, cx| {
        Settings::toggle(cx, |settings| &mut settings.video_view_visible)
    });
    cx.on_action(|_: &ToggleVideoFollowsSlides, cx| {
        Settings::toggle(cx, |settings| &mut settings.video_follows_slides)
    });
    cx.on_action(|_: &ToggleSlidesFollowVideo, cx| {
        Settings::toggle(cx, |settings| &mut settings.slides_follow_video)
    });
    let this = project.clone();
    cx.on_action(move |_: &TogglePlayback, cx| {
        if let Some(video) = this.read(cx).active_video().cloned() {
            video.update(cx, |video, cx| video.toggle_playback(cx));
        }
    });
    cx.on_action(|_: &ToggleMute, cx| Settings::toggle(cx, |settings| &mut settings.muted));
    cx.on_action(|action: &SetPlaybackSpeed, cx| {
        cx.update_global::<Settings, _>(|settings, _| settings.playback_speed = action.0)
    });

    cx.bind_keys([
        KeyBinding::new("secondary-n", NewProject, None),
        KeyBinding::new("secondary-o", OpenProject, None),
        KeyBinding::new("secondary-s", SaveProject, None),
        KeyBinding::new("secondary-q", CloseProject, None),
        KeyBinding::new("secondary-m", ManageProject, None),
        KeyBinding::new("alt-left", TogglePdfView, None),
        KeyBinding::new("alt-right", ToggleVideoView, None),
        KeyBinding::new("left", PreviousSlide, None),
        KeyBinding::new("right", NextSlide, None),
        KeyBinding::new("secondary-plus", ZoomIn, None),
        KeyBinding::new("secondary-minus", ZoomOut, None),
    ]);

    set_menus(MenuState::default(), cx);
}

/// Sets the menu bar's menus, with the items disabled that have nothing to act on.
/// `AppMenuBar` has to be reloaded afterwards to show the change.
pub fn set_menus(state: MenuState, cx: &mut App) {
    let no_project = !state.project_open;
    let no_pdf = !state.pdf_loaded;
    let no_video = !state.video_loaded;
    let speed_items = PLAYBACK_SPEEDS.iter().map(|&speed| {
        MenuItem::action(format!("{speed}×"), SetPlaybackSpeed(speed)).disabled(no_video)
    });

    cx.set_menus([
        Menu::new("File").items([
            MenuItem::action("New Project", NewProject),
            MenuItem::action("Open Project", OpenProject),
            MenuItem::action("Save Project", SaveProject).disabled(no_project),
            MenuItem::action("Close Project", CloseProject).disabled(no_project),
            MenuItem::separator(),
            MenuItem::action("Quit", Quit),
        ]),
        Menu::new("View").items([
            MenuItem::action("Manage Project…", ManageProject).disabled(no_project),
            MenuItem::separator(),
            MenuItem::action("Toggle PDF View", TogglePdfView).disabled(no_project),
            MenuItem::action("Toggle Video View", ToggleVideoView).disabled(no_project),
            MenuItem::separator(),
            MenuItem::action("Video Follows Slides", ToggleVideoFollowsSlides).disabled(no_project),
            MenuItem::action("Slides Follow Video", ToggleSlidesFollowVideo).disabled(no_project),
            MenuItem::separator(),
            MenuItem::action("Previous Slide", PreviousSlide).disabled(no_pdf),
            MenuItem::action("Next Slide", NextSlide).disabled(no_pdf),
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
