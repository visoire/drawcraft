//! L3 deterministic CPU page compositor (the rendering oracle).

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod export;
pub mod pdf;
pub mod window;

pub use export::{export_notebook_pdf, export_notebook_pdf_pages, render_document_svg};
pub use pdf::{
    PdfDocumentSummary, PdfError, PdfPageMetrics, PdfPageRenderer, RenderedPdfPage, inspect_pdf,
    pdf_pt_to_page_size, render_pdf_batch_exact_rgba, render_pdf_page_exact_rgba,
    render_pdf_page_png, render_pdf_page_rgba, with_pdf_renderer,
};
pub use window::{CacheError, PAGE_MARGIN, PageBitmap, PageWindowCache, window_pages};

use drawcraft_color::BlendMode;
use drawcraft_doc::{
    Document, GeometricShape, PageImage, PaperTemplate, PenSettings, StrokeSample, TextItem,
};
use drawcraft_geom::{IRect, Point};
use drawcraft_paint::render_stroke_to_surface;
use drawcraft_raster::Surface;
use std::sync::OnceLock;

pub const ROBOTO_REGULAR_TTF: &[u8] = include_bytes!("../assets/fonts/Roboto-Regular.ttf");
pub const ROBOTO_BOLD_TTF: &[u8] = include_bytes!("../assets/fonts/Roboto-Bold.ttf");
pub const ROBOTO_ITALIC_TTF: &[u8] = include_bytes!("../assets/fonts/Roboto-Italic.ttf");
pub const ROBOTO_BOLD_ITALIC_TTF: &[u8] = include_bytes!("../assets/fonts/Roboto-BoldItalic.ttf");

static TEXT_FONT: OnceLock<Option<fontdue::Font>> = OnceLock::new();
static TEXT_FONT_BOLD: OnceLock<Option<fontdue::Font>> = OnceLock::new();
static TEXT_FONT_ITALIC: OnceLock<Option<fontdue::Font>> = OnceLock::new();
static TEXT_FONT_BOLD_ITALIC: OnceLock<Option<fontdue::Font>> = OnceLock::new();

fn text_font() -> Option<&'static fontdue::Font> {
    TEXT_FONT
        .get_or_init(|| {
            fontdue::Font::from_bytes(ROBOTO_REGULAR_TTF, fontdue::FontSettings::default()).ok()
        })
        .as_ref()
}

fn text_font_variant(bold: bool, italic: bool) -> Option<&'static fontdue::Font> {
    match (bold, italic) {
        (false, false) => text_font(),
        (true, false) => TEXT_FONT_BOLD
            .get_or_init(|| {
                fontdue::Font::from_bytes(ROBOTO_BOLD_TTF, fontdue::FontSettings::default()).ok()
            })
            .as_ref()
            .or_else(text_font),
        (false, true) => TEXT_FONT_ITALIC
            .get_or_init(|| {
                fontdue::Font::from_bytes(ROBOTO_ITALIC_TTF, fontdue::FontSettings::default()).ok()
            })
            .as_ref()
            .or_else(text_font),
        (true, true) => TEXT_FONT_BOLD_ITALIC
            .get_or_init(|| {
                fontdue::Font::from_bytes(ROBOTO_BOLD_ITALIC_TTF, fontdue::FontSettings::default())
                    .ok()
            })
            .as_ref()
            .or_else(text_font),
    }
}

struct TextGlyphContext {
    origin_x: f64,
    origin_y: f64,
    sin: f64,
    cos: f64,
    color: drawcraft_color::Color,
}

fn blend_rasterized_glyph(
    surface: &mut Surface,
    metrics: &fontdue::Metrics,
    bitmap: &[u8],
    left: f64,
    top: f64,
    ctx: &TextGlyphContext,
) {
    for row in 0..metrics.height {
        for col in 0..metrics.width {
            let idx = row.saturating_mul(metrics.width).saturating_add(col);
            let Some(&coverage) = bitmap.get(idx) else {
                continue;
            };
            let alpha = ctx.color.a * f32::from(coverage) / 255.0;
            if alpha <= 0.0 {
                continue;
            }
            let dx = left + col as f64 - ctx.origin_x;
            let dy = top + row as f64 - ctx.origin_y;
            let x = (ctx.origin_x + dx * ctx.cos - dy * ctx.sin).round() as i32;
            let y = (ctx.origin_y + dx * ctx.sin + dy * ctx.cos).round() as i32;
            surface.blend_pixel(
                x,
                y,
                [ctx.color.r, ctx.color.g, ctx.color.b, alpha],
                BlendMode::Normal,
            );
        }
    }
}

fn char_advance_width(
    font: &fontdue::Font,
    ch: char,
    item: &TextItem,
    font_kind: drawcraft_doc::TextFont,
    size: f32,
) -> f32 {
    let base = if font_kind == drawcraft_doc::TextFont::Monospace {
        size * 0.62
    } else {
        font.metrics(ch, size).advance_width
    };
    base + item.letter_spacing
}

fn rotate_about_text_origin(
    x: f64,
    y: f64,
    origin_x: f64,
    origin_y: f64,
    sin: f64,
    cos: f64,
) -> Point {
    let dx = x - origin_x;
    let dy = y - origin_y;
    Point::new(
        origin_x + dx * cos - dy * sin,
        origin_y + dx * sin + dy * cos,
    )
}

#[allow(clippy::too_many_arguments)]
fn fill_local_rect_rotated(
    surface: &mut Surface,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    color: drawcraft_color::Color,
    origin_x: f64,
    origin_y: f64,
    sin: f64,
    cos: f64,
) {
    if w <= 0.0 || h <= 0.0 || color.a <= 0.0 {
        return;
    }
    let poly = [
        rotate_about_text_origin(x, y, origin_x, origin_y, sin, cos),
        rotate_about_text_origin(x + w, y, origin_x, origin_y, sin, cos),
        rotate_about_text_origin(x + w, y + h, origin_x, origin_y, sin, cos),
        rotate_about_text_origin(x, y + h, origin_x, origin_y, sin, cos),
    ];
    fill_polygon_scanline(surface, &poly, color);
}

fn fill_polygon_scanline(surface: &mut Surface, poly: &[Point], color: drawcraft_color::Color) {
    if poly.len() < 3 || color.a <= 0.0 {
        return;
    }
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in poly {
        if !p.x.is_finite() || !p.y.is_finite() {
            return;
        }
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }
    let ix0 = (min_x.floor() as i32).max(0);
    let iy0 = (min_y.floor() as i32).max(0);
    let ix1 = (max_x.ceil() as i32).min(surface.width() as i32 - 1);
    let iy1 = (max_y.ceil() as i32).min(surface.height() as i32 - 1);
    if ix0 > ix1 || iy0 > iy1 {
        return;
    }
    let rgba = [color.r, color.g, color.b, color.a.clamp(0.0, 1.0)];
    let mut nodes: Vec<f64> = Vec::with_capacity(poly.len());
    for py in iy0..=iy1 {
        let sy = f64::from(py) + 0.5;
        nodes.clear();
        let n = poly.len();
        for i in 0..n {
            let a = poly[i];
            let b = poly[(i + 1) % n];
            if (a.y <= sy && b.y > sy) || (b.y <= sy && a.y > sy) {
                let x = a.x + (sy - a.y) * (b.x - a.x) / (b.y - a.y);
                nodes.push(x);
            }
        }
        nodes.sort_by(f64::total_cmp);
        for &[x0, x1] in nodes.as_chunks::<2>().0 {
            let sx0 = (x0.ceil() as i32).max(ix0);
            let sx1 = (x1.floor() as i32).min(ix1);
            for px in sx0..=sx1 {
                surface.blend_pixel(px, py, rgba, BlendMode::Normal);
            }
        }
    }
}

fn measure_run_width(
    item: &TextItem,
    rfont: &fontdue::Font,
    font_kind: drawcraft_doc::TextFont,
    size: f32,
    text: &str,
) -> f64 {
    let mut w = 0.0_f64;
    for ch in text.chars() {
        let repeats = if ch == '\t' { 4 } else { 1 };
        let glyph_ch = if ch == '\t' { ' ' } else { ch };
        let adv = char_advance_width(rfont, glyph_ch, item, font_kind, size);
        w += f64::from(repeats) * f64::from(adv);
    }
    w
}

#[allow(clippy::too_many_arguments)]
fn draw_text_list_marker(
    surface: &mut Surface,
    kind: drawcraft_doc::ListItemKind,
    item: &TextItem,
    line_top: f64,
    gutter_w: f64,
    size: f32,
    color: drawcraft_color::Color,
    sin: f64,
    cos: f64,
) {
    match kind {
        drawcraft_doc::ListItemKind::Bullet => {
            let cy = line_top + f64::from(size) * 0.56;
            let cx = item.x + gutter_w * 0.45;
            let r = f64::from((size * 0.14).clamp(1.8, 6.0));
            fill_local_rect_rotated(
                surface,
                cx - r,
                cy - r,
                r * 2.0,
                r * 2.0,
                color,
                item.x,
                item.y,
                sin,
                cos,
            );
        }
        drawcraft_doc::ListItemKind::Checkbox { checked } => {
            let box_sz = f64::from((size * 0.68).clamp(8.0, 44.0));
            let bx = item.x + (gutter_w - box_sz) * 0.4;
            let by = line_top + f64::from(size) * 0.16;
            let corners = [
                rotate_about_text_origin(bx, by, item.x, item.y, sin, cos),
                rotate_about_text_origin(bx + box_sz, by, item.x, item.y, sin, cos),
                rotate_about_text_origin(bx + box_sz, by + box_sz, item.x, item.y, sin, cos),
                rotate_about_text_origin(bx, by + box_sz, item.x, item.y, sin, cos),
            ];
            if checked {
                fill_polygon_scanline(surface, &corners, color);
                let check_col = if drawcraft_color::relative_luminance(color) > 0.45 {
                    drawcraft_color::Color::from_rgba8(20, 24, 32, 255)
                } else {
                    drawcraft_color::Color::WHITE
                };
                let p1 = rotate_about_text_origin(
                    bx + box_sz * 0.22,
                    by + box_sz * 0.54,
                    item.x,
                    item.y,
                    sin,
                    cos,
                );
                let p2 = rotate_about_text_origin(
                    bx + box_sz * 0.44,
                    by + box_sz * 0.76,
                    item.x,
                    item.y,
                    sin,
                    cos,
                );
                let p3 = rotate_about_text_origin(
                    bx + box_sz * 0.80,
                    by + box_sz * 0.28,
                    item.x,
                    item.y,
                    sin,
                    cos,
                );
                let sw = (box_sz * 0.14).max(1.2);
                surface.draw_line_aa(p1, p2, check_col, sw);
                surface.draw_line_aa(p2, p3, check_col, sw);
            } else {
                let sw = (box_sz * 0.11).max(1.1);
                for i in 0..4 {
                    surface.draw_line_aa(corners[i], corners[(i + 1) % 4], color, sw);
                }
            }
        }
    }
}

fn draw_text_item(surface: &mut Surface, doc_width: u32, item: &TextItem) {
    let Some(base_font) = text_font() else {
        return;
    };
    let item = item.sanitized();
    let color = item.color.sanitized();
    let size = item.size.clamp(6.0, 120.0);
    let angle = item.angle_degrees.to_radians();
    let (sin, cos) = angle.sin_cos();
    let glyph_ctx = TextGlyphContext {
        origin_x: item.x,
        origin_y: item.y,
        sin,
        cos,
        color,
    };
    let max_width = item.effective_max_width(doc_width);
    let lines =
        drawcraft_doc::wrap_styled_text_lines_with(&item, max_width, |ch, ch_font, ch_size| {
            if ch == '\t' {
                char_advance_width(base_font, ' ', &item, ch_font, ch_size) * 4.0
            } else {
                char_advance_width(base_font, ch, &item, ch_font, ch_size)
            }
        });
    let gutter_w = f64::from((size * 1.15).max(14.0));
    let mut cur_top = item.y;

    for line in &lines {
        let line_max_size = line
            .runs
            .iter()
            .filter_map(|r| r.size)
            .fold(size, f32::max)
            .clamp(6.0, 120.0);
        let ascent = base_font
            .horizontal_line_metrics(line_max_size)
            .map_or(line_max_size, |metrics| metrics.ascent);
        let line_height = (line_max_size * item.line_spacing).max(1.0);
        let line_top = cur_top;
        let baseline = line_top + f64::from(ascent);
        cur_top += f64::from(line_height);
        let has_list_indent = line.list_kind.is_some() || line.is_continuation;
        let indent_w = if has_list_indent { gutter_w } else { 0.0 };
        let avail_w = (f64::from(max_width) - indent_w).max(24.0);

        let line_w: f64 = line
            .runs
            .iter()
            .map(|run| {
                let rfont = text_font_variant(run.bold, run.italic).unwrap_or(base_font);
                let run_font = run.font.unwrap_or(item.font);
                let run_size = run.size.unwrap_or(size).clamp(6.0, 120.0);
                measure_run_width(&item, rfont, run_font, run_size, &run.text)
            })
            .sum();
        let align_offset = match item.alignment {
            drawcraft_doc::TextAlignment::Left => 0.0,
            drawcraft_doc::TextAlignment::Center => ((avail_w - line_w) * 0.5).max(0.0),
            drawcraft_doc::TextAlignment::Right => (avail_w - line_w).max(0.0),
        };
        let content_start_x = item.x + indent_w + align_offset;

        if let Some(kind) = line.list_kind {
            draw_text_list_marker(
                surface, kind, &item, line_top, gutter_w, size, color, sin, cos,
            );
        }

        let mut pen_x = content_start_x;
        for run in &line.runs {
            let rfont = text_font_variant(run.bold, run.italic).unwrap_or(base_font);
            let run_font = run.font.unwrap_or(item.font);
            let run_size = run.size.unwrap_or(size).clamp(6.0, 120.0);
            let run_color = run.color.map_or(color, |c| c.sanitized());
            let run_ctx = TextGlyphContext {
                color: run_color,
                ..glyph_ctx
            };
            let run_start_x = pen_x;
            let run_w = measure_run_width(&item, rfont, run_font, run_size, &run.text);
            if let Some(hl) = run.highlight {
                fill_local_rect_rotated(
                    surface,
                    run_start_x - 1.0,
                    baseline - f64::from(run_size) * 0.85,
                    run_w + 2.0,
                    f64::from(run_size) * 1.05,
                    hl,
                    item.x,
                    item.y,
                    sin,
                    cos,
                );
            }
            for ch in run.text.chars() {
                let repeats = if ch == '\t' { 4 } else { 1 };
                let glyph_ch = if ch == '\t' { ' ' } else { ch };
                for _ in 0..repeats {
                    let (metrics, bitmap) = rfont.rasterize(glyph_ch, run_size);
                    let advance = if run_font == drawcraft_doc::TextFont::Monospace {
                        run_size * 0.62
                    } else {
                        metrics.advance_width
                    };
                    let left = pen_x
                        + f64::from((advance - metrics.advance_width) * 0.5)
                        + f64::from(metrics.xmin);
                    let top = baseline - f64::from(metrics.ymin) - metrics.height as f64;
                    blend_rasterized_glyph(surface, &metrics, &bitmap, left, top, &run_ctx);
                    pen_x += f64::from(advance + item.letter_spacing);
                }
            }
            if run.underline && run_w > 0.0 {
                let uy = baseline + f64::from((run_size * 0.14).max(1.5));
                let p0 = rotate_about_text_origin(run_start_x, uy, item.x, item.y, sin, cos);
                let p1 =
                    rotate_about_text_origin(run_start_x + run_w, uy, item.x, item.y, sin, cos);
                surface.draw_line_aa(p0, p1, run_color, f64::from((run_size * 0.075).max(1.0)));
            }
        }
    }
}

fn draw_shape(surface: &mut Surface, item: &GeometricShape) {
    let shape = item.sanitized();
    let segments = shape.path_segments();
    if shape.is_closed()
        && let Some(fill) = shape.fill_color
        && fill.a > 0.0
        && let Some((fill_pts, true)) = segments.first()
    {
        fill_polygon_scanline(surface, fill_pts, fill);
    }

    let dash = shape.line_style.dash_pattern(shape.width);
    for (points, closed) in segments {
        if points.len() < 2 {
            continue;
        }
        if let Some((dash_len, gap_len)) = dash {
            draw_dashed_polyline(
                surface,
                &points,
                closed,
                shape.color,
                f64::from(shape.width),
                f64::from(dash_len),
                f64::from(gap_len),
            );
        } else {
            for pair in points.windows(2) {
                surface.draw_line_aa(pair[0], pair[1], shape.color, f64::from(shape.width));
            }
            if closed && let (Some(first), Some(last)) = (points.first(), points.last()) {
                surface.draw_line_aa(*last, *first, shape.color, f64::from(shape.width));
            }
        }
    }
}

fn draw_dashed_polyline(
    surface: &mut Surface,
    points: &[Point],
    closed: bool,
    color: drawcraft_color::Color,
    width: f64,
    dash_len: f64,
    gap_len: f64,
) {
    let period = (dash_len + gap_len).max(1.0);
    let seg_count = if closed {
        points.len()
    } else {
        points.len().saturating_sub(1)
    };
    let mut dist_in_cycle = 0.0_f64;
    for i in 0..seg_count {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        let len = a.distance(b);
        if len <= 1e-6 {
            continue;
        }
        let ux = (b.x - a.x) / len;
        let uy = (b.y - a.y) / len;
        let mut cursor = 0.0_f64;
        while cursor < len - 1e-6 {
            let phase = dist_in_cycle % period;
            if phase < dash_len {
                let take = (dash_len - phase).min(len - cursor);
                let p0 = Point::new(a.x + ux * cursor, a.y + uy * cursor);
                let p1 = Point::new(a.x + ux * (cursor + take), a.y + uy * (cursor + take));
                surface.draw_line_aa(p0, p1, color, width);
                cursor += take;
                dist_in_cycle += take;
            } else {
                let skip = (period - phase).min(len - cursor);
                cursor += skip;
                dist_in_cycle += skip;
            }
        }
    }
}

fn draw_horizontal_rules(
    surface: &mut Surface,
    doc: &Document,
    spacing: i32,
    start_x: f64,
    start_y: i32,
    end_y: i32,
) {
    let line_c = doc.paper_color.line_color();
    let end_x = f64::from(doc.width).max(start_x);
    let mut y = start_y;
    while y <= end_y {
        let fy = f64::from(y) + 0.5;
        surface.draw_line_aa(Point::new(start_x, fy), Point::new(end_x, fy), line_c, 1.0);
        y = y.saturating_add(spacing.max(8));
    }
}

fn draw_dot(surface: &mut Surface, cx: i32, cy: i32, color: drawcraft_color::Color) {
    let c = color.sanitized();
    for dy in -1..=1 {
        for dx in -1..=1 {
            let dist = f64::hypot(f64::from(dx), f64::from(dy));
            let cov = (1.35 - dist).clamp(0.0, 1.0) as f32;
            if cov > 0.0 {
                surface.blend_pixel(
                    cx + dx,
                    cy + dy,
                    [c.r, c.g, c.b, c.a * cov],
                    BlendMode::Normal,
                );
            }
        }
    }
}

/// Renders the paper background and ruling lines for `doc` onto `surface`.
pub fn render_paper(surface: &mut Surface, doc: &Document) {
    let w = surface.width() as i32;
    let h = surface.height() as i32;
    surface.fill_rect(IRect::new(0, 0, w, h), doc.paper_color.background_color());

    let line_c = doc.paper_color.line_color();
    let margin_c = doc.paper_color.margin_color();

    match doc.paper_template {
        PaperTemplate::Blank => {}
        PaperTemplate::RuledNarrow => {
            draw_horizontal_rules(surface, doc, 24, 0.0, 96, h - 48);
            surface.draw_line_aa(
                Point::new(112.5, 0.0),
                Point::new(112.5, f64::from(h)),
                margin_c,
                1.2,
            );
        }
        PaperTemplate::RuledWide => {
            draw_horizontal_rules(surface, doc, 34, 0.0, 96, h - 48);
            surface.draw_line_aa(
                Point::new(112.5, 0.0),
                Point::new(112.5, f64::from(h)),
                margin_c,
                1.2,
            );
        }
        PaperTemplate::Grid => {
            let step = 24;
            draw_horizontal_rules(surface, doc, step, 0.0, step, h - step);
            let mut x = step;
            while x <= w - step {
                let fx = f64::from(x) + 0.5;
                surface.draw_line_aa(
                    Point::new(fx, 0.0),
                    Point::new(fx, f64::from(h)),
                    line_c,
                    1.0,
                );
                x += step;
            }
        }
        PaperTemplate::Dotted => {
            let step = 24;
            let mut y = step;
            while y <= h - step {
                let mut x = step;
                while x <= w - step {
                    draw_dot(surface, x, y, line_c);
                    x += step;
                }
                y += step;
            }
        }
        PaperTemplate::Cornell => {
            let header_y = 110.min(h / 4);
            let footer_y = (h - 200).max(header_y + 60);
            let cue_x = 280.min(w / 3);
            surface.draw_line_aa(
                Point::new(0.0, f64::from(header_y) + 0.5),
                Point::new(f64::from(w), f64::from(header_y) + 0.5),
                margin_c,
                1.4,
            );
            surface.draw_line_aa(
                Point::new(0.0, f64::from(footer_y) + 0.5),
                Point::new(f64::from(w), f64::from(footer_y) + 0.5),
                margin_c,
                1.4,
            );
            surface.draw_line_aa(
                Point::new(f64::from(cue_x) + 0.5, f64::from(header_y)),
                Point::new(f64::from(cue_x) + 0.5, f64::from(footer_y)),
                margin_c,
                1.4,
            );
            draw_horizontal_rules(
                surface,
                doc,
                28,
                f64::from(cue_x) + 12.0,
                header_y + 28,
                footer_y - 14,
            );
        }
        PaperTemplate::Isometric
        | PaperTemplate::MusicStaff
        | PaperTemplate::WeeklyPlanner
        | PaperTemplate::MonthlyPlanner
        | PaperTemplate::Checklist => {
            render_structured_paper_geometry(surface, doc, line_c, margin_c);
        }
    }
}

fn render_structured_paper_geometry(
    surface: &mut Surface,
    doc: &Document,
    line_c: drawcraft_color::Color,
    margin_c: drawcraft_color::Color,
) {
    let geom = drawcraft_doc::paper_template_geometry(doc.width, doc.height, doc.paper_template);
    for line in &geom.lines {
        let color = if line.accent { margin_c } else { line_c };
        surface.draw_line_aa(
            Point::new(f64::from(line.x1), f64::from(line.y1)),
            Point::new(f64::from(line.x2), f64::from(line.y2)),
            color,
            f64::from(line.stroke_width),
        );
    }
    for box_item in &geom.checkboxes {
        let x0 = f64::from(box_item.x);
        let y0 = f64::from(box_item.y);
        let x1 = x0 + f64::from(box_item.size);
        let y1 = y0 + f64::from(box_item.size);
        surface.draw_line_aa(Point::new(x0, y0), Point::new(x1, y0), margin_c, 1.25);
        surface.draw_line_aa(Point::new(x1, y0), Point::new(x1, y1), margin_c, 1.25);
        surface.draw_line_aa(Point::new(x1, y1), Point::new(x0, y1), margin_c, 1.25);
        surface.draw_line_aa(Point::new(x0, y1), Point::new(x0, y0), margin_c, 1.25);
    }
    for label in &geom.labels {
        let item = TextItem {
            text: label.text.to_string(),
            x: f64::from(label.x),
            y: f64::from(label.y),
            size: label.font_size,
            color: if label.accent { margin_c } else { line_c },
            ..TextItem::default()
        };
        draw_text_item(surface, doc.width, &item);
    }
}

fn draw_page_image(surface: &mut Surface, item: &PageImage) {
    let clean = item.sanitized();
    if clean.png_bytes.is_empty() {
        return;
    }
    let Ok(decoded) = image::load_from_memory(&clean.png_bytes) else {
        return;
    };
    let rgba = decoded.to_rgba8();
    let img_w = rgba.width();
    let img_h = rgba.height();
    if img_w == 0 || img_h == 0 {
        return;
    }

    let bounds = clean.bounds();
    let sw = surface.width() as i32;
    let sh = surface.height() as i32;
    let min_x = (bounds.x.floor() as i32).max(0);
    let max_x = (bounds.right().ceil() as i32).min(sw.saturating_sub(1));
    let min_y = (bounds.y.floor() as i32).max(0);
    let max_y = (bounds.bottom().ceil() as i32).min(sh.saturating_sub(1));
    if min_x > max_x || min_y > max_y {
        return;
    }

    let center = clean.center();
    let (sin, cos) = clean.angle_degrees.to_radians().sin_cos();
    let inv_w = 1.0 / clean.width;
    let inv_h = 1.0 / clean.height;

    for py in min_y..=max_y {
        let dy = f64::from(py) + 0.5 - center.y;
        for px in min_x..=max_x {
            let dx = f64::from(px) + 0.5 - center.x;
            let lx = dx * cos + dy * sin;
            let ly = -dx * sin + dy * cos;
            let u = lx * inv_w + 0.5;
            let v = ly * inv_h + 0.5;
            if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                continue;
            }
            let sx = ((u * f64::from(img_w)).floor() as u32).min(img_w - 1);
            let sy = ((v * f64::from(img_h)).floor() as u32).min(img_h - 1);
            let p = rgba.get_pixel(sx, sy).0;
            if p[3] == 0 {
                continue;
            }
            surface.blend_pixel(
                px,
                py,
                [
                    f32::from(p[0]) / 255.0,
                    f32::from(p[1]) / 255.0,
                    f32::from(p[2]) / 255.0,
                    f32::from(p[3]) / 255.0,
                ],
                BlendMode::Normal,
            );
        }
    }
}

fn blit_opaque_rgba(surface: &mut Surface, rgba: &[u8], w: u32, h: u32) {
    let max_x = surface.width().min(w);
    let max_y = surface.height().min(h);
    let row_stride = w as usize;
    for y in 0..max_y {
        let row_offset = (y as usize) * row_stride;
        for x in 0..max_x {
            let idx = (row_offset + x as usize) * 4;
            if let Some(px) = rgba.get(idx..idx + 4) {
                surface.set_pixel_u8(x as i32, y as i32, [px[0], px[1], px[2], px[3]]);
            }
        }
    }
}

fn rotate_rgba_quarter_turns(rgba: &[u8], w: u32, h: u32, turns: u8) -> (Vec<u8>, u32, u32) {
    let t = turns % 4;
    if t == 0 || w == 0 || h == 0 {
        return (rgba.to_vec(), w, h);
    }
    let (out_w, out_h) = if t.is_multiple_of(2) { (w, h) } else { (h, w) };
    let mut out = vec![0u8; rgba.len()];
    let w_usize = w as usize;
    let out_w_usize = out_w as usize;
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match t {
                1 => (h - 1 - y, x),
                2 => (w - 1 - x, h - 1 - y),
                3 => (y, w - 1 - x),
                _ => (x, y),
            };
            let src = ((y as usize) * w_usize + (x as usize)) * 4;
            let dst = ((ny as usize) * out_w_usize + (nx as usize)) * 4;
            if let (Some(sp), Some(dp)) = (rgba.get(src..src + 4), out.get_mut(dst..dst + 4)) {
                dp.copy_from_slice(sp);
            }
        }
    }
    (out, out_w, out_h)
}

/// Renders the entire `doc` (PDF or custom backdrop or paper template + images + committed strokes + optional active live stroke).
pub fn render_document(
    doc: &Document,
    live_stroke: Option<(&PenSettings, &[StrokeSample])>,
) -> Surface {
    let pdf_backdrop_source = doc.pdf_backdrop.and_then(|backdrop| {
        doc.pdf_documents
            .get(backdrop.pdf_index)
            .map(|pdf| (pdf, backdrop.page_index))
    });
    let custom_image_source = doc
        .custom_image_backdrop
        .and_then(|idx| doc.image_templates.get(idx))
        .filter(|tpl| !tpl.png_bytes.is_empty());
    let bg = if pdf_backdrop_source.is_some() {
        drawcraft_color::Color::WHITE
    } else {
        doc.paper_color.background_color()
    };
    let mut surface = Surface::new(doc.width, doc.height, bg);
    let rot = doc.backdrop_rotation % 4;
    if let Some((pdf, page_index)) = pdf_backdrop_source {
        let (req_w, req_h) = if rot.is_multiple_of(2) {
            (surface.width(), surface.height())
        } else {
            (surface.height(), surface.width())
        };
        if let Ok(rendered) = pdf::render_pdf_page_exact_rgba(
            std::sync::Arc::clone(&pdf.bytes),
            page_index,
            req_w,
            req_h,
        ) {
            let (rgba, rw, rh) =
                rotate_rgba_quarter_turns(&rendered.rgba, rendered.width, rendered.height, rot);
            blit_opaque_rgba(&mut surface, &rgba, rw, rh);
        }
    } else if let Some(tpl) = custom_image_source {
        let sw = f64::from(surface.width());
        let sh = f64::from(surface.height());
        let (unrot_w, unrot_h) = if rot.is_multiple_of(2) {
            (sw, sh)
        } else {
            (sh, sw)
        };
        let bg_img = PageImage {
            x: (sw - unrot_w) * 0.5,
            y: (sh - unrot_h) * 0.5,
            width: unrot_w,
            height: unrot_h,
            angle_degrees: f64::from(rot) * 90.0,
            pixel_width: surface.width(),
            pixel_height: surface.height(),
            png_bytes: std::sync::Arc::clone(&tpl.png_bytes),
        };
        draw_page_image(&mut surface, &bg_img);
    } else {
        render_paper(&mut surface, doc);
    }
    for image in &doc.images {
        draw_page_image(&mut surface, image);
    }
    for stroke in &doc.strokes {
        render_stroke_to_surface(&mut surface, &stroke.pen, &stroke.smoothed_points);
    }
    for item in &doc.texts {
        draw_text_item(&mut surface, doc.width, item);
    }
    for shape in &doc.shapes {
        draw_shape(&mut surface, shape);
    }
    if let Some((pen, samples)) = live_stroke {
        render_stroke_to_surface(&mut surface, pen, samples);
    }
    draw_hidden_study_tapes(&mut surface, doc);
    surface
}

/// Covers content under every study tape that has not been revealed, so exports and previews
/// never leak what the learner is meant to recall.
fn draw_hidden_study_tapes(surface: &mut Surface, doc: &Document) {
    for tape in doc.study_tapes.iter().filter(|t| !t.revealed) {
        let t = tape.sanitized();
        let color = drawcraft_color::Color::from_rgba8(
            (t.color_rgba >> 24) as u8,
            (t.color_rgba >> 16) as u8,
            (t.color_rgba >> 8) as u8,
            t.color_rgba as u8,
        );
        let (x0, y0) = (t.x.floor() as i32, t.y.floor() as i32);
        let (x1, y1) = (
            (t.x + t.width).ceil() as i32,
            (t.y + t.height).ceil() as i32,
        );
        surface.fill_rect(IRect::new(x0, y0, x1 - x0, y1 - y0), color);
    }
}

#[cfg(test)]
mod pdf_render_tests {
    use std::sync::Arc;

    use super::*;
    use drawcraft_doc::{EmbeddedPdf, InkStroke, PdfPageBackdrop};

    fn minimal_blue_rect_pdf() -> Vec<u8> {
        let objects = [
            b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".as_slice(),
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".as_slice(),
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << >> >>\nendobj\n".as_slice(),
            b"4 0 obj\n<< /Length 28 >>\nstream\n0 0 1 rg 0 0 100 100 re f\nendstream\nendobj\n".as_slice(),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0usize];
        for obj in objects {
            offsets.push(pdf.len());
            pdf.extend_from_slice(obj);
        }
        let xref_offset = pdf.len();
        pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for off in &offsets[1..] {
            pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n")
                .as_bytes(),
        );
        pdf
    }

    #[test]
    fn render_document_composites_ink_over_pdf_backdrop() {
        let pdf_bytes = Arc::new(minimal_blue_rect_pdf());
        let summary = pdf::inspect_pdf(Arc::clone(&pdf_bytes), 16).expect("valid pdf");
        assert_eq!(summary.pages.len(), 1);
        assert_eq!(summary.pages[0].page_width, 200);
        assert_eq!(summary.pages[0].page_height, 200);

        let mut doc = Document::new(200, 200);
        doc.pdf_documents.push(EmbeddedPdf {
            name: "sample.pdf".to_string(),
            bytes: pdf_bytes,
        });
        doc.pdf_backdrop = Some(PdfPageBackdrop {
            pdf_index: 0,
            page_index: 0,
        });
        let id = doc.alloc_stroke_id();
        let mut pen = PenSettings::ball_pen();
        pen.color = drawcraft_color::Color::from_rgba8(255, 0, 0, 255);
        pen.size = 14.0;
        let pts = vec![
            StrokeSample::from_xy_pressure(90.0, 100.0, 0.8),
            StrokeSample::from_xy_pressure(115.0, 100.0, 0.8),
        ];
        doc.strokes
            .push(Arc::new(InkStroke::new(id, pen, pts.clone(), pts)));
        doc.sync_active_to_pages();

        let surface = render_document(&doc, None);
        let corner = surface.pixel_u8(5, 5);
        assert!(
            corner[2] > 200 && corner[0] < 40,
            "expected blue PDF backdrop at corner, got {corner:?}"
        );
        let center = surface.pixel_u8(105, 100);
        assert!(
            center[0] > 200 && center[2] < 40,
            "expected red stroke over PDF backdrop, got {center:?}"
        );
    }

    #[test]
    fn with_pdf_renderer_reuses_parsed_pdf_across_renders() {
        let pdf_bytes = Arc::new(minimal_blue_rect_pdf());
        let (count, thumb, hi_res) = pdf::with_pdf_renderer(pdf_bytes, |renderer| {
            let count = renderer.page_count();
            let thumb = renderer.render_page_rgba(0, 120, 120).expect("thumb");
            let hi_res = renderer.render_page_rgba(0, 400, 400).expect("hi_res");
            (count, thumb, hi_res)
        })
        .expect("with_pdf_renderer succeeds");
        assert_eq!(count, 1);
        assert_eq!((thumb.width, thumb.height), (120, 120));
        assert_eq!((hi_res.width, hi_res.height), (400, 400));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use drawcraft_color::Color;
    use drawcraft_doc::{InkStroke, PaperColor};
    use std::sync::Arc;

    #[test]
    fn renders_paper_and_strokes_cleanly_to_png() {
        let mut doc = Document::new(300, 400);
        doc.paper_template = PaperTemplate::Cornell;
        doc.paper_color = PaperColor::WarmCream;
        let id = doc.alloc_stroke_id();
        let pts = vec![
            StrokeSample::from_xy_pressure(50.0, 150.0, 0.3),
            StrokeSample::from_xy_pressure(180.0, 180.0, 0.9),
            StrokeSample::from_xy_pressure(240.0, 160.0, 0.5),
        ];
        doc.strokes.push(Arc::new(InkStroke::new(
            id,
            PenSettings::fountain_pen(),
            pts.clone(),
            pts,
        )));

        let surface = render_document(&doc, None);
        assert_ne!(surface.pixel_u8(10, 10), Color::BLACK.to_rgba8());
        let png = surface.encode_png().unwrap();
        assert!(png.len() > 100);
    }

    #[test]
    fn renders_placed_page_image_beneath_ink_strokes() {
        let mut doc = Document::new(240, 240);
        doc.paper_color = PaperColor::PureWhite;
        let png_bytes = drawcraft_raster::png_encode(
            2,
            2,
            &[
                220, 40, 40, 255, // red
                220, 40, 40, 255, // red
                220, 40, 40, 255, // red
                220, 40, 40, 255, // red
            ],
        )
        .unwrap();
        doc.images.push(Arc::new(PageImage {
            x: 40.0,
            y: 40.0,
            width: 80.0,
            height: 80.0,
            angle_degrees: 0.0,
            pixel_width: 2,
            pixel_height: 2,
            png_bytes: Arc::new(png_bytes),
        }));

        let id = doc.alloc_stroke_id();
        let mut pen = PenSettings::ball_pen();
        pen.color = Color::from_rgba8(20, 40, 220, 255);
        pen.size = 6.0;
        let pts = vec![
            StrokeSample::from_xy_pressure(50.0, 80.0, 1.0),
            StrokeSample::from_xy_pressure(110.0, 80.0, 1.0),
        ];
        doc.strokes
            .push(Arc::new(InkStroke::new(id, pen, pts.clone(), pts)));

        let surface = render_document(&doc, None);
        // Inside image away from stroke should be red
        assert_eq!(surface.pixel_u8(60, 55), [220, 40, 40, 255]);
        // On stroke over image should be ink blue (composite on top)
        assert_eq!(surface.pixel_u8(80, 80), [20, 40, 220, 255]);
        // Outside image stays paper white
        assert_eq!(surface.pixel_u8(10, 10), [255, 255, 255, 255]);
    }

    #[test]
    fn renders_all_new_paper_templates_and_custom_rotated_image_template() {
        use drawcraft_doc::{CustomPaperTemplate, CustomTemplateKind};

        for tpl in [
            PaperTemplate::Isometric,
            PaperTemplate::MusicStaff,
            PaperTemplate::WeeklyPlanner,
            PaperTemplate::MonthlyPlanner,
            PaperTemplate::Checklist,
        ] {
            let mut doc = Document::new(400, 500);
            doc.paper_template = tpl;
            let surface = render_document(&doc, None);
            assert_eq!(surface.width(), 400);
            assert_eq!(surface.height(), 500);
            assert!(!surface.encode_png().unwrap().is_empty());
        }

        // Custom image backdrop: left half red, right half blue. After 90 deg CW rotation,
        // top half should be red and bottom half blue.
        let png = drawcraft_raster::png_encode(
            2,
            1,
            &[
                220, 30, 30, 255, // left red
                30, 30, 220, 255, // right blue
            ],
        )
        .unwrap();
        let mut doc = Document::new(100, 200);
        let tpl = CustomPaperTemplate {
            id: "split".to_string(),
            name: "Split".to_string(),
            kind: CustomTemplateKind::Image,
            width: 100,
            height: 200,
            pdf_page_index: 0,
            bytes: Arc::new(png),
        };
        doc.apply_custom_template_to_page(0, &tpl).unwrap();
        assert!(doc.rotate_page_quarter_turns(0, 1));
        assert_eq!((doc.width, doc.height), (200, 100));
        let surface = render_document(&doc, None);
        assert_eq!(surface.pixel_u8(100, 15), [220, 30, 30, 255]);
        assert_eq!(surface.pixel_u8(100, 85), [30, 30, 220, 255]);
    }
}

#[cfg(test)]
mod study_tape_render_tests {
    use super::*;
    use drawcraft_doc::StudyTapeItem;

    #[test]
    fn hidden_tapes_cover_the_page_and_revealed_tapes_do_not() {
        let mut doc = Document::new(200, 200);
        doc.study_tapes
            .push(StudyTapeItem::new(1, 20.0, 20.0, 100.0, 40.0));
        let hidden = render_document(&doc, None);
        let inside = hidden.pixel_u8(70, 40);
        assert_eq!(
            &inside[..3],
            &[0x0B, 0x5F, 0xFF],
            "tape colour covers the page"
        );
        let outside = hidden.pixel_u8(170, 170);
        assert_ne!(&outside[..3], &[0x0B, 0x5F, 0xFF]);

        doc.study_tapes[0].revealed = true;
        let revealed = render_document(&doc, None);
        assert_ne!(&revealed.pixel_u8(70, 40)[..3], &[0x0B, 0x5F, 0xFF]);
    }

    #[test]
    fn tape_beyond_the_page_edge_is_clipped_not_a_crash() {
        let mut doc = Document::new(100, 100);
        doc.study_tapes
            .push(StudyTapeItem::new(1, 80.0, 80.0, 500.0, 500.0));
        doc.study_tapes
            .push(StudyTapeItem::new(2, -400.0, -400.0, 450.0, 450.0));
        let surface = render_document(&doc, None);
        assert_eq!(&surface.pixel_u8(95, 95)[..3], &[0x0B, 0x5F, 0xFF]);
        assert_eq!(&surface.pixel_u8(10, 10)[..3], &[0x0B, 0x5F, 0xFF]);
    }
}
