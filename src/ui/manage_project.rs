//! A window for adding, removing and reordering the project's PDFs and videos.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::observe::ObservedDocuments;
use super::placeholders::file_name;
use super::video_view::progress_label;
use crate::pdf::{PdfDocument, PdfState};
use crate::project::{Project, add_pdfs, add_videos};
use crate::video::{TranscriptState, VideoDocument, VideoState};

/// The project's PDFs and videos, each in their order, with buttons to change them.
pub struct ManageProjectView {
    project: Entity<Project>,
    /// Show each file's state as it changes.
    pdfs: ObservedDocuments<PdfDocument>,
    videos: ObservedDocuments<VideoDocument>,
    _subscription: Subscription,
}

/// Which of the project's lists.
#[derive(Clone, Copy)]
enum List {
    Pdfs,
    Videos,
}

/// What a list shows for one file.
struct Row {
    name: String,
    status: String,
    failed: bool,
    /// The button for its transcription, if one applies.
    transcription: Option<Transcription>,
}

/// What a video's transcription button does.
#[derive(Clone, Copy)]
enum Transcription {
    /// Queues it; for a video that isn't transcribed, e.g. after it was stopped or
    /// failed.
    Start,
    /// Stops it while it's running.
    Stop,
}

impl ManageProjectView {
    pub fn new(project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let _subscription = cx.observe(&project, |this, _, cx| {
            this.observe_documents(cx);
            cx.notify();
        });
        let mut this = Self {
            project,
            pdfs: ObservedDocuments::new(),
            videos: ObservedDocuments::new(),
            _subscription,
        };
        this.observe_documents(cx);
        this
    }

    /// Observes the project's current files, after one was added or removed.
    fn observe_documents(&mut self, cx: &mut Context<Self>) {
        self.pdfs
            .observe(&self.project, Project::pdfs, cx, |_, cx| cx.notify());
        self.videos
            .observe(&self.project, Project::videos, cx, |_, cx| cx.notify());
    }

    /// Copied out so the documents aren't borrowed while building the list.
    fn pdf_rows(&self, cx: &App) -> Vec<Row> {
        self.project
            .read(cx)
            .pdfs()
            .iter()
            .map(|pdf| {
                let pdf = pdf.read(cx);
                let (status, failed) = match pdf.state() {
                    PdfState::Loaded(loaded) => {
                        let pages = loaded.page_sizes().len();
                        let status = if loaded.page_texts().is_some() {
                            format!("{pages} pages")
                        } else {
                            format!("{pages} pages, reading text…")
                        };
                        (status, false)
                    }
                    PdfState::Loading(_) => ("Loading…".into(), false),
                    PdfState::Failed { error, .. } => (error.clone(), true),
                    PdfState::None => (String::new(), false),
                };
                Row {
                    name: pdf.path().map(file_name).unwrap_or_default(),
                    status,
                    failed,
                    transcription: None,
                }
            })
            .collect()
    }

    fn video_rows(&self, cx: &App) -> Vec<Row> {
        self.project
            .read(cx)
            .videos()
            .iter()
            .map(|video| {
                let video = video.read(cx);
                let (status, failed, transcription) = match video.state() {
                    VideoState::Loaded(loaded) => match &loaded.transcript {
                        TranscriptState::None => {
                            ("Not transcribed".into(), false, Some(Transcription::Start))
                        }
                        TranscriptState::Queued => ("Queued for transcription".into(), false, None),
                        TranscriptState::Running(running) => {
                            (progress_label(running), false, Some(Transcription::Stop))
                        }
                        TranscriptState::Done(_) => ("Transcribed".into(), false, None),
                        TranscriptState::Failed(error) => (
                            format!("Transcription failed: {error}"),
                            true,
                            Some(Transcription::Start),
                        ),
                    },
                    VideoState::Loading(_) => ("Loading…".into(), false, None),
                    VideoState::Failed { error, .. } => (error.clone(), true, None),
                    VideoState::None => (String::new(), false, None),
                };
                Row {
                    name: video.path().map(file_name).unwrap_or_default(),
                    status,
                    failed,
                    transcription,
                }
            })
            .collect()
    }

    fn row(
        &self,
        list: List,
        ix: usize,
        count: usize,
        row: Row,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = match list {
            List::Pdfs => "pdf",
            List::Videos => "video",
        };
        let move_button = |id: SharedString, icon: IconName, tooltip: &'static str, to: usize| {
            Button::new((id, ix))
                .ghost()
                .small()
                .icon(icon)
                .tooltip(tooltip)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.project.update(cx, |project, cx| match list {
                        List::Pdfs => project.move_pdf(ix, to, cx),
                        List::Videos => project.move_video(ix, to, cx),
                    })
                }))
        };
        let status_color = if row.failed {
            cx.theme().danger
        } else {
            cx.theme().muted_foreground
        };

        h_flex()
            .gap_2()
            .px_3()
            .py_2()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().child(row.name))
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(status_color)
                            .child(row.status),
                    ),
            )
            .when_some(row.transcription, |el, transcription| {
                let (icon, tooltip) = match transcription {
                    Transcription::Start => (IconName::Captions, "Transcribe speech"),
                    Transcription::Stop => (IconName::CircleStop, "Stop transcription"),
                };
                el.child(
                    Button::new((SharedString::from(format!("{id}-transcription")), ix))
                        .ghost()
                        .small()
                        .icon(icon)
                        .tooltip(tooltip)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let Some(video) = this.project.read(cx).videos().get(ix).cloned()
                            else {
                                return;
                            };
                            video.update(cx, |video, cx| match transcription {
                                Transcription::Start => video.queue_transcription(cx),
                                Transcription::Stop => video.cancel_transcription(cx),
                            });
                        })),
                )
            })
            .child(
                move_button(
                    format!("{id}-up").into(),
                    IconName::ChevronUp,
                    "Move up",
                    ix.saturating_sub(1),
                )
                .disabled(ix == 0),
            )
            .child(
                move_button(
                    format!("{id}-down").into(),
                    IconName::ChevronDown,
                    "Move down",
                    ix + 1,
                )
                .disabled(ix + 1 == count),
            )
            .child(
                Button::new((SharedString::from(format!("{id}-remove")), ix))
                    .ghost()
                    .small()
                    .icon(IconName::Delete)
                    .tooltip("Remove from the project")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.project.update(cx, |project, cx| match list {
                            List::Pdfs => project.remove_pdf(ix, cx),
                            List::Videos => project.remove_video(ix, cx),
                        })
                    })),
            )
    }

    /// One of the lists with its title, description and add button.
    fn section(&self, list: List, rows: Vec<Row>, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, description, empty, add_label) = match list {
            List::Pdfs => (
                "PDFs",
                "Shown one after the other, in this order.",
                "No PDFs in the project yet.",
                "Add PDFs…",
            ),
            List::Videos => (
                "Videos",
                "Shown in tabs, and transcribed one after the other, in this order.",
                "No videos in the project yet.",
                "Add Videos…",
            ),
        };
        let count = rows.len();
        let content = if rows.is_empty() {
            div()
                .text_color(cx.theme().muted_foreground)
                .child(empty)
                .into_any_element()
        } else {
            let rows: Vec<_> = rows
                .into_iter()
                .enumerate()
                .map(|(ix, row)| self.row(list, ix, count, row, cx).into_any_element())
                .collect();
            v_flex().gap_1().children(rows).into_any_element()
        };
        let project = self.project.clone();

        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_3()
            .child(div().font_semibold().child(title))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(description),
            )
            .child(
                div()
                    .id(title)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new(add_label)
                        .primary()
                        .icon(IconName::Plus)
                        .label(add_label)
                        .on_click(move |_, _, cx| match list {
                            List::Pdfs => add_pdfs(project.clone(), cx),
                            List::Videos => add_videos(project.clone(), cx),
                        }),
                ),
            )
    }
}

impl Render for ManageProjectView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pdf_rows = self.pdf_rows(cx);
        let video_rows = self.video_rows(cx);

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(TitleBar::new().child("Manage Project"))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .p_4()
                    .gap_4()
                    .child(self.section(List::Pdfs, pdf_rows, cx))
                    .child(div().w(px(1.)).bg(cx.theme().border))
                    .child(self.section(List::Videos, video_rows, cx)),
            )
    }
}

/// Opens the window for managing `project`, or brings it to the front if it's open.
/// `window` is where its handle is kept between calls.
pub fn open_manage_project(
    project: Entity<Project>,
    window: &mut Option<AnyWindowHandle>,
    cx: &mut App,
) {
    if let Some(handle) = window
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }

    let bounds = Bounds::centered(None, size(px(880.), px(480.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        ..TitleBar::window_options()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        Theme::sync_system_appearance(Some(window), cx);
        window
            .observe_window_appearance(|window, cx| Theme::sync_system_appearance(Some(window), cx))
            .detach();
        cx.new(|cx| ManageProjectView::new(project, cx))
    }) {
        Ok((handle, _)) => *window = Some(handle),
        Err(err) => eprintln!("Failed to open the Manage Project window: {err:#}"),
    }
}
