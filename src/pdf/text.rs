//! Extracting the text of PDF pages, to match slides with what's said in a lecture.

use std::sync::atomic::{AtomicBool, Ordering};

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, SoftMask, interpret_page,
};
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::page::Page;
use hayro::kurbo::{Affine, BezPath, Point, Rect, Vec2};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Stands in for glyphs whose character isn't known.
pub const UNKNOWN_CHAR: &str = "\u{FFFD}";
/// Glyph outlines are in units of 1/1000 em.
const UNITS_PER_EM: f64 = 1000.;
/// A gap between two glyphs wider than this, in em, is taken as a space. PDFs
/// often position words rather than draw space characters.
const SPACE_GAP_EM: f64 = 0.15;
/// A glyph whose baseline is further than this, in em, from the line's is taken to
/// start a new line.
const LINE_GAP_EM: f64 = 0.5;

/// The text on a page, line by line in the order the PDF draws it, which is the
/// reading order for practically all slides.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PageText {
    pub lines: Vec<TextLine>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextLine {
    pub text: String,
    /// Font size in PDF points; larger for headings.
    pub font_size: f32,
}

/// Lines in at least this fraction of the largest font size count as being in it, so
/// a title isn't missed for being a little smaller than e.g. a symbol in it.
const TITLE_FONT_FRACTION: f32 = 0.95;

/// Which of a slide's lines is its title: the first one in the largest font.
pub fn title_index<'a>(lines: impl IntoIterator<Item = &'a TextLine>) -> Option<usize> {
    let sizes: Vec<f32> = lines.into_iter().map(|line| line.font_size).collect();
    let largest = sizes.iter().copied().fold(0., f32::max);
    sizes
        .iter()
        .position(|&size| size >= largest * TITLE_FONT_FRACTION)
}

/// Extracts the text of every page of `pdf` on rayon's thread pool. Blocking; run it
/// on a background thread. Returns `None` if `cancelled` is set before it's done.
pub fn extract_text(pdf: &Pdf, cancelled: &AtomicBool) -> Option<Vec<PageText>> {
    pdf.pages()
        .par_iter()
        // `InterpreterCache` isn't thread-safe, so each worker thread gets its own.
        .map_init(InterpreterCache::new, |cache, page| {
            if cancelled.load(Ordering::Relaxed) {
                return None;
            }
            Some(page_text(pdf, page, cache))
        })
        .collect()
}

fn page_text<'a>(pdf: &'a Pdf, page: &Page<'a>, cache: &InterpreterCache<'a>) -> PageText {
    let settings = InterpreterSettings {
        // Comments and form fields aren't part of the slide.
        render_annotations: false,
        ..Default::default()
    };
    // Positions are taken in the PDF's own coordinates, so the initial transform and
    // clip don't matter.
    let mut context = Context::new(
        Affine::IDENTITY,
        Rect::new(0., 0., 1., 1.),
        cache,
        pdf.xref(),
        settings,
    );
    let mut collector = TextCollector::default();
    interpret_page(page, &mut context, &mut collector);
    collector.finish()
}

/// Joins the glyphs a page draws into lines, as `Device` that only looks at text.
#[derive(Default)]
struct TextCollector {
    lines: Vec<TextLine>,
    /// The line being built.
    line: Option<LineBuilder>,
}

struct LineBuilder {
    text: String,
    font_size: f64,
    /// Where the line's baseline is, across it.
    baseline: f64,
    /// Where the last glyph ends, along the line.
    end: f64,
}

impl TextCollector {
    fn push_glyph(&mut self, text: &str, transform: Affine, advance: Option<f32>) {
        // The glyph's em box, mapped onto the page.
        let origin = transform * Point::ZERO;
        let up = transform * Point::new(0., UNITS_PER_EM) - origin;
        let font_size = up.hypot();
        if font_size < 0.01 || text.is_empty() {
            return;
        }
        // Lines run perpendicular to "up", so rotated text is handled too.
        let along_dir = Vec2::new(up.y, -up.x) / font_size;
        let across_dir = up / font_size;
        let along = origin.to_vec2().dot(along_dir);
        let across = origin.to_vec2().dot(across_dir);
        let width = advance.map_or(0.5 * font_size, |advance| {
            (transform * Point::new(advance as f64, 0.) - origin).hypot()
        });

        if let Some(line) = &mut self.line {
            let size = line.font_size.max(font_size);
            let same_line = (across - line.baseline).abs() < LINE_GAP_EM * size
                // Going back a lot along the line is a new line, e.g. in a second column.
                && along > line.end - size;
            if same_line {
                let gap = along - line.end;
                if gap > SPACE_GAP_EM * size
                    && !line.text.ends_with(char::is_whitespace)
                    && !text.starts_with(char::is_whitespace)
                {
                    line.text.push(' ');
                }
                line.text.push_str(text);
                line.font_size = line.font_size.max(font_size);
                line.end = line.end.max(along + width);
                return;
            }
        }

        self.end_line();
        self.line = Some(LineBuilder {
            text: text.to_string(),
            font_size,
            baseline: across,
            end: along + width,
        });
    }

    fn end_line(&mut self) {
        let Some(line) = self.line.take() else {
            return;
        };
        let text = line.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            self.lines.push(TextLine {
                text,
                font_size: line.font_size as f32,
            });
        }
    }

    fn finish(mut self) -> PageText {
        self.end_line();
        PageText { lines: self.lines }
    }
}

impl Device<'_> for TextCollector {
    fn draw_glyph_run(&mut self, glyph_run: &GlyphRun<'_, '_>, props: DrawProps<'_>, _: &DrawMode) {
        for glyph in glyph_run.glyphs() {
            let advance = match &**glyph {
                Glyph::Outline(outline) => outline.advance_width(),
                Glyph::Type3(_) => None,
            };
            let transform = props.transform * glyph.transform();
            match glyph.as_unicode() {
                Some(BfString::Char(char)) => {
                    self.push_glyph(char.encode_utf8(&mut [0; 4]), transform, advance)
                }
                Some(BfString::String(text)) => self.push_glyph(&text, transform, advance),
                // Usually a ligature hayro can't map, like "f_t" in fonts without a
                // ToUnicode map. Marked rather than dropped, so the word isn't split.
                None => self.push_glyph(UNKNOWN_CHAR, transform, advance),
            }
        }
    }

    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'_>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'_>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'_, '_>, _: ImageDrawProps<'_>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}
