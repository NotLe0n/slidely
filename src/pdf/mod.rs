//! Loading and rasterizing PDF documents. Nothing in here renders UI.

mod loading;
mod render;

pub use loading::{LoadingPdf, PdfDocument, PdfState, close_pdf, open_pdf};
pub use render::PdfData;
