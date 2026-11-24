//! Pure-Rust PDF inspection and CPU rasterization backed by `hayro`.

use std::fmt;
use std::sync::Arc;

use drawcraft_doc::looks_like_pdf_bytes;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_syntax::page::Page;
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::kurbo::Affine;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{RenderCache, RenderSettings, render_into};

const MAX_RENDER_PIXEL_DIM: u32 = 4096;

/// Errors raised when inspecting or rendering an imported PDF file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfError {
    Empty,
    TooLarge {
        limit_bytes: usize,
    },
    Encrypted,
    Invalid(String),
    NoPages,
    PageLimitExceeded {
        page_count: usize,
        max_pages: usize,
    },
    PageOutOfBounds {
        page_index: usize,
        page_count: usize,
    },
    EncodeFailed(String),
}

impl fmt::Display for PdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "PDF file is empty."),
            Self::TooLarge { limit_bytes } => {
                write!(
                    f,
                    "PDF exceeds the {} MiB maximum file size.",
                    limit_bytes / (1024 * 1024)
                )
            }
            Self::Encrypted => {
                write!(f, "Password-protected PDF files are not supported yet.")
            }
            Self::Invalid(why) => write!(f, "Could not read PDF file: {why}"),
            Self::NoPages => write!(f, "PDF file does not contain any pages."),
            Self::PageLimitExceeded {
                page_count,
                max_pages,
            } => write!(
                f,
                "PDF has {page_count} pages, exceeding the limit of {max_pages} pages."
            ),
            Self::PageOutOfBounds {
                page_index,
                page_count,
            } => write!(
                f,
                "PDF page {} does not exist (document has {page_count} pages).",
                page_index + 1
            ),
            Self::EncodeFailed(why) => write!(f, "Failed to encode PDF page backdrop: {why}"),
        }
    }
}

impl std::error::Error for PdfError {}

/// Page dimensions extracted from an imported PDF page, scaled to DrawCraft page units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PdfPageMetrics {
    pub page_width: u32,
    pub page_height: u32,
}

/// Summary returned by [`inspect_pdf`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfDocumentSummary {
    pub title: Option<String>,
    pub pages: Vec<PdfPageMetrics>,
}

/// Rasterized opaque RGBA8 image for a single PDF page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedPdfPage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Converts PDF user-space points (72 pt/in) into DrawCraft page dimensions.
///
/// Uses a `2x` scale so standard Letter/A4 sheets map near `1200x1600` page points,
/// matching default pen sizes and `0.5x` PDF re-export scaling.
#[must_use]
pub fn pdf_pt_to_page_size(w_pt: f32, h_pt: f32) -> (u32, u32) {
    let w = if w_pt.is_finite() && w_pt > 1.0 {
        f64::from(w_pt)
    } else {
        595.0
    };
    let h = if h_pt.is_finite() && h_pt > 1.0 {
        f64::from(h_pt)
    } else {
        842.0
    };

    let mut sx = w * 2.0;
    let mut sy = h * 2.0;
    let max_side = sx.max(sy);
    if max_side > 4096.0 {
        let k = 4096.0 / max_side;
        sx *= k;
        sy *= k;
    }
    let min_side = sx.min(sy);
    if min_side < 64.0 && min_side > 0.0 {
        let k = 64.0 / min_side;
        sx = (sx * k).min(4096.0);
        sy = (sy * k).min(4096.0);
    }
    (
        (sx.round() as u32).clamp(64, 8192),
        (sy.round() as u32).clamp(64, 8192),
    )
}

fn open_pdf(bytes: Arc<Vec<u8>>) -> Result<Pdf, PdfError> {
    if bytes.is_empty() {
        return Err(PdfError::Empty);
    }
    if !looks_like_pdf_bytes(&bytes) {
        return Err(PdfError::Invalid("missing %PDF- header".to_string()));
    }
    Pdf::new(bytes).map_err(|err| match err {
        LoadPdfError::Decryption(_) => PdfError::Encrypted,
        LoadPdfError::Invalid => PdfError::Invalid("corrupted or unsupported PDF structure".into()),
    })
}

fn decode_pdf_title(raw: &[u8]) -> Option<String> {
    let decoded = if raw.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = raw[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_be_bytes(pair))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(raw).into_owned()
    };
    let clean: String = decoded
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(96)
        .collect();
    let trimmed = clean.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Validates PDF bytes and reads title metadata + per-page scaled canvas dimensions.
///
/// Pass `usize::MAX` for `max_pages` when no page count cap should be applied.
pub fn inspect_pdf(bytes: Arc<Vec<u8>>, max_pages: usize) -> Result<PdfDocumentSummary, PdfError> {
    let pdf = open_pdf(bytes)?;
    let raw_pages = pdf.pages();
    let page_count = raw_pages.len();
    if page_count == 0 {
        return Err(PdfError::NoPages);
    }
    if page_count > max_pages {
        return Err(PdfError::PageLimitExceeded {
            page_count,
            max_pages,
        });
    }

    let pages = raw_pages
        .iter()
        .map(|page| {
            let (w_pt, h_pt) = page.render_dimensions();
            let (page_width, page_height) = pdf_pt_to_page_size(w_pt, h_pt);
            PdfPageMetrics {
                page_width,
                page_height,
            }
        })
        .collect();
    let title = pdf.metadata().title.as_deref().and_then(decode_pdf_title);

    Ok(PdfDocumentSummary { title, pages })
}

fn fit_render_pixels(src_w: f32, src_h: f32, max_w: u32, max_h: u32) -> (u16, u16) {
    let w = if src_w.is_finite() && src_w > 1.0 {
        f64::from(src_w)
    } else {
        595.0
    };
    let h = if src_h.is_finite() && src_h > 1.0 {
        f64::from(src_h)
    } else {
        842.0
    };
    let cap_w = f64::from(max_w.clamp(1, MAX_RENDER_PIXEL_DIM));
    let cap_h = f64::from(max_h.clamp(1, MAX_RENDER_PIXEL_DIM));
    let scale = (cap_w / w).min(cap_h / h).max(0.01);
    let px_w = (w * scale).round().clamp(1.0, cap_w) as u16;
    let px_h = (h * scale).round().clamp(1.0, cap_h) as u16;
    (px_w, px_h)
}

struct PdfRenderState<'a> {
    cache: RenderCache<'a>,
    interpreter_settings: InterpreterSettings,
    render_settings: RenderSettings,
    ctx: RenderContext,
    pixmap: Pixmap,
    resources: Resources,
}

impl<'a> PdfRenderState<'a> {
    fn new(width: u16, height: u16) -> Self {
        Self {
            cache: RenderCache::new(),
            interpreter_settings: InterpreterSettings::default(),
            render_settings: RenderSettings::default(),
            ctx: RenderContext::new(width, height),
            pixmap: Pixmap::new(width, height),
            resources: Resources::default(),
        }
    }

    fn render_page(&mut self, page: &'a Page<'a>, pixel_w: u16, pixel_h: u16) -> RenderedPdfPage {
        let (src_w, src_h) = page.render_dimensions();
        let safe_w = if src_w.is_finite() && src_w > 1.0 {
            f64::from(src_w)
        } else {
            595.0
        };
        let safe_h = if src_h.is_finite() && src_h > 1.0 {
            f64::from(src_h)
        } else {
            842.0
        };
        self.ctx.reset_and_resize(pixel_w, pixel_h);
        self.pixmap.resize(pixel_w, pixel_h);

        let transform =
            Affine::scale_non_uniform(f64::from(pixel_w) / safe_w, f64::from(pixel_h) / safe_h)
                * page.initial_transform(true).to_kurbo();
        render_into(
            page,
            &self.cache,
            &self.interpreter_settings,
            &self.render_settings,
            &mut self.ctx,
            transform,
        );
        self.ctx.flush();
        self.ctx.render_with(
            &mut self.pixmap,
            &mut self.resources,
            RasterizerSettings {
                target_init: TargetInit::Clear(WHITE),
                ..Default::default()
            },
        );

        let mut rgba = self.pixmap.data_as_u8_slice().to_vec();
        // Surface is cleared with opaque white, so clamp alpha to 255 for straight RGBA consumers.
        for px in rgba.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        RenderedPdfPage {
            width: u32::from(pixel_w),
            height: u32::from(pixel_h),
            rgba,
        }
    }
}

/// Reusable per-PDF renderer holding parsed pages and shared font/glyph caches across renders.
pub struct PdfPageRenderer<'a> {
    pages: &'a [Page<'a>],
    state: PdfRenderState<'a>,
}

impl<'a> PdfPageRenderer<'a> {
    /// Number of pages in the opened PDF.
    #[must_use]
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Renders `page_index` preserving its aspect ratio within `max_w x max_h`.
    pub fn render_page_rgba(
        &mut self,
        page_index: usize,
        max_w: u32,
        max_h: u32,
    ) -> Result<RenderedPdfPage, PdfError> {
        let Some(page) = self.pages.get(page_index) else {
            return Err(PdfError::PageOutOfBounds {
                page_index,
                page_count: self.pages.len(),
            });
        };
        let (src_w, src_h) = page.render_dimensions();
        let (px_w, px_h) = fit_render_pixels(src_w, src_h, max_w, max_h);
        Ok(self.state.render_page(page, px_w, px_h))
    }
}

/// Opens a PDF once and passes a [`PdfPageRenderer`] borrowing it to `f`, avoiding
/// repeated document parsing and sharing font/glyph caches across pages.
pub fn with_pdf_renderer<R, F>(bytes: Arc<Vec<u8>>, f: F) -> Result<R, PdfError>
where
    F: FnOnce(&mut PdfPageRenderer<'_>) -> R,
{
    let pdf = open_pdf(bytes)?;
    let pages = pdf.pages();
    let mut renderer = PdfPageRenderer {
        pages,
        state: PdfRenderState::new(0, 0),
    };
    Ok(f(&mut renderer))
}

/// Renders one PDF page preserving its aspect ratio within `max_w x max_h`.
pub fn render_pdf_page_rgba(
    bytes: Arc<Vec<u8>>,
    page_index: usize,
    max_w: u32,
    max_h: u32,
) -> Result<RenderedPdfPage, PdfError> {
    with_pdf_renderer(bytes, |renderer| {
        renderer.render_page_rgba(page_index, max_w, max_h)
    })?
}

/// Renders one PDF page to exact `(width, height)` pixel dimensions (clamped to `1..=4096`).
pub fn render_pdf_page_exact_rgba(
    bytes: Arc<Vec<u8>>,
    page_index: usize,
    width: u32,
    height: u32,
) -> Result<RenderedPdfPage, PdfError> {
    let results = render_pdf_batch_exact_rgba(bytes, &[(page_index, width, height)])?;
    results
        .into_iter()
        .next()
        .flatten()
        .ok_or(PdfError::Invalid("failed to render PDF page".to_string()))
}

/// Renders multiple `(pdf_page_index, exact_width, exact_height)` requests against one PDF,
/// sharing font/glyph caches across pages.
pub fn render_pdf_batch_exact_rgba(
    bytes: Arc<Vec<u8>>,
    requests: &[(usize, u32, u32)],
) -> Result<Vec<Option<RenderedPdfPage>>, PdfError> {
    let pdf = open_pdf(bytes)?;
    let pages = pdf.pages();
    let mut state = PdfRenderState::new(0, 0);

    let mut out = Vec::with_capacity(requests.len());
    for &(page_index, req_w, req_h) in requests {
        let Some(page) = pages.get(page_index) else {
            out.push(None);
            continue;
        };
        let px_w = req_w.clamp(1, MAX_RENDER_PIXEL_DIM) as u16;
        let px_h = req_h.clamp(1, MAX_RENDER_PIXEL_DIM) as u16;
        out.push(Some(state.render_page(page, px_w, px_h)));
    }
    Ok(out)
}

/// Renders one PDF page and encodes it as PNG bytes within `max_w x max_h`.
pub fn render_pdf_page_png(
    bytes: Arc<Vec<u8>>,
    page_index: usize,
    max_w: u32,
    max_h: u32,
) -> Result<(u32, u32, Vec<u8>), PdfError> {
    let rendered = render_pdf_page_rgba(bytes, page_index, max_w, max_h)?;
    let png = drawcraft_raster::png_encode(rendered.width, rendered.height, &rendered.rgba)
        .map_err(|err| PdfError::EncodeFailed(err.to_string()))?;
    Ok((rendered.width, rendered.height, png))
}
