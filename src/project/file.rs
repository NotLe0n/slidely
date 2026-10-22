//! The project file: JSON with the paths to the PDFs and the videos, and what's been
//! extracted from them so it doesn't have to be done again.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

use crate::pdf::PageText;
use crate::video::Segment;

/// The extension of project files.
pub const EXTENSION: &str = "slidely";
/// Bumped when the format changes in a way older versions can't read.
/// 1: a single `pdf`. 2: a list of `pdfs`. 3: a list of `videos`.
const VERSION: u32 = 3;

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectFile {
    version: u32,
    #[serde(default)]
    pub pdfs: Vec<PdfEntry>,
    /// The single PDF of version 1; moved into `pdfs` when read.
    #[serde(default, skip_serializing)]
    pdf: Option<PdfEntry>,
    #[serde(default)]
    pub videos: Vec<VideoEntry>,
    /// The single video of versions 1 and 2; moved into `videos` when read.
    #[serde(default, skip_serializing)]
    video: Option<VideoEntry>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PdfEntry {
    /// Relative to the project file if it's in the same folder or below.
    pub path: PathBuf,
    /// The text of each page; missing until it's extracted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pages: Option<Vec<PageText>>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoEntry {
    /// Relative to the project file if it's in the same folder or below.
    pub path: PathBuf,
    /// Missing until the transcription is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transcript: Option<Vec<SegmentEntry>>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct SegmentEntry {
    /// In seconds.
    start: f64,
    /// In seconds.
    end: f64,
    text: String,
}

impl ProjectFile {
    pub fn new(pdfs: Vec<PdfEntry>, videos: Vec<VideoEntry>) -> Self {
        Self {
            version: VERSION,
            pdfs,
            pdf: None,
            videos,
            video: None,
        }
    }

    /// Whether both have the same PDFs and videos, with the same extracted text and
    /// transcripts.
    pub fn same_content(&self, other: &Self) -> bool {
        self.pdfs == other.pdfs && self.videos == other.videos
    }

    /// Reads the project file at `path`. Blocking; run it on a background thread.
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let mut file: Self = serde_json::from_str(&json).context("not a valid project file")?;
        if file.version > VERSION {
            bail!("it was saved by a newer version of Slidely");
        }

        file.pdfs.extend(file.pdf.take());
        file.videos.extend(file.video.take());

        let dir = path.parent().unwrap_or(Path::new(""));
        for pdf in &mut file.pdfs {
            pdf.path = dir.join(&pdf.path);
        }
        for video in &mut file.videos {
            video.path = dir.join(&video.path);
        }
        Ok(file)
    }

    /// Writes the project file to `path`, replacing what's there. Blocking; run it on
    /// a background thread.
    pub fn write(mut self, path: &Path) -> anyhow::Result<()> {
        // Read from an older version, it's saved in the current format.
        self.version = VERSION;
        // Relative paths keep working when the folder is moved or shared.
        let dir = path.parent().unwrap_or(Path::new(""));
        let relative = |file: &mut PathBuf| {
            if let Ok(relative) = file.strip_prefix(dir) {
                *file = relative.to_path_buf();
            }
        };
        for pdf in &mut self.pdfs {
            relative(&mut pdf.path);
        }
        for video in &mut self.videos {
            relative(&mut video.path);
        }

        let json = serde_json::to_string_pretty(&self)?;
        // Written next to it first, so a failed write doesn't destroy the old file.
        let temp_path = path.with_extension(format!("{EXTENSION}.tmp"));
        std::fs::write(&temp_path, json)?;
        std::fs::rename(&temp_path, path)?;
        Ok(())
    }
}

impl PdfEntry {
    pub fn new(path: PathBuf, pages: Option<&[PageText]>) -> Self {
        Self {
            path,
            pages: pages.map(<[_]>::to_vec),
        }
    }

    /// The PDF's path, and the saved text of each page if it was extracted.
    pub fn into_parts(self) -> (PathBuf, Option<Vec<PageText>>) {
        (self.path, self.pages)
    }
}

impl VideoEntry {
    pub fn new(path: PathBuf, transcript: Option<&[Segment]>) -> Self {
        let transcript = transcript.map(|segments| {
            segments
                .iter()
                .map(|segment| SegmentEntry {
                    start: segment.start.as_secs_f64(),
                    end: segment.end.as_secs_f64(),
                    text: segment.text.clone(),
                })
                .collect()
        });
        Self { path, transcript }
    }

    /// The video's path, and the saved transcript if the transcription was done.
    pub fn into_parts(self) -> (PathBuf, Option<Vec<Segment>>) {
        let seconds = |seconds: f64| Duration::try_from_secs_f64(seconds).unwrap_or_default();
        let transcript = self.transcript.map(|segments| {
            segments
                .into_iter()
                .map(|segment| Segment {
                    start: seconds(segment.start),
                    end: seconds(segment.end),
                    text: segment.text,
                })
                .collect()
        });
        (self.path, transcript)
    }
}
