//! A project: the PDFs of a lecture's slides and the videos of the lecture, saved
//! together with what's been extracted from them. Nothing in here renders UI.

mod alignment;
mod file;
mod slides;

use std::path::{Path, PathBuf};

use gpui_kit::{
    App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, PromptButton, PromptLevel,
};
use rfd::AsyncFileDialog;

use crate::observe::ObservedDocuments;
use crate::pdf::{PageText, PdfDocument};
use crate::video::{Segment, VIDEO_EXTENSIONS, VideoDocument};
use file::{EXTENSION, PdfEntry, ProjectFile, VideoEntry};
pub use slides::Slides;

/// The open project. Its PDFs and videos are separate entities, so the views showing
/// them only redraw when they change.
pub struct Project {
    /// Until a project is created or opened, there's nothing to add PDFs or videos
    /// to.
    is_open: bool,
    /// Where the project was opened from or last saved to; `None` until it's saved.
    path: Option<PathBuf>,
    /// In the order they're shown in, one after the other. Each one is loaded, and
    /// its text extracted, as soon as it's added.
    pdfs: Vec<Entity<PdfDocument>>,
    /// In the order they're shown in the tabs. Each one is loaded as soon as it's
    /// added, and then transcribed one after the other in this order.
    videos: Vec<Entity<VideoDocument>>,
    /// Index into `videos` of the one shown; `None` only if there are none.
    active_video: Option<usize>,
    /// Starts the next transcription when one finishes.
    observed_videos: ObservedDocuments<VideoDocument>,
    /// Updates the alignment when a PDF's text comes in.
    observed_pdfs: ObservedDocuments<PdfDocument>,
    /// Where each slide is discussed in the videos.
    alignment: alignment::ProjectAlignment,
    /// What was last opened or saved, to tell whether there are unsaved changes.
    saved: ProjectFile,
}

pub enum ProjectEvent {
    Saved(PathBuf),
    /// Opening or saving the project failed.
    Error(String),
}

impl EventEmitter<ProjectEvent> for Project {}

impl Project {
    /// No project is open initially.
    pub fn new() -> Self {
        Self {
            is_open: false,
            path: None,
            pdfs: Vec::new(),
            videos: Vec::new(),
            active_video: None,
            observed_videos: ObservedDocuments::new(),
            observed_pdfs: ObservedDocuments::new(),
            alignment: Default::default(),
            saved: ProjectFile::default(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// Whether something would be lost by closing the project: files were added,
    /// removed or moved, or text or transcripts were extracted since it was last
    /// opened or saved.
    pub fn has_unsaved_changes(&self, cx: &App) -> bool {
        self.is_open && !self.to_file(cx).same_content(&self.saved)
    }

    pub fn pdfs(&self) -> &[Entity<PdfDocument>] {
        &self.pdfs
    }

    /// The pages of all loaded PDFs, the way they're shown.
    pub fn slides(&self, cx: &App) -> Slides {
        Slides::new(&self.pdfs, cx)
    }

    pub fn videos(&self) -> &[Entity<VideoDocument>] {
        &self.videos
    }

    pub fn active_video_index(&self) -> Option<usize> {
        self.active_video
    }

    /// The video that's shown.
    pub fn active_video(&self) -> Option<&Entity<VideoDocument>> {
        self.videos.get(self.active_video?)
    }

    /// Where `video` is among the project's videos.
    pub fn video_index(&self, video: &Entity<VideoDocument>) -> Option<usize> {
        self.videos.iter().position(|v| v == video)
    }

    /// Shows the video at `ix`, pausing the one shown before.
    pub fn set_active_video(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.videos.len() || self.active_video == Some(ix) {
            return;
        }
        if let Some(video) = self.active_video() {
            video.update(cx, |video, cx| video.pause(cx));
        }
        self.active_video = Some(ix);
        cx.notify();
    }

    /// Adds the PDF at `path` after the others and starts loading it, with the text
    /// of its pages if that's known already. A PDF that's in the project already
    /// isn't added again, but loaded again if it couldn't be opened.
    fn add_pdf(
        &mut self,
        path: PathBuf,
        page_texts: Option<Vec<PageText>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(pdf) = self
            .pdfs
            .iter()
            .find(|pdf| pdf.read(cx).path() == Some(path.as_path()))
            .cloned()
        {
            pdf.update(cx, |pdf, cx| pdf.retry(cx));
            return;
        }
        let pdf = cx.new(|cx| {
            let mut pdf = PdfDocument::new();
            pdf.load(path, page_texts, cx);
            pdf
        });
        self.pdfs.push(pdf);
        self.observe_pdfs(cx);
        cx.notify();
    }

    /// Removes the PDF at `ix` from the project.
    pub fn remove_pdf(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.pdfs.len() {
            return;
        }
        let pdf = self.pdfs.remove(ix);
        // Frees its rendered pages, which dropping it wouldn't.
        pdf.update(cx, |pdf, cx| pdf.close(cx));
        self.observe_pdfs(cx);
        self.update_alignment(cx);
        cx.notify();
    }

    /// Moves the PDF at `from` to `to`, shifting the ones in between.
    pub fn move_pdf(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from == to || from >= self.pdfs.len() || to >= self.pdfs.len() {
            return;
        }
        let pdf = self.pdfs.remove(from);
        self.pdfs.insert(to, pdf);
        self.update_alignment(cx);
        cx.notify();
    }

    /// Adds the video at `path` after the others and starts loading it. Once it's
    /// loaded, it's queued for transcription unless its `transcript` is given. A video
    /// that's in the project already isn't added again, but loaded again if it
    /// couldn't be opened.
    fn add_video(
        &mut self,
        path: PathBuf,
        transcript: Option<Vec<Segment>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(video) = self
            .videos
            .iter()
            .find(|video| video.read(cx).path() == Some(path.as_path()))
            .cloned()
        {
            video.update(cx, |video, cx| video.retry(cx));
            return;
        }
        let video = cx.new(|cx| {
            let mut video = VideoDocument::new();
            video.load(path, transcript, cx);
            video
        });
        self.videos.push(video);
        self.active_video.get_or_insert(0);
        self.observe_videos(cx);
        cx.notify();
    }

    /// Removes the video at `ix` from the project, cancelling its transcription.
    pub fn remove_video(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.videos.len() {
            return;
        }
        let video = self.videos.remove(ix);
        video.update(cx, |video, cx| video.close(cx));
        // The video after it is shown, or the one before if it was the last.
        self.active_video = match self.active_video {
            _ if self.videos.is_empty() => None,
            Some(active) if active > ix || active == self.videos.len() => Some(active - 1),
            active => active,
        };
        self.observe_videos(cx);
        self.start_next_transcription(cx);
        self.update_alignment(cx);
        cx.notify();
    }

    /// Moves the video at `from` to `to`, shifting the ones in between. The video
    /// shown stays the same.
    pub fn move_video(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from == to || from >= self.videos.len() || to >= self.videos.len() {
            return;
        }
        let active = self.active_video().cloned();
        let video = self.videos.remove(from);
        self.videos.insert(to, video);
        self.active_video = active.and_then(|active| self.video_index(&active));
        self.observe_videos(cx);
        self.update_alignment(cx);
        cx.notify();
    }

    /// Observes the current videos, after one was added or removed.
    fn observe_videos(&mut self, cx: &mut Context<Self>) {
        let videos = self.videos.clone();
        self.observed_videos.observe(&videos, cx, |this, cx| {
            this.start_next_transcription(cx);
            this.update_alignment(cx);
        });
    }

    /// Observes the current PDFs, after one was added or removed.
    fn observe_pdfs(&mut self, cx: &mut Context<Self>) {
        let pdfs = self.pdfs.clone();
        self.observed_pdfs
            .observe(&pdfs, cx, |this, cx| this.update_alignment(cx));
    }

    /// Starts transcribing the first video waiting for it, unless a transcription is
    /// running. Only one runs at a time, since they compete for the GPU. If the first
    /// one is still loading, that's waited for, so they're transcribed in order.
    fn start_next_transcription(&mut self, cx: &mut Context<Self>) {
        if self
            .videos
            .iter()
            .any(|video| video.read(cx).is_transcribing())
        {
            return;
        }
        let next = self
            .videos
            .iter()
            .find(|video| video.read(cx).awaits_transcription());
        if let Some(next) = next
            && next.read(cx).is_queued()
        {
            next.clone()
                .update(cx, |video, cx| video.start_transcription(cx));
        }
    }

    /// Replaces the open project, if any, with an empty one.
    pub fn new_project(&mut self, cx: &mut Context<Self>) {
        self.clear(cx);
        self.is_open = true;
        self.path = None;
        self.saved = ProjectFile::default();
        cx.notify();
    }

    /// Closes the open project, which shows the start screen again.
    pub fn close_project(&mut self, cx: &mut Context<Self>) {
        self.clear(cx);
        self.is_open = false;
        self.path = None;
        cx.notify();
    }

    /// Replaces the open project, if any, with `file`, which was read from `path`.
    fn open(&mut self, path: PathBuf, file: ProjectFile, cx: &mut Context<Self>) {
        self.clear(cx);
        for entry in file.pdfs {
            let (path, page_texts) = entry.into_parts();
            self.add_pdf(path, page_texts, cx);
        }
        for entry in file.videos {
            let (path, transcript) = entry.into_parts();
            self.add_video(path, transcript, cx);
        }
        self.is_open = true;
        self.path = Some(path);
        // What was read, as far as it was added, e.g. without duplicates.
        self.saved = self.to_file(cx);
        cx.notify();
    }

    /// Removes all PDFs and videos.
    fn clear(&mut self, cx: &mut Context<Self>) {
        self.observed_pdfs.clear();
        for pdf in self.pdfs.drain(..) {
            pdf.update(cx, |pdf, cx| pdf.close(cx));
        }
        self.observed_videos.clear();
        for video in self.videos.drain(..) {
            video.update(cx, |video, cx| video.close(cx));
        }
        self.active_video = None;
        self.clear_alignment();
    }

    /// What's saved in the project file. A transcript that's still running and
    /// page texts that are still being extracted are left out; they're redone when
    /// the project is opened. Those of files still loading or that couldn't be opened
    /// are kept.
    fn to_file(&self, cx: &App) -> ProjectFile {
        let pdf_entries = self
            .pdfs
            .iter()
            .filter_map(|pdf| {
                let pdf = pdf.read(cx);
                Some(PdfEntry::new(pdf.path()?.to_path_buf(), pdf.page_texts()))
            })
            .collect();
        let video_entries = self
            .videos
            .iter()
            .filter_map(|video| {
                let video = video.read(cx);
                Some(VideoEntry::new(
                    video.path()?.to_path_buf(),
                    video.finished_transcript(),
                ))
            })
            .collect();
        ProjectFile::new(pdf_entries, video_entries)
    }
}

/// Asks the user for PDF files and adds them to the open project.
pub fn add_pdfs(project: Entity<Project>, cx: &mut App) {
    if !project.read(cx).is_open {
        return;
    }
    cx.spawn(async move |app| {
        let selected_files = AsyncFileDialog::new()
            .add_filter("pdf", &["pdf"])
            .pick_files()
            .await;
        let Some(mut files) = selected_files else {
            return;
        };
        // The dialog doesn't keep the order they were selected in.
        files.sort_by(|a, b| a.path().cmp(b.path()));

        app.update_entity(&project, |project, cx| {
            // The project may have been closed while the dialog was up.
            if !project.is_open {
                return;
            }
            for file in files {
                project.add_pdf(file.path().to_path_buf(), None, cx);
            }
        });
    })
    .detach();
}

/// Asks the user for video files and adds them to the open project. They're
/// transcribed one after the other, in the order they were selected in.
pub fn add_videos(project: Entity<Project>, cx: &mut App) {
    if !project.read(cx).is_open {
        return;
    }
    cx.spawn(async move |app| {
        let selected_files = AsyncFileDialog::new()
            .add_filter("video", VIDEO_EXTENSIONS)
            .pick_files()
            .await;
        let Some(mut files) = selected_files else {
            return;
        };
        // The dialog doesn't keep the order they were selected in, so they're added
        // by name, which keeps numbered parts in order.
        files.sort_by(|a, b| a.path().cmp(b.path()));

        app.update_entity(&project, |project, cx| {
            // The project may have been closed while the dialog was up.
            if !project.is_open {
                return;
            }
            for file in files {
                project.add_video(file.path().to_path_buf(), None, cx);
            }
        });
    })
    .detach();
}

/// Asks the user for a project file and opens it, replacing the open project.
pub fn open_project(project: Entity<Project>, cx: &mut App) {
    cx.spawn(async move |app| {
        let selected_file = AsyncFileDialog::new()
            .add_filter("Slidely project", &[EXTENSION])
            .pick_file()
            .await;
        let Some(file) = selected_file else {
            return;
        };

        let path = file.path().to_path_buf();
        let result = app
            .background_executor()
            .spawn({
                let path = path.clone();
                async move { ProjectFile::read(&path) }
            })
            .await;
        app.update_entity(&project, |project, cx| match result {
            Ok(file) => project.open(path, file, cx),
            Err(err) => cx.emit(ProjectEvent::Error(format!(
                "Couldn't open {}: {err:#}",
                path.display()
            ))),
        });
    })
    .detach();
}

/// Saves the open project to where it was opened from or last saved to, asking the
/// user where to save it the first time.
pub fn save_project(project: Entity<Project>, cx: &mut App) {
    cx.spawn(async move |app| {
        save(&project, app).await;
    })
    .detach();
}

/// Saves the project like `save_project`. Returns whether it was saved; not if the
/// user cancelled choosing where, or writing failed.
async fn save(project: &Entity<Project>, app: &mut AsyncApp) -> bool {
    let Some((file, path)) = app.update_entity(project, |project, cx| {
        project
            .is_open
            .then(|| (project.to_file(cx), project.path.clone()))
    }) else {
        return false;
    };

    let path = match path {
        Some(path) => path,
        None => {
            let selected_file = AsyncFileDialog::new()
                .add_filter("Slidely project", &[EXTENSION])
                .set_file_name(format!("Untitled.{EXTENSION}"))
                .save_file()
                .await;
            let Some(file) = selected_file else {
                return false;
            };
            with_extension(file.path())
        }
    };

    let result = app
        .background_executor()
        .spawn({
            let file = file.clone();
            let path = path.clone();
            async move { file.write(&path) }
        })
        .await;
    app.update_entity(project, |project, cx| match result {
        Ok(()) => {
            project.path = Some(path.clone());
            project.saved = file;
            cx.emit(ProjectEvent::Saved(path));
            cx.notify();
            true
        }
        Err(err) => {
            cx.emit(ProjectEvent::Error(format!(
                "Couldn't save {}: {err:#}",
                path.display()
            )));
            false
        }
    })
}

/// Calls `then` unless the project has unsaved changes. If it has, the user is asked
/// whether to save them first, and `then` is only called once they're saved or the
/// user chose to discard them; not if they cancel.
pub fn confirm_discard(
    project: Entity<Project>,
    cx: &mut App,
    then: impl FnOnce(&mut App) + 'static,
) {
    if !project.read(cx).has_unsaved_changes(cx) {
        then(cx);
        return;
    }
    let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) else {
        then(cx);
        return;
    };
    let answer = window.update(cx, |_, window, cx| {
        window.prompt(
            PromptLevel::Warning,
            "Save changes to the project?",
            Some("Your changes will be lost if you don't save them."),
            &[
                PromptButton::ok("Save"),
                PromptButton::new("Don't Save"),
                PromptButton::cancel("Cancel"),
            ],
            cx,
        )
    });
    let Ok(answer) = answer else {
        then(cx);
        return;
    };

    cx.spawn(async move |app| {
        let discard = match answer.await {
            Ok(0) => save(&project, app).await,
            Ok(1) => true,
            _ => false,
        };
        if discard {
            app.update(then);
        }
    })
    .detach();
}

/// `path`, with the project file extension added unless it has it.
fn with_extension(path: &Path) -> PathBuf {
    if path
        .extension()
        .is_some_and(|extension| extension == EXTENSION)
    {
        path.to_path_buf()
    } else {
        let mut path = path.as_os_str().to_owned();
        path.push(format!(".{EXTENSION}"));
        PathBuf::from(path)
    }
}
