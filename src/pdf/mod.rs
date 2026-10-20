//! Loading and rasterizing PDF documents. Nothing in here renders UI.

mod document;
mod render;

pub use document::{PdfDocument, PdfState, open_pdf};

/// gpui pixels (1/96 in) per PDF point (1/72 in), i.e. the display scale at 100% zoom.
pub const PX_PER_POINT: f32 = 96. / 72.;
