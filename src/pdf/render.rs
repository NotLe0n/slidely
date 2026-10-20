use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::{Pixels, RenderImage, Size, px, size};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::page::Page;
use hayro::vello_cpu::color::AlphaColor;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use rayon::prelude::*;

/// Upper bound for a rendered page's longer side, in pixels. Keeps memory per page
/// bounded (~64 MB) and the image within GPU texture limits at high zoom levels.
const MAX_PAGE_SIDE_PX: f32 = 4096.;

/// A parsed PDF. Pages are rendered on demand with [`render_pages`].
pub struct PdfData {
    pub pdf: Arc<Pdf>,
    /// Size of each page in PDF points.
    pub page_sizes: Vec<Size<Pixels>>,
}

/// Clockwise rotation of a page in quarter turns, applied on top of the PDF's own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rotation(u8);

impl Rotation {
    /// This rotation turned 90° further clockwise.
    pub fn clockwise(self) -> Self {
        Self((self.0 + 1) % 4)
    }

    /// The size of `page` once rotated.
    pub fn apply(self, page: Size<Pixels>) -> Size<Pixels> {
        if self.0 % 2 == 1 {
            size(page.height, page.width)
        } else {
            page
        }
    }
}

pub struct RenderedPage {
    pub image: Arc<RenderImage>,
    /// Pixels per PDF point that were requested for `image`. The image may be smaller
    /// if the page hit `MAX_PAGE_SIDE_PX`.
    pub render_scale: f32,
    /// The rotation `image` was rendered with.
    pub rotation: Rotation,
}

impl PdfData {
    /// Reads and parses the PDF at `path` without rendering anything. Blocking; run it
    /// on a background thread.
    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        let pdf = Pdf::new(std::fs::read(&path)?).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let page_sizes = pdf
            .pages()
            .iter()
            .map(|page| {
                let (width, height) = page.render_dimensions();
                size(px(width), px(height))
            })
            .collect();
        Ok(Self {
            pdf: Arc::new(pdf),
            page_sizes,
        })
    }
}

/// Renders the pages at the given indices, rotated, at `render_scale` pixels per point
/// on rayon's thread pool, calling `on_page` as each one finishes. Blocking; run it on
/// a background thread. Pages that haven't started when `cancelled` is set are skipped.
pub fn render_pages(
    pdf: &Pdf,
    indices: &[(usize, Rotation)],
    render_scale: f32,
    cancelled: &AtomicBool,
    on_page: impl Fn(usize, RenderedPage) + Sync,
) {
    let interpreter_settings = InterpreterSettings::default();
    let render_settings = RenderSettings::default();
    let pages = pdf.pages();

    indices
        .par_iter()
        // `RenderCache` isn't thread-safe, so each worker thread gets its own.
        .for_each_init(RenderCache::new, |cache, &(ix, rotation)| {
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            let image = render_page(
                &pages[ix],
                rotation,
                render_scale,
                cache,
                &interpreter_settings,
                &render_settings,
            );
            on_page(
                ix,
                RenderedPage {
                    image,
                    render_scale,
                    rotation,
                },
            );
        });
}

/// Renders the pages at the given indices, rotated, `height` pixels high on rayon's
/// thread pool, calling `on_page` with each one's index, rotation and image as it
/// finishes. Blocking; run it on a background thread. Pages that haven't started when
/// `cancelled` is set are skipped.
pub fn render_thumbnails(
    pdf: &Pdf,
    indices: &[(usize, Rotation)],
    height: f32,
    cancelled: &AtomicBool,
    on_page: impl Fn(usize, Rotation, Arc<RenderImage>) + Sync,
) {
    let interpreter_settings = InterpreterSettings::default();
    let render_settings = RenderSettings::default();
    let pages = pdf.pages();

    indices
        .par_iter()
        .for_each_init(RenderCache::new, |cache, &(ix, rotation)| {
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            let page = &pages[ix];
            let (width_pt, height_pt) = page.render_dimensions();
            let height_pt = rotation.apply(size(px(width_pt), px(height_pt))).height;
            let image = render_page(
                page,
                rotation,
                height / f32::from(height_pt),
                cache,
                &interpreter_settings,
                &render_settings,
            );
            on_page(ix, rotation, image);
        });
}

fn render_page<'a>(
    page: &'a Page<'a>,
    rotation: Rotation,
    render_scale: f32,
    cache: &RenderCache<'a>,
    interpreter_settings: &InterpreterSettings,
    render_settings: &RenderSettings,
) -> Arc<RenderImage> {
    let (width_pt, height_pt) = page.render_dimensions();
    let scale = render_scale.min(MAX_PAGE_SIDE_PX / width_pt.max(height_pt));
    let pixmap_settings = PixmapSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: AlphaColor::WHITE,
    };

    let pixmap = hayro::render(
        page,
        cache,
        interpreter_settings,
        render_settings,
        &pixmap_settings,
    );
    let (width, height) = (pixmap.width() as u32, pixmap.height() as u32);

    let mut bytes = pixmap.take_rgba8(ImageAlphaType::AlphaPremultiplied);
    // hayro: RGBA, gpui: BGRA.
    for pixel in bytes.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }

    let buffer = image::RgbaImage::from_raw(width, height, bytes)
        .expect("pixmap buffer matches its dimensions");
    let buffer = match rotation.0 {
        1 => image::imageops::rotate90(&buffer),
        2 => image::imageops::rotate180(&buffer),
        3 => image::imageops::rotate270(&buffer),
        _ => buffer,
    };
    Arc::new(RenderImage::new([image::Frame::new(buffer)]))
}
