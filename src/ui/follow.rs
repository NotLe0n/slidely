//! Keeping the slides and the video in step, as the follow settings say.

use std::time::Duration;

use gpui_kit::*;

use super::pdf_view::{PdfView, PdfViewEvent};
use crate::Settings;
use crate::interval::spawn_interval;
use crate::project::Project;

/// How often the slides check where the video is, to follow it.
const FOLLOW_INTERVAL: Duration = Duration::from_millis(250);

/// Seeks the video to the slide scrolled to, and scrolls the slides to the one
/// discussed in the video, while the settings say so.
pub struct SlideFollower {
    project: Entity<Project>,
    pdf_view: Entity<PdfView>,
    /// Scrolls the slides to the one discussed in the video, while they follow it.
    follow_task: Option<Task<()>>,
    /// The slide discussed where the video was when last checked. The slides are
    /// only scrolled when that changes, so they can be scrolled elsewhere meanwhile.
    followed_slide: Option<usize>,
    _subscriptions: [Subscription; 2],
}

impl SlideFollower {
    pub fn new(
        project: Entity<Project>,
        pdf_view: Entity<PdfView>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _subscriptions = [
            cx.subscribe(&pdf_view, |this, _, event, cx| match event {
                PdfViewEvent::PageChanged(ix) => {
                    if cx.global::<Settings>().video_follows_slides {
                        this.project
                            .update(cx, |project, cx| project.play_slide(*ix, cx));
                    }
                }
            }),
            cx.observe_global::<Settings>(Self::update_follow_task),
        ];
        let mut this = Self {
            project,
            pdf_view,
            follow_task: None,
            followed_slide: None,
            _subscriptions,
        };
        this.update_follow_task(cx);
        this
    }

    /// Starts or stops the slides following the video, as the setting says.
    fn update_follow_task(&mut self, cx: &mut Context<Self>) {
        let follow = cx.global::<Settings>().slides_follow_video;
        if follow == self.follow_task.is_some() {
            return;
        }
        // Scrolls to the slide discussed right away.
        self.followed_slide = None;
        self.follow_task = follow.then(|| spawn_interval(FOLLOW_INTERVAL, cx, Self::follow_video));
    }

    /// Scrolls to the slide discussed where the video shown is, if that's another
    /// one than last time. Covers both playing and seeking.
    fn follow_video(&mut self, cx: &mut Context<Self>) {
        let slide = self.project.read(cx).current_slide(cx);
        if slide == self.followed_slide {
            return;
        }
        self.followed_slide = slide;
        if let Some(slide) = slide {
            self.pdf_view.update(cx, |pdf_view, cx| {
                if pdf_view.current_page(cx) != Some(slide) {
                    pdf_view.scroll_to_page(slide, cx);
                }
            });
        }
    }
}
