//! The pages of all of a project's PDFs as one sequence, the way they're shown.

use std::ops::Range;

use gpui_kit::{App, Entity, Pixels, Size};

use crate::pdf::{PdfDocument, PdfState};

/// The pages of a project's loaded PDFs, one PDF after the other in project order.
/// PDFs still loading, or that couldn't be opened, have no pages yet. A snapshot; it
/// doesn't change when the PDFs do.
pub struct Slides {
    /// Each loaded PDF and where its pages are in `sizes`, in order.
    pub documents: Vec<(Entity<PdfDocument>, Range<usize>)>,
    /// Size of each page in PDF points, as displayed (i.e. rotated).
    pub sizes: Vec<Size<Pixels>>,
}

impl Slides {
    pub fn new(pdfs: &[Entity<PdfDocument>], cx: &App) -> Self {
        let mut documents = Vec::new();
        let mut sizes = Vec::new();
        for pdf in pdfs {
            if let PdfState::Loaded(loaded) = pdf.read(cx).state() {
                let start = sizes.len();
                sizes.extend_from_slice(loaded.page_sizes());
                documents.push((pdf.clone(), start..sizes.len()));
            }
        }
        Self { documents, sizes }
    }

    pub fn len(&self) -> usize {
        self.sizes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sizes.is_empty()
    }

    /// The PDF that page `ix` is from, and the page's index in it.
    pub fn locate(&self, ix: usize) -> Option<(&Entity<PdfDocument>, usize)> {
        self.documents
            .iter()
            .find(|(_, pages)| pages.contains(&ix))
            .map(|(document, pages)| (document, ix - pages.start))
    }
}
