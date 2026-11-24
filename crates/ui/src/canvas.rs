//! Interactive drawing & handwriting canvas with cached ribbon meshes,
//! Paper ruling, pulled-string tether, and dynamic nib/eraser cursor.

use std::collections::HashSet;
use std::sync::Arc;

use drawcraft_doc::{
    Document, GeometricShape, InkStroke, ListItemKind, PageImage, PaperColor, PaperTemplate,
    PenKind, PenSettings, ShapeKind, StrokeId, StrokeSample, TextAlignment, TextFont, TextItem,
    TextSpan, TextStyleAttr, UiTheme, normalize_text_spans, parse_list_line_prefix,
};
use drawcraft_engine::ToolKind;
use drawcraft_geom::{Affine2, Point};
use drawcraft_paint::{
    RibbonMesh, SpineNode, apply_streamline, build_stroke_ribbon, evaluate_spine,
    resample_centripetal_catmull_rom,
};
use egui::{
    Color32, CornerRadius, FontId, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2,
};

use crate::theme::{Tokens, to_color32};
use crate::{DrawCraftApp, SelectedObject};

pub(crate) fn page_affine(app: &DrawCraftApp) -> Affine2 {
    Affine2::new(f64::from(app.view.zoom), app.view.pan[0], app.view.pan[1])
}

fn compute_fit_view(app: &DrawCraftApp, rect: Rect) -> (f32, [f64; 2]) {
    let w = f32::from(app.session.doc.width as u16).max(64.0);
    let h = f32::from(app.session.doc.height as u16).max(64.0);
    let pad = 40.0_f32;
    let avail_w = (rect.width() - pad * 2.0).max(100.0);
    let avail_h = (rect.height() - pad * 2.0).max(100.0);
    let scale = (avail_w / w).min(avail_h / h).clamp(0.15, 4.0);
    let ox = f64::from(rect.center().x) - f64::from(w) * f64::from(scale) * 0.5;
    let oy = f64::from(rect.center().y) - f64::from(h) * f64::from(scale) * 0.5;
    (scale, [ox, oy])
}

fn fit_page_into_rect(app: &mut DrawCraftApp, rect: Rect) {
    let (scale, pan) = compute_fit_view(app, rect);
    app.view.zoom = scale;
    app.view.pan = pan;
    app.session.zoom = scale;
}

fn cancel_camera_glide(app: &mut DrawCraftApp) {
    app.view.motion.camera_glide = None;
    app.view.motion.fit_glide_requested = false;
}

fn advance_canvas_camera(app: &mut DrawCraftApp, ui: &egui::Ui, avail: Rect, dt: f32) {
    if avail.width() > 32.0 && avail.height() > 32.0 {
        if app.view.fit_requested {
            fit_page_into_rect(app, avail);
            app.view.fit_requested = false;
            cancel_camera_glide(app);
        } else if app.view.motion.fit_glide_requested {
            let (target_zoom, target_pan) = compute_fit_view(app, avail);
            app.view.motion.camera_glide = Some(crate::motion::CameraGlide::new(
                app.view.zoom,
                app.view.pan,
                target_zoom,
                target_pan,
                [f64::from(avail.center().x), f64::from(avail.center().y)],
            ));
            app.view.motion.fit_glide_requested = false;
        }
    }

    if let Some(mut glide) = app.view.motion.camera_glide.take() {
        let (zoom, pan, finished) = glide.step(dt);
        app.view.zoom = zoom;
        app.view.pan = pan;
        app.session.zoom = zoom;
        if !finished {
            app.view.motion.camera_glide = Some(glide);
            ui.ctx().request_repaint();
        }
    }
}

const MAX_BATCH_VERTICES: usize = 60_000;
const MAX_CACHE_TRANSLATE_STEPS: u16 = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
struct PaperCacheKey {
    page_key: (usize, usize),
    template: PaperTemplate,
    width: u32,
    height: u32,
    color_key: (UiTheme, bool, PaperColor),
    zoom_bits: u32,
}

#[derive(Clone, Debug, Default)]
struct PaperShapeCache {
    key: Option<PaperCacheKey>,
    rendered_pan: [f64; 2],
    translate_steps: u16,
    shapes: Vec<Shape>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CommittedBatchKey {
    page_key: (usize, usize),
    color_key: (UiTheme, bool, PaperColor),
    revision: u64,
    stroke_count: usize,
    last_stroke_id: Option<StrokeId>,
    zoom_bits: u32,
}

#[derive(Clone, Default)]
struct CommittedMeshCache {
    key: Option<CommittedBatchKey>,
    rendered_pan: [f64; 2],
    translate_steps: u16,
    batches: Vec<Arc<Mesh>>,
    hl_rect: Option<Rect>,
    hl_clipped: bool,
    hl_texture: Option<egui::TextureHandle>,
    built_page_key: Option<(usize, usize)>,
    built_clip_page: Option<drawcraft_geom::Rect>,
    built_has_culled_strokes: bool,
    built_zoom_bucket: Option<f32>,
    built_color_key: Option<(UiTheme, bool, PaperColor)>,
    built_stroke_count: usize,
    built_stroke_refs: Vec<(StrokeId, usize)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CommittedShapeKey {
    page_key: (usize, usize),
    color_key: (UiTheme, bool, PaperColor),
    shape_digest: u64,
    shape_count: usize,
    zoom_bits: u32,
}

#[derive(Clone, Debug, Default)]
struct CommittedShapeCache {
    key: Option<CommittedShapeKey>,
    shape_ptrs: Vec<usize>,
    built_clip_page: Option<drawcraft_geom::Rect>,
    built_has_culled: bool,
    rendered_pan: [f64; 2],
    translate_steps: u16,
    shapes: Vec<Shape>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CommittedTextKey {
    page_key: (usize, usize),
    color_key: (UiTheme, bool, PaperColor),
    text_digest: u64,
    text_count: usize,
    doc_width: u32,
}

#[derive(Clone, Debug)]
struct CachedCommittedTextItem {
    clean: TextItem,
    color: Color32,
    angle: f32,
    rot: egui::emath::Rot2,
    rough_page_rect: Rect,
    baked_job: Option<Arc<egui::text::LayoutJob>>,
    galley: Option<Arc<egui::Galley>>,
    prefix_rects: Vec<(usize, ListItemKind, Rect)>,
    emitted_key: Option<(u32, u32, u32, u32, usize)>,
    emitted_galley: Option<Arc<egui::Galley>>,
}

#[derive(Clone, Debug, Default)]
struct CommittedTextCache {
    key: Option<CommittedTextKey>,
    text_ptrs: Vec<usize>,
    baked_zoom: f32,
    baked_ppp_bits: u32,
    last_zoom_bits: u32,
    last_zoom_change_s: f64,
    pending_zoom_rebake: bool,
    items: Vec<CachedCommittedTextItem>,
}

type RibbonSyncKey = (
    (usize, usize),
    u64,
    usize,
    Option<StrokeId>,
    u32,
    (UiTheme, bool, PaperColor),
);

#[derive(Clone, Debug)]
struct SelectionBoundsCache {
    revision: u64,
    doc_ptr: usize,
    page_index: usize,
    selection_sig: u64,
    zoom_bits: u32,
    stroke_bounds_at_origin: Option<Rect>,
}

/// Cached screen-space geometry for paper ruling, committed ink strokes, shapes, and text.
#[derive(Clone, Default)]
pub(crate) struct CanvasRenderCache {
    paper: PaperShapeCache,
    committed: CommittedMeshCache,
    shapes: CommittedShapeCache,
    texts: CommittedTextCache,
    last_ribbon_sync_key: Option<RibbonSyncKey>,
    pub(crate) gpu_scene: crate::gpu_ink::GpuStrokeSceneCache,
    pub(crate) gpu_overlay_hl: Option<Arc<crate::gpu_ink::GpuOverlayHighlighters>>,
    pub(crate) gpu_overlay_ink: Option<Arc<crate::gpu_ink::GpuByteMesh>>,
    selection_bounds: Option<SelectionBoundsCache>,
}

fn append_dot_hex(mesh: &mut Mesh, center: Pos2, r: f32, color: Color32) {
    let base = mesh.vertices.len() as u32;
    mesh.vertices.push(egui::epaint::Vertex {
        pos: center,
        uv: egui::epaint::WHITE_UV,
        color,
    });
    for i in 0..6u32 {
        let angle = (i as f32) * (std::f32::consts::TAU / 6.0);
        mesh.vertices.push(egui::epaint::Vertex {
            pos: pos2(center.x + angle.cos() * r, center.y + angle.sin() * r),
            uv: egui::epaint::WHITE_UV,
            color,
        });
    }
    for i in 0..6u32 {
        let next = (i + 1) % 6;
        mesh.indices
            .extend_from_slice(&[base, base + 1 + i, base + 1 + next]);
    }
}

fn pre_tessellate_shapes(raw_shapes: Vec<Shape>, pixels_per_point: f32) -> Vec<Shape> {
    if raw_shapes.is_empty() {
        return Vec::new();
    }
    let ppp = if pixels_per_point.is_finite() && pixels_per_point > 0.1 {
        pixels_per_point
    } else {
        1.0
    };
    let opts = egui::epaint::TessellationOptions {
        prerasterized_discs: false,
        coarse_tessellation_culling: false,
        round_rects_to_pixels: false,
        round_line_segments_to_pixels: false,
        round_text_to_pixels: false,
        ..Default::default()
    };
    let mut tess = egui::epaint::Tessellator::new(ppp, opts, [1, 1], Vec::new());

    let mut out = Vec::new();
    let mut mesh = Mesh::default();
    let mut scratch = Mesh::default();
    for shape in raw_shapes {
        match shape {
            Shape::Text(_) | Shape::Callback(_) => {
                if !mesh.indices.is_empty() {
                    out.push(Shape::mesh(Arc::new(std::mem::take(&mut mesh))));
                }
                out.push(shape);
            }
            other => {
                scratch.clear();
                tess.tessellate_shape(other, &mut scratch);
                if scratch.indices.is_empty() {
                    continue;
                }
                if !mesh.vertices.is_empty()
                    && mesh.vertices.len().saturating_add(scratch.vertices.len())
                        > MAX_BATCH_VERTICES
                {
                    out.push(Shape::mesh(Arc::new(std::mem::take(&mut mesh))));
                }
                mesh.append_ref(&scratch);
            }
        }
    }
    if !mesh.indices.is_empty() {
        out.push(Shape::mesh(Arc::new(mesh)));
    }
    out
}

fn push_paper_line(shapes: &mut Vec<Shape>, a: Pos2, b: Pos2, stroke: Stroke) {
    shapes.push(Shape::line_segment([a, b], stroke));
}

fn build_dotted_shapes(w: f64, h: f64, line_c: Color32, xf: Affine2) -> Vec<Shape> {
    let step = 24.0;
    let r = (1.25 * app_scale(xf)).clamp(0.7, 2.5);
    let mut out = Vec::new();
    let mut mesh = Mesh::default();
    let mut y = step;
    while y <= h - step {
        let mut x = step;
        while x <= w - step {
            if mesh.vertices.len() + 7 > MAX_BATCH_VERTICES {
                out.push(Shape::mesh(Arc::new(std::mem::take(&mut mesh))));
            }
            let sp = xf.to_screen(Point::new(x, y));
            append_dot_hex(&mut mesh, pos2(sp.x as f32, sp.y as f32), r, line_c);
            x += step;
        }
        y += step;
    }
    if !mesh.indices.is_empty() {
        out.push(Shape::mesh(Arc::new(mesh)));
    }
    out
}

fn build_paper_markings(
    p: &egui::Painter,
    doc: &Document,
    colors: drawcraft_doc::ResolvedPaperColors,
    xf: Affine2,
) -> Vec<Shape> {
    let line_c = to_color32(colors.line);
    let margin_c = to_color32(colors.margin);
    let to_s = |px: f64, py: f64| -> Pos2 {
        let sp = xf.to_screen(Point::new(px, py));
        pos2(sp.x as f32, sp.y as f32)
    };
    let w = f64::from(doc.width);
    let h = f64::from(doc.height);
    let lw = (1.0 * app_scale(xf)).clamp(0.6, 2.0);
    let mut shapes = Vec::new();

    match doc.paper_template {
        PaperTemplate::Blank => {}
        PaperTemplate::RuledNarrow | PaperTemplate::RuledWide => {
            let spacing = if doc.paper_template == PaperTemplate::RuledNarrow {
                24.0
            } else {
                34.0
            };
            let mut y = 96.0;
            while y <= h - 48.0 {
                push_paper_line(
                    &mut shapes,
                    to_s(0.0, y),
                    to_s(w, y),
                    Stroke::new(lw, line_c),
                );
                y += spacing;
            }
            push_paper_line(
                &mut shapes,
                to_s(112.0, 0.0),
                to_s(112.0, h),
                Stroke::new(lw * 1.2, margin_c),
            );
        }
        PaperTemplate::Grid => {
            let step = 24.0;
            let mut y = step;
            while y <= h - step {
                push_paper_line(
                    &mut shapes,
                    to_s(0.0, y),
                    to_s(w, y),
                    Stroke::new(lw, line_c),
                );
                y += step;
            }
            let mut x = step;
            while x <= w - step {
                push_paper_line(
                    &mut shapes,
                    to_s(x, 0.0),
                    to_s(x, h),
                    Stroke::new(lw, line_c),
                );
                x += step;
            }
        }
        PaperTemplate::Dotted => return build_dotted_shapes(w, h, line_c, xf),
        PaperTemplate::Cornell => {
            let header_y = 110.0_f64.min(h * 0.25);
            let footer_y = (h - 200.0).max(header_y + 60.0);
            let cue_x = 280.0_f64.min(w / 3.0);
            push_paper_line(
                &mut shapes,
                to_s(0.0, header_y),
                to_s(w, header_y),
                Stroke::new(lw * 1.4, margin_c),
            );
            push_paper_line(
                &mut shapes,
                to_s(0.0, footer_y),
                to_s(w, footer_y),
                Stroke::new(lw * 1.4, margin_c),
            );
            push_paper_line(
                &mut shapes,
                to_s(cue_x, header_y),
                to_s(cue_x, footer_y),
                Stroke::new(lw * 1.4, margin_c),
            );
            let mut y = header_y + 28.0;
            while y <= footer_y - 14.0 {
                push_paper_line(
                    &mut shapes,
                    to_s(cue_x + 12.0, y),
                    to_s(w, y),
                    Stroke::new(lw, line_c),
                );
                y += 28.0;
            }
        }
        PaperTemplate::Isometric
        | PaperTemplate::MusicStaff
        | PaperTemplate::WeeklyPlanner
        | PaperTemplate::MonthlyPlanner
        | PaperTemplate::Checklist => {
            let geom =
                drawcraft_doc::paper_template_geometry(doc.width, doc.height, doc.paper_template);
            let scale = app_scale(xf);
            for line in &geom.lines {
                let col = if line.accent { margin_c } else { line_c };
                let sw = (line.stroke_width * scale).clamp(0.55, 3.0);
                push_paper_line(
                    &mut shapes,
                    to_s(f64::from(line.x1), f64::from(line.y1)),
                    to_s(f64::from(line.x2), f64::from(line.y2)),
                    Stroke::new(sw, col),
                );
            }
            for bx in &geom.checkboxes {
                let tl = to_s(f64::from(bx.x), f64::from(bx.y));
                let br = to_s(f64::from(bx.x + bx.size), f64::from(bx.y + bx.size));
                shapes.push(Shape::rect_stroke(
                    Rect::from_min_max(tl, br),
                    CornerRadius::same(2),
                    Stroke::new((1.25 * scale).clamp(0.7, 2.5), margin_c),
                    StrokeKind::Inside,
                ));
            }
            for lbl in &geom.labels {
                let col = if lbl.accent { margin_c } else { line_c };
                let px_size = (lbl.font_size * scale).clamp(4.0, 96.0);
                let galley = p.layout_no_wrap(
                    lbl.text.to_string(),
                    egui::FontId::proportional(px_size),
                    col,
                );
                let origin = to_s(f64::from(lbl.x), f64::from(lbl.y));
                shapes.push(make_unrounded_text_shape(
                    origin,
                    galley,
                    col,
                    0.0,
                    p.pixels_per_point(),
                ));
            }
        }
    }
    pre_tessellate_shapes(shapes, p.pixels_per_point())
}

fn paint_page_sheet_base(p: &egui::Painter, page_rect: Rect, bg: Color32) {
    // Layered ambient + soft drop shadow behind the sheet so the paper gently lifts off the desk;
    // disable pixel rounding so zoom remains completely locked and shimmer-free.
    for (dy, expand, radius, alpha) in [
        (10.0_f32, 8.0_f32, 10_u8, 8_u8),
        (4.0, 3.0, 6, 16),
        (1.5, 1.0, 4, 22),
    ] {
        p.add(Shape::Rect(
            egui::epaint::RectShape::filled(
                page_rect.translate(Vec2::new(0.0, dy)).expand(expand),
                CornerRadius::same(radius),
                Color32::from_rgba_unmultiplied(15, 20, 34, alpha),
            )
            .with_round_to_pixels(false),
        ));
    }
    p.add(Shape::Rect(
        egui::epaint::RectShape::filled(page_rect, CornerRadius::same(3), bg)
            .with_round_to_pixels(false),
    ));
}

fn draw_paper_template(
    app: &mut DrawCraftApp,
    p: &egui::Painter,
    colors: drawcraft_doc::ResolvedPaperColors,
    page_rect: Rect,
    xf: Affine2,
) {
    let bg = to_color32(colors.background);
    paint_page_sheet_base(p, page_rect, bg);

    let (auto_contrast, paper_color) = crate::effective_doc_ink_context(app);
    let key = PaperCacheKey {
        page_key: (
            app.session.active_notebook,
            app.session.doc.active_page_index(),
        ),
        template: app.session.doc.paper_template,
        width: app.session.doc.width,
        height: app.session.doc.height,
        color_key: (app.session.prefs.theme, auto_contrast, paper_color),
        zoom_bits: app.view.zoom.to_bits(),
    };
    let cache = &mut app.canvas_cache.paper;
    if cache.key.as_ref() == Some(&key) {
        if cache.rendered_pan != app.view.pan {
            if cache.translate_steps < MAX_CACHE_TRANSLATE_STEPS {
                let dx = (app.view.pan[0] - cache.rendered_pan[0]) as f32;
                let dy = (app.view.pan[1] - cache.rendered_pan[1]) as f32;
                let delta = egui::vec2(dx, dy);
                for s in &mut cache.shapes {
                    s.translate(delta);
                }
                cache.rendered_pan = app.view.pan;
                cache.translate_steps = cache.translate_steps.saturating_add(1);
            } else {
                cache.shapes = build_paper_markings(p, &app.session.doc, colors, xf);
                cache.rendered_pan = app.view.pan;
                cache.translate_steps = 0;
            }
        }
    } else {
        cache.key = Some(key);
        cache.shapes = build_paper_markings(p, &app.session.doc, colors, xf);
        cache.rendered_pan = app.view.pan;
        cache.translate_steps = 0;
    }

    for shape in &cache.shapes {
        p.add(shape.clone());
    }

    p.add(Shape::Rect(
        egui::epaint::RectShape::stroke(
            page_rect,
            CornerRadius::same(2),
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 0, 0, 28)),
            StrokeKind::Outside,
        )
        .with_round_to_pixels(false),
    ));
}

#[inline]
fn app_scale(xf: Affine2) -> f32 {
    xf.scale as f32
}

fn append_ribbon_transformed(batch: &mut Mesh, ribbon: &RibbonMesh, xf: Affine2, is_pencil: bool) {
    let base = batch.vertices.len() as u32;
    batch.vertices.reserve(ribbon.vertices.len());
    batch.indices.reserve(ribbon.indices.len());
    for v in &ribbon.vertices {
        let sp = xf.to_screen(Point::new(f64::from(v.pos[0]), f64::from(v.pos[1])));
        let grain = if is_pencil {
            0.58 + 0.42 * drawcraft_paint::graphite_grain_factor(v.pos[0], v.pos[1])
        } else {
            1.0
        };
        let c = Color32::from_rgba_unmultiplied(
            (v.rgba[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (v.rgba[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (v.rgba[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            ((v.rgba[3] * grain).clamp(0.0, 1.0) * 255.0).round() as u8,
        );
        batch.vertices.push(egui::epaint::Vertex {
            pos: pos2(sp.x as f32, sp.y as f32),
            uv: egui::epaint::WHITE_UV,
            color: c,
        });
    }
    batch
        .indices
        .extend(ribbon.indices.iter().map(|i| base + *i));
}

fn ribbon_to_egui_mesh(ribbon: &RibbonMesh, xf: Affine2, is_pencil: bool) -> Mesh {
    let mut mesh = Mesh::default();
    append_ribbon_transformed(&mut mesh, ribbon, xf, is_pencil);
    mesh
}

pub(crate) fn build_live_ribbon_mesh(
    pen: &PenSettings,
    samples: &[StrokeSample],
    xf: Affine2,
    feather: f32,
) -> Option<Mesh> {
    if samples.is_empty() {
        return None;
    }
    let nodes = evaluate_spine(pen, samples);
    let ribbon = build_stroke_ribbon(pen, &nodes, feather);
    (!ribbon.indices.is_empty())
        .then(|| ribbon_to_egui_mesh(&ribbon, xf, pen.kind == PenKind::Pencil))
}

const MAX_CPU_HL_DIM: f32 = 2048.0;

#[inline]
fn blend_premul_over_color32(dst: Color32, src: Color32) -> Color32 {
    if src.a() == 0 {
        return dst;
    }
    if src.a() == 255 || dst.a() == 0 {
        return src;
    }
    let inv_sa = u16::from(255 - src.a());
    let r = src
        .r()
        .saturating_add(((u16::from(dst.r()) * inv_sa + 127) / 255) as u8);
    let g = src
        .g()
        .saturating_add(((u16::from(dst.g()) * inv_sa + 127) / 255) as u8);
    let b = src
        .b()
        .saturating_add(((u16::from(dst.b()) * inv_sa + 127) / 255) as u8);
    let a = src
        .a()
        .saturating_add(((u16::from(dst.a()) * inv_sa + 127) / 255) as u8);
    Color32::from_rgba_premultiplied(r, g, b, a)
}

fn mesh_screen_bounds(mesh: &Mesh) -> Option<Rect> {
    let mut rect = Rect::NOTHING;
    for v in &mesh.vertices {
        if v.pos.x.is_finite() && v.pos.y.is_finite() {
            rect.extend_with(v.pos);
        }
    }
    rect.is_positive().then_some(rect)
}

fn bounded_texture_dims(rect: Rect, base_ppp: f32) -> ([usize; 2], f32) {
    let ppp = if base_ppp.is_finite() && base_ppp > 0.1 {
        base_ppp.clamp(0.5, 4.0)
    } else {
        1.0
    };
    let w_pts = rect.width().max(1.0);
    let h_pts = rect.height().max(1.0);
    let scale_down = (MAX_CPU_HL_DIM / (w_pts * ppp))
        .min(MAX_CPU_HL_DIM / (h_pts * ppp))
        .min(1.0);
    let eff_ppp = (ppp * scale_down).max(0.1);
    let tex_w = ((w_pts * eff_ppp).ceil() as usize).clamp(1, MAX_CPU_HL_DIM as usize);
    let tex_h = ((h_pts * eff_ppp).ceil() as usize).clamp(1, MAX_CPU_HL_DIM as usize);
    ([tex_w, tex_h], eff_ppp)
}

#[derive(Clone, Copy)]
struct CpuRasterGrid {
    tex_size: [usize; 2],
    origin_pts: Pos2,
    ppp: f32,
}

fn rasterize_mesh_max_premul(
    scratch: &mut [Color32],
    grid: CpuRasterGrid,
    mesh: &Mesh,
) -> Option<[usize; 4]> {
    let [tex_w, tex_h] = grid.tex_size;
    if tex_w == 0 || tex_h == 0 || grid.ppp <= 0.0 || !grid.ppp.is_finite() {
        return None;
    }
    let inv_ppp = 1.0 / grid.ppp;
    let mut dirty_x0 = tex_w;
    let mut dirty_y0 = tex_h;
    let mut dirty_x1 = 0_usize;
    let mut dirty_y1 = 0_usize;

    let (triangles, _) = mesh.indices.as_chunks::<3>();
    for &tri in triangles {
        let (Some(v0), Some(v1), Some(v2)) = (
            mesh.vertices.get(tri[0] as usize),
            mesh.vertices.get(tri[1] as usize),
            mesh.vertices.get(tri[2] as usize),
        ) else {
            continue;
        };
        let (p0, p1, p2) = (v0.pos, v1.pos, v2.pos);
        if !(p0.x.is_finite()
            && p0.y.is_finite()
            && p1.x.is_finite()
            && p1.y.is_finite()
            && p2.x.is_finite()
            && p2.y.is_finite())
        {
            continue;
        }
        let area = (p1.x - p0.x) * (p2.y - p0.y) - (p2.x - p0.x) * (p1.y - p0.y);
        if area.abs() <= 1e-6 {
            continue;
        }
        let inv_area = 1.0 / area;

        let ix0 = (((p0.x.min(p1.x).min(p2.x) - grid.origin_pts.x) * grid.ppp).floor() as isize)
            .max(0) as usize;
        let iy0 = (((p0.y.min(p1.y).min(p2.y) - grid.origin_pts.y) * grid.ppp).floor() as isize)
            .max(0) as usize;
        let ix1 = (((p0.x.max(p1.x).max(p2.x) - grid.origin_pts.x) * grid.ppp).ceil() as isize)
            .min(tex_w as isize - 1);
        let iy1 = (((p0.y.max(p1.y).max(p2.y) - grid.origin_pts.y) * grid.ppp).ceil() as isize)
            .min(tex_h as isize - 1);
        if ix1 < ix0 as isize || iy1 < iy0 as isize {
            continue;
        }
        let (ix1, iy1) = (ix1 as usize, iy1 as usize);
        dirty_x0 = dirty_x0.min(ix0);
        dirty_y0 = dirty_y0.min(iy0);
        dirty_x1 = dirty_x1.max(ix1);
        dirty_y1 = dirty_y1.max(iy1);

        let (c0, c1, c2) = (v0.color, v1.color, v2.color);
        for iy in iy0..=iy1 {
            let py = grid.origin_pts.y + (iy as f32 + 0.5) * inv_ppp;
            let row = iy * tex_w;
            for ix in ix0..=ix1 {
                let px = grid.origin_pts.x + (ix as f32 + 0.5) * inv_ppp;
                let w0 = ((p1.x - px) * (p2.y - py) - (p2.x - px) * (p1.y - py)) * inv_area;
                let w1 = ((p2.x - px) * (p0.y - py) - (p0.x - px) * (p2.y - py)) * inv_area;
                let w2 = 1.0 - w0 - w1;
                if w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4 {
                    let (w0c, w1c, w2c) = (w0.max(0.0), w1.max(0.0), w2.max(0.0));
                    let sum = (w0c + w1c + w2c).max(1e-6);
                    let (a0, a1, a2) = (w0c / sum, w1c / sum, w2c / sum);
                    let r =
                        (a0 * f32::from(c0.r()) + a1 * f32::from(c1.r()) + a2 * f32::from(c2.r()))
                            .round()
                            .clamp(0.0, 255.0) as u8;
                    let g =
                        (a0 * f32::from(c0.g()) + a1 * f32::from(c1.g()) + a2 * f32::from(c2.g()))
                            .round()
                            .clamp(0.0, 255.0) as u8;
                    let b =
                        (a0 * f32::from(c0.b()) + a1 * f32::from(c1.b()) + a2 * f32::from(c2.b()))
                            .round()
                            .clamp(0.0, 255.0) as u8;
                    let a =
                        (a0 * f32::from(c0.a()) + a1 * f32::from(c1.a()) + a2 * f32::from(c2.a()))
                            .round()
                            .clamp(0.0, 255.0) as u8;
                    let cell = &mut scratch[row + ix];
                    *cell = Color32::from_rgba_premultiplied(
                        cell.r().max(r),
                        cell.g().max(g),
                        cell.b().max(b),
                        cell.a().max(a),
                    );
                }
            }
        }
    }

    (dirty_x0 <= dirty_x1 && dirty_y0 <= dirty_y1)
        .then_some([dirty_x0, dirty_y0, dirty_x1, dirty_y1])
}

fn paint_textured_quad(p: &egui::Painter, rect: Rect, tex_id: egui::TextureId) {
    let mut quad = Mesh::with_texture(tex_id);
    quad.add_rect_with_uv(
        rect,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    p.add(Shape::mesh(quad));
}

fn paint_isolated_mesh_via_texture(p: &egui::Painter, tex_id: egui::Id, mesh: &Mesh) {
    let Some(raw_bounds) = mesh_screen_bounds(mesh) else {
        return;
    };
    let bounds = raw_bounds.expand(1.5).intersect(p.clip_rect());
    if !bounds.is_positive() {
        return;
    }
    let (tex_size, eff_ppp) = bounded_texture_dims(bounds, p.pixels_per_point());
    let mut pixels = vec![Color32::TRANSPARENT; tex_size[0] * tex_size[1]];
    let grid = CpuRasterGrid {
        tex_size,
        origin_pts: bounds.min,
        ppp: eff_ppp,
    };
    if rasterize_mesh_max_premul(&mut pixels, grid, mesh).is_none() {
        return;
    }
    let image = egui::ColorImage::new(tex_size, pixels);
    let ctx = p.ctx();
    let existing = ctx.data_mut(|d| d.get_temp::<egui::TextureHandle>(tex_id));
    let handle = match existing {
        Some(mut tex) => {
            tex.set(image, egui::TextureOptions::LINEAR);
            tex
        }
        None => {
            let tex =
                ctx.load_texture("drawcraft_live_hl_tex", image, egui::TextureOptions::LINEAR);
            ctx.data_mut(|d| d.insert_temp(tex_id, tex.clone()));
            tex
        }
    };
    paint_textured_quad(p, bounds, handle.id());
}

pub(crate) fn paint_live_ribbon(
    p: &egui::Painter,
    pen: &PenSettings,
    samples: &[StrokeSample],
    xf: Affine2,
    feather: f32,
) {
    if let Some(mesh) = build_live_ribbon_mesh(pen, samples, xf, feather) {
        if pen.kind == PenKind::Highlighter {
            paint_isolated_mesh_via_texture(p, egui::Id::unique("drawcraft_live_hl_tex"), &mesh);
        } else {
            p.add(Shape::mesh(mesh));
        }
    }
}

fn sync_page_ribbon_cache(
    app: &mut DrawCraftApp,
    page_key: (usize, usize),
    zoom_bucket: f32,
    feather: f32,
    color_key: (UiTheme, bool, PaperColor),
) {
    let sync_key = (
        page_key,
        app.session.revision,
        app.session.doc.strokes.len(),
        app.session.doc.strokes.last().map(|s| s.id),
        zoom_bucket.to_bits(),
        color_key,
    );
    let cache_populated = !app.mesh_cache.is_empty() || app.session.doc.strokes.is_empty();
    if cache_populated && app.canvas_cache.last_ribbon_sync_key == Some(sync_key) {
        return;
    }

    let (theme, auto_contrast, paper_color) = color_key;
    for stroke in &app.session.doc.strokes {
        // Undo can restore the ID allocator, so a new branch may reuse an old stroke ID.
        let needs_rebuild = match app.mesh_cache.get(&stroke.id) {
            Some((cached_stroke, cached_bucket, _)) => {
                !Arc::ptr_eq(cached_stroke, stroke) || (*cached_bucket - zoom_bucket).abs() > 0.05
            }
            None => true,
        };
        if needs_rebuild {
            let mut render_pen = stroke.pen.clone();
            render_pen.color = paper_color.resolve_ink_color(
                theme,
                auto_contrast,
                stroke.pen.color,
                matches!(stroke.pen.kind, drawcraft_doc::PenKind::Highlighter),
            );
            let nodes = evaluate_spine(&render_pen, &stroke.smoothed_points);
            let mut ribbon = build_stroke_ribbon(&render_pen, &nodes, feather);
            if render_pen.kind == PenKind::Pencil {
                for v in &mut ribbon.vertices {
                    let grain =
                        0.58 + 0.42 * drawcraft_paint::graphite_grain_factor(v.pos[0], v.pos[1]);
                    v.rgba[3] *= grain;
                }
            }
            app.mesh_cache
                .insert(stroke.id, (Arc::clone(stroke), zoom_bucket, ribbon));
        }
    }

    if app.mesh_cache.len() > app.session.doc.strokes.len().saturating_add(32) {
        let active_ids: std::collections::HashSet<_> =
            app.session.doc.strokes.iter().map(|s| s.id).collect();
        app.mesh_cache.retain(|id, _| active_ids.contains(id));
    }
    app.canvas_cache.last_ribbon_sync_key = Some(sync_key);
}

fn valid_stroke_page_bounds(stroke: &InkStroke) -> Option<drawcraft_geom::Rect> {
    let b = stroke.bounds;
    if !b.is_empty() && b.x.is_finite() && b.y.is_finite() && b.w.is_finite() && b.h.is_finite() {
        Some(b)
    } else {
        None
    }
}

fn page_rect_for_screen_rect(rect: Rect, xf: Affine2) -> Option<drawcraft_geom::Rect> {
    if !rect.is_positive()
        || !xf.scale.is_finite()
        || xf.scale <= 1e-6
        || !rect.left().is_finite()
        || !rect.top().is_finite()
        || !rect.right().is_finite()
        || !rect.bottom().is_finite()
    {
        return None;
    }
    let tl = xf.to_page(Point::new(f64::from(rect.left()), f64::from(rect.top())));
    let br = xf.to_page(Point::new(
        f64::from(rect.right()),
        f64::from(rect.bottom()),
    ));
    Some(drawcraft_geom::Rect::new(
        tl.x.min(br.x),
        tl.y.min(br.y),
        (br.x - tl.x).abs(),
        (br.y - tl.y).abs(),
    ))
}

fn rect_contains_rect(outer: drawcraft_geom::Rect, inner: drawcraft_geom::Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

fn build_stroke_ribbon_cached(
    stroke: &InkStroke,
    feather: f32,
    color_key: (UiTheme, bool, PaperColor),
) -> RibbonMesh {
    let (theme, auto_contrast, paper_color) = color_key;
    let mut render_pen = stroke.pen.clone();
    render_pen.color = paper_color.resolve_ink_color(
        theme,
        auto_contrast,
        stroke.pen.color,
        matches!(stroke.pen.kind, drawcraft_doc::PenKind::Highlighter),
    );
    let nodes = evaluate_spine(&render_pen, &stroke.smoothed_points);
    let mut ribbon = build_stroke_ribbon(&render_pen, &nodes, feather);
    if render_pen.kind == PenKind::Pencil {
        for v in &mut ribbon.vertices {
            let grain = 0.58 + 0.42 * drawcraft_paint::graphite_grain_factor(v.pos[0], v.pos[1]);
            v.rgba[3] *= grain;
        }
    }
    ribbon
}

struct CommittedBuildOutput {
    batches: Vec<Arc<Mesh>>,
    hl_rect: Option<Rect>,
    hl_clipped: bool,
    hl_image: Option<egui::ColorImage>,
    culled_any: bool,
}

fn build_committed_batches_and_hl(
    app: &DrawCraftApp,
    xf: Affine2,
    avail_rect: Rect,
    ppp: f32,
    clip_page: Option<drawcraft_geom::Rect>,
) -> CommittedBuildOutput {
    let mut batches = Vec::new();
    let mut current = Mesh::default();
    let mut hl_meshes = Vec::new();
    let mut hl_raw_union = Rect::NOTHING;
    let mut culled_any = false;

    for stroke in &app.session.doc.strokes {
        if let Some(clip) = clip_page
            && let Some(bounds) = valid_stroke_page_bounds(stroke)
            && !bounds.intersects(clip)
        {
            culled_any = true;
            continue;
        }
        let Some((_, _, ribbon)) = app.mesh_cache.get(&stroke.id) else {
            continue;
        };
        if ribbon.indices.is_empty() {
            continue;
        }
        if stroke.pen.kind == PenKind::Highlighter {
            let m = ribbon_to_egui_mesh(ribbon, xf, false);
            if let Some(b) = mesh_screen_bounds(&m) {
                hl_raw_union = hl_raw_union.union(b);
                hl_meshes.push(m);
            }
            continue;
        }
        if !current.vertices.is_empty()
            && current.vertices.len().saturating_add(ribbon.vertices.len()) > MAX_BATCH_VERTICES
        {
            batches.push(Arc::new(std::mem::take(&mut current)));
        }
        append_ribbon_transformed(&mut current, ribbon, xf, false);
    }
    if !current.indices.is_empty() {
        batches.push(Arc::new(current));
    }

    let (hl_rect, hl_clipped, hl_image) =
        build_committed_highlighter_image(&hl_meshes, hl_raw_union, avail_rect, ppp);
    CommittedBuildOutput {
        batches,
        hl_rect,
        hl_clipped,
        hl_image,
        culled_any,
    }
}

fn build_committed_highlighter_image(
    hl_meshes: &[Mesh],
    raw_union: Rect,
    avail_rect: Rect,
    ppp: f32,
) -> (Option<Rect>, bool, Option<egui::ColorImage>) {
    if hl_meshes.is_empty() || !raw_union.is_positive() {
        return (None, false, None);
    }
    let expanded_raw = raw_union.expand(1.5);
    let target_rect = expanded_raw.intersect(avail_rect.expand(64.0));
    if !target_rect.is_positive() {
        return (None, false, None);
    }
    let hl_clipped = expanded_raw.min.x < target_rect.min.x
        || expanded_raw.min.y < target_rect.min.y
        || expanded_raw.max.x > target_rect.max.x
        || expanded_raw.max.y > target_rect.max.y;

    let (tex_size, eff_ppp) = bounded_texture_dims(target_rect, ppp);
    let len = tex_size[0] * tex_size[1];
    let mut dest_pixels = vec![Color32::TRANSPARENT; len];
    let mut scratch = vec![Color32::TRANSPARENT; len];
    let grid = CpuRasterGrid {
        tex_size,
        origin_pts: target_rect.min,
        ppp: eff_ppp,
    };

    for mesh in hl_meshes {
        let Some([dx0, dy0, dx1, dy1]) = rasterize_mesh_max_premul(&mut scratch, grid, mesh) else {
            continue;
        };
        for iy in dy0..=dy1 {
            let row = iy * tex_size[0];
            for ix in dx0..=dx1 {
                let idx = row + ix;
                let src = scratch[idx];
                if src.a() > 0 {
                    dest_pixels[idx] = blend_premul_over_color32(dest_pixels[idx], src);
                    scratch[idx] = Color32::TRANSPARENT;
                }
            }
        }
    }

    (
        Some(target_rect),
        hl_clipped,
        Some(egui::ColorImage::new(tex_size, dest_pixels)),
    )
}

#[allow(clippy::too_many_arguments)]
fn try_append_last_committed_stroke(
    app: &mut DrawCraftApp,
    xf: Affine2,
    page_key: (usize, usize),
    color_key: (UiTheme, bool, PaperColor),
    zoom_bucket: f32,
    feather: f32,
    batch_key: &CommittedBatchKey,
    visible_page: Option<drawcraft_geom::Rect>,
) -> bool {
    let strokes = &app.session.doc.strokes;
    let n = strokes.len();
    if n == 0 {
        return false;
    }
    let Some(last_stroke) = strokes.last() else {
        return false;
    };
    if last_stroke.pen.kind == PenKind::Highlighter {
        return false;
    }
    let cache = &app.canvas_cache.committed;
    let viewport_ok = match (cache.built_clip_page, visible_page) {
        (Some(built), Some(vis)) => {
            !cache.built_has_culled_strokes || rect_contains_rect(built, vis)
        }
        _ => true,
    };
    let can_append = !cache.batches.is_empty()
        && cache.key.is_some()
        && cache.built_page_key == Some(page_key)
        && cache.built_color_key == Some(color_key)
        && cache.built_zoom_bucket == Some(zoom_bucket)
        && cache.rendered_pan == app.view.pan
        && viewport_ok
        && n == cache.built_stroke_count.saturating_add(1)
        && cache.built_stroke_refs.len() == cache.built_stroke_count
        && strokes[..cache.built_stroke_count]
            .iter()
            .zip(&cache.built_stroke_refs)
            .all(|(s, &(id, ptr))| s.id == id && (Arc::as_ptr(s) as usize) == ptr);
    if !can_append {
        return false;
    }

    let last_id = last_stroke.id;
    let last_ptr = Arc::as_ptr(last_stroke) as usize;
    let ribbon_empty_or_culled = if let Some(clip) = app.canvas_cache.committed.built_clip_page
        && let Some(bounds) = valid_stroke_page_bounds(last_stroke)
        && !bounds.intersects(clip)
    {
        app.canvas_cache.committed.built_has_culled_strokes = true;
        true
    } else {
        false
    };
    if !ribbon_empty_or_culled {
        let needs_build = match app.mesh_cache.get(&last_id) {
            Some((cached, bucket, _)) => {
                !Arc::ptr_eq(cached, last_stroke) || (*bucket - zoom_bucket).abs() > 0.05
            }
            None => true,
        };
        if needs_build {
            let ribbon = build_stroke_ribbon_cached(last_stroke, feather, color_key);
            app.mesh_cache
                .insert(last_id, (Arc::clone(last_stroke), zoom_bucket, ribbon));
        }
        if let Some((_, _, ribbon)) = app.mesh_cache.get(&last_id)
            && !ribbon.indices.is_empty()
        {
            let batches = &mut app.canvas_cache.committed.batches;
            let append_existing = batches.last().is_some_and(|b| {
                b.vertices.len().saturating_add(ribbon.vertices.len()) <= MAX_BATCH_VERTICES
            });
            if append_existing && let Some(last_batch) = batches.last_mut() {
                append_ribbon_transformed(Arc::make_mut(last_batch), ribbon, xf, false);
            } else {
                let mut next = Mesh::default();
                append_ribbon_transformed(&mut next, ribbon, xf, false);
                if !next.indices.is_empty() {
                    batches.push(Arc::new(next));
                }
            }
        }
    }
    let cache = &mut app.canvas_cache.committed;
    cache.key = Some(batch_key.clone());
    cache.built_stroke_count = n;
    cache.built_stroke_refs.push((last_id, last_ptr));
    true
}

fn emit_committed_batches(p: &egui::Painter, cache: &CommittedMeshCache) {
    if let (Some(hl_rect), Some(tex)) = (cache.hl_rect, cache.hl_texture.as_ref()) {
        paint_textured_quad(p, hl_rect, tex.id());
    }
    for batch in &cache.batches {
        p.add(Shape::mesh(Arc::clone(batch)));
    }
}

fn paint_committed_strokes(
    app: &mut DrawCraftApp,
    p: &egui::Painter,
    avail_rect: Rect,
    xf: Affine2,
) {
    let page_key = (
        app.session.active_notebook,
        app.session.doc.active_page_index(),
    );
    let (auto_contrast, paper_color) = crate::effective_doc_ink_context(app);
    let color_key = (app.session.prefs.theme, auto_contrast, paper_color);
    if app.view.cached_page_key != page_key || app.view.cached_color_key != Some(color_key) {
        if app.view.cached_page_key != page_key {
            app.view.clear_selection();
            app.view.text_style_open = false;
        }
        app.mesh_cache.clear();
        app.canvas_cache.committed.key = None;
        app.canvas_cache.last_ribbon_sync_key = None;
        app.canvas_cache.gpu_scene.clear();
        app.canvas_cache.selection_bounds = None;
        app.view.cached_page_key = page_key;
        app.view.cached_color_key = Some(color_key);
    }
    if app.wgpu_enabled {
        crate::gpu_ink::paint_committed_strokes_gpu(app, p, avail_rect, xf, page_key, color_key);
        return;
    }

    let visible_page = page_rect_for_screen_rect(avail_rect, xf);
    let batch_key = CommittedBatchKey {
        page_key,
        color_key,
        revision: app.session.revision,
        stroke_count: app.session.doc.strokes.len(),
        last_stroke_id: app.session.doc.strokes.last().map(|s| s.id),
        zoom_bits: app.view.zoom.to_bits(),
    };
    let page_cache_ready = !app.mesh_cache.is_empty() || app.session.doc.strokes.is_empty();
    let cache_matches =
        page_cache_ready && app.canvas_cache.committed.key.as_ref() == Some(&batch_key);

    if cache_matches {
        let cache = &mut app.canvas_cache.committed;
        let viewport_ok = match (cache.built_clip_page, visible_page) {
            (Some(built), Some(vis)) => {
                !cache.built_has_culled_strokes || rect_contains_rect(built, vis)
            }
            _ => true,
        };
        if viewport_ok && cache.rendered_pan == app.view.pan {
            emit_committed_batches(p, cache);
            return;
        }
        let hl_pan_ok = !cache.hl_clipped || cache.rendered_pan == app.view.pan;
        if viewport_ok && hl_pan_ok && cache.translate_steps < MAX_CACHE_TRANSLATE_STEPS {
            let dx = (app.view.pan[0] - cache.rendered_pan[0]) as f32;
            let dy = (app.view.pan[1] - cache.rendered_pan[1]) as f32;
            let delta = egui::vec2(dx, dy);
            for batch in &mut cache.batches {
                Arc::make_mut(batch).translate(delta);
            }
            if let Some(hl_rect) = &mut cache.hl_rect {
                *hl_rect = hl_rect.translate(delta);
            }
            cache.rendered_pan = app.view.pan;
            cache.translate_steps = cache.translate_steps.saturating_add(1);
            emit_committed_batches(p, cache);
            return;
        }
    }

    let feather = (0.9 / app.view.zoom.max(0.25)).clamp(0.3, 2.5);
    let zoom_bucket = (feather * 4.0).round() / 4.0;
    if page_cache_ready
        && try_append_last_committed_stroke(
            app,
            xf,
            page_key,
            color_key,
            zoom_bucket,
            feather,
            &batch_key,
            visible_page,
        )
    {
        emit_committed_batches(p, &app.canvas_cache.committed);
        return;
    }

    let clip_page = visible_page.map(|v| {
        let margin = v.w.max(v.h).max(1.0) * 0.5;
        v.inflated(margin)
    });
    sync_page_ribbon_cache(app, page_key, zoom_bucket, feather, color_key);
    let built =
        build_committed_batches_and_hl(app, xf, avail_rect, p.pixels_per_point(), clip_page);
    let stroke_refs: Vec<(StrokeId, usize)> = app
        .session
        .doc
        .strokes
        .iter()
        .map(|s| (s.id, Arc::as_ptr(s) as usize))
        .collect();
    let cache = &mut app.canvas_cache.committed;
    cache.key = Some(batch_key);
    cache.rendered_pan = app.view.pan;
    cache.translate_steps = 0;
    cache.batches = built.batches;
    cache.hl_rect = built.hl_rect;
    cache.hl_clipped = built.hl_clipped;
    match built.hl_image {
        Some(image) => match &mut cache.hl_texture {
            Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
            None => {
                cache.hl_texture = Some(p.ctx().load_texture(
                    "drawcraft_cpu_hl_layer",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        },
        None => {
            cache.hl_texture = None;
        }
    }
    cache.built_page_key = Some(page_key);
    cache.built_clip_page = clip_page;
    cache.built_has_culled_strokes = built.culled_any;
    cache.built_zoom_bucket = Some(zoom_bucket);
    cache.built_color_key = Some(color_key);
    cache.built_stroke_count = stroke_refs.len();
    cache.built_stroke_refs = stroke_refs;
    emit_committed_batches(p, cache);
}

fn apply_zoom_around_anchor(
    app: &mut DrawCraftApp,
    xf: &mut Affine2,
    anchor_pt: Point,
    factor: f64,
) {
    let page_pt = xf.to_page(anchor_pt);
    xf.zoom_around(anchor_pt, factor, 0.15, 6.0);
    let zoom = xf.scale as f32;
    let snapped_scale = f64::from(zoom);
    let offset_x = anchor_pt.x - page_pt.x * snapped_scale;
    let offset_y = anchor_pt.y - page_pt.y * snapped_scale;
    *xf = Affine2::new(snapped_scale, offset_x, offset_y);
    app.view.zoom = zoom;
    app.view.pan = [offset_x, offset_y];
    app.session.zoom = zoom;
}

fn handle_pan_and_zoom(app: &mut DrawCraftApp, ui: &mut egui::Ui, resp: &egui::Response) {
    let mut xf = page_affine(app);
    let space_held = !app.view.text_editing && ui.input(|i| i.key_down(egui::Key::Space));
    let stylus_pan_held = !app.view.text_editing && app.stylus.pan_held();
    let mid_drag =
        !app.stylus.suppress_middle_pan() && resp.dragged_by(egui::PointerButton::Middle);
    let space_drag = space_held && resp.dragged_by(egui::PointerButton::Primary);
    let stylus_pan_drag = stylus_pan_held
        && (resp.dragged_by(egui::PointerButton::Primary)
            || resp.dragged_by(egui::PointerButton::Secondary)
            || resp.dragged_by(egui::PointerButton::Middle));

    if mid_drag || space_drag || stylus_pan_drag {
        cancel_camera_glide(app);
        let delta = ui.input(|i| i.pointer.delta());
        app.view.pan[0] += f64::from(delta.x);
        app.view.pan[1] += f64::from(delta.y);
        app.view.pan_dragging = true;
    } else if !ui.input(|i| i.pointer.any_down()) {
        app.view.pan_dragging = false;
    }

    let (scroll, zoom_delta, modifiers, hover_pos, multi_touch) = ui.input(|i| {
        (
            i.smooth_scroll_delta,
            i.zoom_delta(),
            i.modifiers,
            i.pointer.hover_pos(),
            i.multi_touch(),
        )
    });

    if let Some(mt) = multi_touch {
        let anchor_pt = Point::new(f64::from(mt.center_pos.x), f64::from(mt.center_pos.y));
        let moved = mt.translation_delta.length_sq() > 1e-4;
        let zoomed = (mt.zoom_delta - 1.0).abs() > 1e-4;
        if (moved || zoomed) && (resp.hovered() || resp.rect.contains(mt.center_pos)) {
            cancel_camera_glide(app);
            if moved {
                app.view.pan[0] += f64::from(mt.translation_delta.x);
                app.view.pan[1] += f64::from(mt.translation_delta.y);
                xf = page_affine(app);
            }
            if zoomed {
                apply_zoom_around_anchor(app, &mut xf, anchor_pt, f64::from(mt.zoom_delta));
            }
        }
        return;
    }

    if resp.hovered() {
        let anchor = hover_pos.unwrap_or_else(|| resp.rect.center());
        let anchor_pt = Point::new(f64::from(anchor.x), f64::from(anchor.y));

        if (zoom_delta - 1.0).abs() > 1e-4 {
            cancel_camera_glide(app);
            apply_zoom_around_anchor(app, &mut xf, anchor_pt, f64::from(zoom_delta));
        } else if modifiers.ctrl && scroll.y.abs() > 0.1 {
            cancel_camera_glide(app);
            let factor = (1.0 + f64::from(scroll.y) * 0.0025).clamp(0.7, 1.4);
            apply_zoom_around_anchor(app, &mut xf, anchor_pt, factor);
        } else if scroll.length_sq() > 0.01 && !modifiers.ctrl {
            cancel_camera_glide(app);
            app.view.pan[0] += f64::from(scroll.x);
            app.view.pan[1] += f64::from(scroll.y);
        }
    }
}

const MIN_TOUCH_CONTACT_PRESSURE: f32 = 0.02;
pub(crate) const RULER_DEPTH: f64 = 64.0;
const RULER_EDGE_SNAP_SCREEN: f64 = 16.0;

struct SampleCursor {
    idx: usize,
    total: usize,
}

fn has_active_live(app: &DrawCraftApp) -> bool {
    app.session.live_stroke.is_some()
        || app.session.live_eraser.is_some()
        || (app.session.active_tool == ToolKind::LaserPointer
            && (app.view.active_pointer_stroke || app.view.active_touch_id.is_some()))
}

fn finish_active_stroke(app: &mut DrawCraftApp) {
    if app.session.live_stroke.is_some() {
        let _ = app.session.finish_live_stroke_with_shape();
    } else if app.session.live_eraser.is_some() {
        let _ = app.session.finish_live_eraser();
    } else if app.session.active_tool == ToolKind::LaserPointer && !app.view.laser_trail_mode {
        app.view.laser_points.clear();
    }
    app.view.active_touch_id = None;
    app.view.active_pointer_stroke = false;
    app.view.straighten_reference_point = None;
    app.view.straighten_candidate_since_s = None;
    app.view.ruler_stroke_edge = None;
}

fn cancel_active_stroke(app: &mut DrawCraftApp) {
    app.session.cancel_live_stroke();
    app.session.cancel_live_eraser();
    if app.session.active_tool == ToolKind::LaserPointer
        && (!app.view.laser_trail_mode || app.view.laser_points.len() == 1)
    {
        app.view.laser_points.clear();
    }
    app.view.active_touch_id = None;
    app.view.active_pointer_stroke = false;
    app.view.straighten_reference_point = None;
    app.view.straighten_candidate_since_s = None;
    app.view.ruler_stroke_edge = None;
}

fn normalized_touch_fallback(force: Option<f32>) -> f32 {
    match force {
        Some(f) if f.is_finite() => f.clamp(0.0, 1.0).max(MIN_TOUCH_CONTACT_PRESSURE),
        _ => 1.0,
    }
}

fn can_start_on_canvas(ui: &egui::Ui, resp: &egui::Response, pos: Pos2) -> bool {
    resp.rect.contains(pos)
        && ui
            .ctx()
            .layer_id_at(pos)
            .is_none_or(|layer| layer == resp.layer_id)
}

fn ruler_geometry(app: &DrawCraftApp) -> (Point, Point, Point, f64, f64) {
    let center = Point::new(app.view.ruler_center[0], app.view.ruler_center[1]);
    let angle = if app.view.ruler_angle.is_finite() {
        app.view.ruler_angle
    } else {
        0.0
    };
    let direction = Point::new(angle.cos(), angle.sin());
    let normal = Point::new(-direction.y, direction.x);
    let length = if app.view.ruler_length.is_finite() && app.view.ruler_length > 0.0 {
        app.view.ruler_length
    } else {
        f64::from(app.session.doc.width) * 0.72
    };
    let depth = if app.view.ruler_depth.is_finite() && app.view.ruler_depth > 0.0 {
        app.view.ruler_depth
    } else {
        RULER_DEPTH
    };
    (center, direction, normal, length * 0.5, depth * 0.5)
}

fn ruler_local_position(app: &DrawCraftApp, point: Point) -> (f64, f64) {
    let (center, direction, normal, _, _) = ruler_geometry(app);
    let delta = Point::new(point.x - center.x, point.y - center.y);
    (
        delta.x * direction.x + delta.y * direction.y,
        delta.x * normal.x + delta.y * normal.y,
    )
}

fn ruler_edge_for_point(app: &DrawCraftApp, point: Point) -> Option<f64> {
    if !app.view.ruler_active || !point.x.is_finite() || !point.y.is_finite() {
        return None;
    }
    let (_, _, _, half_length, half_width) = ruler_geometry(app);
    let (along, across) = ruler_local_position(app, point);
    let tolerance =
        (RULER_EDGE_SNAP_SCREEN / f64::from(app.view.zoom.max(0.05))).min(half_width * 0.6);
    if along.abs() > half_length + tolerance {
        return None;
    }
    let upper_distance = (across + half_width).abs();
    let lower_distance = (across - half_width).abs();
    if upper_distance <= lower_distance && upper_distance <= tolerance {
        Some(-1.0)
    } else if lower_distance <= tolerance {
        Some(1.0)
    } else {
        None
    }
}

fn ruler_project_point(app: &DrawCraftApp, point: Point, edge: f64) -> Point {
    let (center, direction, normal, half_length, half_width) = ruler_geometry(app);
    let (along, _) = ruler_local_position(app, point);
    let distance = along.clamp(-half_length, half_length);
    Point::new(
        center.x + direction.x * distance + normal.x * half_width * edge,
        center.y + direction.y * distance + normal.y * half_width * edge,
    )
}

fn ruler_rotation_handle(app: &DrawCraftApp) -> Point {
    let (center, _, normal, _, half_width) = ruler_geometry(app);
    let offset = half_width + 24.0 / f64::from(app.view.zoom.max(0.05));
    Point::new(center.x - normal.x * offset, center.y - normal.y * offset)
}

/// Keeps the ruler centre on the page. Only the centre is bounded, so a long ruler can still be
/// parked against a page corner at any zoom.
pub(crate) fn clamp_ruler_center(app: &DrawCraftApp, point: Point) -> Point {
    let width = f64::from(app.session.doc.width);
    let height = f64::from(app.session.doc.height);
    let x = if point.x.is_finite() {
        point.x.clamp(0.0, width)
    } else {
        width * 0.5
    };
    let y = if point.y.is_finite() {
        point.y.clamp(0.0, height)
    } else {
        height * 0.5
    };
    Point::new(x, y)
}

fn to_stroke_sample(
    app: &DrawCraftApp,
    screen_pos: Pos2,
    xf: Affine2,
    force: Option<f32>,
    sub: (usize, usize),
) -> StrokeSample {
    let mut page_pt = xf.to_page(Point::new(f64::from(screen_pos.x), f64::from(screen_pos.y)));
    if let Some(edge) = app.view.ruler_stroke_edge {
        page_pt = ruler_project_point(app, page_pt, edge);
    }
    let fallback_pressure = normalized_touch_fallback(force);
    let s = app.stylus.sample();
    // Use per-event Touch force verbatim when no separate OS tablet packet stream is active so
    // ignored palm/pointer events never skew Touch pressure interpolation.
    let pressure = if force.is_some() && !app.stylus.has_hardware_samples() {
        fallback_pressure
    } else {
        s.map_or(fallback_pressure, |v| v.pressure)
    };
    let tilt_x = s.map_or(0.0, |v| v.tilt_x);
    let tilt_y = s.map_or(0.0, |v| v.tilt_y);
    let rotation = s.map_or(0.0, |v| v.rotation);
    let sub_offset_ms = (sub.0 as f64) * (4.0 / sub.1.max(1) as f64);
    let time_ms = s
        .and_then(|v| v.time_ms)
        .unwrap_or(app.stylus.clock_ms + sub_offset_ms);

    StrokeSample {
        x: page_pt.x,
        y: page_pt.y,
        pressure,
        tilt_x,
        tilt_y,
        rotation,
        time_ms,
    }
    .sanitized()
}

fn push_laser_sample(app: &mut DrawCraftApp, point: Point, now_s: f64, stroke_break: bool) {
    if !point.x.is_finite() || !point.y.is_finite() || !now_s.is_finite() {
        return;
    }
    if !app.view.laser_trail_mode {
        app.view.laser_points.clear();
        app.view.laser_points.push(crate::LaserPoint {
            x: point.x,
            y: point.y,
            time_s: now_s,
            stroke_break: true,
        });
        return;
    }
    if !stroke_break
        && let Some(prev) = app.view.laser_points.last()
        && !prev.stroke_break
        && (point.x - prev.x).hypot(point.y - prev.y) < 1.25
    {
        return;
    }
    if app.view.laser_points.len() >= 240 {
        app.view.laser_points.remove(0);
    }
    app.view.laser_points.push(crate::LaserPoint {
        x: point.x,
        y: point.y,
        time_s: now_s,
        stroke_break,
    });
}

fn begin_stroke_sample(
    app: &mut DrawCraftApp,
    pos: Pos2,
    xf: Affine2,
    force: Option<f32>,
    cursor: &mut SampleCursor,
    now_s: f64,
) {
    cancel_camera_glide(app);
    app.view.ruler_stroke_edge = if matches!(app.session.active_tool, ToolKind::Pen(_)) {
        let point = xf.to_page(Point::new(f64::from(pos.x), f64::from(pos.y)));
        ruler_edge_for_point(app, point)
    } else {
        None
    };
    if app.view.ruler_stroke_edge.is_some() {
        app.session.straight_line_hold = true;
    }
    app.stylus.select(cursor.idx, cursor.total);
    let sample = to_stroke_sample(app, pos, xf, force, (cursor.idx, cursor.total));
    app.stylus.clear_selection();
    cursor.idx += 1;
    app.view.straighten_reference_point = Some(sample.point());
    app.view.straighten_candidate_since_s = None;
    match app.session.active_tool {
        ToolKind::Pen(_) => app.session.begin_live_stroke(sample),
        ToolKind::Eraser => app.session.begin_live_eraser(sample.point()),
        ToolKind::LaserPointer => push_laser_sample(app, sample.point(), now_s, true),
        ToolKind::Select => {}
    }
}

fn update_stroke_sample(
    app: &mut DrawCraftApp,
    pos: Pos2,
    xf: Affine2,
    force: Option<f32>,
    cursor: &mut SampleCursor,
    now_s: f64,
) {
    app.stylus.select(cursor.idx, cursor.total);
    let sample = to_stroke_sample(app, pos, xf, force, (cursor.idx, cursor.total));
    app.stylus.clear_selection();
    cursor.idx += 1;
    let movement_threshold = 2.0 / xf.scale.abs().max(0.01);
    if let Some(reference) = app.view.straighten_reference_point
        && reference.distance(sample.point()) >= movement_threshold
    {
        app.view.straighten_reference_point = Some(sample.point());
        app.view.straighten_candidate_since_s = Some(now_s);
    }
    match app.session.active_tool {
        ToolKind::Pen(_) => app.session.update_live_stroke_deferred(sample),
        ToolKind::Eraser => app.session.update_live_eraser(sample.point()),
        ToolKind::LaserPointer => push_laser_sample(app, sample.point(), now_s, false),
        ToolKind::Select => {}
    }
}

fn active_last_page_pos(app: &DrawCraftApp) -> Option<Point> {
    match app.session.active_tool {
        ToolKind::Pen(_) => app
            .session
            .live_stroke
            .as_ref()
            .and_then(|s| s.raw_points.last().copied())
            .map(StrokeSample::point),
        ToolKind::Eraser => app
            .session
            .live_eraser
            .as_ref()
            .and_then(|e| e.path.last().map(|(pt, _)| *pt)),
        ToolKind::LaserPointer => app.view.laser_points.last().map(|p| Point::new(p.x, p.y)),
        ToolKind::Select => None,
    }
}

fn last_live_distance_sq(app: &DrawCraftApp, screen_pos: Pos2, xf: Affine2) -> f64 {
    let page_pt = xf.to_page(Point::new(f64::from(screen_pos.x), f64::from(screen_pos.y)));
    let prev = active_last_page_pos(app);
    prev.map_or(f64::MAX, |p| {
        let dx = page_pt.x - p.x;
        let dy = page_pt.y - p.y;
        dx * dx + dy * dy
    })
}

fn pointer_pos_matches_touch(pointer_pos: Pos2, touch_pos: Pos2) -> bool {
    pointer_pos.x.is_finite()
        && pointer_pos.y.is_finite()
        && touch_pos.x.is_finite()
        && touch_pos.y.is_finite()
        && (pointer_pos - touch_pos).length_sq() <= 4.0
}

fn scan_paired_pointer_press<'a>(
    iter: impl Iterator<Item = &'a egui::Event>,
    touch_pos: Pos2,
) -> bool {
    for ev in iter {
        match ev {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                ..
            } if pointer_pos_matches_touch(*pos, touch_pos) => return true,
            egui::Event::Touch {
                phase: egui::TouchPhase::Start | egui::TouchPhase::End | egui::TouchPhase::Cancel,
                ..
            }
            | egui::Event::PointerButton {
                button: egui::PointerButton::Primary,
                pressed: false,
                ..
            } => return false,
            _ => {}
        }
    }
    false
}

fn touch_start_has_paired_pointer(events: &[egui::Event], start_idx: usize) -> bool {
    let Some(&egui::Event::Touch { pos: touch_pos, .. }) = events.get(start_idx) else {
        return false;
    };
    let after = events
        .get(start_idx + 1..)
        .is_some_and(|tail| scan_paired_pointer_press(tail.iter(), touch_pos));
    let before = events
        .get(..start_idx)
        .is_some_and(|head| scan_paired_pointer_press(head.iter().rev(), touch_pos));
    after || before
}

/// Pencil-only mode: when `finger_drawing` is off (default), a finger must never draw.
///
/// The platform layer reports the physical source of every touch (Safari `touchType`), because on
/// iPadOS a finger produces `PointerButton` *and* `Touch` events just like the Pencil does, so
/// "has a paired pointer press" says nothing about who is touching. A reported `Direct` finger is
/// blocked immediately — even before the first Apple Pencil stroke in the session. Without a
/// report (desktop touch screens) the heuristic applies: no pressure and no paired pointer press.
fn finger_is_blocked(
    app: &DrawCraftApp,
    events: &[egui::Event],
    start_idx: usize,
    id: egui::TouchId,
    force: Option<f32>,
) -> bool {
    use crate::TouchSource;
    if app.session.prefs.finger_drawing {
        return false;
    }
    match app.view.study.touch_source(id) {
        Some(TouchSource::Stylus) => false,
        Some(TouchSource::Direct) => true,
        None => {
            app.stylus.is_connected()
                && force.is_none_or(|f| !f.is_finite() || f <= 0.0)
                && !touch_start_has_paired_pointer(events, start_idx)
        }
    }
}

/// Finger navigation in Pencil-only mode: one finger on the paper pans the page. Pinch-zoom with
/// two fingers is handled by [`handle_pan_and_zoom`]. Returns `true` while a finger is
/// navigating (including `eframe`'s synchronous `pointerdown("touch")` frame that arrives before
/// `touchstart` on iPadOS Safari), so no drawing, selection, shape or ruler tool sees that finger.
fn handle_finger_navigation(app: &mut DrawCraftApp, ctx: &DrawingEventCtx<'_>) -> bool {
    let mut engaged = app.view.study.finger_pan.is_some();
    let mut saw_touch_event = false;
    for (idx, ev) in ctx.events.iter().enumerate() {
        let egui::Event::Touch {
            id,
            phase,
            pos,
            force,
            ..
        } = *ev
        else {
            continue;
        };
        saw_touch_event = true;
        match phase {
            egui::TouchPhase::Start => {
                // Palm rejection: while the Pencil is writing, a resting palm or finger is ignored
                // outright (no pan, no draw) instead of dragging the page around.
                if !app.session.prefs.finger_drawing && has_active_live(app) {
                    continue;
                }
                if finger_is_blocked(app, ctx.events, idx, id, force)
                    && can_start_on_canvas(ctx.ui, ctx.resp, pos)
                {
                    engaged = true;
                    if app.view.study.finger_pan.is_none() {
                        app.view.study.finger_pan = Some((id, pos));
                    }
                }
            }
            egui::TouchPhase::Move => {
                if let Some((pan_id, last)) = app.view.study.finger_pan
                    && pan_id == id
                {
                    cancel_camera_glide(app);
                    app.view.pan[0] += f64::from(pos.x - last.x);
                    app.view.pan[1] += f64::from(pos.y - last.y);
                    app.view.study.finger_pan = Some((id, pos));
                    ctx.ui.ctx().request_repaint();
                } else if app.view.study.finger_pan.is_none()
                    && !app.session.prefs.finger_drawing
                    && app.view.study.touch_source(id) == Some(crate::TouchSource::Direct)
                    && can_start_on_canvas(ctx.ui, ctx.resp, pos)
                {
                    // Lifting one finger from a two-finger pinch leaves a single direct finger
                    // moving with no fresh `Start`; resume panning instead of falling through.
                    engaged = true;
                    app.view.study.finger_pan = Some((id, pos));
                }
            }
            egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                if app
                    .view
                    .study
                    .finger_pan
                    .is_some_and(|(pan_id, _)| pan_id == id)
                {
                    app.view.study.finger_pan = None;
                }
            }
        }
    }
    if !engaged
        && !saw_touch_event
        && !app.session.prefs.finger_drawing
        && app.view.study.pointer_is_direct_touch
        && ctx.events.iter().any(|ev| {
            matches!(
                ev,
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    ..
                } | egui::Event::PointerMoved(_)
            )
        })
    {
        // On iPadOS Safari, `eframe` runs `runner.logic()` synchronously inside `pointerdown`
        // (emitting only `PointerButton`) before `touchstart` fires. Block finger drawing here.
        engaged = true;
    }
    engaged
}
fn touch_start_claims_stroke(
    app: &DrawCraftApp,
    events: &[egui::Event],
    start_idx: usize,
    id: egui::TouchId,
    force: Option<f32>,
) -> bool {
    !app.stylus.is_ignored_touch(id)
        && (force.is_some() || !touch_start_has_paired_pointer(events, start_idx))
}

fn pointer_press_superseded_by_following_touch(
    app: &DrawCraftApp,
    events: &[egui::Event],
    press_idx: usize,
    press_pos: Pos2,
) -> bool {
    for (offset, ev) in events.iter().skip(press_idx + 1).enumerate() {
        let touch_idx = press_idx + 1 + offset;
        match *ev {
            egui::Event::Touch {
                id,
                phase: egui::TouchPhase::Start,
                pos,
                force,
                ..
            } => {
                if pointer_pos_matches_touch(press_pos, pos) {
                    return app.stylus.is_ignored_touch(id)
                        || touch_start_claims_stroke(app, events, touch_idx, id, force);
                }
                return false;
            }
            egui::Event::Touch {
                phase: egui::TouchPhase::End | egui::TouchPhase::Cancel,
                ..
            }
            | egui::Event::PointerButton {
                button: egui::PointerButton::Primary,
                ..
            } => return false,
            _ => {}
        }
    }
    false
}

fn batch_has_touch_activity(events: &[egui::Event], target: egui::TouchId) -> bool {
    events.iter().any(|ev| {
        matches!(
            ev,
            egui::Event::Touch {
                id,
                phase: egui::TouchPhase::Start | egui::TouchPhase::Move,
                ..
            } if *id == target
        )
    })
}

fn single_live_start_page_pos(app: &DrawCraftApp) -> Option<Point> {
    match app.session.active_tool {
        ToolKind::Pen(_) => app.session.live_stroke.as_ref().and_then(|s| {
            (s.raw_points.len() == 1)
                .then(|| s.raw_points.first().copied().map(StrokeSample::point))
                .flatten()
        }),
        ToolKind::Eraser => app.session.live_eraser.as_ref().and_then(|e| {
            (e.path.len() == 1)
                .then(|| e.path.first().map(|(pt, _)| *pt))
                .flatten()
        }),
        ToolKind::LaserPointer => (app.view.laser_points.len() == 1)
            .then(|| app.view.laser_points.first().map(|p| Point::new(p.x, p.y)))
            .flatten(),
        ToolKind::Select => None,
    }
}

/// True when an unmoved 1-sample pointer stroke is immediately followed by a touch contact at
/// the same screen position (for example, when a browser splits emulated pointer press and
/// touch start across consecutive frames).
fn should_replace_unmoved_pointer_stroke(app: &DrawCraftApp, touch_pos: Pos2, xf: Affine2) -> bool {
    if !app.view.active_pointer_stroke
        || app.view.active_touch_id.is_some()
        || !touch_pos.x.is_finite()
        || !touch_pos.y.is_finite()
    {
        return false;
    }
    let Some(start_page) = single_live_start_page_pos(app) else {
        return false;
    };
    let start_screen = xf.to_screen(start_page);
    if !start_screen.x.is_finite() || !start_screen.y.is_finite() {
        return false;
    }
    let dx = f64::from(touch_pos.x) - start_screen.x;
    let dy = f64::from(touch_pos.y) - start_screen.y;
    dx * dx + dy * dy <= 16.0
}

struct DrawingEventCounter {
    touch_id: Option<egui::TouchId>,
    pointer_active: bool,
    has_live: bool,
    saw_touch_start: bool,
    last_page_pos: Option<Point>,
    count: usize,
}

impl DrawingEventCounter {
    fn step_touch(
        &mut self,
        app: &DrawCraftApp,
        ctx: &DrawingEventCtx<'_>,
        idx: usize,
        touch: (egui::TouchId, egui::TouchPhase, Pos2, Option<f32>),
    ) {
        let (id, phase, pos, force) = touch;
        let page_pt = ctx
            .xf
            .to_page(Point::new(f64::from(pos.x), f64::from(pos.y)));
        match phase {
            egui::TouchPhase::Start => {
                self.saw_touch_start = true;
                if touch_start_claims_stroke(app, ctx.events, idx, id, force)
                    && can_start_on_canvas(ctx.ui, ctx.resp, pos)
                {
                    self.touch_id = Some(id);
                    self.pointer_active = false;
                    self.has_live = true;
                    self.last_page_pos = Some(page_pt);
                    self.count += 1;
                }
            }
            egui::TouchPhase::Move => {
                if self.touch_id == Some(id) && self.has_live {
                    self.last_page_pos = Some(page_pt);
                    self.count += 1;
                }
            }
            egui::TouchPhase::End => {
                if self.touch_id == Some(id) {
                    let moved_enough = self.last_page_pos.is_none_or(|p| {
                        let dx = page_pt.x - p.x;
                        let dy = page_pt.y - p.y;
                        dx * dx + dy * dy >= 0.0625
                    });
                    if self.has_live && moved_enough {
                        self.count += 1;
                    }
                    self.clear();
                }
            }
            egui::TouchPhase::Cancel => {
                if self.touch_id == Some(id)
                    || (self.touch_id.is_none() && self.pointer_active && ctx.has_pointer_gone)
                {
                    self.clear();
                }
            }
        }
    }

    fn step_pointer_press(
        &mut self,
        app: &DrawCraftApp,
        ctx: &DrawingEventCtx<'_>,
        idx: usize,
        pos: Pos2,
    ) {
        if pointer_press_superseded_by_following_touch(app, ctx.events, idx, pos) {
            return;
        }
        if let Some(active_id) = self.touch_id {
            if self.saw_touch_start || batch_has_touch_activity(ctx.events, active_id) {
                return;
            }
            self.touch_id = None;
        }
        if can_start_on_canvas(ctx.ui, ctx.resp, pos) {
            self.pointer_active = true;
            self.touch_id = None;
            self.has_live = true;
            self.last_page_pos = Some(
                ctx.xf
                    .to_page(Point::new(f64::from(pos.x), f64::from(pos.y))),
            );
            self.count += 1;
        }
    }

    fn clear(&mut self) {
        self.touch_id = None;
        self.pointer_active = false;
        self.has_live = false;
        self.last_page_pos = None;
    }
}

/// Counts only the input events that will actually emit a sample into `cursor`,
/// preventing paired mouse-emulation or hover moves from skewing `Stylus::select`.
fn count_drawing_events(app: &DrawCraftApp, ctx: &DrawingEventCtx<'_>) -> usize {
    let mut sim = DrawingEventCounter {
        touch_id: app.view.active_touch_id,
        pointer_active: app.view.active_pointer_stroke,
        has_live: has_active_live(app),
        saw_touch_start: false,
        last_page_pos: active_last_page_pos(app),
        count: 0,
    };

    for (idx, ev) in ctx.events.iter().enumerate() {
        match *ev {
            egui::Event::Touch {
                id,
                phase,
                pos,
                force,
                ..
            } => sim.step_touch(app, ctx, idx, (id, phase, pos, force)),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                ..
            } => sim.step_pointer_press(app, ctx, idx, pos),
            egui::Event::PointerMoved(pos) => {
                if sim.touch_id.is_none() && sim.pointer_active && sim.has_live {
                    sim.last_page_pos = Some(
                        ctx.xf
                            .to_page(Point::new(f64::from(pos.x), f64::from(pos.y))),
                    );
                    sim.count += 1;
                }
            }
            egui::Event::PointerButton {
                button: egui::PointerButton::Primary,
                pressed: false,
                ..
            } if sim.touch_id.is_none() && sim.pointer_active => sim.clear(),
            _ => {}
        }
    }

    sim.count.max(1)
}

struct DrawingEventCtx<'a> {
    ui: &'a egui::Ui,
    resp: &'a egui::Response,
    events: &'a [egui::Event],
    xf: Affine2,
    has_pointer_gone: bool,
    now_s: f64,
}

fn handle_touch_canvas_event(
    app: &mut DrawCraftApp,
    ctx: &DrawingEventCtx<'_>,
    idx: usize,
    cursor: &mut SampleCursor,
) {
    let Some(&egui::Event::Touch {
        id,
        phase,
        pos,
        force,
        ..
    }) = ctx.events.get(idx)
    else {
        return;
    };

    match phase {
        egui::TouchPhase::Start => {
            if finger_is_blocked(app, ctx.events, idx, id, force) {
                // Pencil-only: a finger never draws (navigation is handled earlier).
                return;
            }
            if touch_start_claims_stroke(app, ctx.events, idx, id, force)
                && can_start_on_canvas(ctx.ui, ctx.resp, pos)
            {
                if has_active_live(app) {
                    if should_replace_unmoved_pointer_stroke(app, pos, ctx.xf) {
                        cancel_active_stroke(app);
                    } else {
                        finish_active_stroke(app);
                    }
                }
                app.view.active_touch_id = Some(id);
                app.view.active_pointer_stroke = false;
                begin_stroke_sample(app, pos, ctx.xf, force, cursor, ctx.now_s);
            }
        }
        egui::TouchPhase::Move => {
            if app.view.active_touch_id == Some(id) && has_active_live(app) {
                update_stroke_sample(app, pos, ctx.xf, force, cursor, ctx.now_s);
            }
        }
        egui::TouchPhase::End => {
            if app.view.active_touch_id == Some(id) {
                if has_active_live(app) && last_live_distance_sq(app, pos, ctx.xf) >= 0.0625 {
                    update_stroke_sample(app, pos, ctx.xf, force, cursor, ctx.now_s);
                }
                finish_active_stroke(app);
            }
        }
        egui::TouchPhase::Cancel => {
            if app.view.active_touch_id == Some(id)
                || (app.view.active_touch_id.is_none()
                    && app.view.active_pointer_stroke
                    && ctx.has_pointer_gone)
            {
                finish_active_stroke(app);
            }
        }
    }
}

fn handle_pointer_canvas_event(
    app: &mut DrawCraftApp,
    ctx: &DrawingEventCtx<'_>,
    idx: usize,
    cursor: &mut SampleCursor,
    saw_touch_start: bool,
) {
    if !app.session.prefs.finger_drawing && app.view.study.pointer_is_direct_touch {
        return;
    }
    let Some(ev) = ctx.events.get(idx) else {
        return;
    };
    match ev {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            ..
        } => {
            if pointer_press_superseded_by_following_touch(app, ctx.events, idx, *pos) {
                return;
            }
            if let Some(active_id) = app.view.active_touch_id {
                if saw_touch_start || batch_has_touch_activity(ctx.events, active_id) {
                    return;
                }
                finish_active_stroke(app);
            }
            if can_start_on_canvas(ctx.ui, ctx.resp, *pos) {
                if has_active_live(app) {
                    finish_active_stroke(app);
                }
                app.view.active_pointer_stroke = true;
                app.view.active_touch_id = None;
                begin_stroke_sample(app, *pos, ctx.xf, None, cursor, ctx.now_s);
            }
        }
        egui::Event::PointerMoved(pos) => {
            if app.view.active_touch_id.is_none()
                && app.view.active_pointer_stroke
                && has_active_live(app)
            {
                update_stroke_sample(app, *pos, ctx.xf, None, cursor, ctx.now_s);
            }
        }
        egui::Event::PointerButton {
            button: egui::PointerButton::Primary,
            pressed: false,
            ..
        } if app.view.active_touch_id.is_none() && app.view.active_pointer_stroke => {
            finish_active_stroke(app);
        }
        _ => {}
    }
}

fn primary_canvas_press(app: &DrawCraftApp, events: &[egui::Event]) -> Option<Pos2> {
    let pointer_pos = events.iter().find_map(|event| match event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            ..
        } => Some(*pos),
        _ => None,
    });
    let touch_start = events
        .iter()
        .enumerate()
        .find_map(|(idx, event)| match event {
            egui::Event::Touch {
                id,
                phase: egui::TouchPhase::Start,
                pos,
                force,
                ..
            } => Some((idx, *id, *pos, force.is_some())),
            _ => None,
        });
    match touch_start {
        Some((idx, id, pos, has_force)) if !app.stylus.is_ignored_touch(id) => {
            if has_force || !touch_start_has_paired_pointer(events, idx) {
                Some(pos)
            } else {
                pointer_pos
            }
        }
        Some(_) => None,
        None => pointer_pos,
    }
}

fn find_text_item_at_screen_pos(
    app: &DrawCraftApp,
    painter: &egui::Painter,
    screen_pos: Pos2,
    xf: Affine2,
) -> Option<usize> {
    let mut best: Option<HitCandidate> = None;
    for (index, item) in app.session.doc.texts.iter().enumerate() {
        if let Some(cand) = text_hit_detail(
            painter,
            app.session.doc.width,
            index,
            item,
            screen_pos,
            xf,
            app.view.zoom,
        ) && best.is_none_or(|cur| is_better_hit(&cand, &cur))
        {
            best = Some(cand);
        }
    }
    match best.map(|b| b.object) {
        Some(SelectedObject::Text(index)) => Some(index),
        _ => None,
    }
}

pub(crate) fn start_editing_text_item(app: &mut DrawCraftApp, index: usize, item: &TextItem) {
    let item = item.sanitized();
    let primary_link = item.primary_link_url().unwrap_or_default();
    app.view.text_insert_position = [item.x, item.y];
    app.view.text_draft = item.text.clone();
    app.view.text_prev_draft = item.text;
    app.view.text_spans_draft = item.spans;
    app.view.text_alignment_draft = item.alignment;
    app.view.text_selection_bytes = None;
    app.view.text_selection_needs_restore = false;
    app.view.text_editor_page_size = None;
    app.view.text_edit_index = Some(index);
    app.view.text_size_draft = item.size;
    app.view.text_color_draft = item.color;
    app.view.text_font_draft = item.font;
    app.view.text_line_spacing_draft = item.line_spacing;
    app.view.text_letter_spacing_draft = item.letter_spacing;
    app.view.text_max_width_draft = item.max_width;
    app.view.text_link_draft = item.link_url;
    app.view.text_link_input = primary_link;
    app.view.text_link_editor_open = false;
    app.view.text_insert_error = None;
    app.view.text_editing = true;
    app.view.text_editor_needs_focus = true;
    app.view.text_style_open = false;
    app.view.clear_selection();
}

fn active_inline_text_editor_rect(app: &DrawCraftApp, xf: Affine2, avail: Rect) -> Option<Rect> {
    if !app.view.text_editing {
        return None;
    }
    let safe_zoom = app.view.zoom.max(0.05);
    let screen = xf.to_screen(Point::new(
        app.view.text_insert_position[0],
        app.view.text_insert_position[1],
    ));
    let origin = pos2(screen.x as f32, screen.y as f32);
    let (w, h) = match app.view.text_editor_page_size {
        Some([pw, ph]) if pw.is_finite() && ph.is_finite() => {
            ((pw * safe_zoom).max(24.0), (ph * safe_zoom).max(16.0))
        }
        _ => {
            let max_w = match app.view.text_max_width_draft {
                Some(w) => w.clamp(
                    drawcraft_doc::MIN_TEXT_BOX_WIDTH,
                    drawcraft_doc::MAX_TEXT_BOX_WIDTH,
                ),
                None => drawcraft_doc::text_max_page_width(
                    app.session.doc.width,
                    app.view.text_insert_position[0],
                ),
            };
            (
                (max_w * safe_zoom).max(48.0),
                (app.view.text_size_draft * app.view.text_line_spacing_draft * safe_zoom).max(24.0),
            )
        }
    };
    let clipped = Rect::from_min_size(origin, Vec2::new(w, h))
        .expand(4.0)
        .intersect(avail);
    (clipped.width() > 0.0 && clipped.height() > 0.0).then_some(clipped)
}

fn handle_text_canvas_press(
    app: &mut DrawCraftApp,
    ui: &egui::Ui,
    resp: &egui::Response,
    screen_pos: Pos2,
) {
    if !can_start_on_canvas(ui, resp, screen_pos) {
        return;
    }
    if has_active_live(app) {
        finish_active_stroke(app);
    }
    let xf = page_affine(app);
    if ui.input(|i| i.modifiers.command)
        && let Some(url) = link_at_screen_pos(app, &ui.painter_at(resp.rect), screen_pos, xf)
    {
        let _ = crate::open_external_url(app, ui.ctx(), &url);
        return;
    }
    if app.view.text_editing
        && active_inline_text_editor_rect(app, xf, resp.rect)
            .is_some_and(|r| r.contains(screen_pos))
    {
        return;
    }
    if !app.view.text_editing
        && let Some((index, paragraph_index)) =
            checkbox_at_screen_pos(app, &ui.painter_at(resp.rect), screen_pos, xf)
    {
        let _ = app.session.execute(
            "text.toggleCheckbox",
            &serde_json::json!({ "index": index, "paragraph_index": paragraph_index }),
        );
        return;
    }
    if app.view.text_editing {
        commit_inline_text(app);
    }
    if !app.view.text_insert_mode {
        return;
    }
    if let Some(index) =
        find_text_item_at_screen_pos(app, &ui.painter_at(resp.rect), screen_pos, xf)
        && let Some(item) = app.session.doc.texts.get(index).cloned()
    {
        start_editing_text_item(app, index, &item);
        app.view.clear_selection();
        return;
    }
    let page_pos = xf.to_page(Point::new(f64::from(screen_pos.x), f64::from(screen_pos.y)));
    if page_pos.x >= 0.0
        && page_pos.y >= 0.0
        && page_pos.x <= f64::from(app.session.doc.width)
        && page_pos.y <= f64::from(app.session.doc.height)
    {
        app.view.last_canvas_page_pos = Some([page_pos.x, page_pos.y]);
        app.view.text_insert_position = [page_pos.x, page_pos.y];
        app.view.text_draft.clear();
        app.view.text_prev_draft.clear();
        app.view.text_spans_draft.clear();
        app.view.text_selection_bytes = None;
        app.view.text_selection_needs_restore = false;
        app.view.text_editor_page_size = None;
        app.view.text_edit_index = None;
        app.view.text_max_width_draft = None;
        app.view.text_link_draft = None;
        app.view.text_link_input.clear();
        app.view.text_link_editor_open = false;
        app.view.text_insert_error = None;
        app.view.text_editing = true;
        app.view.text_editor_needs_focus = true;
        app.view.clear_selection();
        app.view.text_style_open = false;
    }
}

fn handle_drawing_input(app: &mut DrawCraftApp, ui: &mut egui::Ui, resp: &egui::Response) {
    let (space_held, shift_held, focused, multi_touch_active, events, now_s) = ui.input(|i| {
        (
            i.key_down(egui::Key::Space),
            i.modifiers.shift,
            i.focused,
            i.multi_touch().is_some(),
            i.events.clone(),
            i.time,
        )
    });
    app.session.straight_line_hold = shift_held || app.view.ruler_stroke_edge.is_some();

    if !has_active_live(app) {
        app.view.active_touch_id = None;
        app.view.active_pointer_stroke = false;
    }

    let pan_space = !app.view.text_editing && space_held;
    let stylus_pan_held = !app.view.text_editing && app.stylus.pan_held();
    if multi_touch_active {
        // Two fingers pinch and pan together; a single-finger pan must not fight them.
        app.view.study.finger_pan = None;
    }
    if !focused || pan_space || stylus_pan_held || app.view.pan_dragging || multi_touch_active {
        clear_ruler_drag(app);
        if app.view.shape_insert_mode {
            app.view.shape_drag_start = None;
            app.view.shape_drag_end = None;
            app.view.shape_drag_touch_id = None;
        }
        if has_active_live(app) {
            finish_active_stroke(app);
        }
        return;
    }

    if app.view.text_editing || app.view.text_insert_mode {
        if has_active_live(app) {
            finish_active_stroke(app);
        }
        if let Some(screen_pos) = primary_canvas_press(app, &events) {
            handle_text_canvas_press(app, ui, resp, screen_pos);
        }
        return;
    }

    if app.view.study.tape_mode {
        if has_active_live(app) {
            finish_active_stroke(app);
        }
        let xf = page_affine(app);
        crate::study_ui::handle_tape_input(app, ui, resp, xf);
        return;
    }

    let finger_ctx = DrawingEventCtx {
        ui,
        resp,
        events: &events,
        xf: page_affine(app),
        has_pointer_gone: false,
        now_s,
    };
    if handle_finger_navigation(app, &finger_ctx) {
        if has_active_live(app) {
            finish_active_stroke(app);
        }
        return;
    }

    if app.view.shape_insert_mode {
        handle_shape_input(app, ui, resp, &events);
        return;
    }

    if app.session.active_tool == ToolKind::Select {
        if has_active_live(app) {
            finish_active_stroke(app);
        }
        handle_selection_input(app, ui, resp, &events);
        return;
    }

    if app.view.ruler_active && handle_ruler_input(app, ui, resp, &events) {
        return;
    }

    let ctx = DrawingEventCtx {
        ui,
        resp,
        events: &events,
        xf: page_affine(app),
        has_pointer_gone: events.iter().any(|e| matches!(e, egui::Event::PointerGone)),
        now_s,
    };
    let mut cursor = SampleCursor {
        idx: 0,
        total: count_drawing_events(app, &ctx),
    };
    let mut saw_touch_start = false;

    for (idx, ev) in events.iter().enumerate() {
        match ev {
            egui::Event::Touch { phase, .. } => {
                if *phase == egui::TouchPhase::Start {
                    saw_touch_start = true;
                }
                handle_touch_canvas_event(app, &ctx, idx, &mut cursor);
            }
            egui::Event::PointerButton { .. } | egui::Event::PointerMoved(_) => {
                handle_pointer_canvas_event(app, &ctx, idx, &mut cursor, saw_touch_start);
            }
            _ => {}
        }
    }
    app.session.refresh_live_stroke_preview();

    if let Some(since_s) = app.view.straighten_candidate_since_s
        && now_s.is_finite()
        && app.session.live_stroke.is_some()
    {
        let held_s = now_s - since_s;
        if (held_s >= 0.72 && app.session.lock_live_stroke_recognized_shape())
            || (held_s >= 1.0 && app.session.lock_live_stroke_straight())
        {
            app.view.straighten_candidate_since_s = None;
            ui.ctx().request_repaint();
        } else if held_s < 1.05 {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(55));
        }
    }
}

fn handle_shape_input(
    app: &mut DrawCraftApp,
    ui: &egui::Ui,
    resp: &egui::Response,
    events: &[egui::Event],
) {
    let (focused, space_held, modifiers) =
        ui.input(|i| (i.focused, i.key_down(egui::Key::Space), i.modifiers));
    let stylus_pan_held = !app.view.text_editing && app.stylus.pan_held();
    if !focused || space_held || stylus_pan_held || app.view.pan_dragging {
        app.view.shape_drag_start = None;
        app.view.shape_drag_end = None;
        app.view.shape_drag_touch_id = None;
        if has_active_live(app) {
            finish_active_stroke(app);
        }
        return;
    }

    let pointer_start = events.iter().find_map(|event| match event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            ..
        } => Some(*pos),
        _ => None,
    });
    let touch_start = events
        .iter()
        .enumerate()
        .find_map(|(idx, event)| match event {
            egui::Event::Touch {
                id,
                phase: egui::TouchPhase::Start,
                pos,
                force,
                ..
            } => Some((idx, *id, *pos, force.is_some())),
            _ => None,
        });

    if app.view.shape_drag_start.is_none() {
        let start = match touch_start {
            Some((idx, id, pos, has_force)) if !app.stylus.is_ignored_touch(id) => {
                if has_force || !touch_start_has_paired_pointer(events, idx) {
                    Some((pos, Some(id)))
                } else {
                    pointer_start.map(|pointer| (pointer, None))
                }
            }
            Some(_) => None,
            None => pointer_start.map(|pointer| (pointer, None)),
        };
        if let Some((screen_pos, touch_id)) = start
            && can_start_on_canvas(ui, resp, screen_pos)
        {
            let xf = page_affine(app);
            let page_pos = xf.to_page(Point::new(f64::from(screen_pos.x), f64::from(screen_pos.y)));
            if page_pos.x >= 0.0
                && page_pos.y >= 0.0
                && page_pos.x <= f64::from(app.session.doc.width)
                && page_pos.y <= f64::from(app.session.doc.height)
            {
                if has_active_live(app) {
                    finish_active_stroke(app);
                }
                let start = [page_pos.x, page_pos.y];
                app.view.shape_drag_start = Some(start);
                app.view.shape_drag_end = Some(start);
                app.view.shape_drag_touch_id = touch_id;
            }
        }
    }

    let Some(start) = app.view.shape_drag_start else {
        return;
    };
    let xf = page_affine(app);
    let end_pos = if let Some(active_touch) = app.view.shape_drag_touch_id {
        events.iter().find_map(|event| match event {
            egui::Event::Touch {
                id,
                phase: egui::TouchPhase::Move | egui::TouchPhase::End,
                pos,
                ..
            } if *id == active_touch => Some(*pos),
            _ => None,
        })
    } else {
        events.iter().rev().find_map(|event| match event {
            egui::Event::PointerMoved(pos) => Some(*pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                ..
            } => Some(*pos),
            _ => None,
        })
    };
    if let Some(pos) = end_pos {
        let page = xf.to_page(Point::new(f64::from(pos.x), f64::from(pos.y)));
        app.view.shape_drag_end = Some([
            page.x.clamp(0.0, f64::from(app.session.doc.width)),
            page.y.clamp(0.0, f64::from(app.session.doc.height)),
        ]);
        ui.ctx().request_repaint();
    }

    let cancelled = events.iter().any(|event| match event {
        egui::Event::Touch {
            id,
            phase: egui::TouchPhase::Cancel,
            ..
        } => app.view.shape_drag_touch_id == Some(*id),
        egui::Event::PointerGone => app.view.shape_drag_touch_id.is_none(),
        _ => false,
    });
    if cancelled {
        app.view.shape_drag_start = None;
        app.view.shape_drag_end = None;
        app.view.shape_drag_touch_id = None;
        return;
    }

    let released = if let Some(active_touch) = app.view.shape_drag_touch_id {
        events.iter().any(|event| {
            matches!(
                event,
                egui::Event::Touch { id, phase: egui::TouchPhase::End, .. }
                    if *id == active_touch
            )
        })
    } else {
        events.iter().any(|event| {
            matches!(
                event,
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                }
            )
        })
    };
    if released {
        let raw_end = app.view.shape_drag_end.unwrap_or(start);
        let (start, end) =
            constrain_shape_drag(app.view.selected_shape_kind, start, raw_end, modifiers);
        let max_x = f64::from(app.session.doc.width);
        let max_y = f64::from(app.session.doc.height);
        let start = [start[0].clamp(0.0, max_x), start[1].clamp(0.0, max_y)];
        let end = [end[0].clamp(0.0, max_x), end[1].clamp(0.0, max_y)];
        let pen_color = app.session.active_pen().color;
        let fill_color = app
            .view
            .shape_fill_enabled
            .then(|| pen_color.with_alpha(app.view.shape_fill_opacity.clamp(0.15, 0.95)));
        let mut params = serde_json::json!({
            "kind": app.view.selected_shape_kind,
            "x1": start[0],
            "y1": start[1],
            "x2": end[0],
            "y2": end[1],
            "angle_degrees": app.view.shape_angle_degrees,
            "curve": app.view.shape_curve,
            "line_style": app.view.shape_line_style,
        });
        if let Some(fill) = fill_color {
            params["fill_color"] = serde_json::json!(fill);
        }
        let _ = app.session.execute("shape.insert", &params);
        app.view.shape_drag_start = None;
        app.view.shape_drag_end = None;
        app.view.shape_drag_touch_id = None;
    }
}

fn constrain_shape_drag(
    kind: ShapeKind,
    mut start: [f64; 2],
    mut end: [f64; 2],
    modifiers: egui::Modifiers,
) -> ([f64; 2], [f64; 2]) {
    let center = start;
    let centered =
        modifiers.alt || (modifiers.ctrl && !matches!(kind, ShapeKind::Line | ShapeKind::Arrow));
    if centered {
        let dx = end[0] - center[0];
        let dy = end[1] - center[1];
        start = [center[0] - dx, center[1] - dy];
        end = [center[0] + dx, center[1] + dy];
    }

    if matches!(kind, ShapeKind::Line | ShapeKind::Arrow) && (modifiers.shift || modifiers.ctrl) {
        let dx = end[0] - start[0];
        let dy = end[1] - start[1];
        let length = dx.hypot(dy);
        if length <= f64::EPSILON {
            return (start, end);
        }
        let step = if modifiers.ctrl {
            std::f64::consts::FRAC_PI_4
        } else {
            std::f64::consts::PI / 12.0
        };
        let angle = (dy.atan2(dx) / step).round() * step;
        let (sin, cos) = angle.sin_cos();
        if centered {
            start = [
                center[0] - cos * length * 0.5,
                center[1] - sin * length * 0.5,
            ];
            end = [
                center[0] + cos * length * 0.5,
                center[1] + sin * length * 0.5,
            ];
        } else {
            end = [start[0] + cos * length, start[1] + sin * length];
        }
        return (start, end);
    }

    if modifiers.shift && !matches!(kind, ShapeKind::Line | ShapeKind::Arrow) {
        let dx = end[0] - start[0];
        let dy = end[1] - start[1];
        let sign_x = if dx < 0.0 { -1.0 } else { 1.0 };
        let sign_y = if dy < 0.0 { -1.0 } else { 1.0 };
        let (width, height) = if kind == ShapeKind::Triangle {
            let ratio = 2.0 / 3.0_f64.sqrt();
            if dx.abs() > dy.abs() * ratio {
                (dx.abs(), dx.abs() / ratio)
            } else {
                (dy.abs() * ratio, dy.abs())
            }
        } else {
            let side = dx.abs().max(dy.abs());
            (side, side)
        };
        if centered {
            start = [
                center[0] - sign_x * width * 0.5,
                center[1] - sign_y * height * 0.5,
            ];
            end = [
                center[0] + sign_x * width * 0.5,
                center[1] + sign_y * height * 0.5,
            ];
        } else {
            end = [start[0] + sign_x * width, start[1] + sign_y * height];
        }
    }
    (start, end)
}

fn clear_ruler_drag(app: &mut DrawCraftApp) {
    app.view.ruler_drag_mode = None;
    app.view.ruler_drag_touch_id = None;
    app.view.ruler_drag_anchor = None;
}

fn start_ruler_drag(
    app: &mut DrawCraftApp,
    mode: crate::RulerDragMode,
    page_pos: Point,
    touch_id: Option<egui::TouchId>,
) {
    app.view.ruler_drag_mode = Some(mode);
    app.view.ruler_drag_touch_id = touch_id;
    app.view.ruler_drag_anchor = Some([page_pos.x, page_pos.y]);
    app.view.ruler_drag_origin_center = app.view.ruler_center;
    app.view.ruler_drag_origin_angle = app.view.ruler_angle;
}

fn update_ruler_drag(app: &mut DrawCraftApp, page_pos: Point) {
    if !page_pos.x.is_finite() || !page_pos.y.is_finite() {
        return;
    }
    let Some(mode) = app.view.ruler_drag_mode else {
        return;
    };
    match mode {
        crate::RulerDragMode::Move => {
            if let Some(anchor) = app.view.ruler_drag_anchor {
                let candidate = Point::new(
                    app.view.ruler_drag_origin_center[0] + page_pos.x - anchor[0],
                    app.view.ruler_drag_origin_center[1] + page_pos.y - anchor[1],
                );
                let center = clamp_ruler_center(app, candidate);
                app.view.ruler_center = [center.x, center.y];
            }
        }
        crate::RulerDragMode::Rotate => {
            let center = Point::new(
                app.view.ruler_drag_origin_center[0],
                app.view.ruler_drag_origin_center[1],
            );
            let dx = page_pos.x - center.x;
            let dy = page_pos.y - center.y;
            if dx.hypot(dy) > 1.0 {
                let start_handle_angle =
                    app.view.ruler_drag_origin_angle - std::f64::consts::FRAC_PI_2;
                let mut angle_delta = dy.atan2(dx) - start_handle_angle;
                if angle_delta > std::f64::consts::PI {
                    angle_delta -= std::f64::consts::TAU;
                } else if angle_delta < -std::f64::consts::PI {
                    angle_delta += std::f64::consts::TAU;
                }
                app.view.ruler_angle = app.view.ruler_drag_origin_angle + angle_delta;
                let center = clamp_ruler_center(app, center);
                app.view.ruler_center = [center.x, center.y];
            }
        }
    }
}

fn handle_ruler_input(
    app: &mut DrawCraftApp,
    ui: &egui::Ui,
    resp: &egui::Response,
    events: &[egui::Event],
) -> bool {
    if !app.view.ruler_active || has_active_live(app) {
        return false;
    }

    let pointer_start = events.iter().find_map(|event| match event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            ..
        } => Some(*pos),
        _ => None,
    });
    let touch_start = events
        .iter()
        .enumerate()
        .find_map(|(idx, event)| match event {
            egui::Event::Touch {
                id,
                phase: egui::TouchPhase::Start,
                pos,
                force,
                ..
            } => Some((idx, *id, *pos, force.is_some())),
            _ => None,
        });

    if app.view.ruler_drag_mode.is_none() {
        let start = match touch_start {
            Some((idx, id, pos, has_force)) if !app.stylus.is_ignored_touch(id) => {
                if has_force || !touch_start_has_paired_pointer(events, idx) {
                    Some((pos, Some(id)))
                } else {
                    pointer_start.map(|pointer| (pointer, None))
                }
            }
            Some(_) => None,
            None => pointer_start.map(|pointer| (pointer, None)),
        };
        if let Some((screen_pos, touch_id)) = start
            && can_start_on_canvas(ui, resp, screen_pos)
        {
            let xf = page_affine(app);
            let page_pos = xf.to_page(Point::new(f64::from(screen_pos.x), f64::from(screen_pos.y)));
            let handle = ruler_rotation_handle(app);
            let handle_screen = xf.to_screen(handle);
            let handle_dx = f64::from(screen_pos.x) - handle_screen.x;
            let handle_dy = f64::from(screen_pos.y) - handle_screen.y;
            if handle_dx.hypot(handle_dy) <= 16.0 {
                start_ruler_drag(app, crate::RulerDragMode::Rotate, page_pos, touch_id);
                return true;
            }
            if ruler_edge_for_point(app, page_pos).is_some() {
                return false;
            }
            let (along, across) = ruler_local_position(app, page_pos);
            let (_, _, _, half_length, half_width) = ruler_geometry(app);
            let end_margin = 4.0 / f64::from(app.view.zoom.max(0.05));
            if along.abs() <= half_length + end_margin && across.abs() <= half_width {
                start_ruler_drag(app, crate::RulerDragMode::Move, page_pos, touch_id);
                return true;
            }
        }
    }

    let Some(_) = app.view.ruler_drag_mode else {
        return false;
    };
    let end_pos = if let Some(active_touch) = app.view.ruler_drag_touch_id {
        events.iter().find_map(|event| match event {
            egui::Event::Touch {
                id,
                phase: egui::TouchPhase::Move | egui::TouchPhase::End,
                pos,
                ..
            } if *id == active_touch => Some(*pos),
            _ => None,
        })
    } else {
        events.iter().rev().find_map(|event| match event {
            egui::Event::PointerMoved(pos) => Some(*pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                ..
            } => Some(*pos),
            _ => None,
        })
    };
    if let Some(screen_pos) = end_pos {
        let page_pos =
            page_affine(app).to_page(Point::new(f64::from(screen_pos.x), f64::from(screen_pos.y)));
        update_ruler_drag(app, page_pos);
        ui.ctx().request_repaint();
    }

    let cancelled = events.iter().any(|event| match event {
        egui::Event::Touch {
            id,
            phase: egui::TouchPhase::Cancel,
            ..
        } => app.view.ruler_drag_touch_id == Some(*id),
        egui::Event::PointerGone => app.view.ruler_drag_touch_id.is_none(),
        _ => false,
    });
    let released = if let Some(active_touch) = app.view.ruler_drag_touch_id {
        events.iter().any(|event| {
            matches!(event, egui::Event::Touch { id, phase: egui::TouchPhase::End, .. } if *id == active_touch)
        })
    } else {
        events.iter().any(|event| {
            matches!(
                event,
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                }
            )
        })
    };
    if cancelled || released {
        clear_ruler_drag(app);
    }
    true
}

#[inline]
fn color_luma(c: Color32) -> f32 {
    0.2126 * f32::from(c.r()) + 0.7152 * f32::from(c.g()) + 0.0722 * f32::from(c.b())
}

fn contrast_halo(ring: Color32, paper_bg: Color32) -> Color32 {
    let luma = if ring.a() >= 96 {
        color_luma(ring)
    } else {
        color_luma(paper_bg)
    };
    if luma < 128.0 {
        Color32::from_rgba_unmultiplied(255, 255, 255, 200)
    } else {
        Color32::from_rgba_unmultiplied(15, 23, 42, 175)
    }
}

fn live_pointer_screen_pos(app: &DrawCraftApp, xf: Affine2) -> Option<Pos2> {
    let pt = match app.session.active_tool {
        ToolKind::Pen(_) => app
            .session
            .live_stroke
            .as_ref()
            .and_then(|s| s.raw_points.last())
            .map(|s| s.point()),
        ToolKind::Eraser => app
            .session
            .live_eraser
            .as_ref()
            .and_then(|e| e.path.last().map(|(pt, _)| *pt)),
        ToolKind::LaserPointer => app.view.laser_points.last().map(|p| Point::new(p.x, p.y)),
        ToolKind::Select => None,
    }?;
    let sp = xf.to_screen(pt);
    let out = pos2(sp.x as f32, sp.y as f32);
    (out.x.is_finite() && out.y.is_finite()).then_some(out)
}

fn update_canvas_cursor_icon(app: &DrawCraftApp, ui: &egui::Ui, resp: &egui::Response) {
    if !resp.hovered() {
        return;
    }
    let space_held = !app.view.text_editing && ui.input(|i| i.key_down(egui::Key::Space));
    let stylus_pan_held = !app.view.text_editing && app.stylus.pan_held();
    let selection_cursor = if app.session.active_tool == ToolKind::Select {
        match app.view.selection_drag.as_ref().map(|d| &d.mode) {
            Some(crate::SelectionDragMode::ResizeTextWidth { angle_degrees, .. }) => {
                Some(resize_cursor_for_angle(*angle_degrees))
            }
            Some(crate::SelectionDragMode::Resize { cursor, .. }) => Some(*cursor),
            Some(crate::SelectionDragMode::Rotate { .. }) => Some(egui::CursorIcon::Crosshair),
            _ => ui.input(|i| i.pointer.hover_pos()).and_then(|pos| {
                selection_handle_for_app(app, &ui.painter_at(resp.rect), page_affine(app), pos).map(
                    |handle| match handle {
                        SelectionHandle::Rotate { .. } => egui::CursorIcon::Crosshair,
                        SelectionHandle::Resize { cursor, .. }
                        | SelectionHandle::ResizeTextWidth { cursor, .. } => cursor,
                    },
                )
            }),
        }
    } else {
        None
    };
    let ruler_move_hover = app.view.ruler_active
        && ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|screen_pos| {
                let xf = page_affine(app);
                let page_pos =
                    xf.to_page(Point::new(f64::from(screen_pos.x), f64::from(screen_pos.y)));
                let handle = xf.to_screen(ruler_rotation_handle(app));
                let handle_dx = f64::from(screen_pos.x) - handle.x;
                let handle_dy = f64::from(screen_pos.y) - handle.y;
                let (along, across) = ruler_local_position(app, page_pos);
                let (_, _, _, half_length, half_width) = ruler_geometry(app);
                handle_dx.hypot(handle_dy) <= 16.0
                    || (along.abs() <= half_length + 4.0 / f64::from(app.view.zoom.max(0.05))
                        && across.abs() <= half_width
                        && ruler_edge_for_point(app, page_pos).is_none())
            });
    let icon = if app.view.pan_dragging {
        egui::CursorIcon::Grabbing
    } else if space_held || stylus_pan_held {
        egui::CursorIcon::Grab
    } else if app.view.ruler_drag_mode.is_some() {
        egui::CursorIcon::Grabbing
    } else if ruler_move_hover {
        egui::CursorIcon::Grab
    } else if app.view.shape_insert_mode {
        egui::CursorIcon::Crosshair
    } else if ui.input(|i| i.modifiers.command)
        && ui
            .input(|i| i.pointer.hover_pos())
            .and_then(|pos| {
                link_at_screen_pos(app, &ui.painter_at(resp.rect), pos, page_affine(app))
            })
            .is_some()
    {
        egui::CursorIcon::PointingHand
    } else if app.view.text_insert_mode || app.view.text_editing {
        egui::CursorIcon::Text
    } else if app.session.active_tool == ToolKind::Select {
        selection_cursor.unwrap_or(if app.view.freeform_lasso_mode {
            egui::CursorIcon::Crosshair
        } else {
            egui::CursorIcon::Default
        })
    } else {
        // Hide the OS arrow over the canvas so only the low-clutter nib/eraser reticle appears.
        egui::CursorIcon::None
    };
    ui.ctx().set_cursor_icon(icon);
}

fn text_font_id_styled(font: TextFont, size: f32, bold: bool, italic: bool) -> FontId {
    let family = match (font, bold, italic) {
        (TextFont::Sans, false, false) => egui::FontFamily::Name("RichSansRegular".into()),
        (TextFont::Sans, true, false) => egui::FontFamily::Name("RichSansBold".into()),
        (TextFont::Sans, false, true) => egui::FontFamily::Name("RichSansItalic".into()),
        (TextFont::Sans, true, true) => egui::FontFamily::Name("RichSansBoldItalic".into()),
        (TextFont::Monospace, false, false) => egui::FontFamily::Monospace,
        (TextFont::Monospace, true, false) => egui::FontFamily::Name("RichMonoBold".into()),
        (TextFont::Monospace, false, true) => egui::FontFamily::Name("RichMonoItalic".into()),
        (TextFont::Monospace, true, true) => egui::FontFamily::Name("RichMonoBoldItalic".into()),
    };
    FontId::new(size, family)
}

#[derive(Clone, Copy)]
struct TextInkResolveCtx {
    theme: UiTheme,
    auto_contrast: bool,
    paper_color: PaperColor,
}

#[derive(Clone)]
struct TextLayoutSpec {
    font: TextFont,
    size_px: f32,
    zoom: f32,
    line_spacing: f32,
    letter_spacing_px: f32,
    max_width_px: f32,
    color: Color32,
    ink_ctx: Option<TextInkResolveCtx>,
    alignment: TextAlignment,
    spans: Vec<TextSpan>,
    link_spans: Vec<(usize, usize)>,
    hide_list_prefix: bool,
}

fn resolve_span_ink_color(spec: &TextLayoutSpec, raw: drawcraft_color::Color) -> Color32 {
    let clean = raw.sanitized();
    if let Some(ctx) = spec.ink_ctx {
        to_color32(
            ctx.paper_color
                .resolve_ink_color(ctx.theme, ctx.auto_contrast, clean, false),
        )
    } else {
        to_color32(clean)
    }
}

fn find_list_prefix_ranges(text: &str) -> Vec<(usize, usize, usize, ListItemKind)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for (para_idx, line) in text.split('\n').enumerate() {
        let trimmed_line = line.strip_suffix('\r').unwrap_or(line);
        let (kind_opt, prefix_bytes) = parse_list_line_prefix(trimmed_line);
        if let Some(kind) = kind_opt
            && prefix_bytes > 0
            && offset + prefix_bytes <= text.len()
        {
            out.push((para_idx, offset, offset + prefix_bytes, kind));
        }
        offset = offset.saturating_add(line.len()).saturating_add(1);
    }
    out
}

fn collect_list_prefix_rects(
    galley: &egui::Galley,
    text: &str,
) -> Vec<(usize, ListItemKind, Rect)> {
    let mut out = Vec::new();
    for (para_idx, start_b, end_b, kind) in find_list_prefix_ranges(text) {
        if start_b >= end_b || end_b > text.len() {
            continue;
        }
        let c0 = text[..start_b].chars().count();
        let c1 = text[..end_b].chars().count();
        let r0 = galley.pos_from_cursor(egui::text::CCursor::new(c0));
        let r1 = galley.pos_from_cursor(egui::text::CCursor::new(c1));
        let left = r0.left().min(r1.left());
        let right = r0.right().max(r1.left()).max(left + 4.0);
        let top = r0.top().min(r1.top());
        let bottom = r0.bottom().max(r1.bottom());
        if left.is_finite() && right.is_finite() && top.is_finite() && bottom.is_finite() {
            out.push((
                para_idx,
                kind,
                Rect::from_min_max(pos2(left, top), pos2(right, bottom)),
            ));
        }
    }
    out
}

fn draft_link_byte_spans(text: &str, link_draft: Option<&str>) -> Vec<(usize, usize)> {
    let spans: Vec<(usize, usize)> = drawcraft_doc::find_text_url_spans(text)
        .into_iter()
        .map(|s| (s.byte_start, s.byte_end))
        .collect();
    if !spans.is_empty() {
        return spans;
    }
    if !text.is_empty()
        && link_draft
            .and_then(drawcraft_doc::sanitize_link_url)
            .is_some()
    {
        return vec![(0, text.len())];
    }
    Vec::new()
}

fn quantize_layout_font_size(size_px: f32) -> f32 {
    if !size_px.is_finite() {
        return 16.0;
    }
    size_px.clamp(1.0, 400.0)
}

fn build_text_layout_job(text: &str, spec: &TextLayoutSpec) -> egui::text::LayoutJob {
    let font_size = quantize_layout_font_size(spec.size_px);
    let line_spacing = spec.line_spacing.clamp(0.8, 3.0);
    let base_format = egui::TextFormat {
        font_id: text_font_id_styled(spec.font, font_size, false, false),
        color: spec.color,
        extra_letter_spacing: spec.letter_spacing_px,
        line_height: Some((font_size * line_spacing).max(1.0)),
        ..Default::default()
    };
    let list_prefixes = if spec.hide_list_prefix {
        find_list_prefix_ranges(text)
    } else {
        Vec::new()
    };
    let norm_spans = normalize_text_spans(text, &spec.spans);

    let mut job = if text.is_empty() {
        egui::text::LayoutJob::single_section(String::new(), base_format)
    } else if spec.link_spans.is_empty() && norm_spans.is_empty() && list_prefixes.is_empty() {
        egui::text::LayoutJob::single_section(text.to_owned(), base_format)
    } else {
        let mut bounds = vec![0usize, text.len()];
        for &(a, b) in &spec.link_spans {
            if a < b
                && a <= text.len()
                && b <= text.len()
                && text.is_char_boundary(a)
                && text.is_char_boundary(b)
            {
                bounds.push(a);
                bounds.push(b);
            }
        }
        for span in &norm_spans {
            if span.start < span.end
                && span.end <= text.len()
                && text.is_char_boundary(span.start)
                && text.is_char_boundary(span.end)
            {
                bounds.push(span.start);
                bounds.push(span.end);
            }
        }
        for &(_, a, b, _) in &list_prefixes {
            if a < b && b <= text.len() && text.is_char_boundary(a) && text.is_char_boundary(b) {
                bounds.push(a);
                bounds.push(b);
            }
        }
        bounds.sort_unstable();
        bounds.dedup();

        let mut built = egui::text::LayoutJob::default();
        for pair in bounds.windows(2) {
            let start = pair[0];
            let end = pair[1];
            if start >= end {
                continue;
            }
            let hidden_kind = list_prefixes
                .iter()
                .find(|&&(_, ps, pe, _)| ps <= start && end <= pe)
                .map(|&(_, _, _, kind)| kind);
            if let Some(kind) = hidden_kind {
                let mut fmt = base_format.clone();
                fmt.color = Color32::TRANSPARENT;
                let slice = &text[start..end];
                if matches!(kind, ListItemKind::Checkbox { .. }) && slice.chars().count() == 4 {
                    // Use identical glyph metrics for checked and unchecked checkboxes so toggling never shifts position.
                    built.append("[x] ", 0.0, fmt);
                } else {
                    built.append(slice, 0.0, fmt);
                }
                continue;
            }
            let is_link = spec
                .link_spans
                .iter()
                .any(|&(ls, le)| ls <= start && end <= le);
            let mut bold = false;
            let mut italic = false;
            let mut underline = is_link;
            let mut hl: Option<drawcraft_color::Color> = None;
            let mut span_font: Option<TextFont> = None;
            let mut span_size: Option<f32> = None;
            let mut span_color: Option<drawcraft_color::Color> = None;
            for sp in &norm_spans {
                if sp.start <= start && end <= sp.end {
                    bold |= sp.bold;
                    italic |= sp.italic;
                    underline |= sp.underline;
                    if sp.highlight.is_some() {
                        hl = sp.highlight;
                    }
                    if sp.font.is_some() {
                        span_font = sp.font;
                    }
                    if sp.size.is_some() {
                        span_size = sp.size;
                    }
                    if sp.color.is_some() {
                        span_color = sp.color;
                    }
                }
            }
            let eff_font = span_font.unwrap_or(spec.font);
            let eff_size_px = span_size.map_or(font_size, |s| {
                quantize_layout_font_size(s.clamp(6.0, 120.0) * spec.zoom.max(0.05))
            });
            let eff_color = span_color.map_or(spec.color, |c| resolve_span_ink_color(spec, c));
            let mut fmt = base_format.clone();
            fmt.font_id = text_font_id_styled(eff_font, eff_size_px, bold, italic);
            fmt.color = eff_color;
            fmt.line_height = Some((eff_size_px * line_spacing).max(1.0));
            if underline {
                let u_color = if is_link && span_color.is_none() {
                    spec.color
                } else {
                    eff_color
                };
                fmt.underline = Stroke::new((eff_size_px * 0.065).clamp(1.0, 3.0), u_color);
            }
            if let Some(bg) = hl {
                fmt.background = to_color32(bg);
            }
            built.append(&text[start..end], 0.0, fmt);
        }
        built
    };
    job.halign = match spec.alignment {
        TextAlignment::Left => egui::Align::LEFT,
        TextAlignment::Center => egui::Align::Center,
        TextAlignment::Right => egui::Align::RIGHT,
    };
    job.keep_trailing_whitespace = true;
    job.wrap = egui::text::TextWrapping {
        max_width: spec.max_width_px.max(8.0),
        ..Default::default()
    };
    job
}

fn build_text_layout_job_for_item(
    p: &egui::Painter,
    doc_width: u32,
    item: &TextItem,
    zoom: f32,
    color: Color32,
    ink_ctx: Option<TextInkResolveCtx>,
) -> egui::text::LayoutJob {
    let size_px = (item.size * zoom).clamp(1.0, 400.0);
    let max_width_px = item.effective_max_width(doc_width) * zoom.max(0.05);
    let link_spans = item
        .resolved_links()
        .into_iter()
        .map(|s| (s.byte_start, s.byte_end))
        .collect();
    let mut spec = TextLayoutSpec {
        font: item.font,
        size_px,
        zoom: zoom.max(0.05),
        line_spacing: item.line_spacing,
        letter_spacing_px: item.letter_spacing * zoom,
        max_width_px,
        color,
        ink_ctx,
        alignment: item.alignment,
        spans: item.spans.clone(),
        link_spans,
        hide_list_prefix: true,
    };
    if item.max_width.is_none() && item.alignment != TextAlignment::Left {
        let mut measure_spec = spec.clone();
        measure_spec.alignment = TextAlignment::Left;
        let measured = p.layout_job(build_text_layout_job(&item.text, &measure_spec));
        spec.max_width_px = measured.size().x.clamp(24.0, max_width_px.max(24.0));
    }
    build_text_layout_job(&item.text, &spec)
}

fn layout_text_item_galley(
    p: &egui::Painter,
    doc_width: u32,
    item: &TextItem,
    zoom: f32,
    color: Color32,
    ink_ctx: Option<TextInkResolveCtx>,
) -> Arc<egui::Galley> {
    p.layout_job(build_text_layout_job_for_item(
        p, doc_width, item, zoom, color, ink_ctx,
    ))
}

fn text_item_may_have_link(item: &TextItem) -> bool {
    item.link_url.as_ref().is_some_and(|u| !u.trim().is_empty())
        || item.text.contains('.')
        || item.text.contains(':')
}

fn text_item_rough_page_rect(doc_width: u32, item: &TextItem) -> Rect {
    let base_size = if item.size.is_finite() {
        item.size
    } else {
        16.0
    };
    let max_span_size = item
        .spans
        .iter()
        .filter_map(|s| s.size.filter(|sz| sz.is_finite()))
        .fold(base_size, f32::max)
        .clamp(6.0, 120.0);
    let raw_ls = if item.line_spacing.is_finite() {
        item.line_spacing
    } else {
        1.2
    };
    let line_h = (max_span_size * raw_ls.clamp(0.8, 3.0)).max(max_span_size);
    let max_w = item.effective_max_width(doc_width).max(32.0);
    let approx_chars_per_line = ((max_w / (max_span_size * 0.35).max(2.0)).floor() as usize).max(1);
    let explicit_lines = item.text.split('\n').count().max(1);
    let wrapped_extra = item.text.len() / approx_chars_per_line;
    let est_lines = explicit_lines
        .saturating_add(wrapped_extra)
        .saturating_add(2) as f32;
    let letter_extra = if item.letter_spacing.is_finite() && item.letter_spacing > 0.0 {
        item.letter_spacing.clamp(0.0, 40.0) * approx_chars_per_line as f32
    } else {
        0.0
    };
    let box_w = max_w + letter_extra + max_span_size * 2.0 + 32.0;
    let box_h = (est_lines * line_h).max(line_h * 1.5) + 24.0;
    let origin = pos2(
        if item.x.is_finite() {
            item.x as f32
        } else {
            0.0
        },
        if item.y.is_finite() {
            item.y as f32
        } else {
            0.0
        },
    );
    let angle = if item.angle_degrees.is_finite() {
        item.angle_degrees.to_radians() as f32
    } else {
        0.0
    };
    let rot = egui::emath::Rot2::from_angle(angle);
    let corners = [
        origin + rot * Vec2::new(-24.0, -16.0),
        origin + rot * Vec2::new(box_w, -16.0),
        origin + rot * Vec2::new(-24.0, box_h),
        origin + rot * Vec2::new(box_w, box_h),
    ];
    let mut min = pos2(f32::INFINITY, f32::INFINITY);
    let mut max = pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for c in corners {
        min.x = min.x.min(c.x);
        min.y = min.y.min(c.y);
        max.x = max.x.max(c.x);
        max.y = max.y.max(c.y);
    }
    Rect::from_min_max(min, max)
}

fn rough_page_rect_to_screen(page_rect: Rect, xf: Affine2) -> Rect {
    let min_s = xf.to_screen(Point::new(
        f64::from(page_rect.min.x),
        f64::from(page_rect.min.y),
    ));
    let max_s = xf.to_screen(Point::new(
        f64::from(page_rect.max.x),
        f64::from(page_rect.max.y),
    ));
    Rect::from_two_pos(
        pos2(min_s.x as f32, min_s.y as f32),
        pos2(max_s.x as f32, max_s.y as f32),
    )
}

fn link_at_screen_pos(
    app: &DrawCraftApp,
    painter: &egui::Painter,
    screen_pos: Pos2,
    xf: Affine2,
) -> Option<String> {
    let has_candidate = app.session.doc.texts.iter().any(|item| {
        text_item_may_have_link(item)
            && rough_page_rect_to_screen(text_item_rough_page_rect(app.session.doc.width, item), xf)
                .expand(12.0)
                .contains(screen_pos)
    });
    if !has_candidate {
        return None;
    }
    let index = find_text_item_at_screen_pos(app, painter, screen_pos, xf)?;
    let item = app.session.doc.texts.get(index)?.as_ref().sanitized();
    let spans = item.resolved_links();
    if spans.len() > 1 {
        let screen_origin = xf.to_screen(Point::new(item.x, item.y));
        let origin = pos2(screen_origin.x as f32, screen_origin.y as f32);
        let rot = egui::emath::Rot2::from_angle(item.angle_degrees.to_radians() as f32);
        let galley = layout_text_item_galley(
            painter,
            app.session.doc.width,
            &item,
            app.view.zoom,
            Color32::WHITE,
            None,
        );
        let local = (rot.inverse() * (screen_pos - origin)) + Vec2::new(galley.rect.left(), 0.0);
        let char_idx: usize = galley.cursor_from_pos(local).index.into();
        let byte_offset = item
            .text
            .char_indices()
            .nth(char_idx)
            .map_or(item.text.len(), |(b, _)| b);
        if let Some(hit) = spans
            .iter()
            .find(|s| byte_offset >= s.byte_start && byte_offset <= s.byte_end)
        {
            return Some(hit.url.clone());
        }
    }
    item.primary_link_url()
}

struct TextSelectionGeometry {
    origin: Pos2,
    box_size: Vec2,
    angle: f32,
    angle_degrees: f64,
    rot: egui::emath::Rot2,
    corners: [Pos2; 4],
    outer_corners: [Pos2; 4],
    handle_corners: [Pos2; 4],
    left_mid: Pos2,
    right_mid: Pos2,
    rotate_anchor: Pos2,
    rotate_handle: Pos2,
    center: Pos2,
    content_width_page: f64,
}

impl TextSelectionGeometry {
    fn aabb(&self) -> Rect {
        let mut min = pos2(f32::INFINITY, f32::INFINITY);
        let mut max = pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
        for corner in self.corners {
            min.x = min.x.min(corner.x);
            min.y = min.y.min(corner.y);
            max.x = max.x.max(corner.x);
            max.y = max.y.max(corner.y);
        }
        Rect::from_min_max(min, max)
    }

    fn contains_screen_pos(&self, pos: Pos2, pad: f32) -> bool {
        let local = self.rot.inverse() * (pos - self.origin);
        local.x >= -pad
            && local.x <= self.box_size.x + pad
            && local.y >= -pad
            && local.y <= self.box_size.y + pad
    }
}

fn text_selection_geometry_from_galley(
    item: &TextItem,
    xf: Affine2,
    zoom: f32,
    galley: &egui::Galley,
) -> TextSelectionGeometry {
    let safe_zoom = zoom.max(0.05);
    let size_px = (item.size * zoom).clamp(1.0, 400.0);
    let line_spacing = item.line_spacing.clamp(0.8, 3.0);
    let min_h = (size_px * line_spacing).max(size_px);
    let content_w = match item.max_width {
        Some(width) => {
            (width.max(drawcraft_doc::MIN_TEXT_BOX_WIDTH) * safe_zoom).max(galley.size().x)
        }
        None => galley.size().x.max(8.0),
    };
    let box_w = content_w + 6.0;
    let box_h = galley.size().y.max(min_h) + 4.0;
    let box_size = Vec2::new(box_w, box_h);
    let screen = xf.to_screen(Point::new(item.x, item.y));
    let origin = pos2(screen.x as f32, screen.y as f32);
    let angle = item.angle_degrees.to_radians() as f32;
    let rot = egui::emath::Rot2::from_angle(angle);
    let corners = [
        origin,
        origin + rot * Vec2::new(box_w, 0.0),
        origin + rot * Vec2::new(0.0, box_h),
        origin + rot * Vec2::new(box_w, box_h),
    ];
    let pad = SELECTION_FRAME_PAD;
    let c_off = SELECTION_CORNER_HANDLE_OFFSET;
    let s_off = SELECTION_SIDE_HANDLE_OFFSET;
    let outer_corners = [
        origin + rot * Vec2::new(-pad, -pad),
        origin + rot * Vec2::new(box_w + pad, -pad),
        origin + rot * Vec2::new(-pad, box_h + pad),
        origin + rot * Vec2::new(box_w + pad, box_h + pad),
    ];
    let handle_corners = [
        origin + rot * Vec2::new(-c_off, -c_off),
        origin + rot * Vec2::new(box_w + c_off, -c_off),
        origin + rot * Vec2::new(-c_off, box_h + c_off),
        origin + rot * Vec2::new(box_w + c_off, box_h + c_off),
    ];
    let left_mid = origin + rot * Vec2::new(-s_off, box_h * 0.5);
    let right_mid = origin + rot * Vec2::new(box_w + s_off, box_h * 0.5);
    let center = origin + rot * (box_size * 0.5);
    let (rotate_anchor, rotate_handle) = if angle.abs() <= f32::EPSILON {
        let bounds = Rect::from_min_size(origin, box_size);
        let handle = rotation_handle_center(bounds);
        let anchor = if handle.y < bounds.top() {
            pos2(bounds.center().x, bounds.top() - pad)
        } else {
            pos2(bounds.center().x, bounds.bottom() + pad)
        };
        (anchor, handle)
    } else {
        let top_anchor = origin + rot * Vec2::new(box_w * 0.5, -pad);
        let top_handle = top_anchor + rot * Vec2::new(0.0, -(SELECTION_ROTATE_HANDLE_OFFSET - pad));
        if top_handle.y >= 8.0 {
            (top_anchor, top_handle)
        } else {
            let bottom_anchor = origin + rot * Vec2::new(box_w * 0.5, box_h + pad);
            (
                bottom_anchor,
                bottom_anchor + rot * Vec2::new(0.0, SELECTION_ROTATE_HANDLE_OFFSET - pad),
            )
        }
    };
    let content_width_page = match item.max_width {
        Some(width) => f64::from(width).max(f64::from(galley.size().x) / f64::from(safe_zoom)),
        None => f64::from(galley.size().x.max(8.0)) / f64::from(safe_zoom),
    };
    TextSelectionGeometry {
        origin,
        box_size,
        angle,
        angle_degrees: item.angle_degrees,
        rot,
        corners,
        outer_corners,
        handle_corners,
        left_mid,
        right_mid,
        rotate_anchor,
        rotate_handle,
        center,
        content_width_page,
    }
}

fn text_selection_geometry(
    p: &egui::Painter,
    doc_width: u32,
    item: &TextItem,
    xf: Affine2,
    zoom: f32,
) -> TextSelectionGeometry {
    let galley = layout_text_item_galley(p, doc_width, item, zoom, Color32::WHITE, None);
    text_selection_geometry_from_galley(item, xf, zoom, &galley)
}

fn text_item_bounds(
    p: &egui::Painter,
    doc_width: u32,
    item: &TextItem,
    xf: Affine2,
    zoom: f32,
) -> Rect {
    text_selection_geometry(p, doc_width, item, xf, zoom).aabb()
}

fn point_in_polygon(point: Pos2, polygon: &[Pos2]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let px = f64::from(point.x);
    let py = f64::from(point.y);
    let mut inside = false;
    let Some(mut previous) = polygon.last().copied() else {
        return false;
    };
    for current in polygon {
        let ax = f64::from(previous.x);
        let ay = f64::from(previous.y);
        let bx = f64::from(current.x);
        let by = f64::from(current.y);
        let crosses = (ay > py) != (by > py);
        if crosses {
            let edge_x = (bx - ax) * (py - ay) / (by - ay) + ax;
            if px < edge_x {
                inside = !inside;
            }
        }
        previous = *current;
    }
    inside
}

fn distance_sq_to_segment(point: Pos2, start: Pos2, end: Pos2) -> f64 {
    let px = f64::from(point.x);
    let py = f64::from(point.y);
    let ax = f64::from(start.x);
    let ay = f64::from(start.y);
    let dx = f64::from(end.x) - ax;
    let dy = f64::from(end.y) - ay;
    let length_sq = dx * dx + dy * dy;
    let t = if length_sq > f64::EPSILON {
        (((px - ax) * dx + (py - ay) * dy) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let near_x = ax + t * dx;
    let near_y = ay + t * dy;
    (px - near_x).powi(2) + (py - near_y).powi(2)
}

fn rect_distance_sq(point: Pos2, rect: Rect) -> f32 {
    let dx = if point.x < rect.left() {
        rect.left() - point.x
    } else if point.x > rect.right() {
        point.x - rect.right()
    } else {
        0.0
    };
    let dy = if point.y < rect.top() {
        rect.top() - point.y
    } else if point.y > rect.bottom() {
        point.y - rect.bottom()
    } else {
        0.0
    };
    dx * dx + dy * dy
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum HitTier {
    /// Background / large-area elements clicked in interior padding or image area.
    BackgroundArea = 0,
    /// Interior fill of a closed shape (`fill_color.is_some()`).
    FilledShapeInterior = 1,
    /// Wide translucent highlighter stroke rendered behind ink and text.
    HighlighterStroke = 2,
    /// Direct hit on rendered text lines, ink strokes, or shape outlines.
    DirectForeground = 3,
}

#[derive(Clone, Copy, Debug)]
struct HitCandidate {
    object: SelectedObject,
    tier: HitTier,
    core_hit: bool,
    screen_area: f32,
    norm_dist: f32,
    paint_rank: (u8, usize),
}

fn is_better_hit(cand: &HitCandidate, best: &HitCandidate) -> bool {
    if cand.tier != best.tier {
        return cand.tier > best.tier;
    }
    if cand.tier == HitTier::DirectForeground && cand.core_hit != best.core_hit {
        return cand.core_hit;
    }
    let min_area = cand.screen_area.min(best.screen_area).max(1.0);
    let max_area = cand.screen_area.max(best.screen_area);
    if max_area > min_area * 1.5 {
        return cand.screen_area < best.screen_area;
    }
    if (cand.norm_dist - best.norm_dist).abs() > 0.15 {
        return cand.norm_dist < best.norm_dist;
    }
    cand.paint_rank > best.paint_rank
}

fn shape_broadphase_page_rect(shape: &GeometricShape) -> drawcraft_geom::Rect {
    let s = shape.sanitized();
    let min_x = s.x1.min(s.x2);
    let max_x = s.x1.max(s.x2);
    let min_y = s.y1.min(s.y2);
    let max_y = s.y1.max(s.y2);
    let cx = (min_x + max_x) * 0.5;
    let cy = (min_y + max_y) * 0.5;
    let hx = (max_x - min_x) * 0.5;
    let hy = (max_y - min_y) * 0.5;
    let (rx, ry) = if s.angle_degrees.abs() > f64::EPSILON {
        let angle = s.angle_degrees.to_radians();
        let (sin, cos) = angle.sin_cos();
        let ac = cos.abs();
        let as_ = sin.abs();
        (hx * ac + hy * as_, hx * as_ + hy * ac)
    } else {
        (hx, hy)
    };
    let arrow_pad = if s.kind == ShapeKind::Arrow {
        (s.x2 - s.x1).hypot(s.y2 - s.y1) * 0.35 + f64::from(s.width) * 2.5
    } else {
        0.0
    };
    let pad = f64::from(s.width) * 0.75 + 4.0 + arrow_pad;
    drawcraft_geom::Rect::new(
        cx - rx - pad,
        cy - ry - pad,
        (rx + pad) * 2.0,
        (ry + pad) * 2.0,
    )
}

fn shape_broadphase_screen_rect(shape: &GeometricShape, xf: Affine2) -> Option<Rect> {
    let b = shape_broadphase_page_rect(shape);
    let min_s = xf.to_screen(Point::new(b.x, b.y));
    let max_s = xf.to_screen(Point::new(b.right(), b.bottom()));
    let left = min_s.x.min(max_s.x) as f32;
    let top = min_s.y.min(max_s.y) as f32;
    let right = min_s.x.max(max_s.x) as f32;
    let bottom = min_s.y.max(max_s.y) as f32;
    if left.is_finite() && top.is_finite() && right.is_finite() && bottom.is_finite() {
        Some(Rect::from_min_max(pos2(left, top), pos2(right, bottom)))
    } else {
        None
    }
}

fn shape_hit_detail(
    index: usize,
    shape: &GeometricShape,
    pointer: Pos2,
    xf: Affine2,
    zoom: f32,
) -> Option<HitCandidate> {
    let shape = shape.sanitized();
    let safe_zoom = zoom.max(0.05);
    let stroke_w = (shape.width * safe_zoom).clamp(0.5, 80.0);
    let tolerance = 8.0 + (shape.width * safe_zoom * 0.5).clamp(0.5, 40.0);
    if let Some(broadphase) = shape_broadphase_screen_rect(&shape, xf)
        && !broadphase.expand(tolerance + 4.0).contains(pointer)
    {
        return None;
    }
    let tolerance_sq = f64::from(tolerance).powi(2);
    let core_tolerance = (stroke_w * 0.5 + 2.0).min(tolerance);
    let has_fill = shape.fill_color.is_some_and(|c| c.a > 0.0);

    let mut min_edge_dist_sq = f64::INFINITY;
    let mut in_fill = false;
    let mut total_len_px = 0.0_f32;
    for (points, closed) in shape.path_segments() {
        let screen_points: Vec<Pos2> = points
            .iter()
            .map(|point| {
                let screen = xf.to_screen(*point);
                pos2(screen.x as f32, screen.y as f32)
            })
            .filter(|point| point.x.is_finite() && point.y.is_finite())
            .collect();
        let (Some(first), Some(last)) = (
            screen_points.first().copied(),
            screen_points.last().copied(),
        ) else {
            continue;
        };
        if screen_points.len() == 1 {
            min_edge_dist_sq = min_edge_dist_sq.min(f64::from(first.distance_sq(pointer)));
        } else {
            for seg in screen_points.windows(2) {
                total_len_px += seg[0].distance(seg[1]);
                min_edge_dist_sq =
                    min_edge_dist_sq.min(distance_sq_to_segment(pointer, seg[0], seg[1]));
            }
            if closed {
                total_len_px += last.distance(first);
                min_edge_dist_sq =
                    min_edge_dist_sq.min(distance_sq_to_segment(pointer, last, first));
            }
        }
        if has_fill
            && closed
            && screen_points.len() >= 3
            && point_in_polygon(pointer, &screen_points)
        {
            in_fill = true;
        }
    }

    let screen_area = if matches!(shape.kind, ShapeKind::Line | ShapeKind::Arrow) {
        (total_len_px * stroke_w.max(6.0)).max(1.0)
    } else {
        ((shape.x2 - shape.x1).abs() as f32 * safe_zoom).max(1.0)
            * ((shape.y2 - shape.y1).abs() as f32 * safe_zoom).max(1.0)
    };

    if min_edge_dist_sq <= tolerance_sq {
        let dist = min_edge_dist_sq.sqrt() as f32;
        return Some(HitCandidate {
            object: SelectedObject::Shape(index),
            tier: HitTier::DirectForeground,
            core_hit: dist <= core_tolerance,
            screen_area,
            norm_dist: (dist / tolerance).clamp(0.0, 1.0),
            paint_rank: (4, index),
        });
    }
    if in_fill {
        return Some(HitCandidate {
            object: SelectedObject::Shape(index),
            tier: HitTier::FilledShapeInterior,
            core_hit: true,
            screen_area,
            norm_dist: 0.5,
            paint_rank: (4, index),
        });
    }
    None
}

fn text_hit_detail(
    painter: &egui::Painter,
    doc_width: u32,
    index: usize,
    item: &TextItem,
    screen_pos: Pos2,
    xf: Affine2,
    zoom: f32,
) -> Option<HitCandidate> {
    let clean = item.sanitized();
    let rough_screen = rough_page_rect_to_screen(text_item_rough_page_rect(doc_width, &clean), xf);
    if !rough_screen.expand(16.0).contains(screen_pos) {
        return None;
    }
    let galley = layout_text_item_galley(painter, doc_width, &clean, zoom, Color32::WHITE, None);
    let geom = text_selection_geometry_from_galley(&clean, xf, zoom, &galley);
    if !geom.contains_screen_pos(screen_pos, 4.0) {
        return None;
    }
    let aligned_local = (geom.rot.inverse() * (screen_pos - geom.origin)).to_pos2()
        + Vec2::new(galley.rect.left(), 0.0);
    let mut min_row_dist_sq = f32::INFINITY;
    for placed in &galley.rows {
        let r = placed.rect();
        if r.width() > 0.5 || !placed.row.glyphs.is_empty() {
            min_row_dist_sq = min_row_dist_sq.min(rect_distance_sq(aligned_local, r));
        }
    }
    for (_para_idx, _kind, prefix_rect) in collect_list_prefix_rects(&galley, &clean.text) {
        min_row_dist_sq = min_row_dist_sq.min(rect_distance_sq(aligned_local, prefix_rect));
    }
    let box_area = (geom.box_size.x.max(8.0) * geom.box_size.y.max(8.0)).max(1.0);
    let content_area = (galley.size().x.max(8.0) * galley.size().y.max(8.0)).max(1.0);
    let (tier, core_hit, norm_dist, screen_area) = if !min_row_dist_sq.is_finite() {
        (HitTier::DirectForeground, true, 0.25, box_area)
    } else if min_row_dist_sq <= 16.0 {
        let dist = min_row_dist_sq.sqrt();
        (
            HitTier::DirectForeground,
            dist <= 0.5,
            (dist / 4.0).clamp(0.0, 1.0),
            content_area,
        )
    } else {
        (HitTier::BackgroundArea, false, 0.85, box_area)
    };
    Some(HitCandidate {
        object: SelectedObject::Text(index),
        tier,
        core_hit,
        screen_area,
        norm_dist,
        paint_rank: (3, index),
    })
}

fn stroke_broadphase_screen_rect(stroke: &InkStroke, xf: Affine2) -> Option<Rect> {
    let b = valid_stroke_page_bounds(stroke)?;
    let min_s = xf.to_screen(Point::new(b.x, b.y));
    let max_s = xf.to_screen(Point::new(b.right(), b.bottom()));
    let left = min_s.x.min(max_s.x) as f32;
    let top = min_s.y.min(max_s.y) as f32;
    let right = min_s.x.max(max_s.x) as f32;
    let bottom = min_s.y.max(max_s.y) as f32;
    if left.is_finite() && top.is_finite() && right.is_finite() && bottom.is_finite() {
        Some(Rect::from_min_max(pos2(left, top), pos2(right, bottom)))
    } else {
        None
    }
}

fn stroke_hit_detail(
    index: usize,
    stroke: &InkStroke,
    pointer: Pos2,
    xf: Affine2,
    zoom: f32,
) -> Option<HitCandidate> {
    let safe_zoom = zoom.max(0.05);
    let stroke_px = (stroke.pen.size * safe_zoom).clamp(1.0, 96.0);
    let tolerance = 7.0 + (stroke.pen.size * safe_zoom * 0.5).clamp(0.5, 48.0);
    if let Some(broadphase) = stroke_broadphase_screen_rect(stroke, xf)
        && !broadphase.expand(tolerance + 4.0).contains(pointer)
    {
        return None;
    }
    let tolerance_sq = f64::from(tolerance).powi(2);
    let core_tolerance = (stroke_px * 0.5 + 2.0).min(tolerance);

    let mut min_dist_sq = f64::INFINITY;
    let mut total_len_px = 0.0_f32;
    let mut previous: Option<Pos2> = None;
    for sample in stroke.smoothed_points.iter() {
        let screen = xf.to_screen(sample.point());
        let point = pos2(screen.x as f32, screen.y as f32);
        if !point.x.is_finite() || !point.y.is_finite() {
            continue;
        }
        if let Some(start) = previous {
            total_len_px += start.distance(point);
            min_dist_sq = min_dist_sq.min(distance_sq_to_segment(pointer, start, point));
        }
        previous = Some(point);
    }
    if total_len_px == 0.0
        && let Some(point) = previous
    {
        min_dist_sq = min_dist_sq.min(distance_sq_to_segment(pointer, point, point));
    }
    if min_dist_sq > tolerance_sq {
        return None;
    }
    let dist = min_dist_sq.sqrt() as f32;
    let is_hl = stroke.pen.kind == PenKind::Highlighter;
    Some(HitCandidate {
        object: SelectedObject::Stroke(stroke.id),
        tier: if is_hl {
            HitTier::HighlighterStroke
        } else {
            HitTier::DirectForeground
        },
        core_hit: dist <= core_tolerance,
        screen_area: (total_len_px.max(stroke_px) * stroke_px.max(6.0)).max(1.0),
        norm_dist: (dist / tolerance).clamp(0.0, 1.0),
        paint_rank: (if is_hl { 1 } else { 2 }, index),
    })
}

fn image_hit_detail(
    index: usize,
    image: &PageImage,
    screen_pos: Pos2,
    xf: Affine2,
    zoom: f32,
) -> Option<HitCandidate> {
    if !image_hit_test(image, screen_pos, xf) {
        return None;
    }
    let clean = image.sanitized();
    let safe_zoom = zoom.max(0.05);
    let screen_area =
        ((clean.width as f32 * safe_zoom).max(1.0)) * ((clean.height as f32 * safe_zoom).max(1.0));
    Some(HitCandidate {
        object: SelectedObject::Image(index),
        tier: HitTier::BackgroundArea,
        core_hit: true,
        screen_area,
        norm_dist: 0.5,
        paint_rank: (0, index),
    })
}

fn object_at_screen_position(
    app: &DrawCraftApp,
    painter: &egui::Painter,
    screen_pos: Pos2,
    xf: Affine2,
) -> Option<SelectedObject> {
    let mut best: Option<HitCandidate> = None;
    let mut consider = |cand: Option<HitCandidate>| {
        if let Some(cand) = cand
            && best.is_none_or(|cur| is_better_hit(&cand, &cur))
        {
            best = Some(cand);
        }
    };
    for (index, item) in app.session.doc.shapes.iter().enumerate() {
        consider(shape_hit_detail(index, item, screen_pos, xf, app.view.zoom));
    }
    for (index, item) in app.session.doc.texts.iter().enumerate() {
        consider(text_hit_detail(
            painter,
            app.session.doc.width,
            index,
            item,
            screen_pos,
            xf,
            app.view.zoom,
        ));
    }
    for (index, stroke) in app.session.doc.strokes.iter().enumerate() {
        consider(stroke_hit_detail(
            index,
            stroke,
            screen_pos,
            xf,
            app.view.zoom,
        ));
    }
    for (index, image) in app.session.doc.images.iter().enumerate() {
        consider(image_hit_detail(
            index,
            image,
            screen_pos,
            xf,
            app.view.zoom,
        ));
    }
    best.map(|cand| cand.object)
}

fn handle_selection_press(
    app: &mut DrawCraftApp,
    ui: &egui::Ui,
    resp: &egui::Response,
    pos: Pos2,
    modifiers: egui::Modifiers,
    touch_id: Option<egui::TouchId>,
) -> bool {
    if !can_start_on_canvas(ui, resp, pos) {
        return false;
    }
    let painter = ui.painter_at(resp.rect);
    let xf = page_affine(app);
    let page_pt = xf.to_page(Point::new(f64::from(pos.x), f64::from(pos.y)));
    if page_pt.x >= 0.0
        && page_pt.y >= 0.0
        && page_pt.x <= f64::from(app.session.doc.width)
        && page_pt.y <= f64::from(app.session.doc.height)
    {
        app.view.last_canvas_page_pos = Some([page_pt.x, page_pt.y]);
    }
    if touch_id.is_none()
        && modifiers.command
        && !modifiers.shift
        && selection_handle_for_app(app, &painter, xf, pos).is_none()
        && let Some(url) = link_at_screen_pos(app, &painter, pos, xf)
    {
        let _ = crate::open_external_url(app, ui.ctx(), &url);
        return false;
    }
    if touch_id.is_none()
        && !modifiers.command
        && !modifiers.shift
        && selection_handle_for_app(app, &painter, xf, pos).is_none()
        && let Some((index, paragraph_index)) = checkbox_at_screen_pos(app, &painter, pos, xf)
    {
        let _ = app.session.execute(
            "text.toggleCheckbox",
            &serde_json::json!({ "index": index, "paragraph_index": paragraph_index }),
        );
        return false;
    }
    if touch_id.is_none()
        && ui.input(|i| {
            i.pointer
                .button_double_clicked(egui::PointerButton::Primary)
        })
        && selection_handle_for_app(app, &painter, xf, pos).is_none()
        && let Some(SelectedObject::Text(index)) = object_at_screen_position(app, &painter, pos, xf)
        && let Some(item) = app.session.doc.texts.get(index).cloned()
    {
        app.view.selection_drag = None;
        start_editing_text_item(app, index, &item);
        return false;
    }
    begin_selection_drag(app, &painter, pos, modifiers, touch_id);
    true
}

fn handle_selection_input(
    app: &mut DrawCraftApp,
    ui: &egui::Ui,
    resp: &egui::Response,
    events: &[egui::Event],
) {
    let ui_constrain = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl);
    let mut active_constrain = ui_constrain;
    let mut saw_touch_start = false;
    for (index, event) in events.iter().enumerate() {
        match *event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            } => {
                active_constrain = ui_constrain || modifiers.shift || modifiers.ctrl;
                if pointer_press_superseded_by_following_touch(app, events, index, pos)
                    || (saw_touch_start
                        && app
                            .view
                            .selection_drag
                            .as_ref()
                            .is_some_and(|d| d.touch_id.is_some()))
                {
                    continue;
                }
                if !handle_selection_press(app, ui, resp, pos, modifiers, None) {
                    return;
                }
            }
            egui::Event::PointerMoved(pos) => {
                if app
                    .view
                    .selection_drag
                    .as_ref()
                    .is_some_and(|drag| drag.touch_id.is_none())
                {
                    update_selection_drag(app, pos, active_constrain);
                }
            }
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers,
            } if app
                .view
                .selection_drag
                .as_ref()
                .is_some_and(|drag| drag.touch_id.is_none()) =>
            {
                active_constrain = ui_constrain || modifiers.shift || modifiers.ctrl;
                let dragged_far = match app.view.selection_drag.as_ref().map(|d| &d.mode) {
                    Some(crate::SelectionDragMode::Lasso { dragged, .. }) => *dragged,
                    Some(crate::SelectionDragMode::Marquee { start, current, .. }) => {
                        pos2(start[0], start[1]).distance_sq(pos2(current[0], current[1])) >= 16.0
                    }
                    _ => false,
                };
                update_selection_drag(app, pos, active_constrain);
                let painter = ui.painter_at(resp.rect);
                let xf = page_affine(app);
                finish_selection_drag(app, &painter, xf);
                if !dragged_far
                    && ui.input(|i| {
                        i.pointer
                            .button_double_clicked(egui::PointerButton::Primary)
                    })
                    && selection_handle_for_app(app, &painter, xf, pos).is_none()
                    && let Some(SelectedObject::Text(index)) =
                        object_at_screen_position(app, &painter, pos, xf)
                    && let Some(item) = app.session.doc.texts.get(index).cloned()
                {
                    start_editing_text_item(app, index, &item);
                    return;
                }
            }
            egui::Event::PointerGone
                if app
                    .view
                    .selection_drag
                    .as_ref()
                    .is_some_and(|drag| drag.touch_id.is_none()) =>
            {
                finish_selection_drag(app, &ui.painter_at(resp.rect), page_affine(app));
            }
            egui::Event::Touch {
                id,
                phase: egui::TouchPhase::Start,
                pos,
                force,
                ..
            } => {
                saw_touch_start = true;
                if touch_start_claims_stroke(app, events, index, id, force)
                    && !handle_selection_press(app, ui, resp, pos, egui::Modifiers::NONE, Some(id))
                {
                    return;
                }
            }
            egui::Event::Touch {
                id,
                phase: egui::TouchPhase::Move,
                pos,
                ..
            } if app
                .view
                .selection_drag
                .as_ref()
                .is_some_and(|drag| drag.touch_id == Some(id)) =>
            {
                update_selection_drag(app, pos, active_constrain);
            }
            egui::Event::Touch {
                id,
                phase: egui::TouchPhase::End | egui::TouchPhase::Cancel,
                pos,
                ..
            } if app
                .view
                .selection_drag
                .as_ref()
                .is_some_and(|drag| drag.touch_id == Some(id)) =>
            {
                update_selection_drag(app, pos, active_constrain);
                finish_selection_drag(app, &ui.painter_at(resp.rect), page_affine(app));
            }
            _ => {}
        }
    }
    if let Some(crate::SelectionDragState {
        mode:
            crate::SelectionDragMode::Resize {
                last_pointer,
                last_constrained,
                ..
            },
        ..
    }) = app.view.selection_drag
        && active_constrain != last_constrained
    {
        update_selection_drag(
            app,
            pos2(last_pointer[0], last_pointer[1]),
            active_constrain,
        );
    }
}

fn bounds_from_page_points(points: impl IntoIterator<Item = Point>, xf: Affine2) -> Option<Rect> {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut count = 0_usize;
    for point in points {
        let screen = xf.to_screen(point);
        let x = screen.x as f32;
        let y = screen.y as f32;
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
        count = count.saturating_add(1);
    }
    (count > 0).then(|| Rect::from_min_max(pos2(min_x, min_y), pos2(max_x, max_y)))
}

fn union_screen_bounds(bounds: Option<Rect>, next: Rect) -> Option<Rect> {
    Some(match bounds {
        Some(current) => Rect::from_min_max(
            pos2(
                current.left().min(next.left()),
                current.top().min(next.top()),
            ),
            pos2(
                current.right().max(next.right()),
                current.bottom().max(next.bottom()),
            ),
        ),
        None => next,
    })
}

fn stroke_screen_bounds(stroke: &InkStroke, app: &DrawCraftApp, xf: Affine2) -> Option<Rect> {
    let bounds = bounds_from_page_points(
        stroke.smoothed_points.iter().map(|sample| sample.point()),
        xf,
    )?;
    Some(bounds.expand((stroke.pen.size * app.view.zoom * 0.5).clamp(1.0, 64.0)))
}

fn shape_screen_bounds(shape: &GeometricShape, app: &DrawCraftApp, xf: Affine2) -> Option<Rect> {
    let bounds = bounds_from_page_points(
        shape
            .path_segments()
            .into_iter()
            .flat_map(|(points, _)| points),
        xf,
    )?;
    Some(bounds.expand((shape.width * app.view.zoom * 0.5).clamp(1.0, 48.0)))
}

fn image_screen_bounds(image: &PageImage, xf: Affine2) -> Option<Rect> {
    bounds_from_page_points(image.corners(), xf)
}

fn image_hit_test(image: &PageImage, screen_pos: Pos2, xf: Affine2) -> bool {
    let clean = image.sanitized();
    let page_pt = xf.to_page(Point::new(f64::from(screen_pos.x), f64::from(screen_pos.y)));
    let center = clean.center();
    let dx = page_pt.x - center.x;
    let dy = page_pt.y - center.y;
    let (sin, cos) = (-clean.angle_degrees.to_radians()).sin_cos();
    let lx = dx * cos - dy * sin;
    let ly = dx * sin + dy * cos;
    lx.abs() <= clean.width * 0.5 && ly.abs() <= clean.height * 0.5
}

fn selected_object_bounds(
    app: &DrawCraftApp,
    painter: &egui::Painter,
    object: SelectedObject,
    xf: Affine2,
) -> Option<Rect> {
    match object {
        SelectedObject::Stroke(id) => {
            let stroke = app
                .session
                .doc
                .strokes
                .iter()
                .find(|stroke| stroke.id == id)?;
            stroke_screen_bounds(stroke, app, xf)
        }
        SelectedObject::Shape(index) => {
            let shape = app.session.doc.shapes.get(index)?.as_ref().sanitized();
            shape_screen_bounds(&shape, app, xf)
        }
        SelectedObject::Text(index) => {
            let item = app.session.doc.texts.get(index)?.as_ref().sanitized();
            Some(text_item_bounds(
                painter,
                app.session.doc.width,
                &item,
                xf,
                app.view.zoom,
            ))
        }
        SelectedObject::Image(index) => {
            let image = app.session.doc.images.get(index)?.as_ref().sanitized();
            image_screen_bounds(&image, xf)
        }
    }
}

fn selected_objects_bounds(
    app: &DrawCraftApp,
    painter: &egui::Painter,
    xf: Affine2,
) -> Option<Rect> {
    let mut bounds: Option<Rect> = None;
    if app.view.selected_objects.is_empty() {
        return app
            .view
            .selected_object
            .and_then(|object| selected_object_bounds(app, painter, object, xf));
    }
    let mut stroke_ids = HashSet::new();
    for &object in &app.view.selected_objects {
        match object {
            SelectedObject::Stroke(id) => {
                stroke_ids.insert(id);
            }
            SelectedObject::Shape(index) => {
                if let Some(shape) = app.session.doc.shapes.get(index)
                    && let Some(next) = shape_screen_bounds(&shape.as_ref().sanitized(), app, xf)
                {
                    bounds = union_screen_bounds(bounds, next);
                }
            }
            SelectedObject::Text(index) => {
                if let Some(item) = app.session.doc.texts.get(index) {
                    let next = text_item_bounds(
                        painter,
                        app.session.doc.width,
                        &item.as_ref().sanitized(),
                        xf,
                        app.view.zoom,
                    );
                    bounds = union_screen_bounds(bounds, next);
                }
            }
            SelectedObject::Image(index) => {
                if let Some(image) = app.session.doc.images.get(index)
                    && let Some(next) = image_screen_bounds(&image.as_ref().sanitized(), xf)
                {
                    bounds = union_screen_bounds(bounds, next);
                }
            }
        }
    }
    for stroke in &app.session.doc.strokes {
        if stroke_ids.contains(&stroke.id)
            && let Some(next) = stroke_screen_bounds(stroke, app, xf)
        {
            bounds = union_screen_bounds(bounds, next);
        }
    }
    bounds
}

const SELECTION_FRAME_PAD: f32 = 6.0;
const SELECTION_CORNER_HANDLE_SIZE: f32 = 9.0;
const SELECTION_CORNER_HANDLE_OFFSET: f32 = 10.5;
const SELECTION_SIDE_HANDLE_THICKNESS: f32 = 5.5;
const SELECTION_SIDE_HANDLE_OFFSET: f32 = 8.5;
const SELECTION_ROTATE_HANDLE_OFFSET: f32 = 34.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SelectionHandleVisual {
    Corner(usize),
    Side(usize),
    Rotate,
}

#[derive(Clone, Copy)]
struct HandleStyle<'a> {
    angle: f32,
    rot: egui::emath::Rot2,
    t: &'a Tokens,
    active: bool,
}

fn non_text_corner_handles(bounds: Rect) -> [Pos2; 4] {
    let outer = bounds.expand(SELECTION_CORNER_HANDLE_OFFSET);
    [
        outer.left_top(),
        outer.right_top(),
        outer.left_bottom(),
        outer.right_bottom(),
    ]
}

fn non_text_side_handles(bounds: Rect) -> [(Pos2, Vec2); 4] {
    let pill_h = (bounds.height() - 4.0).clamp(8.0, 16.0);
    let pill_w = (bounds.width() - 4.0).clamp(8.0, 16.0);
    let off = SELECTION_SIDE_HANDLE_OFFSET;
    let th = SELECTION_SIDE_HANDLE_THICKNESS;
    [
        (
            pos2(bounds.left() - off, bounds.center().y),
            Vec2::new(th, pill_h),
        ),
        (
            pos2(bounds.right() + off, bounds.center().y),
            Vec2::new(th, pill_h),
        ),
        (
            pos2(bounds.center().x, bounds.top() - off),
            Vec2::new(pill_w, th),
        ),
        (
            pos2(bounds.center().x, bounds.bottom() + off),
            Vec2::new(pill_w, th),
        ),
    ]
}

fn rotation_handle_center(bounds: Rect) -> Pos2 {
    let y = if bounds.top() >= 42.0 {
        bounds.top() - SELECTION_ROTATE_HANDLE_OFFSET
    } else {
        bounds.bottom() + SELECTION_ROTATE_HANDLE_OFFSET
    };
    pos2(bounds.center().x, y)
}

fn non_text_rotation_handle_center(bounds: Rect) -> Pos2 {
    rotation_handle_center(bounds)
}

fn resize_cursor_for_angle(angle_degrees: f64) -> egui::CursorIcon {
    let a = angle_degrees.rem_euclid(180.0);
    if !(22.5..157.5).contains(&a) {
        egui::CursorIcon::ResizeHorizontal
    } else if a < 67.5 {
        egui::CursorIcon::ResizeNwSe
    } else if a < 112.5 {
        egui::CursorIcon::ResizeVertical
    } else {
        egui::CursorIcon::ResizeNeSw
    }
}

fn rotated_box_points(center: Pos2, size: Vec2, rot: egui::emath::Rot2) -> Vec<Pos2> {
    let hw = size.x * 0.5;
    let hh = size.y * 0.5;
    [
        center + rot * Vec2::new(-hw, -hh),
        center + rot * Vec2::new(hw, -hh),
        center + rot * Vec2::new(hw, hh),
        center + rot * Vec2::new(-hw, hh),
    ]
    .to_vec()
}

fn paint_corner_handle(painter: &egui::Painter, center: Pos2, style: HandleStyle<'_>) {
    let size_px = if style.active {
        SELECTION_CORNER_HANDLE_SIZE + 1.5
    } else {
        SELECTION_CORNER_HANDLE_SIZE
    };
    let fill = if style.active {
        style.t.accent_soft
    } else {
        style.t.field_bg
    };
    let stroke_w = if style.active { 1.7 } else { 1.35 };
    let halo = style.t.panel_bg.gamma_multiply(0.65);
    if style.angle.abs() <= f32::EPSILON {
        let handle = Rect::from_center_size(center, Vec2::splat(size_px));
        painter.rect_stroke(
            handle.expand(0.9),
            CornerRadius::same(3),
            Stroke::new(1.5, halo),
            StrokeKind::Inside,
        );
        painter.rect_filled(handle, CornerRadius::same(2), fill);
        painter.rect_stroke(
            handle,
            CornerRadius::same(2),
            Stroke::new(stroke_w, style.t.accent),
            StrokeKind::Inside,
        );
    } else {
        painter.add(egui::Shape::convex_polygon(
            rotated_box_points(center, Vec2::splat(size_px + 1.8), style.rot),
            Color32::TRANSPARENT,
            Stroke::new(1.5, halo),
        ));
        painter.add(egui::Shape::convex_polygon(
            rotated_box_points(center, Vec2::splat(size_px), style.rot),
            fill,
            Stroke::new(stroke_w, style.t.accent),
        ));
    }
}

fn paint_edge_pill_handle(
    painter: &egui::Painter,
    center: Pos2,
    size: Vec2,
    style: HandleStyle<'_>,
) {
    let adj_size = if style.active {
        size + Vec2::splat(1.2)
    } else {
        size
    };
    let fill = if style.active {
        style.t.accent_soft
    } else {
        style.t.field_bg
    };
    let stroke_w = if style.active { 1.7 } else { 1.35 };
    let halo = style.t.panel_bg.gamma_multiply(0.65);
    if style.angle.abs() <= f32::EPSILON {
        let handle = Rect::from_center_size(center, adj_size);
        painter.rect_stroke(
            handle.expand(0.9),
            CornerRadius::same(4),
            Stroke::new(1.5, halo),
            StrokeKind::Inside,
        );
        painter.rect_filled(handle, CornerRadius::same(3), fill);
        painter.rect_stroke(
            handle,
            CornerRadius::same(3),
            Stroke::new(stroke_w, style.t.accent),
            StrokeKind::Inside,
        );
    } else {
        painter.add(egui::Shape::convex_polygon(
            rotated_box_points(center, adj_size + Vec2::splat(1.6), style.rot),
            Color32::TRANSPARENT,
            Stroke::new(1.5, halo),
        ));
        painter.add(egui::Shape::convex_polygon(
            rotated_box_points(center, adj_size, style.rot),
            fill,
            Stroke::new(stroke_w, style.t.accent),
        ));
    }
}

fn paint_width_pill_handle(
    painter: &egui::Painter,
    center: Pos2,
    box_height: f32,
    style: HandleStyle<'_>,
) {
    let pill_len = (box_height - 4.0).clamp(8.0, 16.0);
    paint_edge_pill_handle(
        painter,
        center,
        Vec2::new(SELECTION_SIDE_HANDLE_THICKNESS, pill_len),
        style,
    );
}

fn paint_rotation_handle(
    painter: &egui::Painter,
    anchor: Pos2,
    handle: Pos2,
    t: &Tokens,
    active: bool,
) {
    let halo = t.panel_bg.gamma_multiply(0.65);
    let radius = if active { 6.6 } else { 5.8 };
    let fill = if active { t.accent_soft } else { t.field_bg };
    let stroke_w = if active { 1.8 } else { 1.4 };
    painter.line_segment([anchor, handle], Stroke::new(2.4, halo));
    painter.line_segment([anchor, handle], Stroke::new(1.3, t.accent));
    painter.circle_stroke(handle, radius + 0.9, Stroke::new(1.5, halo));
    painter.circle_filled(handle, radius, fill);
    painter.circle_stroke(handle, radius, Stroke::new(stroke_w, t.accent));
    painter.circle_filled(handle, 1.9, t.accent);
}

fn paint_selection_handles(
    painter: &egui::Painter,
    bounds: Rect,
    t: &Tokens,
    active_handle: Option<SelectionHandleVisual>,
) {
    let identity = egui::emath::Rot2::IDENTITY;
    for (idx, corner) in non_text_corner_handles(bounds).into_iter().enumerate() {
        paint_corner_handle(
            painter,
            corner,
            HandleStyle {
                angle: 0.0,
                rot: identity,
                t,
                active: active_handle == Some(SelectionHandleVisual::Corner(idx)),
            },
        );
    }
    for (idx, (center, size)) in non_text_side_handles(bounds).into_iter().enumerate() {
        paint_edge_pill_handle(
            painter,
            center,
            size,
            HandleStyle {
                angle: 0.0,
                rot: identity,
                t,
                active: active_handle == Some(SelectionHandleVisual::Side(idx)),
            },
        );
    }
    let rotate_handle = non_text_rotation_handle_center(bounds);
    let stem_pad = SELECTION_SIDE_HANDLE_OFFSET + SELECTION_SIDE_HANDLE_THICKNESS * 0.5;
    let rotate_anchor = if rotate_handle.y < bounds.top() {
        pos2(bounds.center().x, bounds.top() - stem_pad)
    } else {
        pos2(bounds.center().x, bounds.bottom() + stem_pad)
    };
    paint_rotation_handle(
        painter,
        rotate_anchor,
        rotate_handle,
        t,
        active_handle == Some(SelectionHandleVisual::Rotate),
    );
}

fn paint_text_selection_handles(
    painter: &egui::Painter,
    geom: &TextSelectionGeometry,
    t: &Tokens,
    active_handle: Option<SelectionHandleVisual>,
) {
    for (idx, corner) in geom.handle_corners.into_iter().enumerate() {
        paint_corner_handle(
            painter,
            corner,
            HandleStyle {
                angle: geom.angle,
                rot: geom.rot,
                t,
                active: active_handle == Some(SelectionHandleVisual::Corner(idx)),
            },
        );
    }
    paint_width_pill_handle(
        painter,
        geom.left_mid,
        geom.box_size.y,
        HandleStyle {
            angle: geom.angle,
            rot: geom.rot,
            t,
            active: active_handle == Some(SelectionHandleVisual::Side(0)),
        },
    );
    paint_width_pill_handle(
        painter,
        geom.right_mid,
        geom.box_size.y,
        HandleStyle {
            angle: geom.angle,
            rot: geom.rot,
            t,
            active: active_handle == Some(SelectionHandleVisual::Side(1)),
        },
    );
    paint_rotation_handle(
        painter,
        geom.rotate_anchor,
        geom.rotate_handle,
        t,
        active_handle == Some(SelectionHandleVisual::Rotate),
    );
}

enum SelectionHandle {
    Rotate {
        pivot: Pos2,
    },
    Resize {
        pivot: Pos2,
        sign: [f32; 2],
        resize_x: bool,
        resize_y: bool,
        proportional: bool,
        cursor: egui::CursorIcon,
    },
    ResizeTextWidth {
        index: usize,
        from_left: bool,
        cursor: egui::CursorIcon,
    },
}

fn selected_single_text(app: &DrawCraftApp) -> Option<(usize, TextItem)> {
    let index = if app.view.selected_objects.len() == 1 {
        match app.view.selected_objects[0] {
            SelectedObject::Text(index) => Some(index),
            _ => None,
        }
    } else if app.view.selected_objects.is_empty() {
        match app.view.selected_object {
            Some(SelectedObject::Text(index)) => Some(index),
            _ => None,
        }
    } else {
        None
    }?;
    app.session
        .doc
        .texts
        .get(index)
        .map(|item| (index, item.as_ref().sanitized()))
}

fn pick_closest_handle(
    best: &mut Option<(f32, SelectionHandleVisual, SelectionHandle)>,
    dist_sq: f32,
    visual: SelectionHandleVisual,
    handle: SelectionHandle,
) {
    if dist_sq <= 100.0 && best.as_ref().is_none_or(|(d, _, _)| dist_sq < *d) {
        *best = Some((dist_sq, visual, handle));
    }
}

fn non_text_handle_hit(
    bounds: Rect,
    pos: Pos2,
) -> Option<(SelectionHandleVisual, SelectionHandle)> {
    let rotate_handle = non_text_rotation_handle_center(bounds);
    if rotate_handle.distance_sq(pos) <= 100.0 {
        return Some((
            SelectionHandleVisual::Rotate,
            SelectionHandle::Rotate {
                pivot: bounds.center(),
            },
        ));
    }
    let mut best: Option<(f32, SelectionHandleVisual, SelectionHandle)> = None;
    let handle_corners = non_text_corner_handles(bounds);
    let corner_specs = [
        (
            bounds.right_bottom(),
            [-1.0, -1.0],
            egui::CursorIcon::ResizeNwSe,
        ),
        (
            bounds.left_bottom(),
            [1.0, -1.0],
            egui::CursorIcon::ResizeNeSw,
        ),
        (
            bounds.right_top(),
            [-1.0, 1.0],
            egui::CursorIcon::ResizeNeSw,
        ),
        (bounds.left_top(), [1.0, 1.0], egui::CursorIcon::ResizeNwSe),
    ];
    for (idx, (&corner, (pivot, sign, cursor))) in
        handle_corners.iter().zip(corner_specs).enumerate()
    {
        pick_closest_handle(
            &mut best,
            corner.distance_sq(pos),
            SelectionHandleVisual::Corner(idx),
            SelectionHandle::Resize {
                pivot,
                sign,
                resize_x: true,
                resize_y: true,
                proportional: false,
                cursor,
            },
        );
    }
    let sides = non_text_side_handles(bounds);
    let side_pivots = [
        (
            pos2(bounds.right(), bounds.center().y),
            [-1.0, 0.0],
            true,
            false,
            egui::CursorIcon::ResizeHorizontal,
        ),
        (
            pos2(bounds.left(), bounds.center().y),
            [1.0, 0.0],
            true,
            false,
            egui::CursorIcon::ResizeHorizontal,
        ),
        (
            pos2(bounds.center().x, bounds.bottom()),
            [0.0, -1.0],
            false,
            true,
            egui::CursorIcon::ResizeVertical,
        ),
        (
            pos2(bounds.center().x, bounds.top()),
            [0.0, 1.0],
            false,
            true,
            egui::CursorIcon::ResizeVertical,
        ),
    ];
    for (idx, ((handle_pos, _), (pivot, sign, resize_x, resize_y, cursor))) in
        sides.into_iter().zip(side_pivots).enumerate()
    {
        pick_closest_handle(
            &mut best,
            handle_pos.distance_sq(pos),
            SelectionHandleVisual::Side(idx),
            SelectionHandle::Resize {
                pivot,
                sign,
                resize_x,
                resize_y,
                proportional: false,
                cursor,
            },
        );
    }
    best.map(|(_, visual, handle)| (visual, handle))
}

fn text_handle_hit(
    index: usize,
    geom: &TextSelectionGeometry,
    pos: Pos2,
) -> Option<(SelectionHandleVisual, SelectionHandle)> {
    if geom.rotate_handle.distance_sq(pos) <= 100.0 {
        return Some((
            SelectionHandleVisual::Rotate,
            SelectionHandle::Rotate { pivot: geom.center },
        ));
    }
    let corner_specs = [
        (geom.corners[3], [-1.0, -1.0]),
        (geom.corners[2], [1.0, -1.0]),
        (geom.corners[1], [-1.0, 1.0]),
        (geom.corners[0], [1.0, 1.0]),
    ];
    let mut best: Option<(f32, SelectionHandleVisual, SelectionHandle)> = None;
    for (corner_idx, (&corner, (pivot, sign))) in
        geom.handle_corners.iter().zip(corner_specs).enumerate()
    {
        let diagonal_deg = if corner_idx == 0 || corner_idx == 3 {
            geom.angle_degrees + 45.0
        } else {
            geom.angle_degrees + 135.0
        };
        pick_closest_handle(
            &mut best,
            corner.distance_sq(pos),
            SelectionHandleVisual::Corner(corner_idx),
            SelectionHandle::Resize {
                pivot,
                sign,
                resize_x: true,
                resize_y: true,
                proportional: true,
                cursor: resize_cursor_for_angle(diagonal_deg),
            },
        );
    }
    let width_cursor = resize_cursor_for_angle(geom.angle_degrees);
    for (idx, (mid, from_left)) in [(geom.left_mid, true), (geom.right_mid, false)]
        .into_iter()
        .enumerate()
    {
        pick_closest_handle(
            &mut best,
            mid.distance_sq(pos),
            SelectionHandleVisual::Side(idx),
            SelectionHandle::ResizeTextWidth {
                index,
                from_left,
                cursor: width_cursor,
            },
        );
    }
    best.map(|(_, visual, handle)| (visual, handle))
}

fn selection_handle_for_app(
    app: &DrawCraftApp,
    painter: &egui::Painter,
    xf: Affine2,
    pos: Pos2,
) -> Option<SelectionHandle> {
    if let Some((index, item)) = selected_single_text(app) {
        let geom =
            text_selection_geometry(painter, app.session.doc.width, &item, xf, app.view.zoom);
        return text_handle_hit(index, &geom, pos).map(|(_, h)| h);
    }
    selected_objects_bounds(app, painter, xf)
        .and_then(|bounds| non_text_handle_hit(bounds, pos).map(|(_, h)| h))
}

fn set_selection(app: &mut DrawCraftApp, objects: Vec<SelectedObject>) {
    let mut unique = Vec::with_capacity(objects.len());
    let mut seen = HashSet::with_capacity(objects.len());
    for object in objects {
        if seen.insert(object) {
            unique.push(object);
        }
    }
    app.view.selected_object = if unique.len() == 1 {
        unique.first().copied()
    } else {
        None
    };
    app.view.selected_text = match app.view.selected_object {
        Some(SelectedObject::Text(index)) => Some(index),
        _ => None,
    };
    app.view.selected_objects = unique;
    app.view.text_style_open = false;
    app.view.text_insert_error = None;
}

fn begin_selection_drag(
    app: &mut DrawCraftApp,
    painter: &egui::Painter,
    pos: Pos2,
    modifiers: egui::Modifiers,
    touch_id: Option<egui::TouchId>,
) {
    let xf = page_affine(app);
    let bounds = selected_objects_bounds(app, painter, xf);
    if let Some(handle) = selection_handle_for_app(app, painter, xf, pos) {
        let mode = match handle {
            SelectionHandle::Rotate { pivot } => crate::SelectionDragMode::Rotate {
                pivot: [pivot.x, pivot.y],
                last_angle: (pos.y - pivot.y).atan2(pos.x - pivot.x),
                record_history: true,
            },
            SelectionHandle::Resize {
                pivot,
                sign,
                resize_x,
                resize_y,
                proportional,
                cursor,
            } => crate::SelectionDragMode::Resize {
                pivot: [pivot.x, pivot.y],
                start_last: [pos.x, pos.y],
                last_pointer: [pos.x, pos.y],
                applied_scale: [1.0, 1.0],
                last_constrained: proportional
                    || (resize_x && resize_y && (modifiers.shift || modifiers.ctrl)),
                sign,
                resize_x,
                resize_y,
                proportional,
                cursor,
                record_history: true,
            },
            SelectionHandle::ResizeTextWidth {
                index, from_left, ..
            } => {
                let Some(item) = app
                    .session
                    .doc
                    .texts
                    .get(index)
                    .map(|item| item.as_ref().sanitized())
                else {
                    return;
                };
                let geom = text_selection_geometry(
                    painter,
                    app.session.doc.width,
                    &item,
                    xf,
                    app.view.zoom,
                );
                let pointer_page = xf.to_page(Point::new(f64::from(pos.x), f64::from(pos.y)));
                crate::SelectionDragMode::ResizeTextWidth {
                    index,
                    from_left,
                    start_pointer_page: [pointer_page.x, pointer_page.y],
                    start_origin: [item.x, item.y],
                    start_width: geom.content_width_page,
                    angle_degrees: item.angle_degrees,
                    record_history: true,
                }
            }
        };
        app.view.selection_drag = Some(crate::SelectionDragState { mode, touch_id });
        return;
    }

    let hit_object = object_at_screen_position(app, painter, pos, xf);
    if app.view.freeform_lasso_mode {
        let hit_selected = hit_object.is_some_and(|obj| app.view.selected_objects.contains(&obj));
        let single_unfilled_shape = app.view.selected_objects.len() == 1
            && matches!(
                app.view.selected_objects.first(),
                Some(SelectedObject::Shape(idx))
                    if app
                        .session
                        .doc
                        .shapes
                        .get(*idx)
                        .is_some_and(|s| s.fill_color.is_none_or(|c| c.a <= 0.0))
            );
        let inside_move_frame = !app.view.selected_objects.is_empty()
            && !single_unfilled_shape
            && hit_object.is_none_or(|obj| matches!(obj, SelectedObject::Image(_)) && !matches!(app.view.selected_objects.first(), Some(SelectedObject::Image(_)) if app.view.selected_objects.len() == 1))
            && (selected_single_text(app).is_some_and(|(_, item)| {
                text_selection_geometry(painter, app.session.doc.width, &item, xf, app.view.zoom)
                    .contains_screen_pos(pos, SELECTION_FRAME_PAD)
            }) || bounds.is_some_and(|bounds| bounds.expand(SELECTION_FRAME_PAD).contains(pos)));
        if !modifiers.shift && (hit_selected || inside_move_frame) {
            app.view.selection_drag = Some(crate::SelectionDragState {
                mode: crate::SelectionDragMode::Move {
                    last: [pos.x, pos.y],
                    record_history: true,
                },
                touch_id,
            });
            return;
        }
        app.view.selection_drag = Some(crate::SelectionDragState {
            mode: crate::SelectionDragMode::Lasso {
                points: vec![[pos.x, pos.y]],
                additive: modifiers.shift,
                dragged: false,
            },
            touch_id,
        });
        return;
    }

    if let Some(object) = hit_object {
        if modifiers.shift {
            let mut selection = app.view.selected_objects.clone();
            if let Some(existing) = selection.iter().position(|selected| *selected == object) {
                selection.remove(existing);
                set_selection(app, selection);
                app.view.selection_drag = None;
                return;
            }
            selection.push(object);
            set_selection(app, selection);
        } else if !app.view.selected_objects.contains(&object) {
            set_selection(app, vec![object]);
        }
        app.view.selection_drag = Some(crate::SelectionDragState {
            mode: crate::SelectionDragMode::Move {
                last: [pos.x, pos.y],
                record_history: true,
            },
            touch_id,
        });
        return;
    }

    let inside_selection = selected_single_text(app).is_some_and(|(_, item)| {
        text_selection_geometry(painter, app.session.doc.width, &item, xf, app.view.zoom)
            .contains_screen_pos(pos, SELECTION_FRAME_PAD)
    }) || bounds
        .is_some_and(|bounds| bounds.expand(SELECTION_FRAME_PAD).contains(pos));
    if inside_selection {
        app.view.selection_drag = Some(crate::SelectionDragState {
            mode: crate::SelectionDragMode::Move {
                last: [pos.x, pos.y],
                record_history: true,
            },
            touch_id,
        });
        return;
    }

    if !modifiers.shift {
        set_selection(app, Vec::new());
    }
    app.view.selection_drag = Some(crate::SelectionDragState {
        mode: crate::SelectionDragMode::Marquee {
            start: [pos.x, pos.y],
            current: [pos.x, pos.y],
            additive: modifiers.shift,
        },
        touch_id,
    });
}

fn selection_targets(app: &DrawCraftApp) -> Vec<serde_json::Value> {
    app.view
        .selected_objects
        .iter()
        .map(|object| match object {
            SelectedObject::Stroke(id) => serde_json::json!({ "kind": "stroke", "id": id.0 }),
            SelectedObject::Shape(index) => {
                serde_json::json!({ "kind": "shape", "index": index })
            }
            SelectedObject::Text(index) => {
                serde_json::json!({ "kind": "text", "index": index })
            }
            SelectedObject::Image(index) => {
                serde_json::json!({ "kind": "image", "index": index })
            }
        })
        .collect()
}

fn apply_selection_transform_axes(
    app: &mut DrawCraftApp,
    pivot: Point,
    translation: Point,
    scale_xy: (f64, f64),
    rotation_degrees: f64,
    record_history: bool,
) -> bool {
    let targets = selection_targets(app);
    if targets.is_empty() {
        return false;
    }
    app.session
        .execute(
            "selection.transform",
            &serde_json::json!({
                "targets": targets,
                "pivot": pivot,
                "translation": translation,
                "scale_x": scale_xy.0,
                "scale_y": scale_xy.1,
                "rotation_degrees": rotation_degrees,
                "record_history": record_history,
            }),
        )
        .is_ok()
}

fn apply_selection_transform(
    app: &mut DrawCraftApp,
    pivot: Point,
    translation: Point,
    scale: f64,
    rotation_degrees: f64,
    record_history: bool,
) -> bool {
    apply_selection_transform_axes(
        app,
        pivot,
        translation,
        (scale, scale),
        rotation_degrees,
        record_history,
    )
}

fn selection_resize_floor(app: &DrawCraftApp) -> (f32, bool) {
    let has_image = app
        .view
        .selected_objects
        .iter()
        .any(|obj| matches!(obj, SelectedObject::Image(_)))
        || (app.view.selected_objects.is_empty()
            && matches!(app.view.selected_object, Some(SelectedObject::Image(_))));
    if has_image {
        ((9.0 * app.view.zoom).clamp(4.0, 64.0), true)
    } else {
        (4.0, false)
    }
}

fn compute_axis_resize(pivot: f32, last: f32, target: f32, sign: f32, min_ext: f32) -> (f64, f32) {
    let prev_ext = f64::from(sign) * f64::from(last - pivot);
    if prev_ext <= 0.5 {
        return (1.0, last);
    }
    let curr_ext = f64::from(sign) * f64::from(target - pivot);
    let eff_min = f64::from(min_ext).min(prev_ext);
    let scale = (curr_ext.max(eff_min) / prev_ext).clamp(0.05, 20.0);
    let next_last = pivot + sign * (prev_ext * scale) as f32;
    (scale, next_last)
}

fn compute_proportional_resize(
    pivot: [f32; 2],
    last: [f32; 2],
    pos: Pos2,
    min_ext: f32,
    strict_axis_floor: bool,
) -> Option<(f64, f64, [f32; 2])> {
    let vx = f64::from(last[0] - pivot[0]);
    let vy = f64::from(last[1] - pivot[1]);
    let len_sq = vx * vx + vy * vy;
    if len_sq <= 1.0 {
        return None;
    }
    let px = f64::from(pos.x - pivot[0]);
    let py = f64::from(pos.y - pivot[1]);
    let proj = (px * vx + py * vy) / len_sq;
    if !proj.is_finite() {
        return None;
    }
    let mut min_scale = (f64::from(min_ext) / len_sq.sqrt()).clamp(0.05, 1.0);
    if strict_axis_floor {
        let min_x = f64::from(min_ext).min(vx.abs().max(0.5)) / vx.abs().max(1.0);
        let min_y = f64::from(min_ext).min(vy.abs().max(0.5)) / vy.abs().max(1.0);
        min_scale = min_scale.max(min_x).max(min_y).clamp(0.05, 1.0);
    }
    let scale = proj.clamp(min_scale, 20.0);
    let next_last = [
        pivot[0] + (vx * scale) as f32,
        pivot[1] + (vy * scale) as f32,
    ];
    Some((scale, scale, next_last))
}

fn update_selection_drag(app: &mut DrawCraftApp, pos: Pos2, constrain_held: bool) {
    if !pos.x.is_finite() || !pos.y.is_finite() {
        return;
    }
    let (min_ext, strict_axis_floor) = selection_resize_floor(app);
    let Some(mut drag) = app.view.selection_drag.take() else {
        return;
    };
    let xf = page_affine(app);
    match &mut drag.mode {
        crate::SelectionDragMode::Marquee { current, .. } => *current = [pos.x, pos.y],
        crate::SelectionDragMode::Lasso {
            points, dragged, ..
        } => {
            if let Some(&first) = points.first() {
                let dx0 = pos.x - first[0];
                let dy0 = pos.y - first[1];
                if dx0 * dx0 + dy0 * dy0 >= 16.0 {
                    *dragged = true;
                }
            }
            let should_push = points.last().is_none_or(|prev| {
                let dx = pos.x - prev[0];
                let dy = pos.y - prev[1];
                dx * dx + dy * dy >= 4.0
            });
            if should_push && points.len() < 2048 {
                points.push([pos.x, pos.y]);
            }
        }
        crate::SelectionDragMode::Move {
            last,
            record_history,
        } => {
            let previous = Point::new(f64::from(last[0]), f64::from(last[1]));
            let current = Point::new(f64::from(pos.x), f64::from(pos.y));
            let from = xf.to_page(previous);
            let to = xf.to_page(current);
            let translation = Point::new(to.x - from.x, to.y - from.y);
            if (translation.x.abs() > 1e-6 || translation.y.abs() > 1e-6)
                && apply_selection_transform(
                    app,
                    Point::ZERO,
                    translation,
                    1.0,
                    0.0,
                    *record_history,
                )
            {
                *record_history = false;
            }
            *last = [pos.x, pos.y];
        }
        crate::SelectionDragMode::Resize {
            pivot,
            start_last,
            last_pointer,
            applied_scale,
            last_constrained,
            sign,
            resize_x,
            resize_y,
            proportional,
            record_history,
            ..
        } => {
            *last_pointer = [pos.x, pos.y];
            let constrained = *proportional || (*resize_x && *resize_y && constrain_held);
            *last_constrained = constrained;
            let pivot_screen = Point::new(f64::from(pivot[0]), f64::from(pivot[1]));
            let target_scale = if constrained {
                compute_proportional_resize(*pivot, *start_last, pos, min_ext, strict_axis_floor)
                    .map(|(s, _, _)| [s, s])
            } else {
                let sx = if *resize_x {
                    compute_axis_resize(pivot[0], start_last[0], pos.x, sign[0], min_ext).0
                } else {
                    1.0
                };
                let sy = if *resize_y {
                    compute_axis_resize(pivot[1], start_last[1], pos.y, sign[1], min_ext).0
                } else {
                    1.0
                };
                Some([sx, sy])
            };
            if let Some([target_sx, target_sy]) = target_scale {
                let ratio_x = target_sx / applied_scale[0];
                let ratio_y = target_sy / applied_scale[1];
                if (ratio_x - 1.0).abs() > 1e-4 || (ratio_y - 1.0).abs() > 1e-4 {
                    let delta_sx = ratio_x.clamp(0.01, 100.0);
                    let delta_sy = ratio_y.clamp(0.01, 100.0);
                    if apply_selection_transform_axes(
                        app,
                        xf.to_page(pivot_screen),
                        Point::ZERO,
                        (delta_sx, delta_sy),
                        0.0,
                        *record_history,
                    ) {
                        *applied_scale = [applied_scale[0] * delta_sx, applied_scale[1] * delta_sy];
                        *record_history = false;
                    }
                }
            }
        }
        crate::SelectionDragMode::ResizeTextWidth {
            index,
            from_left,
            start_pointer_page,
            start_origin,
            start_width,
            angle_degrees,
            record_history,
        } => {
            let Some(current) = app.session.doc.texts.get(*index) else {
                app.view.selection_drag = None;
                return;
            };
            let current = current.as_ref().sanitized();
            let pointer_page = xf.to_page(Point::new(f64::from(pos.x), f64::from(pos.y)));
            let theta = angle_degrees.to_radians();
            let (sin, cos) = theta.sin_cos();
            let dx = pointer_page.x - start_pointer_page[0];
            let dy = pointer_page.y - start_pointer_page[1];
            let delta_along = dx * cos + dy * sin;
            let min_w = (f64::from(current.size) * 0.95)
                .max(f64::from(drawcraft_doc::MIN_TEXT_BOX_WIDTH))
                .min((*start_width).max(f64::from(drawcraft_doc::MIN_TEXT_BOX_WIDTH)));
            let page_w = f64::from(app.session.doc.width);
            let unrotated = angle_degrees.abs() <= 1e-3;
            let (new_x, new_y, new_width) = if *from_left {
                let max_w = if unrotated {
                    (start_origin[0] + *start_width).max(min_w)
                } else {
                    f64::from(drawcraft_doc::MAX_TEXT_BOX_WIDTH).max(min_w)
                };
                let clamped_w = (*start_width - delta_along).clamp(min_w, max_w);
                let shift = *start_width - clamped_w;
                (
                    start_origin[0] + cos * shift,
                    start_origin[1] + sin * shift,
                    clamped_w,
                )
            } else {
                let max_w = if unrotated {
                    (page_w - start_origin[0] - drawcraft_doc::TEXT_PAGE_RIGHT_MARGIN_PT).max(min_w)
                } else {
                    f64::from(drawcraft_doc::MAX_TEXT_BOX_WIDTH).max(min_w)
                };
                let clamped_w = (*start_width + delta_along).clamp(min_w, max_w);
                (start_origin[0], start_origin[1], clamped_w)
            };
            let drag_moved = delta_along.abs() > 0.25 || current.max_width.is_some();
            let changed = drag_moved
                && (current
                    .max_width
                    .is_none_or(|w| (f64::from(w) - new_width).abs() > 1e-2)
                    || (current.x - new_x).abs() > 1e-3
                    || (current.y - new_y).abs() > 1e-3);
            if changed
                && app
                    .session
                    .execute(
                        "text.update",
                        &serde_json::json!({
                            "index": *index,
                            "x": new_x,
                            "y": new_y,
                            "max_width": new_width as f32,
                            "record_history": *record_history,
                        }),
                    )
                    .is_ok()
            {
                *record_history = false;
            }
        }
        crate::SelectionDragMode::Rotate {
            pivot,
            last_angle,
            record_history,
        } => {
            let pivot_screen = Point::new(f64::from(pivot[0]), f64::from(pivot[1]));
            let angle =
                (f64::from(pos.y) - pivot_screen.y).atan2(f64::from(pos.x) - pivot_screen.x) as f32;
            let delta = (angle - *last_angle + std::f32::consts::PI)
                .rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            if delta.abs() > 1e-4
                && apply_selection_transform(
                    app,
                    xf.to_page(pivot_screen),
                    Point::ZERO,
                    1.0,
                    f64::from(delta.to_degrees()),
                    *record_history,
                )
            {
                *record_history = false;
            }
            *last_angle = angle;
        }
    }
    app.view.selection_drag = Some(drag);
}

fn on_segment_2d(a: Pos2, b: Pos2, p: Pos2) -> bool {
    p.x >= a.x.min(b.x) - 1e-4
        && p.x <= a.x.max(b.x) + 1e-4
        && p.y >= a.y.min(b.y) - 1e-4
        && p.y <= a.y.max(b.y) + 1e-4
}

fn orient_2d(a: Pos2, b: Pos2, c: Pos2) -> f64 {
    (f64::from(b.x) - f64::from(a.x)) * (f64::from(c.y) - f64::from(a.y))
        - (f64::from(b.y) - f64::from(a.y)) * (f64::from(c.x) - f64::from(a.x))
}

fn segments_intersect(a1: Pos2, a2: Pos2, b1: Pos2, b2: Pos2) -> bool {
    if !a1.x.is_finite()
        || !a1.y.is_finite()
        || !a2.x.is_finite()
        || !a2.y.is_finite()
        || !b1.x.is_finite()
        || !b1.y.is_finite()
        || !b2.x.is_finite()
        || !b2.y.is_finite()
    {
        return false;
    }
    if a1.x.max(a2.x) < b1.x.min(b2.x)
        || b1.x.max(b2.x) < a1.x.min(a2.x)
        || a1.y.max(a2.y) < b1.y.min(b2.y)
        || b1.y.max(b2.y) < a1.y.min(a2.y)
    {
        return false;
    }
    let o1 = orient_2d(a1, a2, b1);
    let o2 = orient_2d(a1, a2, b2);
    let o3 = orient_2d(b1, b2, a1);
    let o4 = orient_2d(b1, b2, a2);
    let eps = 1e-6;
    if ((o1 > eps && o2 < -eps) || (o1 < -eps && o2 > eps))
        && ((o3 > eps && o4 < -eps) || (o3 < -eps && o4 > eps))
    {
        return true;
    }
    (o1.abs() <= eps && on_segment_2d(a1, a2, b1))
        || (o2.abs() <= eps && on_segment_2d(a1, a2, b2))
        || (o3.abs() <= eps && on_segment_2d(b1, b2, a1))
        || (o4.abs() <= eps && on_segment_2d(b1, b2, a2))
}

fn segment_intersects_rect(a: Pos2, b: Pos2, rect: Rect) -> bool {
    if rect.contains(a) || rect.contains(b) {
        return true;
    }
    if a.x.max(b.x) < rect.left()
        || a.x.min(b.x) > rect.right()
        || a.y.max(b.y) < rect.top()
        || a.y.min(b.y) > rect.bottom()
    {
        return false;
    }
    let lt = rect.left_top();
    let rt = rect.right_top();
    let rb = rect.right_bottom();
    let lb = rect.left_bottom();
    segments_intersect(a, b, lt, rt)
        || segments_intersect(a, b, rt, rb)
        || segments_intersect(a, b, rb, lb)
        || segments_intersect(a, b, lb, lt)
}

fn polyline_intersects_rect(pts: &[Pos2], closed: bool, rect: Rect) -> bool {
    if pts.is_empty() {
        return false;
    }
    if pts.iter().any(|&p| rect.contains(p)) {
        return true;
    }
    if pts.len() == 1 {
        return false;
    }
    if pts
        .windows(2)
        .any(|seg| segment_intersects_rect(seg[0], seg[1], rect))
    {
        return true;
    }
    if closed
        && let (Some(&first), Some(&last)) = (pts.first(), pts.last())
        && segment_intersects_rect(last, first, rect)
    {
        return true;
    }
    false
}

fn decimated_stroke_screen_points(stroke: &InkStroke, xf: Affine2) -> Vec<Pos2> {
    let mut out = Vec::with_capacity(stroke.smoothed_points.len().min(256));
    let mut last_kept: Option<Pos2> = None;
    for (idx, sample) in stroke.smoothed_points.iter().enumerate() {
        let s = xf.to_screen(sample.point());
        let p = pos2(s.x as f32, s.y as f32);
        if !p.x.is_finite() || !p.y.is_finite() {
            continue;
        }
        let is_last = idx + 1 == stroke.smoothed_points.len();
        if is_last || last_kept.is_none_or(|prev| prev.distance_sq(p) >= 2.25) {
            out.push(p);
            last_kept = Some(p);
        }
    }
    out
}

fn objects_intersecting_rect(
    app: &DrawCraftApp,
    painter: &egui::Painter,
    rect: Rect,
    xf: Affine2,
) -> Vec<SelectedObject> {
    let mut objects = Vec::new();
    for (index, shape) in app.session.doc.shapes.iter().enumerate() {
        let shape = shape.as_ref().sanitized();
        let Some(bounds) = shape_screen_bounds(&shape, app, xf) else {
            continue;
        };
        if !bounds.intersects(rect) {
            continue;
        }
        let hit_rect = rect.expand((shape.width * app.view.zoom * 0.5).clamp(0.5, 40.0));
        let hit = shape.path_segments().into_iter().any(|(pts, closed)| {
            let screen_pts: Vec<Pos2> = pts
                .iter()
                .map(|p| {
                    let s = xf.to_screen(*p);
                    pos2(s.x as f32, s.y as f32)
                })
                .filter(|p| p.x.is_finite() && p.y.is_finite())
                .collect();
            polyline_intersects_rect(&screen_pts, closed, hit_rect)
        });
        if hit {
            objects.push(SelectedObject::Shape(index));
        }
    }
    for (index, item) in app.session.doc.texts.iter().enumerate() {
        let clean = item.as_ref().sanitized();
        let geom =
            text_selection_geometry(painter, app.session.doc.width, &clean, xf, app.view.zoom);
        if !geom.aabb().intersects(rect) {
            continue;
        }
        let cyclic_corners = [
            geom.corners[0],
            geom.corners[1],
            geom.corners[3],
            geom.corners[2],
        ];
        if polyline_intersects_rect(&cyclic_corners, true, rect)
            || geom.contains_screen_pos(rect.center(), 0.0)
        {
            objects.push(SelectedObject::Text(index));
        }
    }
    for stroke in &app.session.doc.strokes {
        let Some(bounds) = stroke_broadphase_screen_rect(stroke, xf)
            .or_else(|| stroke_screen_bounds(stroke, app, xf))
        else {
            continue;
        };
        if !bounds.intersects(rect) {
            continue;
        }
        let hit_rect = rect.expand((stroke.pen.size * app.view.zoom * 0.5).clamp(0.5, 48.0));
        let screen_pts = decimated_stroke_screen_points(stroke, xf);
        if polyline_intersects_rect(&screen_pts, false, hit_rect) {
            objects.push(SelectedObject::Stroke(stroke.id));
        }
    }
    for (index, image) in app.session.doc.images.iter().enumerate() {
        let clean = image.as_ref().sanitized();
        let Some(bounds) = image_screen_bounds(&clean, xf) else {
            continue;
        };
        if !bounds.intersects(rect) {
            continue;
        }
        let corners: Vec<Pos2> = clean
            .corners()
            .into_iter()
            .map(|p| {
                let s = xf.to_screen(p);
                pos2(s.x as f32, s.y as f32)
            })
            .filter(|p| p.x.is_finite() && p.y.is_finite())
            .collect();
        if polyline_intersects_rect(&corners, true, rect) {
            objects.push(SelectedObject::Image(index));
        }
    }
    objects
}

fn point_in_lasso(pt: Pos2, poly: &[[f32; 2]]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let xi = poly[i][0];
        let yi = poly[i][1];
        let xj = poly[j][0];
        let yj = poly[j][1];
        let intersect = ((yi > pt.y) != (yj > pt.y))
            && (pt.x
                < (xj - xi) * (pt.y - yi) / ((yj - yi).abs().max(1e-6) * (yj - yi).signum()) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn segment_intersects_lasso(a: Pos2, b: Pos2, poly: &[[f32; 2]], poly_aabb: Rect) -> bool {
    if a.x.max(b.x) < poly_aabb.left()
        || a.x.min(b.x) > poly_aabb.right()
        || a.y.max(b.y) < poly_aabb.top()
        || a.y.min(b.y) > poly_aabb.bottom()
    {
        return false;
    }
    let n = poly.len();
    for i in 0..n {
        let p0 = poly[i];
        let p1 = poly[(i + 1) % n];
        if segments_intersect(a, b, pos2(p0[0], p0[1]), pos2(p1[0], p1[1])) {
            return true;
        }
    }
    false
}

fn polyline_intersects_lasso(
    pts: &[Pos2],
    closed: bool,
    poly: &[[f32; 2]],
    poly_aabb: Rect,
) -> bool {
    if pts.is_empty() || poly.len() < 3 {
        return false;
    }
    if pts.iter().any(|&p| point_in_lasso(p, poly)) {
        return true;
    }
    if pts.len() == 1 {
        return false;
    }
    if pts
        .windows(2)
        .any(|seg| segment_intersects_lasso(seg[0], seg[1], poly, poly_aabb))
    {
        return true;
    }
    if closed
        && let (Some(&first), Some(&last)) = (pts.first(), pts.last())
        && segment_intersects_lasso(last, first, poly, poly_aabb)
    {
        return true;
    }
    false
}

fn point_hits_text_glyphs(
    galley: &egui::Galley,
    item: &TextItem,
    geom: &TextSelectionGeometry,
    screen_pos: Pos2,
) -> bool {
    if !geom.contains_screen_pos(screen_pos, 4.0) {
        return false;
    }
    let aligned_local = (geom.rot.inverse() * (screen_pos - geom.origin)).to_pos2()
        + Vec2::new(galley.rect.left(), 0.0);
    for placed in &galley.rows {
        let r = placed.rect();
        if (r.width() > 0.5 || !placed.row.glyphs.is_empty())
            && r.expand(4.0).contains(aligned_local)
        {
            return true;
        }
    }
    collect_list_prefix_rects(galley, &item.text)
        .into_iter()
        .any(|(_, _, prefix_rect)| prefix_rect.expand(4.0).contains(aligned_local))
}

fn objects_intersecting_lasso(
    app: &DrawCraftApp,
    painter: &egui::Painter,
    poly: &[[f32; 2]],
    xf: Affine2,
) -> Vec<SelectedObject> {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut cx_sum = 0.0_f32;
    let mut cy_sum = 0.0_f32;
    for &[x, y] in poly {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
        cx_sum += x;
        cy_sum += y;
    }
    if poly.len() < 3 {
        return Vec::new();
    }
    let poly_aabb = Rect::from_min_max(pos2(min_x, min_y), pos2(max_x, max_y));
    let poly_centroid = pos2(cx_sum / poly.len() as f32, cy_sum / poly.len() as f32);
    let mut objects = Vec::new();

    for (index, shape) in app.session.doc.shapes.iter().enumerate() {
        let shape = shape.as_ref().sanitized();
        let Some(bounds) = shape_screen_bounds(&shape, app, xf) else {
            continue;
        };
        if !bounds.intersects(poly_aabb) {
            continue;
        }
        let hit = shape.path_segments().into_iter().any(|(pts, closed)| {
            let screen_pts: Vec<Pos2> = pts
                .iter()
                .map(|p| {
                    let s = xf.to_screen(*p);
                    pos2(s.x as f32, s.y as f32)
                })
                .filter(|p| p.x.is_finite() && p.y.is_finite())
                .collect();
            polyline_intersects_lasso(&screen_pts, closed, poly, poly_aabb)
        });
        if hit {
            objects.push(SelectedObject::Shape(index));
        }
    }

    for (index, item) in app.session.doc.texts.iter().enumerate() {
        let clean = item.as_ref().sanitized();
        let galley = layout_text_item_galley(
            painter,
            app.session.doc.width,
            &clean,
            app.view.zoom,
            Color32::WHITE,
            None,
        );
        let geom = text_selection_geometry_from_galley(&clean, xf, app.view.zoom, &galley);
        if !geom.aabb().intersects(poly_aabb) {
            continue;
        }
        let cyclic_corners = [
            geom.corners[0],
            geom.corners[1],
            geom.corners[3],
            geom.corners[2],
        ];
        let hit = point_in_lasso(geom.center, poly)
            || polyline_intersects_lasso(&cyclic_corners, true, poly, poly_aabb)
            || point_hits_text_glyphs(&galley, &clean, &geom, poly_centroid)
            || poly
                .iter()
                .any(|&[x, y]| point_hits_text_glyphs(&galley, &clean, &geom, pos2(x, y)));
        if hit {
            objects.push(SelectedObject::Text(index));
        }
    }

    for stroke in &app.session.doc.strokes {
        let Some(bounds) = stroke_broadphase_screen_rect(stroke, xf)
            .or_else(|| stroke_screen_bounds(stroke, app, xf))
        else {
            continue;
        };
        if !bounds.intersects(poly_aabb) {
            continue;
        }
        let screen_pts = decimated_stroke_screen_points(stroke, xf);
        if polyline_intersects_lasso(&screen_pts, false, poly, poly_aabb) {
            objects.push(SelectedObject::Stroke(stroke.id));
        }
    }

    for (index, image) in app.session.doc.images.iter().enumerate() {
        let clean = image.as_ref().sanitized();
        let Some(bounds) = image_screen_bounds(&clean, xf) else {
            continue;
        };
        if !bounds.intersects(poly_aabb) {
            continue;
        }
        let corners: Vec<Pos2> = clean
            .corners()
            .into_iter()
            .map(|p| {
                let s = xf.to_screen(p);
                pos2(s.x as f32, s.y as f32)
            })
            .filter(|p| p.x.is_finite() && p.y.is_finite())
            .collect();
        if polyline_intersects_lasso(&corners, true, poly, poly_aabb) {
            objects.push(SelectedObject::Image(index));
        }
    }

    objects
}

fn finish_selection_drag(app: &mut DrawCraftApp, painter: &egui::Painter, xf: Affine2) {
    let Some(drag) = app.view.selection_drag.take() else {
        return;
    };
    match drag.mode {
        crate::SelectionDragMode::Marquee {
            start,
            current,
            additive,
        } => {
            let rect = Rect::from_two_pos(pos2(start[0], start[1]), pos2(current[0], current[1]));
            if rect.width() < 3.0 && rect.height() < 3.0 {
                if !additive {
                    set_selection(app, Vec::new());
                }
                return;
            }
            let mut selection = if additive {
                app.view.selected_objects.clone()
            } else {
                Vec::new()
            };
            selection.extend(objects_intersecting_rect(app, painter, rect, xf));
            set_selection(app, selection);
        }
        crate::SelectionDragMode::Lasso {
            points,
            additive,
            dragged,
        } => {
            if !dragged {
                if let Some(&[sx, sy]) = points.first() {
                    let click_pos = pos2(sx, sy);
                    if let Some(object) = object_at_screen_position(app, painter, click_pos, xf) {
                        if additive {
                            let mut selection = app.view.selected_objects.clone();
                            if let Some(existing) =
                                selection.iter().position(|selected| *selected == object)
                            {
                                selection.remove(existing);
                            } else {
                                selection.push(object);
                            }
                            set_selection(app, selection);
                        } else {
                            set_selection(app, vec![object]);
                        }
                        return;
                    }
                }
                if !additive {
                    set_selection(app, Vec::new());
                }
                return;
            }
            if points.len() < 3 {
                if !additive {
                    set_selection(app, Vec::new());
                }
                return;
            }
            let mut selection = if additive {
                app.view.selected_objects.clone()
            } else {
                Vec::new()
            };
            selection.extend(objects_intersecting_lasso(app, painter, &points, xf));
            set_selection(app, selection);
        }
        _ => {}
    }
}

fn render_selected_object_outline(
    app: &mut DrawCraftApp,
    painter: &egui::Painter,
    xf: Affine2,
    t: &Tokens,
) {
    if app.view.text_editing {
        return;
    }
    let selection_drag_keeps_active = matches!(
        app.view.selection_drag.as_ref().map(|d| &d.mode),
        Some(crate::SelectionDragMode::Marquee { .. } | crate::SelectionDragMode::Lasso { .. })
    );
    match app.view.selection_drag.as_ref().map(|d| &d.mode) {
        Some(crate::SelectionDragMode::Marquee { start, current, .. }) => {
            let rect = Rect::from_two_pos(pos2(start[0], start[1]), pos2(current[0], current[1]));
            painter.rect_filled(
                rect,
                CornerRadius::same(2),
                t.accent_soft.gamma_multiply(0.45),
            );
            painter.rect_stroke(
                rect,
                CornerRadius::same(2),
                Stroke::new(1.2, t.accent),
                StrokeKind::Inside,
            );
        }
        Some(crate::SelectionDragMode::Lasso {
            points, dragged, ..
        }) if *dragged && points.len() >= 2 => {
            let pts: Vec<Pos2> = points.iter().map(|&[x, y]| pos2(x, y)).collect();
            if pts.len() >= 3 {
                paint_polygon_fill(painter, &pts, t.accent_soft.gamma_multiply(0.32));
                if let (Some(&first), Some(&last)) = (pts.first(), pts.last()) {
                    paint_polyline_with_style(
                        painter,
                        &[last, first],
                        false,
                        1.1,
                        t.accent.gamma_multiply(0.55),
                        drawcraft_doc::ShapeLineStyle::Dashed,
                    );
                }
            }
            paint_polyline_with_style(
                painter,
                &pts,
                false,
                1.5,
                t.accent,
                drawcraft_doc::ShapeLineStyle::Solid,
            );
        }
        _ => {}
    }
    let pointer_pos = painter.ctx().input(|i| i.pointer.interact_pos());
    let halo = t.panel_bg.gamma_multiply(0.55);
    if let Some((index, item)) = selected_single_text(app) {
        let geom =
            text_selection_geometry(painter, app.session.doc.width, &item, xf, app.view.zoom);
        if geom.angle.abs() <= f32::EPSILON {
            let frame_rect = geom.aabb().expand(SELECTION_FRAME_PAD);
            painter.add(Shape::Rect(
                egui::epaint::RectShape::stroke(
                    frame_rect,
                    CornerRadius::same(3),
                    Stroke::new(2.8, halo),
                    StrokeKind::Middle,
                )
                .with_round_to_pixels(false),
            ));
            painter.add(Shape::Rect(
                egui::epaint::RectShape::stroke(
                    frame_rect,
                    CornerRadius::same(3),
                    Stroke::new(1.5, t.accent),
                    StrokeKind::Middle,
                )
                .with_round_to_pixels(false),
            ));
        } else {
            let [lt, rt, lb, rb] = geom.outer_corners;
            let loop_pts = vec![lt, rt, rb, lb, lt];
            painter.add(Shape::line(loop_pts.clone(), Stroke::new(2.8, halo)));
            painter.add(Shape::line(loop_pts, Stroke::new(1.5, t.accent)));
        }
        if app.session.active_tool != ToolKind::Select {
            return;
        }
        let active_handle =
            pointer_pos.and_then(|pos| text_handle_hit(index, &geom, pos).map(|(v, _)| v));
        paint_text_selection_handles(painter, &geom, t, active_handle);
        return;
    }
    let Some(bounds) = cached_selected_objects_bounds(app, painter, xf) else {
        if !selection_drag_keeps_active {
            app.view.clear_selection();
        } else {
            app.view.selected_text = None;
            app.view.selected_object = None;
            app.view.selected_objects.clear();
        }
        return;
    };
    let frame_rect = bounds.expand(SELECTION_FRAME_PAD);
    painter.add(Shape::Rect(
        egui::epaint::RectShape::stroke(
            frame_rect,
            CornerRadius::same(3),
            Stroke::new(2.8, halo),
            StrokeKind::Middle,
        )
        .with_round_to_pixels(false),
    ));
    painter.add(Shape::Rect(
        egui::epaint::RectShape::stroke(
            frame_rect,
            CornerRadius::same(3),
            Stroke::new(1.5, t.accent),
            StrokeKind::Middle,
        )
        .with_round_to_pixels(false),
    ));
    if app.session.active_tool != ToolKind::Select {
        return;
    }
    let active_handle =
        pointer_pos.and_then(|pos| non_text_handle_hit(bounds, pos).map(|(v, _)| v));
    paint_selection_handles(painter, bounds, t, active_handle);
}

fn selection_signature(app: &DrawCraftApp) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mix = |h: &mut u64, val: u64| {
        *h ^= val;
        *h = h.wrapping_mul(0x0100_0000_01b3);
    };
    let encode_obj = |obj: SelectedObject| -> u64 {
        match obj {
            SelectedObject::Stroke(id) => (1_u64 << 60) ^ id.0,
            SelectedObject::Shape(idx) => (2_u64 << 60) ^ (idx as u64),
            SelectedObject::Text(idx) => (3_u64 << 60) ^ (idx as u64),
            SelectedObject::Image(idx) => (4_u64 << 60) ^ (idx as u64),
        }
    };
    if let Some(obj) = app.view.selected_object {
        mix(&mut hash, encode_obj(obj));
    }
    mix(&mut hash, app.view.selected_objects.len() as u64);
    for &obj in &app.view.selected_objects {
        mix(&mut hash, encode_obj(obj));
    }
    hash
}

fn cached_selected_objects_bounds(
    app: &mut DrawCraftApp,
    painter: &egui::Painter,
    xf: Affine2,
) -> Option<Rect> {
    let has_stroke = matches!(app.view.selected_object, Some(SelectedObject::Stroke(_)))
        || app
            .view
            .selected_objects
            .iter()
            .any(|o| matches!(o, SelectedObject::Stroke(_)));
    if !has_stroke {
        return selected_objects_bounds(app, painter, xf);
    }

    let sig = selection_signature(app);
    let doc_ptr = Arc::as_ptr(&app.session.doc) as usize;
    let page_index = app.session.doc.active_page_index();
    let zoom_bits = app.view.zoom.to_bits();
    let revision = app.session.revision;
    let stroke_origin_bounds = match &app.canvas_cache.selection_bounds {
        Some(c)
            if c.revision == revision
                && c.doc_ptr == doc_ptr
                && c.page_index == page_index
                && c.selection_sig == sig
                && c.zoom_bits == zoom_bits =>
        {
            c.stroke_bounds_at_origin
        }
        _ => {
            let origin_xf = Affine2 {
                scale: xf.scale,
                offset_x: 0.0,
                offset_y: 0.0,
            };
            let mut stroke_ids = HashSet::new();
            if app.view.selected_objects.is_empty() {
                if let Some(SelectedObject::Stroke(id)) = app.view.selected_object {
                    stroke_ids.insert(id);
                }
            } else {
                for &obj in &app.view.selected_objects {
                    if let SelectedObject::Stroke(id) = obj {
                        stroke_ids.insert(id);
                    }
                }
            }
            let mut sb: Option<Rect> = None;
            for stroke in &app.session.doc.strokes {
                if stroke_ids.contains(&stroke.id)
                    && let Some(next) = stroke_screen_bounds(stroke, app, origin_xf)
                {
                    sb = union_screen_bounds(sb, next);
                }
            }
            app.canvas_cache.selection_bounds = Some(SelectionBoundsCache {
                revision,
                doc_ptr,
                page_index,
                selection_sig: sig,
                zoom_bits,
                stroke_bounds_at_origin: sb,
            });
            sb
        }
    };

    let mut bounds = stroke_origin_bounds
        .map(|r| r.translate(egui::vec2(xf.offset_x as f32, xf.offset_y as f32)));
    for &obj in &app.view.selected_objects {
        if !matches!(obj, SelectedObject::Stroke(_))
            && let Some(next) = selected_object_bounds(app, painter, obj, xf)
        {
            bounds = union_screen_bounds(bounds, next);
        }
    }
    bounds
}

fn commit_inline_text(app: &mut DrawCraftApp) {
    if app.view.text_draft.trim().is_empty() {
        app.view.text_draft.clear();
        app.view.text_prev_draft.clear();
        app.view.text_spans_draft.clear();
        app.view.text_selection_bytes = None;
        app.view.text_selection_needs_restore = false;
        app.view.text_editor_page_size = None;
        app.view.text_editing = false;
        app.view.text_edit_index = None;
        app.view.text_max_width_draft = None;
        app.view.text_link_draft = None;
        app.view.text_link_input.clear();
        app.view.text_link_editor_open = false;
        app.view.text_editor_needs_focus = false;
        app.view.text_insert_error = None;
        return;
    }
    let clean_spans = normalize_text_spans(&app.view.text_draft, &app.view.text_spans_draft);
    let result = if let Some(index) = app.view.text_edit_index {
        app.session.execute(
            "text.update",
            &serde_json::json!({
                "index": index,
                "text": app.view.text_draft,
                "size": app.view.text_size_draft,
                "color": app.view.text_color_draft,
                "font": app.view.text_font_draft,
                "alignment": app.view.text_alignment_draft,
                "spans": clean_spans,
                "line_spacing": app.view.text_line_spacing_draft,
                "letter_spacing": app.view.text_letter_spacing_draft,
                "max_width": app.view.text_max_width_draft,
                "link_url": app.view.text_link_draft,
                "clear_link_url": app.view.text_link_draft.is_none(),
            }),
        )
    } else {
        let [x, y] = app.view.text_insert_position;
        app.session.execute(
            "text.insert",
            &serde_json::json!({
                "text": app.view.text_draft,
                "x": x,
                "y": y,
                "size": app.view.text_size_draft,
                "color": app.view.text_color_draft,
                "font": app.view.text_font_draft,
                "alignment": app.view.text_alignment_draft,
                "spans": clean_spans,
                "line_spacing": app.view.text_line_spacing_draft,
                "letter_spacing": app.view.text_letter_spacing_draft,
                "max_width": app.view.text_max_width_draft,
                "link_url": app.view.text_link_draft,
            }),
        )
    };
    match result {
        Ok(_) => {
            let committed_index = app
                .view
                .text_edit_index
                .or_else(|| app.session.doc.texts.len().checked_sub(1));
            if app.session.active_tool == ToolKind::Select && !app.view.text_insert_mode {
                app.view.selected_object = committed_index.map(SelectedObject::Text);
                app.view.selected_objects = app.view.selected_object.into_iter().collect();
                app.view.selected_text = committed_index;
            } else {
                app.view.clear_selection();
            }
            app.view.text_style_open = false;
            app.view.text_editing = false;
            app.view.text_edit_index = None;
            app.view.text_max_width_draft = None;
            app.view.text_link_draft = None;
            app.view.text_link_input.clear();
            app.view.text_link_editor_open = false;
            app.view.text_editor_needs_focus = false;
            app.view.text_draft.clear();
            app.view.text_prev_draft.clear();
            app.view.text_spans_draft.clear();
            app.view.text_selection_bytes = None;
            app.view.text_selection_needs_restore = false;
            app.view.text_editor_page_size = None;
            app.view.text_insert_error = None;
        }
        Err(error) => app.view.text_insert_error = Some(error.to_string()),
    }
}

pub(crate) fn commit_pending_text(app: &mut DrawCraftApp) {
    if app.view.text_editing {
        commit_inline_text(app);
    }
}

fn configure_inline_text_visuals(ui: &mut egui::Ui, text_color: Color32) {
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let visuals = ui.visuals_mut();
    visuals.override_text_color = Some(text_color);
    visuals.extreme_bg_color = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.hovered.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.active.bg_fill = Color32::TRANSPARENT;
    visuals.text_cursor.stroke = Stroke::new(1.5, text_color);
}

fn char_idx_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map_or(text.len(), |(byte_idx, _)| byte_idx)
}

fn auto_continue_list_on_enter(old_text: &str, new_text: &str) -> Option<(String, usize)> {
    if new_text.len() != old_text.len() + 1 {
        return None;
    }
    let prefix = old_text
        .char_indices()
        .zip(new_text.char_indices())
        .find_map(|((i, a), (_, b))| if a != b { Some(i) } else { None })
        .unwrap_or(old_text.len());
    if new_text.as_bytes().get(prefix) != Some(&b'\n') {
        return None;
    }
    let line_start = old_text[..prefix].rfind('\n').map_or(0, |idx| idx + 1);
    let prev_line = &old_text[line_start..prefix];
    let (kind_opt, p_len) = parse_list_line_prefix(prev_line);
    let kind = kind_opt?;
    if prev_line.trim().len() <= prev_line[..p_len].trim().len() {
        let mut trimmed = String::with_capacity(old_text.len());
        trimmed.push_str(&old_text[..line_start]);
        trimmed.push('\n');
        trimmed.push_str(&old_text[prefix..]);
        let cursor_char = trimmed[..=line_start].chars().count();
        return Some((trimmed, cursor_char));
    }
    let insert_prefix = match kind {
        ListItemKind::Bullet => "- ",
        ListItemKind::Checkbox { .. } => "[ ] ",
    };
    let mut continued = String::with_capacity(new_text.len() + insert_prefix.len());
    continued.push_str(&new_text[..=prefix]);
    continued.push_str(insert_prefix);
    continued.push_str(&new_text[prefix + 1..]);
    let cursor_byte = prefix + 1 + insert_prefix.len();
    let cursor_char = continued[..cursor_byte].chars().count();
    Some((continued, cursor_char))
}

fn byte_to_char_idx(text: &str, byte_idx: usize) -> usize {
    let mut b = byte_idx.min(text.len());
    while b > 0 && !text.is_char_boundary(b) {
        b -= 1;
    }
    text[..b].chars().count()
}

fn paint_preserved_selection_bg(
    painter: &egui::Painter,
    galley: &egui::Galley,
    galley_pos: Pos2,
    text: &str,
    sel_bytes: [usize; 2],
    fill: Color32,
) {
    let [ba, bb] = sel_bytes;
    if ba >= bb || bb > text.len() || !text.is_char_boundary(ba) || !text.is_char_boundary(bb) {
        return;
    }
    let sel_c0 = text[..ba].chars().count();
    let sel_c1 = text[..bb].chars().count();
    let mut row_c0 = 0usize;
    for placed in &galley.rows {
        let len = placed.row.glyphs.len();
        let row_c1 = row_c0 + len;
        if sel_c1 > row_c0 && sel_c0 <= row_c1 {
            let a = sel_c0.clamp(row_c0, row_c1);
            let b = sel_c1.clamp(row_c0, row_c1);
            if a < b {
                let mut ca = egui::text::CCursor::new(a);
                ca.prefer_next_row = true;
                let mut cb = egui::text::CCursor::new(b);
                cb.prefer_next_row = false;
                let ra = galley.pos_from_cursor(ca);
                let rb = galley.pos_from_cursor(cb);
                let left = ra.left().min(rb.left());
                let right = ra.left().max(rb.left());
                if right > left + 0.5 {
                    let top = placed.rect().top().min(ra.top().min(rb.top()));
                    let bottom = placed.rect().bottom().max(ra.bottom().max(rb.bottom()));
                    let rect = Rect::from_min_max(pos2(left, top), pos2(right, bottom))
                        .translate(galley_pos.to_vec2());
                    painter.rect_filled(rect, CornerRadius::same(2), fill);
                }
            }
        }
        row_c0 = row_c1 + usize::from(placed.ends_with_newline);
    }
}

fn inline_text_wrap_width(
    text: &str,
    spec: &TextLayoutSpec,
    has_explicit_max_width: bool,
    max_width_px: f32,
    layout_fn: &mut dyn FnMut(egui::text::LayoutJob) -> Arc<egui::Galley>,
) -> f32 {
    if !has_explicit_max_width && spec.alignment != TextAlignment::Left {
        let mut measure_spec = spec.clone();
        measure_spec.alignment = TextAlignment::Left;
        measure_spec.max_width_px = max_width_px;
        let measured = layout_fn(build_text_layout_job(text, &measure_spec));
        measured.size().x.clamp(24.0, max_width_px.max(24.0))
    } else {
        max_width_px
    }
}

fn restore_inline_text_selection(
    ctx: &egui::Context,
    editor_id: egui::Id,
    text: &str,
    sel_bytes: Option<[usize; 2]>,
) {
    let Some([a, b]) = sel_bytes else {
        return;
    };
    if a > b || b > text.len() {
        return;
    }
    let mut state = egui::TextEdit::load_state(ctx, editor_id).unwrap_or_default();
    let c0 = byte_to_char_idx(text, a);
    let c1 = byte_to_char_idx(text, b);
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(c0),
            egui::text::CCursor::new(c1),
        )));
    state.store(ctx, editor_id);
}

fn nudge_pan_to_keep_text_cursor_visible(
    app: &mut DrawCraftApp,
    ctx: &egui::Context,
    avail: Rect,
    galley: &egui::Galley,
    galley_pos: Pos2,
    cursor: egui::text::CCursor,
) {
    let safe_avail = avail.shrink(16.0);
    if safe_avail.width() <= 32.0 || safe_avail.height() <= 32.0 {
        return;
    }
    let cursor_rect = galley
        .pos_from_cursor(cursor)
        .translate(galley_pos.to_vec2());
    let mut dx = 0.0_f32;
    let mut dy = 0.0_f32;
    if cursor_rect.right() > safe_avail.right() {
        dx = safe_avail.right() - cursor_rect.right();
    } else if cursor_rect.left() < safe_avail.left() {
        dx = safe_avail.left() - cursor_rect.left();
    }
    if cursor_rect.bottom() > safe_avail.bottom() {
        dy = safe_avail.bottom() - cursor_rect.bottom();
    } else if cursor_rect.top() < safe_avail.top() {
        dy = safe_avail.top() - cursor_rect.top();
    }
    if (dx.abs() > 0.5 || dy.abs() > 0.5) && dx.is_finite() && dy.is_finite() {
        app.view.pan[0] += f64::from(dx);
        app.view.pan[1] += f64::from(dy);
        ctx.request_repaint();
    }
}

fn handle_inline_style_shortcuts(app: &mut DrawCraftApp, ctx: &egui::Context) {
    let toggle_attr = ctx.input(|i| {
        if !i.modifiers.command || i.modifiers.alt {
            return None;
        }
        if i.key_pressed(egui::Key::B) {
            Some(TextStyleAttr::Bold)
        } else if i.key_pressed(egui::Key::I) {
            Some(TextStyleAttr::Italic)
        } else if i.key_pressed(egui::Key::U) {
            Some(TextStyleAttr::Underline)
        } else {
            None
        }
    });
    let Some(attr) = toggle_attr else {
        return;
    };
    let len = app.view.text_draft.len();
    let (s, e) = app
        .view
        .text_selection_bytes
        .and_then(|[a, b]| (a < b && b <= len).then_some((a, b)))
        .unwrap_or((0, len));
    let mut temp = TextItem {
        text: app.view.text_draft.clone(),
        spans: std::mem::take(&mut app.view.text_spans_draft),
        ..Default::default()
    };
    temp.toggle_span_style(s, e, attr);
    app.view.text_spans_draft = temp.spans;
    ctx.request_repaint();
}

fn render_inline_text_editor(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    xf: Affine2,
    avail: Rect,
    t: &Tokens,
) {
    if !app.view.text_editing {
        return;
    }
    let ctx = ui.ctx().clone();
    if !app.view.color_picker.open
        && app.view.active_modal.is_none()
        && ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        commit_inline_text(app);
        return;
    }

    let screen = xf.to_screen(Point::new(
        app.view.text_insert_position[0],
        app.view.text_insert_position[1],
    ));
    let pos = pos2(screen.x as f32, screen.y as f32);
    let safe_zoom = app.view.zoom.max(0.05);
    let editor_font_size = (app.view.text_size_draft * app.view.zoom).clamp(1.0, 400.0);
    let line_spacing = app.view.text_line_spacing_draft.clamp(0.8, 3.0);
    let letter_spacing_px = app.view.text_letter_spacing_draft.clamp(-4.0, 32.0) * app.view.zoom;
    let has_explicit_max_width = app.view.text_max_width_draft.is_some();
    let max_width_px = match app.view.text_max_width_draft {
        Some(width) => {
            width.clamp(
                drawcraft_doc::MIN_TEXT_BOX_WIDTH,
                drawcraft_doc::MAX_TEXT_BOX_WIDTH,
            ) * safe_zoom
        }
        None => {
            drawcraft_doc::text_max_page_width(
                app.session.doc.width,
                app.view.text_insert_position[0],
            ) * safe_zoom
        }
    };
    let (auto_contrast, paper_color) = crate::effective_doc_ink_context(app);
    let ink_ctx = Some(TextInkResolveCtx {
        theme: app.session.prefs.theme,
        auto_contrast,
        paper_color,
    });
    let resolved_color = paper_color.resolve_ink_color(
        app.session.prefs.theme,
        auto_contrast,
        app.view.text_color_draft,
        false,
    );
    let text_color = to_color32(resolved_color);
    let link_draft = app.view.text_link_draft.clone();
    let alignment_draft = app.view.text_alignment_draft;
    let spans_draft = app.view.text_spans_draft.clone();
    let mut spec = TextLayoutSpec {
        font: app.view.text_font_draft,
        size_px: editor_font_size,
        zoom: safe_zoom,
        line_spacing,
        letter_spacing_px,
        max_width_px,
        color: text_color,
        ink_ctx,
        alignment: alignment_draft,
        spans: spans_draft.clone(),
        link_spans: draft_link_byte_spans(&app.view.text_draft, link_draft.as_deref()),
        hide_list_prefix: false,
    };
    spec.max_width_px = inline_text_wrap_width(
        &app.view.text_draft,
        &spec,
        has_explicit_max_width,
        max_width_px,
        &mut |job| p.layout_job(job),
    );
    let preview_galley = p.layout_job(build_text_layout_job(&app.view.text_draft, &spec));
    let min_box_w = (editor_font_size * 0.55).clamp(16.0, max_width_px.max(16.0));
    let box_w = if has_explicit_max_width {
        max_width_px.max(min_box_w)
    } else if alignment_draft != TextAlignment::Left {
        preview_galley
            .size()
            .x
            .clamp(min_box_w.max(24.0), max_width_px.max(24.0))
    } else {
        (preview_galley.size().x + editor_font_size * 0.8 + 8.0)
            .clamp(min_box_w.max(24.0), max_width_px.max(24.0))
    };

    let editor_id = egui::Id::unique("inline_page_text_editor");
    let approx_h = preview_galley.size().y.max(editor_font_size * line_spacing);
    let editor_hit_rect = Rect::from_min_size(pos, Vec2::new(box_w.max(min_box_w), approx_h))
        .expand(4.0)
        .intersect(avail);
    let popup_open = egui::Popup::is_any_open(&ctx);
    let other_widget_focused = ctx.memory(|m| m.focused().is_some_and(|id| id != editor_id));
    let dragging_other_widget = ctx.dragged_id().is_some_and(|id| id != editor_id);
    let toolbar_overlay_active = popup_open
        || other_widget_focused
        || dragging_other_widget
        || app.view.color_picker.open
        || app.view.text_link_editor_open;
    let was_focused = ctx.memory(|m| m.has_focus(editor_id));
    let has_ime_ownership = ctx.memory(|m| m.owns_ime_events(editor_id));
    let canvas_layer_id = ui.layer_id();
    let pressed_pos = ctx.input(|i| {
        i.pointer
            .primary_pressed()
            .then(|| i.pointer.interact_pos())
            .flatten()
    });
    let pointer_pressed_in_editor = pressed_pos.is_some_and(|p_pos| {
        editor_hit_rect.contains(p_pos)
            && ctx.layer_id_at(p_pos).is_none_or(|l| l == canvas_layer_id)
    });

    if toolbar_overlay_active {
        if let Some(sel_bytes) = app.view.text_selection_bytes {
            paint_preserved_selection_bg(
                p,
                &preview_galley,
                pos - Vec2::new(preview_galley.rect.left(), 0.0),
                &app.view.text_draft,
                sel_bytes,
                t.accent.gamma_multiply(0.28),
            );
            app.view.text_selection_needs_restore = true;
        }
    } else if pointer_pressed_in_editor {
        app.view.text_selection_needs_restore = false;
    } else {
        if app.view.text_editor_needs_focus || !was_focused || app.view.text_selection_needs_restore
        {
            ctx.memory_mut(|m| m.request_focus(editor_id));
            if app.view.text_selection_needs_restore || !was_focused {
                restore_inline_text_selection(
                    &ctx,
                    editor_id,
                    &app.view.text_draft,
                    app.view.text_selection_bytes,
                );
                if has_ime_ownership {
                    app.view.text_selection_needs_restore = false;
                }
            }
        }
        if !has_ime_ownership && let Some(sel_bytes) = app.view.text_selection_bytes {
            paint_preserved_selection_bg(
                p,
                &preview_galley,
                pos - Vec2::new(preview_galley.rect.left(), 0.0),
                &app.view.text_draft,
                sel_bytes,
                t.accent.gamma_multiply(0.28),
            );
        }
    }

    let extra_top = (avail.top() - pos.y).max(0.0);
    let editor_h = (preview_galley.size().y + editor_font_size * line_spacing * 8.0 + 400.0)
        .max(avail.height() + extra_top + 200.0);
    let editor_max_rect = Rect::from_min_size(pos, Vec2::new(box_w.max(24.0), editor_h));
    let mut editor_ui = ui.new_child(egui::UiBuilder::new().max_rect(editor_max_rect));
    editor_ui.set_clip_rect(avail);
    configure_inline_text_visuals(&mut editor_ui, text_color);

    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap_width: f32| {
        let mut live_spec = spec.clone();
        live_spec.spans = normalize_text_spans(buf.as_str(), &spans_draft);
        live_spec.alignment = alignment_draft;
        live_spec.link_spans = draft_link_byte_spans(buf.as_str(), link_draft.as_deref());
        live_spec.max_width_px = inline_text_wrap_width(
            buf.as_str(),
            &live_spec,
            has_explicit_max_width,
            max_width_px,
            &mut |j| ui.fonts_mut(|f| f.layout_job(j)),
        );
        let job = build_text_layout_job(buf.as_str(), &live_spec);
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut output = egui::TextEdit::multiline(&mut app.view.text_draft)
        .id(editor_id)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::ZERO)
        .background_color(Color32::TRANSPARENT)
        .text_color(text_color)
        .desired_rows(1)
        .desired_width(box_w)
        .lock_focus(true)
        .char_limit(drawcraft_doc::MAX_TEXT_CHARS)
        .layouter(&mut layouter)
        .show(&mut editor_ui);

    let mut editor_focused = output.response.response.has_focus();
    let toolbar_overlay_after = popup_open
        || ctx.memory(|m| m.focused().is_some_and(|id| id != editor_id))
        || ctx.dragged_id().is_some_and(|id| id != editor_id)
        || app.view.color_picker.open
        || app.view.text_link_editor_open
        || app.view.active_modal.is_some();
    if !toolbar_overlay_after {
        output.response.response.request_focus();
        app.view.text_editor_needs_focus = false;
        editor_focused = true;
    }
    let mut new_cursor_char: Option<usize> = None;
    let text_changed = app.view.text_draft != app.view.text_prev_draft;
    if text_changed {
        if let Some((continued, c_idx)) =
            auto_continue_list_on_enter(&app.view.text_prev_draft, &app.view.text_draft)
        {
            app.view.text_draft = continued;
            new_cursor_char = Some(c_idx);
            output
                .state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(c_idx),
                )));
            output
                .state
                .clone()
                .store(&ctx, output.response.response.id);
            ctx.request_repaint();
        }
        let mut temp = TextItem {
            text: app.view.text_draft.clone(),
            spans: std::mem::take(&mut app.view.text_spans_draft),
            ..Default::default()
        };
        temp.adjust_spans_for_edit(&app.view.text_prev_draft, &app.view.text_draft);
        app.view.text_spans_draft = temp.spans;
        app.view.text_prev_draft = app.view.text_draft.clone();
    }

    if !has_explicit_max_width && output.galley.size().x + 4.0 > box_w && box_w + 0.5 < max_width_px
    {
        ctx.request_repaint();
    }
    if text_changed
        && editor_focused
        && !toolbar_overlay_after
        && let Some(crange) = output.state.cursor.char_range()
    {
        nudge_pan_to_keep_text_cursor_visible(
            app,
            &ctx,
            avail,
            &output.galley,
            output.galley_pos - Vec2::new(output.galley.rect.left(), 0.0),
            crange.primary,
        );
    }
    if let Some(c_idx) = new_cursor_char {
        let b = char_idx_to_byte(&app.view.text_draft, c_idx);
        app.view.text_selection_bytes = Some([b, b]);
        app.view.text_selection_needs_restore = false;
    } else if editor_focused && !toolbar_overlay_after {
        if app.view.text_selection_needs_restore && !has_ime_ownership {
            restore_inline_text_selection(
                &ctx,
                editor_id,
                &app.view.text_draft,
                app.view.text_selection_bytes,
            );
            ctx.request_repaint();
        } else {
            if let Some(crange) = output.state.cursor.char_range() {
                let a = char_idx_to_byte(&app.view.text_draft, crange.primary.index.into());
                let b = char_idx_to_byte(&app.view.text_draft, crange.secondary.index.into());
                app.view.text_selection_bytes = Some([a.min(b), a.max(b)]);
            }
            app.view.text_selection_needs_restore = false;
        }
        handle_inline_style_shortcuts(app, &ctx);
    }

    let galley_size = output.galley.size();
    let min_h = (editor_font_size * line_spacing).max(editor_font_size);
    let border_w = if has_explicit_max_width {
        max_width_px.max(galley_size.x).max(min_box_w)
    } else {
        galley_size
            .x
            .max(min_box_w)
            .min(max_width_px.max(min_box_w))
    };
    let border_h = galley_size.y.max(min_h);
    app.view.text_editor_page_size = Some([border_w / safe_zoom, border_h / safe_zoom]);
    let border_rect = Rect::from_min_size(output.galley_pos, Vec2::new(border_w, border_h));
    p.rect_stroke(
        border_rect.expand(3.0),
        CornerRadius::same(3),
        Stroke::new(1.2, t.accent),
        StrokeKind::Outside,
    );
}

fn checkbox_at_screen_pos(
    app: &DrawCraftApp,
    painter: &egui::Painter,
    screen_pos: Pos2,
    xf: Affine2,
) -> Option<(usize, usize)> {
    let doc = &app.session.doc;
    for (index, item) in doc.texts.iter().enumerate().rev() {
        if !item.text.contains('[') {
            continue;
        }
        let clean = item.as_ref().sanitized();
        let rough_screen =
            rough_page_rect_to_screen(text_item_rough_page_rect(doc.width, &clean), xf);
        if !rough_screen.expand(16.0).contains(screen_pos) {
            continue;
        }
        let screen = xf.to_screen(Point::new(clean.x, clean.y));
        let top_left = pos2(screen.x as f32, screen.y as f32);
        let rot = egui::emath::Rot2::from_angle(clean.angle_degrees.to_radians() as f32);
        let local = (rot.inverse() * (screen_pos - top_left)).to_pos2();
        let galley = layout_text_item_galley(
            painter,
            doc.width,
            &clean,
            app.view.zoom,
            Color32::WHITE,
            None,
        );
        let aligned_local = local + Vec2::new(galley.rect.left(), 0.0);
        for (para_idx, kind, prefix_rect) in collect_list_prefix_rects(&galley, &clean.text) {
            if matches!(kind, ListItemKind::Checkbox { .. })
                && prefix_rect.expand(4.0).contains(aligned_local)
            {
                return Some((index, para_idx));
            }
        }
    }
    None
}

fn rounded_checkbox_points(
    box_c: Pos2,
    half: f32,
    corner_r: f32,
    to_screen: impl Fn(Pos2) -> Pos2,
) -> Vec<Pos2> {
    let r = corner_r.clamp(0.0, half * 0.9);
    const SUBDIVISIONS: usize = 4;
    let corners = [
        (
            Vec2::new(half - r, -half + r),
            -std::f32::consts::FRAC_PI_2,
            0.0,
        ),
        (
            Vec2::new(half - r, half - r),
            0.0,
            std::f32::consts::FRAC_PI_2,
        ),
        (
            Vec2::new(-half + r, half - r),
            std::f32::consts::FRAC_PI_2,
            std::f32::consts::PI,
        ),
        (
            Vec2::new(-half + r, -half + r),
            std::f32::consts::PI,
            3.0 * std::f32::consts::FRAC_PI_2,
        ),
    ];
    let mut pts = Vec::with_capacity(corners.len() * (SUBDIVISIONS + 1));
    for (center_offset, a0, a1) in corners {
        for step in 0..=SUBDIVISIONS {
            let frac = step as f32 / SUBDIVISIONS as f32;
            let angle = a0 + (a1 - a0) * frac;
            let (sin, cos) = angle.sin_cos();
            pts.push(to_screen(
                box_c + center_offset + Vec2::new(r * cos, r * sin),
            ));
        }
    }
    pts
}

fn paint_list_prefix_markers_from_rects(
    painter: &egui::Painter,
    prefix_rects: &[(usize, ListItemKind, Rect)],
    item: &TextItem,
    top_left: Pos2,
    colors: [Color32; 2],
    zoom: f32,
) {
    if prefix_rects.is_empty() {
        return;
    }
    let [color, accent] = colors;
    let rot = egui::emath::Rot2::from_angle(item.angle_degrees.to_radians() as f32);
    let to_screen = |local: Pos2| top_left + rot * local.to_vec2();
    let size_px = (item.size * zoom).clamp(4.0, 200.0);
    for (_para_idx, kind, r) in prefix_rects {
        let cx = r.center().x;
        let cy = r.center().y;
        match *kind {
            ListItemKind::Bullet => {
                let rad = (size_px * 0.14).clamp(1.6, 8.0);
                painter.circle_filled(to_screen(pos2(cx, cy)), rad, color);
            }
            ListItemKind::Checkbox { checked } => {
                let half = (size_px * 0.34).clamp(4.5, 16.0);
                let corner_r = (half * 0.28).clamp(1.6, 4.5);
                let sw = (half * 0.22).clamp(1.1, 2.5);
                let box_c = pos2(cx, cy);
                if checked {
                    let pts = rounded_checkbox_points(box_c, half, corner_r, to_screen);
                    painter.add(Shape::convex_polygon(pts, accent, Stroke::NONE));
                    let stroke_w = (half * 0.28).clamp(1.2, 3.2);
                    let p_a = to_screen(box_c + Vec2::new(-half * 0.52, half * 0.04));
                    let p_b = to_screen(box_c + Vec2::new(-half * 0.10, half * 0.48));
                    let p_c = to_screen(box_c + Vec2::new(half * 0.56, -half * 0.42));
                    painter.add(Shape::line(
                        vec![p_a, p_b, p_c],
                        Stroke::new(stroke_w, Color32::WHITE),
                    ));
                } else {
                    let pts = rounded_checkbox_points(
                        box_c,
                        (half - sw * 0.5).max(1.0),
                        (corner_r - sw * 0.5).max(0.8),
                        to_screen,
                    );
                    painter.add(Shape::closed_line(pts, Stroke::new(sw, color)));
                }
            }
        }
    }
}

fn arc_slice_digest<T>(items: &[Arc<T>]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for item in items {
        hash ^= Arc::as_ptr(item) as usize as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn arc_ptrs_match<T>(cached: &[usize], items: &[Arc<T>]) -> bool {
    cached.len() == items.len()
        && cached
            .iter()
            .zip(items)
            .all(|(&ptr, item)| ptr == Arc::as_ptr(item) as usize)
}

#[inline]
fn scale_rect_uniform(rect: Rect, scale: f32) -> Rect {
    Rect::from_min_max(
        pos2(
            if rect.min.x.is_finite() {
                rect.min.x * scale
            } else {
                rect.min.x
            },
            if rect.min.y.is_finite() {
                rect.min.y * scale
            } else {
                rect.min.y
            },
        ),
        pos2(
            if rect.max.x.is_finite() {
                rect.max.x * scale
            } else {
                rect.max.x
            },
            if rect.max.y.is_finite() {
                rect.max.y * scale
            } else {
                rect.max.y
            },
        ),
    )
}

/// Scales a laid-out [`egui::Galley`] uniformly in point space and folds the subpixel
/// residual into row positions so `Tessellator`'s `round_text_to_pixels` never snaps
/// canvas text by ±1px during zoom or pan.
fn transform_galley_for_subpixel_zoom(
    base: &Arc<egui::Galley>,
    scale: f32,
    local_offset: Vec2,
) -> Arc<egui::Galley> {
    let scale_is_one = (scale - 1.0).abs() <= 1e-5;
    if scale_is_one && local_offset.length_sq() <= 1e-8 {
        return Arc::clone(base);
    }
    let mut g = (**base).clone();
    g.rect = scale_rect_uniform(g.rect, scale).translate(local_offset);
    g.mesh_bounds = scale_rect_uniform(g.mesh_bounds, scale).translate(local_offset);
    for placed_row in &mut g.rows {
        placed_row.pos = pos2(
            placed_row.pos.x * scale + local_offset.x,
            placed_row.pos.y * scale + local_offset.y,
        );
        if !scale_is_one {
            let row = Arc::make_mut(&mut placed_row.row);
            row.size *= scale;
            row.visuals.mesh_bounds = scale_rect_uniform(row.visuals.mesh_bounds, scale);
            for v in &mut row.visuals.mesh.vertices {
                v.pos = (v.pos.to_vec2() * scale).to_pos2();
            }
        }
    }
    Arc::new(g)
}

fn make_unrounded_text_shape(
    galley_origin: Pos2,
    galley: Arc<egui::Galley>,
    color: Color32,
    angle: f32,
    ppp: f32,
) -> Shape {
    let safe_ppp = if ppp.is_finite() && ppp > 0.1 {
        ppp
    } else {
        1.0
    };
    let snapped_pos = egui::emath::GuiRounding::round_to_pixels(galley_origin, safe_ppp);
    let world_residual = galley_origin - snapped_pos;
    let rot = egui::emath::Rot2::from_angle(angle);
    let local_residual = if angle.abs() <= f32::EPSILON {
        world_residual
    } else {
        rot.inverse() * world_residual
    };
    let adjusted = transform_galley_for_subpixel_zoom(&galley, 1.0, local_residual);
    Shape::Text(egui::epaint::TextShape::new(snapped_pos, adjusted, color).with_angle(angle))
}

fn sync_committed_text_cache(
    app: &mut DrawCraftApp,
    p: &egui::Painter,
    key: CommittedTextKey,
    now_s: f64,
    zoom: f32,
) {
    let doc = &app.session.doc;
    let theme = app.session.prefs.theme;
    let (_, auto_contrast, eff_paper_color) = key.color_key;
    let ppp = p.pixels_per_point();
    let ppp_bits = ppp.to_bits();
    let zoom_bits = zoom.to_bits();
    let glide_active = app.view.motion.camera_glide.is_some();

    let cache = &mut app.canvas_cache.texts;
    let text_cache_hit =
        cache.key.as_ref() == Some(&key) && arc_ptrs_match(&cache.text_ptrs, &doc.texts);

    if !text_cache_hit {
        cache.text_ptrs = doc.texts.iter().map(|i| Arc::as_ptr(i) as usize).collect();
        cache.items.clear();
        cache.items.reserve(doc.texts.len());
        for item in &doc.texts {
            let clean = item.as_ref().sanitized();
            let color = to_color32(eff_paper_color.resolve_ink_color(
                theme,
                auto_contrast,
                clean.color,
                false,
            ));
            let angle = clean.angle_degrees.to_radians() as f32;
            let rot = egui::emath::Rot2::from_angle(angle);
            let rough_page_rect = text_item_rough_page_rect(doc.width, &clean);
            cache.items.push(CachedCommittedTextItem {
                clean,
                color,
                angle,
                rot,
                rough_page_rect,
                baked_job: None,
                galley: None,
                prefix_rects: Vec::new(),
                emitted_key: None,
                emitted_galley: None,
            });
        }
        cache.key = Some(key);
        cache.baked_zoom = zoom;
        cache.baked_ppp_bits = ppp_bits;
        cache.last_zoom_bits = zoom_bits;
        cache.last_zoom_change_s = now_s;
        cache.pending_zoom_rebake = false;
        return;
    }

    if cache.last_zoom_bits != zoom_bits {
        cache.last_zoom_bits = zoom_bits;
        cache.last_zoom_change_s = now_s;
        cache.pending_zoom_rebake = true;
        p.ctx()
            .request_repaint_after(std::time::Duration::from_millis(85));
    } else if cache.pending_zoom_rebake && (now_s - cache.last_zoom_change_s) < 0.075 {
        p.ctx()
            .request_repaint_after(std::time::Duration::from_millis(85));
    }

    let zoom_active =
        glide_active || (cache.pending_zoom_rebake && (now_s - cache.last_zoom_change_s) < 0.075);
    let ratio = if cache.baked_zoom > 0.0 {
        zoom / cache.baked_zoom
    } else {
        0.0
    };
    let out_of_range = !(0.55..=1.80).contains(&ratio);
    let settled_mismatch =
        !zoom_active && cache.pending_zoom_rebake && (zoom - cache.baked_zoom).abs() > 1e-3;
    let should_rebake = cache.baked_zoom <= 0.0
        || cache.baked_ppp_bits != ppp_bits
        || out_of_range
        || settled_mismatch;

    if should_rebake {
        for entry in &mut cache.items {
            entry.baked_job = None;
            entry.galley = None;
            entry.emitted_key = None;
            entry.emitted_galley = None;
        }
        cache.baked_zoom = zoom;
        cache.baked_ppp_bits = ppp_bits;
        if !zoom_active {
            cache.pending_zoom_rebake = false;
        }
    } else if !zoom_active {
        cache.pending_zoom_rebake = false;
    }
}

fn paint_committed_text(app: &mut DrawCraftApp, p: &egui::Painter, xf: Affine2, t: &Tokens) {
    let doc = &app.session.doc;
    if doc.texts.is_empty() {
        app.canvas_cache.texts.key = None;
        app.canvas_cache.texts.text_ptrs.clear();
        app.canvas_cache.texts.baked_zoom = 0.0;
        app.canvas_cache.texts.pending_zoom_rebake = false;
        app.canvas_cache.texts.items.clear();
        return;
    }
    let theme = app.session.prefs.theme;
    let (auto_contrast, eff_paper_color) = crate::effective_doc_ink_context(app);
    let clip = xf.to_screen(Point::new(f64::from(doc.width), f64::from(doc.height)));
    let page_rect = Rect::from_min_max(
        pos2(xf.offset_x as f32, xf.offset_y as f32),
        pos2(clip.x as f32, clip.y as f32),
    );
    let page_clip = page_rect.intersect(p.clip_rect());
    if !page_clip.is_positive() {
        return;
    }
    let page_painter = p.with_clip_rect(page_clip);
    let visible_clip = page_clip.expand(48.0);
    let page_key = (app.session.active_notebook, doc.active_page_index());
    let color_key = (theme, auto_contrast, eff_paper_color);
    let text_digest = arc_slice_digest(&doc.texts);
    let key = CommittedTextKey {
        page_key,
        color_key,
        text_digest,
        text_count: doc.texts.len(),
        doc_width: doc.width,
    };
    let doc_width = doc.width;
    let zoom = app.view.zoom;
    let now_s = p.ctx().input(|i| i.time);
    sync_committed_text_cache(app, &page_painter, key, now_s, zoom);

    let ink_ctx = Some(TextInkResolveCtx {
        theme,
        auto_contrast,
        paper_color: eff_paper_color,
    });
    let text_editing = app.view.text_editing;
    let text_edit_index = app.view.text_edit_index;
    let accent = t.accent;
    let safe_ppp = page_painter.pixels_per_point().max(0.1);
    let layout_zoom = app.canvas_cache.texts.baked_zoom.max(0.05);
    let scale_ratio = (zoom / layout_zoom).clamp(0.25, 4.0);
    let scaled_diff = (scale_ratio - 1.0).abs() > 1e-5;

    for (index, entry) in app.canvas_cache.texts.items.iter_mut().enumerate() {
        if text_editing && text_edit_index == Some(index) {
            continue;
        }
        let rough_screen = rough_page_rect_to_screen(entry.rough_page_rect, xf);
        if !rough_screen.intersects(visible_clip) {
            continue;
        }
        let base_galley = if let Some(job) = &entry.baked_job {
            let fresh = page_painter.layout_job((**job).clone());
            if entry
                .galley
                .as_ref()
                .is_none_or(|prev| !Arc::ptr_eq(prev, &fresh))
            {
                entry.prefix_rects = collect_list_prefix_rects(&fresh, &entry.clean.text);
                entry.galley = Some(Arc::clone(&fresh));
                entry.emitted_key = None;
                entry.emitted_galley = None;
            }
            fresh
        } else {
            let job = build_text_layout_job_for_item(
                &page_painter,
                doc_width,
                &entry.clean,
                layout_zoom,
                entry.color,
                ink_ctx,
            );
            let built = page_painter.layout_job(job.clone());
            entry.prefix_rects = collect_list_prefix_rects(&built, &entry.clean.text);
            entry.baked_job = Some(Arc::new(job));
            entry.galley = Some(Arc::clone(&built));
            entry.emitted_key = None;
            entry.emitted_galley = None;
            built
        };

        let screen = xf.to_screen(Point::new(entry.clean.x, entry.clean.y));
        let top_left = pos2(screen.x as f32, screen.y as f32);
        let galley_origin =
            top_left + entry.rot * Vec2::new(-base_galley.rect.left() * scale_ratio, 0.0);
        let snapped_pos = egui::emath::GuiRounding::round_to_pixels(galley_origin, safe_ppp);
        let world_residual = galley_origin - snapped_pos;
        let local_residual = if entry.angle.abs() <= f32::EPSILON {
            world_residual
        } else {
            entry.rot.inverse() * world_residual
        };

        let emit_key = (
            zoom.to_bits(),
            safe_ppp.to_bits(),
            local_residual.x.to_bits(),
            local_residual.y.to_bits(),
            Arc::as_ptr(&base_galley) as usize,
        );
        let emitted_galley = if entry.emitted_key == Some(emit_key)
            && let Some(cached) = &entry.emitted_galley
        {
            Arc::clone(cached)
        } else {
            let built =
                transform_galley_for_subpixel_zoom(&base_galley, scale_ratio, local_residual);
            entry.emitted_key = Some(emit_key);
            entry.emitted_galley = Some(Arc::clone(&built));
            built
        };

        page_painter.add(egui::Shape::Text(
            egui::epaint::TextShape::new(snapped_pos, emitted_galley, entry.color)
                .with_angle(entry.angle),
        ));
        if scaled_diff && !entry.prefix_rects.is_empty() {
            let scaled_rects: Vec<(usize, ListItemKind, Rect)> = entry
                .prefix_rects
                .iter()
                .map(|&(p_idx, kind, r)| (p_idx, kind, scale_rect_uniform(r, scale_ratio)))
                .collect();
            paint_list_prefix_markers_from_rects(
                &page_painter,
                &scaled_rects,
                &entry.clean,
                galley_origin,
                [entry.color, accent],
                zoom,
            );
        } else {
            paint_list_prefix_markers_from_rects(
                &page_painter,
                &entry.prefix_rects,
                &entry.clean,
                galley_origin,
                [entry.color, accent],
                zoom,
            );
        }
    }
}

/// Triangulates a simple polygon (including concave shapes such as stars) via safe ear-clipping
/// and constructs its filled `Shape::mesh` if non-degenerate.
pub(crate) fn build_polygon_fill_shape(pts: &[Pos2], fill_color: Color32) -> Option<Shape> {
    if pts.len() < 3 || fill_color.a() == 0 {
        return None;
    }
    let mut clean: Vec<Pos2> = Vec::with_capacity(pts.len());
    for &p in pts {
        if !p.x.is_finite() || !p.y.is_finite() {
            continue;
        }
        if clean.last().is_none_or(|prev| prev.distance_sq(p) > 0.04) {
            clean.push(p);
        }
    }
    if clean.len() >= 3
        && clean
            .first()
            .zip(clean.last())
            .is_some_and(|(a, b)| a.distance_sq(*b) <= 0.04)
    {
        clean.pop();
    }
    if clean.len() < 3 {
        return None;
    }

    let area2: f32 = clean
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let b = clean[(i + 1) % clean.len()];
            a.x * b.y - b.x * a.y
        })
        .sum();
    if area2.abs() < 1e-4 {
        return None;
    }
    let ccw = area2 > 0.0;

    let mut mesh = Mesh::default();
    mesh.vertices
        .extend(clean.iter().map(|&p| egui::epaint::Vertex {
            pos: p,
            uv: egui::epaint::WHITE_UV,
            color: fill_color,
        }));

    let mut rem: Vec<usize> = (0..clean.len()).collect();
    let max_iters = rem
        .len()
        .saturating_mul(rem.len())
        .saturating_mul(2)
        .saturating_add(rem.len());
    let mut iters = 0usize;
    let mut cursor = 0usize;
    let mut steps_since_clip = 0usize;
    while rem.len() > 2 && iters < max_iters {
        iters += 1;
        let n = rem.len();
        cursor %= n;
        let i_prev = rem[(cursor + n - 1) % n];
        let i_curr = rem[cursor];
        let i_next = rem[(cursor + 1) % n];

        let a = clean[i_prev];
        let b = clean[i_curr];
        let c = clean[i_next];
        let cross = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        let is_convex = if ccw { cross > 1e-4 } else { cross < -1e-4 };
        let can_clip = if steps_since_clip >= 2 * n {
            true
        } else if steps_since_clip >= n {
            is_convex
        } else if is_convex {
            !rem.iter().copied().any(|idx| {
                idx != i_prev
                    && idx != i_curr
                    && idx != i_next
                    && point_in_triangle_2d(clean[idx], a, b, c)
            })
        } else {
            false
        };
        if can_clip {
            mesh.indices
                .extend([i_prev as u32, i_curr as u32, i_next as u32]);
            rem.remove(cursor);
            steps_since_clip = 0;
            continue;
        }
        steps_since_clip = steps_since_clip.saturating_add(1);
        cursor = (cursor + 1) % n;
    }

    if mesh.indices.is_empty() && clean.len() >= 3 {
        for i in 1..(clean.len() - 1) {
            mesh.indices.extend([0, i as u32, (i + 1) as u32]);
        }
    }
    if mesh.indices.is_empty() {
        None
    } else {
        Some(Shape::mesh(Arc::new(mesh)))
    }
}

/// Triangulates a simple polygon (including concave shapes such as stars) via safe ear-clipping
/// and paints its filled interior with `fill_color`.
pub(crate) fn paint_polygon_fill(painter: &egui::Painter, pts: &[Pos2], fill_color: Color32) {
    if let Some(shape) = build_polygon_fill_shape(pts, fill_color) {
        painter.add(shape);
    }
}

fn point_in_triangle_2d(p: Pos2, a: Pos2, b: Pos2, c: Pos2) -> bool {
    let min_x = a.x.min(b.x).min(c.x);
    let max_x = a.x.max(b.x).max(c.x);
    let min_y = a.y.min(b.y).min(c.y);
    let max_y = a.y.max(b.y).max(c.y);
    if p.x < min_x || p.x > max_x || p.y < min_y || p.y > max_y {
        return false;
    }
    let s1 = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    let s2 = (c.x - b.x) * (p.y - b.y) - (c.y - b.y) * (p.x - b.x);
    let s3 = (a.x - c.x) * (p.y - c.y) - (a.y - c.y) * (p.x - c.x);
    // Use an open-interior test so collinear edge/duplicate vertices do not block ear clipping.
    (s1 > 1e-4 && s2 > 1e-4 && s3 > 1e-4) || (s1 < -1e-4 && s2 < -1e-4 && s3 < -1e-4)
}

pub(crate) trait ShapeSink {
    fn push_shape(&mut self, shape: Shape);
}

impl ShapeSink for Vec<Shape> {
    fn push_shape(&mut self, shape: Shape) {
        self.push(shape);
    }
}

impl ShapeSink for &egui::Painter {
    fn push_shape(&mut self, shape: Shape) {
        self.add(shape);
    }
}

fn emit_polyline_with_style<S: ShapeSink>(
    sink: &mut S,
    pts: &[Pos2],
    closed: bool,
    stroke_w: f32,
    stroke_color: Color32,
    line_style: drawcraft_doc::ShapeLineStyle,
) {
    if pts.len() < 2 {
        return;
    }
    let Some((dash_raw, gap_raw)) = line_style.dash_pattern(stroke_w) else {
        let mut path = pts.to_vec();
        if closed && let Some(first) = path.first().copied() {
            path.push(first);
        }
        sink.push_shape(Shape::line(path, Stroke::new(stroke_w, stroke_color)));
        return;
    };
    let dash = f64::from(dash_raw.max(1.5));
    let gap = f64::from(gap_raw.max(2.0));
    let cycle = (dash + gap).max(2.0);
    let is_dotted = line_style == drawcraft_doc::ShapeLineStyle::Dotted;
    let dot_r = (stroke_w * 0.54).clamp(1.0, 24.0);
    let stroke = Stroke::new(stroke_w, stroke_color);

    let mut path = pts.to_vec();
    if closed && let Some(first) = path.first().copied() {
        path.push(first);
    }

    let mut phase = 0.0_f64;
    let mut seg_count = 0usize;
    for pair in path.windows(2) {
        if seg_count >= 10_000 {
            break;
        }
        let a = pair[0];
        let b = pair[1];
        let dx = f64::from(b.x - a.x);
        let dy = f64::from(b.y - a.y);
        let len = dx.hypot(dy);
        if len <= 1e-3 || !len.is_finite() {
            continue;
        }
        let ux = dx / len;
        let uy = dy / len;
        let mut d = 0.0_f64;
        while d + 1e-4 < len && seg_count < 10_000 {
            phase = phase.rem_euclid(cycle);
            if phase < dash - 1e-3 {
                let take = (dash - phase).min(len - d).max(0.0);
                let p0 = pos2(a.x + (ux * d) as f32, a.y + (uy * d) as f32);
                let p1 = pos2(
                    a.x + (ux * (d + take)) as f32,
                    a.y + (uy * (d + take)) as f32,
                );
                if is_dotted {
                    if phase < 0.5 || d == 0.0 {
                        sink.push_shape(Shape::circle_filled(p0, dot_r, stroke_color));
                        seg_count += 1;
                    }
                } else if take > 1e-3 {
                    sink.push_shape(Shape::line_segment([p0, p1], stroke));
                    seg_count += 1;
                }
                let advance = take.max((len - d).min(1e-2));
                d = (d + advance).min(len);
                phase = (phase + advance).rem_euclid(cycle);
                if (phase - dash).abs() <= 1e-3 {
                    phase = dash;
                }
            } else {
                let skip = (cycle - phase).min(len - d).max(0.0);
                let advance = skip.max((len - d).min(1e-2));
                d = (d + advance).min(len);
                phase = (phase + advance).rem_euclid(cycle);
                if phase >= cycle - 1e-3 {
                    phase = 0.0;
                }
            }
        }
    }
}

/// Paints a polyline stroke using `Solid`, `Dashed`, or `Dotted` style.
pub(crate) fn paint_polyline_with_style(
    painter: &egui::Painter,
    pts: &[Pos2],
    closed: bool,
    stroke_w: f32,
    stroke_color: Color32,
    line_style: drawcraft_doc::ShapeLineStyle,
) {
    let mut sink = painter;
    emit_polyline_with_style(&mut sink, pts, closed, stroke_w, stroke_color, line_style);
}

fn emit_shape_outline<S: ShapeSink>(
    sink: &mut S,
    shape: &GeometricShape,
    xf: Affine2,
    zoom: f32,
    color: Color32,
    fill_color: Option<Color32>,
) {
    let shape = shape.sanitized();
    let stroke_w = (shape.width * zoom).clamp(0.5, 80.0);
    for (points, closed) in shape.path_segments() {
        if points.len() < 2 {
            continue;
        }
        let screen_points: Vec<Pos2> = points
            .iter()
            .map(|point| {
                let screen = xf.to_screen(*point);
                pos2(screen.x as f32, screen.y as f32)
            })
            .collect();
        if closed
            && let Some(fc) = fill_color
            && let Some(fill_shape) = build_polygon_fill_shape(&screen_points, fc)
        {
            sink.push_shape(fill_shape);
        }
        emit_polyline_with_style(
            sink,
            &screen_points,
            closed,
            stroke_w,
            color,
            shape.line_style,
        );
    }
}

fn paint_shape_outline(
    painter: &egui::Painter,
    shape: &GeometricShape,
    xf: Affine2,
    zoom: f32,
    color: Color32,
    fill_color: Option<Color32>,
) {
    let mut sink = painter;
    emit_shape_outline(&mut sink, shape, xf, zoom, color, fill_color);
}

fn paint_committed_shapes(app: &mut DrawCraftApp, p: &egui::Painter, xf: Affine2) {
    let doc = &app.session.doc;
    if doc.shapes.is_empty() {
        app.canvas_cache.shapes.key = None;
        app.canvas_cache.shapes.shape_ptrs.clear();
        app.canvas_cache.shapes.shapes.clear();
        return;
    }
    let bottom_right = xf.to_screen(Point::new(f64::from(doc.width), f64::from(doc.height)));
    let page_rect = Rect::from_min_max(
        pos2(xf.offset_x as f32, xf.offset_y as f32),
        pos2(bottom_right.x as f32, bottom_right.y as f32),
    );
    let page_clip = page_rect.intersect(p.clip_rect());
    if !page_clip.is_positive() {
        return;
    }
    let clipped = p.with_clip_rect(page_clip);
    let visible_page = page_rect_for_screen_rect(page_clip, xf);
    let page_key = (
        app.session.active_notebook,
        app.session.doc.active_page_index(),
    );
    let (auto_contrast, eff_paper_color) = crate::effective_doc_ink_context(app);
    let color_key = (app.session.prefs.theme, auto_contrast, eff_paper_color);
    let shape_digest = arc_slice_digest(&doc.shapes);
    let key = CommittedShapeKey {
        page_key,
        color_key,
        shape_digest,
        shape_count: doc.shapes.len(),
        zoom_bits: app.view.zoom.to_bits(),
    };

    let shapes_cache_hit = app.canvas_cache.shapes.key.as_ref() == Some(&key)
        && arc_ptrs_match(&app.canvas_cache.shapes.shape_ptrs, &doc.shapes);
    if shapes_cache_hit {
        let cache = &mut app.canvas_cache.shapes;
        let viewport_ok = match (cache.built_clip_page, visible_page) {
            (Some(built), Some(vis)) => !cache.built_has_culled || rect_contains_rect(built, vis),
            _ => true,
        };
        if viewport_ok && cache.rendered_pan == app.view.pan {
            for shape in &cache.shapes {
                clipped.add(shape.clone());
            }
            return;
        }
        if viewport_ok && cache.translate_steps < MAX_CACHE_TRANSLATE_STEPS {
            let dx = (app.view.pan[0] - cache.rendered_pan[0]) as f32;
            let dy = (app.view.pan[1] - cache.rendered_pan[1]) as f32;
            let delta = egui::vec2(dx, dy);
            for shape in &mut cache.shapes {
                shape.translate(delta);
                clipped.add(shape.clone());
            }
            cache.rendered_pan = app.view.pan;
            cache.translate_steps = cache.translate_steps.saturating_add(1);
            return;
        }
    }

    let clip_page = visible_page.map(|v| {
        let margin = v.w.max(v.h).max(1.0) * 0.5;
        v.inflated(margin)
    });
    let mut culled_any = false;
    let mut raw_shapes = Vec::with_capacity(doc.shapes.len().saturating_mul(2));
    for item in &doc.shapes {
        if let Some(clip_rect) = clip_page {
            let page_bounds = shape_broadphase_page_rect(item);
            if !page_bounds.intersects(clip_rect) {
                culled_any = true;
                continue;
            }
        }
        let shape = item.as_ref().sanitized();
        let color = eff_paper_color.resolve_ink_color(
            app.session.prefs.theme,
            auto_contrast,
            shape.color,
            false,
        );
        let fill = shape.fill_color.map(|fc| {
            let resolved = eff_paper_color.resolve_ink_color(
                app.session.prefs.theme,
                auto_contrast,
                fc,
                false,
            );
            to_color32(resolved)
        });
        emit_shape_outline(
            &mut raw_shapes,
            &shape,
            xf,
            app.view.zoom,
            to_color32(color),
            fill,
        );
    }
    let cached_shapes = pre_tessellate_shapes(raw_shapes, p.pixels_per_point());
    for shape in &cached_shapes {
        clipped.add(shape.clone());
    }
    let shape_ptrs: Vec<usize> = doc.shapes.iter().map(|s| Arc::as_ptr(s) as usize).collect();
    let cache = &mut app.canvas_cache.shapes;
    cache.key = Some(key);
    cache.shape_ptrs = shape_ptrs;
    cache.built_clip_page = clip_page;
    cache.built_has_culled = culled_any;
    cache.rendered_pan = app.view.pan;
    cache.translate_steps = 0;
    cache.shapes = cached_shapes;
}

fn paint_shape_preview(
    app: &DrawCraftApp,
    p: &egui::Painter,
    xf: Affine2,
    modifiers: egui::Modifiers,
) {
    let (Some(start), Some(end)) = (app.view.shape_drag_start, app.view.shape_drag_end) else {
        return;
    };
    let (start, end) = constrain_shape_drag(app.view.selected_shape_kind, start, end, modifiers);
    let max_x = f64::from(app.session.doc.width);
    let max_y = f64::from(app.session.doc.height);
    let start = [start[0].clamp(0.0, max_x), start[1].clamp(0.0, max_y)];
    let end = [end[0].clamp(0.0, max_x), end[1].clamp(0.0, max_y)];
    let doc = &app.session.doc;
    let bottom_right = xf.to_screen(Point::new(f64::from(doc.width), f64::from(doc.height)));
    let page_rect = Rect::from_min_max(
        pos2(xf.offset_x as f32, xf.offset_y as f32),
        pos2(bottom_right.x as f32, bottom_right.y as f32),
    );
    let pen = app.session.active_pen();
    let fill_color = app.view.shape_fill_enabled.then(|| {
        pen.color
            .with_alpha(app.view.shape_fill_opacity.clamp(0.15, 0.95))
    });
    let shape = GeometricShape {
        kind: app.view.selected_shape_kind,
        x1: start[0],
        y1: start[1],
        x2: end[0],
        y2: end[1],
        angle_degrees: app.view.shape_angle_degrees,
        curve: app.view.shape_curve,
        color: pen.color,
        width: pen.size,
        fill_color,
        line_style: app.view.shape_line_style,
        points: Vec::new(),
    };
    let (auto_contrast, eff_paper_color) = crate::effective_doc_ink_context(app);
    let color = eff_paper_color.resolve_ink_color(
        app.session.prefs.theme,
        auto_contrast,
        shape.color,
        false,
    );
    let fill = shape.fill_color.map(|fc| {
        to_color32(eff_paper_color.resolve_ink_color(
            app.session.prefs.theme,
            auto_contrast,
            fc,
            false,
        ))
    });
    paint_shape_outline(
        &p.with_clip_rect(page_rect),
        &shape,
        xf,
        app.view.zoom,
        to_color32(color),
        fill,
    );
}

fn ruler_snap_preview(
    app: &DrawCraftApp,
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: Affine2,
) -> Option<(f64, f64)> {
    if !app.view.ruler_active || !matches!(app.session.active_tool, ToolKind::Pen(_)) {
        return None;
    }
    let screen_pos = ui.input(|input| input.pointer.hover_pos())?;
    if !can_start_on_canvas(ui, resp, screen_pos) {
        return None;
    }
    let page_pos = xf.to_page(Point::new(f64::from(screen_pos.x), f64::from(screen_pos.y)));
    let edge = ruler_edge_for_point(app, page_pos)?;
    let (along, _) = ruler_local_position(app, page_pos);
    Some((edge, along))
}

fn paint_ruler(
    app: &DrawCraftApp,
    p: &egui::Painter,
    xf: Affine2,
    t: &Tokens,
    snap_preview: Option<(f64, f64)>,
) {
    if !app.view.ruler_active {
        return;
    }
    let (center, direction, normal, half_length, half_width) = ruler_geometry(app);
    let corner = |along: f64, across: f64| {
        xf.to_screen(Point::new(
            center.x + direction.x * along + normal.x * across,
            center.y + direction.y * along + normal.y * across,
        ))
    };
    let corners = [
        corner(-half_length, -half_width),
        corner(half_length, -half_width),
        corner(half_length, half_width),
        corner(-half_length, half_width),
    ]
    .map(|point| pos2(point.x as f32, point.y as f32));
    let fill = if t.is_dark {
        Color32::from_rgba_unmultiplied(42, 52, 68, 210)
    } else {
        Color32::from_rgba_unmultiplied(220, 230, 242, 215)
    };
    p.add(Shape::convex_polygon(
        corners.to_vec(),
        fill,
        Stroke::new(1.5, t.accent),
    ));

    let upper_left = corner(-half_length, -half_width);
    let upper_right = corner(half_length, -half_width);
    let lower_right = corner(half_length, half_width);
    let lower_left = corner(-half_length, half_width);
    for (a, b) in [
        (upper_left, upper_right),
        (upper_right, lower_right),
        (lower_right, lower_left),
        (lower_left, upper_left),
    ] {
        p.line_segment(
            [pos2(a.x as f32, a.y as f32), pos2(b.x as f32, b.y as f32)],
            Stroke::new(1.2, t.accent),
        );
    }

    // Tick spacing and length follow the ruler depth so a thin ruler at high zoom keeps readable marks.
    let depth = half_width * 2.0;
    let tick_step = if depth >= 96.0 {
        25.0
    } else if depth >= 48.0 {
        10.0
    } else if depth >= 24.0 {
        5.0
    } else if depth >= 12.0 {
        2.5
    } else {
        2.0
    };
    let tick_count = (half_length * 2.0 / tick_step).floor().clamp(0.0, 2048.0) as usize;
    let tick_color = t.text.gamma_multiply(0.72);
    for idx in 0..=tick_count {
        let along = -half_length + idx as f64 * tick_step;
        let major = idx % 5 == 0;
        let tick_length = if major {
            half_width * 0.375
        } else {
            half_width * 0.1875
        };
        let start = corner(along, half_width);
        let end = corner(along, half_width - tick_length);
        p.line_segment(
            [
                pos2(start.x as f32, start.y as f32),
                pos2(end.x as f32, end.y as f32),
            ],
            Stroke::new(if major { 1.1 } else { 0.75 }, tick_color),
        );
    }

    if let Some((edge, along)) = snap_preview {
        let edge_start = corner(-half_length, half_width * edge);
        let edge_end = corner(half_length, half_width * edge);
        p.line_segment(
            [
                pos2(edge_start.x as f32, edge_start.y as f32),
                pos2(edge_end.x as f32, edge_end.y as f32),
            ],
            Stroke::new(9.0, t.accent.gamma_multiply(0.28)),
        );
        p.line_segment(
            [
                pos2(edge_start.x as f32, edge_start.y as f32),
                pos2(edge_end.x as f32, edge_end.y as f32),
            ],
            Stroke::new(3.0, t.accent),
        );
        let marker = corner(along.clamp(-half_length, half_length), half_width * edge);
        let marker = pos2(marker.x as f32, marker.y as f32);
        p.circle_filled(marker, 6.0, t.accent);
        p.circle_stroke(marker, 6.0, Stroke::new(2.0, t.toolbar_bg));

        let label = "STRAIGHT LINE";
        let galley = p.layout_no_wrap(label.to_string(), FontId::proportional(10.0), t.text);
        let direction_screen = Vec2::new(direction.x as f32, direction.y as f32);
        let normal_screen = Vec2::new(normal.x as f32, normal.y as f32);
        let label_center = marker + direction_screen * 12.0 + normal_screen * (edge as f32 * 14.0);
        let label_rect = Rect::from_center_size(label_center, galley.size() + Vec2::splat(10.0));
        p.rect_filled(label_rect, CornerRadius::same(5), t.panel_bg);
        p.rect_stroke(
            label_rect,
            CornerRadius::same(5),
            Stroke::new(1.0, t.accent),
            StrokeKind::Inside,
        );
        p.galley(label_rect.min + Vec2::splat(5.0), galley, t.text);
    }

    let zoom = app.view.zoom.max(0.05);
    if (half_width * 2.0) as f32 * zoom >= 18.0 {
        let label_size = (11.0_f32 * (zoom / 0.6).sqrt()).clamp(8.0, 14.0);
        let center_screen = xf.to_screen(center);
        let center_screen = pos2(center_screen.x as f32, center_screen.y as f32);
        let angle_degrees = if app.view.ruler_angle.is_finite() {
            (app.view.ruler_angle.to_degrees() + 180.0).rem_euclid(360.0) - 180.0
        } else {
            0.0
        };
        let angle_label = format!("{angle_degrees:+.0}°");
        let angle_galley = p.layout_no_wrap(angle_label, FontId::monospace(label_size), t.text);
        let angle_badge =
            Rect::from_center_size(center_screen, angle_galley.size() + Vec2::new(10.0, 4.0));
        p.rect_filled(angle_badge, CornerRadius::same(4), t.panel_bg);
        p.rect_stroke(
            angle_badge,
            CornerRadius::same(4),
            Stroke::new(1.0, t.border),
            StrokeKind::Inside,
        );
        p.galley(angle_badge.min + Vec2::new(5.0, 2.0), angle_galley, t.text);
        let direction_screen = Vec2::new(direction.x as f32, direction.y as f32);
        p.text(
            center_screen + direction_screen * (half_length as f32 * 0.72 * zoom),
            egui::Align2::CENTER_CENTER,
            "MOVE",
            FontId::proportional(label_size.min(9.0)),
            t.text_muted,
        );
    }

    let handle_page = ruler_rotation_handle(app);
    let handle_screen = xf.to_screen(handle_page);
    let handle = pos2(handle_screen.x as f32, handle_screen.y as f32);
    let edge_center = corner(0.0, -half_width);
    p.line_segment(
        [pos2(edge_center.x as f32, edge_center.y as f32), handle],
        Stroke::new(1.0, t.accent.gamma_multiply(0.8)),
    );
    p.circle_filled(handle, 8.0, t.accent);
    p.circle_stroke(handle, 8.0, Stroke::new(1.5, t.toolbar_bg));
    p.circle_filled(handle, 2.0, t.toolbar_bg);
    p.text(
        handle + Vec2::new(11.0, 0.0),
        egui::Align2::LEFT_CENTER,
        "ROTATE",
        FontId::proportional(9.5),
        t.text_muted,
    );
}

fn draw_locator_ticks(p: &egui::Painter, center: Pos2, r: f32, fg: Color32, halo: Color32) {
    let inner = r + 2.5;
    let outer = r + 6.0;
    let offsets = [
        (Vec2::new(inner, 0.0), Vec2::new(outer, 0.0)),
        (Vec2::new(-inner, 0.0), Vec2::new(-outer, 0.0)),
        (Vec2::new(0.0, inner), Vec2::new(0.0, outer)),
        (Vec2::new(0.0, -inner), Vec2::new(0.0, -outer)),
    ];
    for (a, b) in offsets {
        p.line_segment([center + a, center + b], Stroke::new(2.0, halo));
        p.line_segment([center + a, center + b], Stroke::new(1.0, fg));
    }
}

fn draw_pen_cursor(app: &DrawCraftApp, p: &egui::Painter, hover: Pos2, paper_bg: Color32) {
    let pen = app.session.active_pen();
    let cur_p = if has_active_live(app) {
        app.stylus.sample().map_or(1.0, |s| s.pressure)
    } else {
        1.0
    };
    let p_factor = if pen.pressure_enabled {
        pen.pressure_size_min + (1.0 - pen.pressure_size_min) * pen.pressure_curve.eval(cur_p)
    } else {
        1.0
    };
    let r = (pen.size * 0.5 * p_factor * app.view.zoom).clamp(2.0, 120.0);
    let (auto_contrast, eff_paper_color) = crate::effective_doc_ink_context(app);
    let resolved = eff_paper_color.resolve_ink_color(
        app.session.prefs.theme,
        auto_contrast,
        pen.color,
        matches!(pen.kind, drawcraft_doc::PenKind::Highlighter),
    );
    // A visible dark-gray ring and center dot (white halo on dark paper): it must read on every
    // paper tint and never be confused with the ink color. `resolved` only matters for the tip.
    let _ = resolved;
    let ring = CURSOR_GRAY;
    let halo = contrast_halo(ring, paper_bg);

    p.circle_stroke(hover, r, Stroke::new(2.8, halo));
    p.circle_stroke(hover, r, Stroke::new(1.5, ring));
    if pen.kind == PenKind::Pencil {
        let off = (r + 3.0).clamp(5.0, 14.0);
        for [dx, dy] in [[-1.0_f32, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]] {
            p.circle_filled(hover + Vec2::new(dx * off * 0.7, dy * off * 0.7), 1.1, ring);
        }
    }
    if r < 6.5 {
        draw_locator_ticks(p, hover, r, ring, halo);
    }
    p.circle_filled(hover, 3.1, halo);
    p.circle_filled(hover, 2.2, ring);
}

/// Dark gray used for the pen and eraser cursor rings and dots.
const CURSOR_GRAY: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3C);

fn draw_cursor_and_tether(
    app: &DrawCraftApp,
    ui: &egui::Ui,
    p: &egui::Painter,
    resp: &egui::Response,
    xf: Affine2,
    paper_bg: Color32,
    t: &Tokens,
) {
    if app.view.pan_dragging
        || (!app.view.text_editing
            && (ui.input(|i| i.key_down(egui::Key::Space)) || app.stylus.pan_held()))
    {
        return;
    }

    if let Some(live) = &app.session.live_stroke
        && let Some((brush_pt, pointer_pt)) = live.string_tether()
    {
        let sb = xf.to_screen(brush_pt);
        let sp = xf.to_screen(pointer_pt);
        let a = pos2(sb.x as f32, sb.y as f32);
        let b = pos2(sp.x as f32, sp.y as f32);
        p.line_segment([a, b], Stroke::new(1.2, t.accent.gamma_multiply(0.7)));
        p.circle_stroke(b, 4.0, Stroke::new(1.2, t.accent));
    }

    let hover = resp
        .hover_pos()
        .filter(|pos| resp.rect.contains(*pos))
        .or_else(|| {
            has_active_live(app)
                .then(|| live_pointer_screen_pos(app, xf))
                .flatten()
                .filter(|pos| resp.rect.contains(*pos))
        });

    let Some(hover) = hover else {
        return;
    };

    if app.view.text_insert_mode || app.view.text_editing {
        return;
    }

    if app.view.shape_insert_mode {
        p.text(
            hover + Vec2::new(7.0, 6.0),
            egui::Align2::LEFT_TOP,
            app.view.selected_shape_kind.label(),
            FontId::proportional(13.0),
            t.accent,
        );
        return;
    }

    match app.session.active_tool {
        ToolKind::Pen(_) => draw_pen_cursor(app, p, hover, paper_bg),
        ToolKind::Eraser => {
            let r = (app.session.eraser_size * 0.5 * app.view.zoom).clamp(3.0, 140.0);
            let ring = CURSOR_GRAY;
            let halo = contrast_halo(ring, paper_bg);
            p.circle_filled(
                hover,
                r,
                Color32::from_rgba_unmultiplied(0x3A, 0x3A, 0x3C, 36),
            );
            p.circle_stroke(hover, r, Stroke::new(3.0, halo));
            p.circle_stroke(hover, r, Stroke::new(1.7, ring));
            p.circle_filled(hover, 3.1, halo);
            p.circle_filled(hover, 2.2, ring);
            if app.session.eraser_highlighter_only {
                // Highlighter-only mode keeps a small amber center so it stays recognizable.
                p.circle_filled(hover, 1.3, Color32::from_rgb(0xF5, 0xC5, 0x18));
            }
        }
        ToolKind::LaserPointer => {
            let laser = to_color32(app.view.laser_color);
            p.circle_filled(hover, 8.0, laser.gamma_multiply(0.22));
            p.circle_filled(hover, 3.6, laser);
            p.circle_filled(hover, 1.6, Color32::WHITE);
        }
        ToolKind::Select => {}
    }
}

pub(crate) fn decode_png_color_image(png_bytes: &[u8]) -> Option<egui::ColorImage> {
    if png_bytes.is_empty() {
        return None;
    }
    let decoded = image::load_from_memory(png_bytes).ok()?;
    let rgba = decoded.to_rgba8();
    let w = usize::try_from(rgba.width()).ok()?;
    let h = usize::try_from(rgba.height()).ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [w, h],
        rgba.as_raw(),
    ))
}

/// Looks up or decodes and caches an `egui::TextureHandle` for `image` in `ctx` temp storage.
pub(crate) fn cached_page_image_texture(
    ctx: &egui::Context,
    cache_salt: &str,
    image: &PageImage,
) -> Option<egui::TextureHandle> {
    let clean = image.sanitized();
    if clean.png_bytes.is_empty() {
        return None;
    }
    let cache_id = egui::Id::unique((cache_salt, 0u8));
    let key = (
        Arc::as_ptr(&image.png_bytes) as usize,
        image.png_bytes.len(),
    );
    if let Some(existing) = ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<std::collections::HashMap<(usize, usize), egui::TextureHandle>>(
            cache_id,
        )
        .get(&key)
        .cloned()
    }) {
        return Some(existing);
    }
    let color_image = decode_png_color_image(&clean.png_bytes)?;
    let loaded = ctx.load_texture(
        format!("{cache_salt}_{}_{}", key.0, key.1),
        color_image,
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<std::collections::HashMap<(usize, usize), egui::TextureHandle>>(cache_id);
        if cache.len() > 256 {
            cache.clear();
        }
        cache.insert(key, loaded.clone());
    });
    Some(loaded)
}

pub(crate) fn paint_textured_image_quad(
    painter: &egui::Painter,
    texture_id: egui::TextureId,
    corners: [Pos2; 4],
) {
    let mut mesh = Mesh::with_texture(texture_id);
    mesh.vertices = vec![
        egui::epaint::Vertex {
            pos: corners[0],
            uv: pos2(0.0, 0.0),
            color: Color32::WHITE,
        },
        egui::epaint::Vertex {
            pos: corners[1],
            uv: pos2(1.0, 0.0),
            color: Color32::WHITE,
        },
        egui::epaint::Vertex {
            pos: corners[2],
            uv: pos2(1.0, 1.0),
            color: Color32::WHITE,
        },
        egui::epaint::Vertex {
            pos: corners[3],
            uv: pos2(0.0, 1.0),
            color: Color32::WHITE,
        },
    ];
    mesh.indices = vec![0, 1, 2, 0, 2, 3];
    painter.add(Shape::mesh(mesh));
}

fn paint_committed_images(
    app: &mut DrawCraftApp,
    ctx: &egui::Context,
    painter: &egui::Painter,
    page_rect: Rect,
    xf: Affine2,
) {
    let img_count = app.session.doc.images.len();
    app.image_textures.retain(|idx, _| *idx < img_count);
    if img_count == 0 {
        return;
    }
    let clipped = painter.with_clip_rect(page_rect);
    for (idx, image) in app.session.doc.images.iter().enumerate() {
        let clean = image.sanitized();
        let needs_reload = app
            .image_textures
            .get(&idx)
            .is_none_or(|(cached_bytes, _)| !Arc::ptr_eq(cached_bytes, &image.png_bytes));
        if needs_reload {
            let Some(color_image) = decode_png_color_image(&clean.png_bytes) else {
                continue;
            };
            let handle = ctx.load_texture(
                format!("drawcraft_page_img_{idx}"),
                color_image,
                egui::TextureOptions::LINEAR,
            );
            app.image_textures
                .insert(idx, (Arc::clone(&image.png_bytes), handle));
        }
        let Some((_, handle)) = app.image_textures.get(&idx) else {
            continue;
        };
        let corners_page = clean.corners();
        let to_pos = |pt: Point| {
            let s = xf.to_screen(pt);
            pos2(s.x as f32, s.y as f32)
        };
        let corners = [
            to_pos(corners_page[0]),
            to_pos(corners_page[1]),
            to_pos(corners_page[2]),
            to_pos(corners_page[3]),
        ];
        paint_textured_image_quad(&clipped, handle.id(), corners);
    }
}

fn smooth_laser_run(
    run: &[crate::LaserPoint],
    xf: Affine2,
    now_s: f64,
    lifetime_s: f64,
) -> Vec<StrokeSample> {
    let mut raw = Vec::with_capacity(run.len());
    for pt in run {
        if !pt.x.is_finite() || !pt.y.is_finite() || !pt.time_s.is_finite() {
            continue;
        }
        let sp = xf.to_screen(Point::new(pt.x, pt.y));
        if !sp.x.is_finite() || !sp.y.is_finite() {
            continue;
        }
        let age = ((now_s - pt.time_s) / lifetime_s).clamp(0.0, 1.0) as f32;
        let fade = (1.0 - age) * (1.0 - age);
        raw.push(StrokeSample::from_xy_pressure(sp.x, sp.y, fade));
    }
    if raw.len() < 2 {
        return raw;
    }

    let faired = apply_streamline(&raw, 0.40);
    let mut thinned: Vec<StrokeSample> = Vec::with_capacity(faired.len());
    for (idx, &s) in faired.iter().enumerate() {
        let is_last = idx + 1 == faired.len();
        if is_last {
            if thinned.len() > 1
                && let Some(prev) = thinned.last_mut()
                && prev.point().distance(s.point()) < 2.0
            {
                *prev = s;
            } else {
                thinned.push(s);
            }
        } else if thinned
            .last()
            .is_none_or(|prev: &StrokeSample| prev.point().distance(s.point()) >= 4.5)
        {
            thinned.push(s);
        }
    }
    if thinned.len() < 2 {
        return thinned;
    }
    resample_centripetal_catmull_rom(&thinned, 0.5, 3.8)
}

#[allow(clippy::too_many_arguments)]
fn append_laser_layer(
    p: &egui::Painter,
    mesh: &mut Mesh,
    pen: &PenSettings,
    samples: &[StrokeSample],
    color: drawcraft_color::Color,
    zoom_scale: f32,
    base_radius: f32,
    base_alpha: f32,
    feather: f32,
) {
    let nodes: Vec<SpineNode> = samples
        .iter()
        .map(|s| {
            let fade = s.pressure.clamp(0.0, 1.0);
            let glow_scale = zoom_scale * (0.4 + 0.6 * fade);
            SpineNode {
                point: s.point(),
                radius: (base_radius * glow_scale).max(0.25),
                alpha: (base_alpha * fade).clamp(0.0, 1.0),
                color,
                angle_rad: 0.0,
                roundness: 1.0,
                direction_rad: 0.0,
                arc_length: 0.0,
            }
        })
        .collect();
    let ribbon = build_stroke_ribbon(pen, &nodes, feather);
    if ribbon.indices.is_empty() {
        return;
    }
    if !mesh.vertices.is_empty()
        && mesh.vertices.len().saturating_add(ribbon.vertices.len()) > MAX_BATCH_VERTICES
    {
        p.add(Shape::mesh(std::mem::take(mesh)));
    }
    append_ribbon_transformed(mesh, &ribbon, Affine2::IDENTITY, false);
}

fn paint_laser_pointer(
    app: &mut DrawCraftApp,
    ctx: &egui::Context,
    p: &egui::Painter,
    xf: Affine2,
) {
    let now_s = ctx.input(|i| i.time);
    const TRAIL_LIFETIME_S: f64 = 2.2;
    app.view.laser_points.retain(|pt| {
        pt.x.is_finite()
            && pt.y.is_finite()
            && pt.time_s.is_finite()
            && (now_s - pt.time_s) <= TRAIL_LIFETIME_S
    });
    if app.view.laser_points.is_empty() {
        return;
    }
    ctx.request_repaint();

    let laser = to_color32(app.view.laser_color);
    let pts = &app.view.laser_points;
    let to_screen = |pt: &crate::LaserPoint| {
        let s = xf.to_screen(Point::new(pt.x, pt.y));
        pos2(s.x as f32, s.y as f32)
    };

    if app.view.laser_trail_mode && pts.len() >= 2 {
        let zoom = if app.view.zoom.is_finite() {
            app.view.zoom.clamp(0.4, 3.0)
        } else {
            1.0
        };
        let zoom_scale = zoom * 0.75 + 0.25;
        let ribbon_pen = PenSettings::ball_pen();
        let laser_doc = app.view.laser_color.sanitized();

        let mut runs: Vec<Vec<StrokeSample>> = Vec::new();
        let mut start = 0usize;
        for (idx, pt) in pts.iter().enumerate().skip(1) {
            if pt.stroke_break {
                let smoothed = smooth_laser_run(&pts[start..idx], xf, now_s, TRAIL_LIFETIME_S);
                if smoothed.len() >= 2 && smoothed.iter().any(|s| s.pressure > 0.01) {
                    runs.push(smoothed);
                }
                start = idx;
            }
        }
        let tail = smooth_laser_run(&pts[start..], xf, now_s, TRAIL_LIFETIME_S);
        if tail.len() >= 2 && tail.iter().any(|s| s.pressure > 0.01) {
            runs.push(tail);
        }

        if !runs.is_empty() {
            let mut mesh = Mesh::default();
            // Render outer halo, mid glow, and crisp core as continuous round-joined ribbons
            // across all runs so the trail fades smoothly without per-segment joints or dashes.
            let layers = [
                (4.2_f32, 0.16_f32, 3.2 * zoom_scale),
                (2.4_f32, 0.38_f32, 1.8 * zoom_scale),
                (1.3_f32, 0.95_f32, 0.9_f32),
            ];
            for (base_radius, base_alpha, feather) in layers {
                for run_samples in &runs {
                    append_laser_layer(
                        p,
                        &mut mesh,
                        &ribbon_pen,
                        run_samples,
                        laser_doc,
                        zoom_scale,
                        base_radius,
                        base_alpha,
                        feather,
                    );
                }
            }
            if !mesh.indices.is_empty() {
                p.add(Shape::mesh(mesh));
            }
        }
    }

    if let Some(tip) = pts.last() {
        let age = ((now_s - tip.time_s) / TRAIL_LIFETIME_S).clamp(0.0, 1.0) as f32;
        let fade = (1.0 - age) * (1.0 - age);
        let c = to_screen(tip);
        if c.x.is_finite() && c.y.is_finite() {
            p.circle_filled(c, 12.0, laser.gamma_multiply(0.22 * fade));
            p.circle_filled(c, 5.2, laser.gamma_multiply(0.85 * fade));
            p.circle_filled(c, 2.2, Color32::WHITE.gamma_multiply(0.92 * fade));
        }
    }
}

pub fn show(app: &mut DrawCraftApp, ui: &mut egui::Ui) {
    app.install_rich_fonts_if_needed(ui.ctx());
    let t = Tokens::for_theme(app.session.prefs.theme);
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(t.canvas_bg))
        .show(ui, |ui| {
            let avail = ui.available_rect_before_wrap();
            let dt = ui.ctx().input(|i| i.stable_dt);
            advance_canvas_camera(app, ui, avail, dt);
            app.view.last_canvas_rect = Some(avail);

            let page_key = (
                app.session.active_notebook,
                app.session.doc.active_page_index(),
            );
            if app.view.motion.page_fade.advance(page_key, dt) {
                ui.ctx().request_repaint();
            }

            let resp = ui.allocate_rect(avail, Sense::click_and_drag());
            let input_active =
                app.view.active_modal.is_none() && !app.view.motion.screen.is_exiting();
            if input_active {
                handle_pan_and_zoom(app, ui, &resp);
                handle_drawing_input(app, ui, &resp);
            } else if has_active_live(app) {
                finish_active_stroke(app);
            }

            let xf = page_affine(app);
            let p = ui.painter_at(avail);

            let top_left = xf.to_screen(Point::ZERO);
            let bot_right = xf.to_screen(Point::new(
                f64::from(app.session.doc.width),
                f64::from(app.session.doc.height),
            ));
            let page_rect = Rect::from_min_max(
                pos2(top_left.x as f32, top_left.y as f32),
                pos2(bot_right.x as f32, bot_right.y as f32),
            );

            let theme = app.session.prefs.theme;
            let (auto_contrast, eff_paper_color) = crate::effective_doc_ink_context(app);
            let paper_colors = eff_paper_color.resolved_colors(theme, auto_contrast);
            if app.session.doc.pdf_backdrop.is_some()
                || app.session.doc.custom_image_backdrop.is_some()
            {
                let base_bg = if app.session.doc.pdf_backdrop.is_some() {
                    if app.session.prefs.pdf_dark_mode {
                        Color32::from_gray(drawcraft_color::PDF_DARK_FLOOR as u8)
                    } else {
                        Color32::WHITE
                    }
                } else {
                    to_color32(paper_colors.background)
                };
                paint_page_sheet_base(&p, page_rect, base_bg);
                let tier_dim = crate::pdf_view::canvas_pdf_tier_dim(ui.ctx(), page_rect);
                crate::pdf_view::paint_doc_pdf_backdrop_if_present(
                    ui.ctx(),
                    &p,
                    page_rect,
                    &app.session.doc,
                    tier_dim,
                );
            } else {
                draw_paper_template(app, &p, paper_colors, page_rect, xf);
            }
            paint_committed_images(app, ui.ctx(), &p, page_rect, xf);
            paint_committed_strokes(app, &p, avail, xf);
            paint_committed_text(app, &p, xf, &t);
            paint_committed_shapes(app, &p, xf);
            paint_shape_preview(app, &p, xf, ui.input(|i| i.modifiers));
            crate::study_ui::paint_tapes(app, &p, xf, &t);
            let snap_preview = ruler_snap_preview(app, ui, &resp, xf);
            paint_ruler(app, &p, xf, &t, snap_preview);

            let page_veil = app.view.motion.page_fade.veil_alpha();
            if page_veil > 0.001 {
                let paper_tint = to_color32(paper_colors.background);
                p.add(Shape::Rect(
                    egui::epaint::RectShape::filled(
                        page_rect,
                        CornerRadius::same(2),
                        paper_tint.gamma_multiply(page_veil),
                    )
                    .with_round_to_pixels(false),
                ));
            }

            if let Some(live) = &app.session.live_stroke {
                if let Some(shape) = &live.recognized_shape {
                    let display_c = eff_paper_color.resolve_ink_color(
                        theme,
                        auto_contrast,
                        shape.color,
                        matches!(live.pen.kind, drawcraft_doc::PenKind::Highlighter),
                    );
                    let display_fill = shape.fill_color.map(|fc| {
                        to_color32(eff_paper_color.resolve_ink_color(
                            theme,
                            auto_contrast,
                            fc,
                            false,
                        ))
                    });
                    paint_shape_outline(
                        &p.with_clip_rect(page_rect),
                        shape,
                        xf,
                        app.view.zoom,
                        to_color32(display_c),
                        display_fill,
                    );
                    let top_mid = xf.to_screen(Point::new(
                        (shape.x1 + shape.x2) * 0.5,
                        shape.y1.min(shape.y2),
                    ));
                    p.text(
                        pos2(top_mid.x as f32, (top_mid.y as f32) - 10.0),
                        egui::Align2::CENTER_BOTTOM,
                        format!("Snapped: {}", shape.kind.label()),
                        FontId::proportional(11.5),
                        t.accent,
                    );
                } else if !(app.wgpu_enabled && live.pen.kind == PenKind::Highlighter) {
                    let preview = live.preview_samples();
                    let feather = (0.9 / app.view.zoom.max(0.25)).clamp(0.3, 2.5);
                    let mut display_pen = live.pen.clone();
                    display_pen.color = eff_paper_color.resolve_ink_color(
                        theme,
                        auto_contrast,
                        live.pen.color,
                        matches!(live.pen.kind, drawcraft_doc::PenKind::Highlighter),
                    );
                    paint_live_ribbon(&p, &display_pen, &preview, xf, feather);
                }
            }
            paint_laser_pointer(app, ui.ctx(), &p, xf);

            if input_active {
                update_canvas_cursor_icon(app, ui, &resp);
                let paper_bg = to_color32(paper_colors.background);
                draw_cursor_and_tether(app, ui, &p, &resp, xf, paper_bg, &t);
            }
            render_selected_object_outline(app, &p, xf, &t);
            render_inline_text_editor(app, ui, &p, xf, avail, &t);
            render_floating_canvas_pills(app, ui.ctx(), avail, &t);
            render_canvas_error_banners(app, ui.ctx(), avail, &t);
        });
}

/// Renders calm floating Undo/Redo and Page Indicator pills perched unobtrusively on the canvas
/// corners (hidden in focus mode or when a modal is open).
fn render_floating_canvas_pills(
    app: &mut DrawCraftApp,
    ctx: &egui::Context,
    avail: Rect,
    t: &Tokens,
) {
    if app.view.study.focus_mode
        || app.view.active_modal.is_some()
        || avail.width() < 260.0
        || avail.height() < 180.0
    {
        return;
    }

    let left_handed = app.session.prefs.left_handed;
    let margin = 14.0_f32;
    let btn_size = crate::icons::min_target(ctx, 32.0);
    let pill_shadow = egui::Shadow {
        offset: [0, 3],
        blur: 14,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(0, 0, 0, if t.is_dark { 96 } else { 22 }),
    };

    // 1. Floating `[ Undo | Redo ]` pill — top-left (or top-right when left-handed).
    let pill_w = btn_size * 2.0 + 17.0;
    let undo_x = if left_handed {
        avail.right() - margin - pill_w
    } else {
        avail.left() + margin
    };
    let undo_y = avail.top() + margin;
    let can_undo = app.session.history.can_undo();
    let can_redo = app.session.history.can_redo();
    let mut do_undo = false;
    let mut do_redo = false;

    egui::Area::new(egui::Id::unique("drawcraft_floating_undo_pill"))
        .order(egui::Order::Middle)
        .fixed_pos(pos2(undo_x, undo_y))
        .fade_in(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(t.toolbar_bg.gamma_multiply(0.96))
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(crate::theme::CARD_RADIUS))
                .shadow(pill_shadow)
                .inner_margin(egui::Margin::same(4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if crate::icons::icon_button(
                            ui,
                            crate::icons::Icon::Undo,
                            None,
                            false,
                            can_undo,
                            t,
                        )
                        .on_hover_text("Undo (Cmd/Ctrl+Z or two-finger tap)")
                        .clicked()
                        {
                            do_undo = true;
                        }
                        let (div_rect, _) =
                            ui.allocate_exact_size(Vec2::new(5.0, btn_size * 0.65), Sense::hover());
                        ui.painter().line_segment(
                            [
                                pos2(div_rect.center().x, div_rect.top()),
                                pos2(div_rect.center().x, div_rect.bottom()),
                            ],
                            Stroke::new(1.0, t.border),
                        );
                        if crate::icons::icon_button(
                            ui,
                            crate::icons::Icon::Redo,
                            None,
                            false,
                            can_redo,
                            t,
                        )
                        .on_hover_text("Redo (Cmd/Ctrl+Shift+Z or three-finger tap)")
                        .clicked()
                        {
                            do_redo = true;
                        }
                    });
                });
        });

    if do_undo {
        let _ = app.session.execute("edit.undo", &serde_json::json!({}));
    } else if do_redo {
        let _ = app.session.execute("edit.redo", &serde_json::json!({}));
    }

    // 2. Quiet floating Page indicator pill at bottom-right (or bottom-left when left-handed).
    let page_count = app.session.doc.page_count().max(1);
    let active_idx = app
        .session
        .doc
        .active_page_index()
        .min(page_count.saturating_sub(1));
    let label = format!("{} / {}", active_idx + 1, page_count);
    let pill_h = crate::icons::min_target(ctx, 30.0);
    let show_arrows = page_count > 1;
    let est_w = if show_arrows {
        btn_size * 2.0 + 62.0
    } else {
        64.0
    };
    let page_x = if left_handed {
        avail.left() + margin
    } else {
        avail.right() - margin - est_w
    };
    let page_y = avail.bottom() - margin - pill_h - 8.0;

    let mut select_page: Option<usize> = None;
    let mut toggle_pages = false;

    egui::Area::new(egui::Id::unique("drawcraft_floating_page_pill"))
        .order(egui::Order::Middle)
        .fixed_pos(pos2(page_x, page_y))
        .fade_in(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(t.toolbar_bg.gamma_multiply(0.94))
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(crate::theme::CARD_RADIUS))
                .shadow(pill_shadow)
                .inner_margin(egui::Margin::symmetric(6, 3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if show_arrows
                            && crate::icons::compact_icon_button(
                                ui,
                                crate::icons::Icon::PrevPage,
                                None,
                                false,
                                active_idx > 0,
                                t,
                            )
                            .on_hover_text("Previous page")
                            .clicked()
                        {
                            select_page = Some(active_idx.saturating_sub(1));
                        }
                        let (lbl_rect, lbl_resp) =
                            ui.allocate_exact_size(Vec2::new(48.0, pill_h - 6.0), Sense::click());
                        let hov = lbl_resp.hovered();
                        if hov {
                            ui.painter().rect_filled(
                                lbl_rect,
                                CornerRadius::same(crate::theme::CONTROL_RADIUS),
                                t.ghost_hover,
                            );
                        }
                        ui.painter().text(
                            lbl_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            &label,
                            FontId::monospace(11.5),
                            if hov { t.text } else { t.text_muted },
                        );
                        if lbl_resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Open Pages navigator (P)")
                            .clicked()
                        {
                            toggle_pages = true;
                        }
                        if show_arrows
                            && crate::icons::compact_icon_button(
                                ui,
                                crate::icons::Icon::NextPage,
                                None,
                                false,
                                active_idx + 1 < page_count,
                                t,
                            )
                            .on_hover_text("Next page")
                            .clicked()
                        {
                            select_page = Some((active_idx + 1).min(page_count - 1));
                        }
                    });
                });
        });

    if let Some(idx) = select_page {
        commit_pending_text(app);
        let _ = app
            .session
            .execute("page.select", &serde_json::json!({ "index": idx }));
        app.mesh_cache.clear();
        app.view.fit_requested = true;
        ctx.request_repaint();
    } else if toggle_pages {
        app.view.pages_sidebar_open = !app.view.pages_sidebar_open;
    }
}

fn render_canvas_error_banners(
    app: &mut DrawCraftApp,
    ctx: &egui::Context,
    avail: Rect,
    t: &Tokens,
) {
    if let Some(message) = app.view.image_insert_error.as_deref()
        && crate::pdf_view::render_floating_error_banner(
            ctx,
            avail,
            "canvas_image_insert_error",
            message,
            t,
        )
    {
        app.view.image_insert_error = None;
    }
    if let Some(message) = app.view.pdf_import_error.as_deref()
        && crate::pdf_view::render_floating_error_banner(
            ctx,
            avail,
            "canvas_pdf_import_error",
            message,
            t,
        )
    {
        app.view.pdf_import_error = None;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::AppScreen;

    fn run_editor_frame(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        time: f64,
        events: Vec<egui::Event>,
    ) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 800.0))),
            time: Some(time),
            predicted_dt: 0.016,
            events,
            ..Default::default()
        };
        let out = ctx.run_ui(raw, |ui| app.show_ui(ui));
        out.drop_without_applying_deltas();
    }

    fn setup_editor_app(ctx: &egui::Context) -> DrawCraftApp {
        let mut app = DrawCraftApp::default();
        app.view.screen = AppScreen::Editor;
        app.view.motion.screen = crate::motion::ScreenTransition::new(AppScreen::Editor);
        app.view.pages_sidebar_open = false;
        app.view.inspector_open = false;
        run_editor_frame(ctx, &mut app, 0.0, Vec::new());
        app
    }

    #[test]
    fn same_frame_release_then_press_commits_previous_stroke_and_keeps_new_stroke_live() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);

        run_editor_frame(
            &ctx,
            &mut app,
            0.016,
            vec![
                egui::Event::PointerButton {
                    pos: pos2(400.0, 300.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(pos2(440.0, 310.0)),
            ],
        );
        assert!(app.session.live_stroke.is_some());

        // Fast lift and immediate re-touch in the same frame must commit Stroke 1 and keep Stroke 2 live.
        run_editor_frame(
            &ctx,
            &mut app,
            0.032,
            vec![
                egui::Event::PointerMoved(pos2(460.0, 315.0)),
                egui::Event::PointerButton {
                    pos: pos2(460.0, 315.0),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(pos2(500.0, 340.0)),
                egui::Event::PointerButton {
                    pos: pos2(520.0, 350.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(pos2(550.0, 360.0)),
            ],
        );
        assert_eq!(app.session.doc.strokes.len(), 1);
        assert!(app.session.live_stroke.is_some());
        assert_eq!(
            app.session.live_stroke.as_ref().unwrap().raw_points.len(),
            2
        );

        // Next frame continues Stroke 2 cleanly.
        run_editor_frame(
            &ctx,
            &mut app,
            0.048,
            vec![
                egui::Event::PointerMoved(pos2(590.0, 380.0)),
                egui::Event::PointerButton {
                    pos: pos2(590.0, 380.0),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(app.session.doc.strokes.len(), 2);
        assert!(app.session.live_stroke.is_none());
    }

    #[test]
    fn single_frame_press_move_release_preserves_intermediate_points_and_excludes_hover() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        let xf = page_affine(&app);
        let expected_start = xf.to_page(Point::new(500.0, 300.0));

        run_editor_frame(
            &ctx,
            &mut app,
            0.016,
            vec![
                egui::Event::PointerMoved(pos2(200.0, 200.0)),
                egui::Event::PointerButton {
                    pos: pos2(500.0, 300.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(pos2(540.0, 320.0)),
                egui::Event::PointerMoved(pos2(580.0, 340.0)),
                egui::Event::PointerButton {
                    pos: pos2(580.0, 340.0),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(app.session.doc.strokes.len(), 1);
        let stroke = &app.session.doc.strokes[0];
        assert_eq!(stroke.raw_points.len(), 3);
        assert!(stroke.raw_points[0].point().distance(expected_start) < 1e-4);
    }

    #[test]
    fn stylus_touch_with_pressure_draws_even_when_palm_holds_pointer_and_cancels() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        let palm_id = egui::TouchId(1);
        let pen_id = egui::TouchId(2);
        let dev = egui::TouchDeviceId(1);

        // Palm holds pointer button; stylus Touch starts without any paired PointerButton.
        run_editor_frame(
            &ctx,
            &mut app,
            0.016,
            vec![
                egui::Event::Touch {
                    device_id: dev,
                    id: palm_id,
                    phase: egui::TouchPhase::Start,
                    pos: pos2(300.0, 500.0),
                    force: None,
                },
                egui::Event::PointerButton {
                    pos: pos2(300.0, 500.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id,
                    phase: egui::TouchPhase::Start,
                    pos: pos2(500.0, 300.0),
                    force: Some(0.45),
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id,
                    phase: egui::TouchPhase::Move,
                    pos: pos2(550.0, 320.0),
                    force: Some(0.6),
                },
                // Palm lifting emits PointerButton(Primary, false) — must not interrupt active stylus!
                egui::Event::Touch {
                    device_id: dev,
                    id: palm_id,
                    phase: egui::TouchPhase::End,
                    pos: pos2(300.0, 500.0),
                    force: None,
                },
                egui::Event::PointerButton {
                    pos: pos2(300.0, 500.0),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert!(app.session.live_stroke.is_some());

        // Stylus Move then Cancel must commit smoothly with stylus pressure preserved.
        run_editor_frame(
            &ctx,
            &mut app,
            0.032,
            vec![
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id,
                    phase: egui::TouchPhase::Move,
                    pos: pos2(600.0, 340.0),
                    force: Some(0.8),
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id,
                    phase: egui::TouchPhase::Cancel,
                    pos: pos2(600.0, 340.0),
                    force: None,
                },
                egui::Event::PointerGone,
            ],
        );
        assert!(app.session.live_stroke.is_none());
        let last_stroke = app.session.doc.strokes.last().unwrap();
        assert_eq!(last_stroke.raw_points.len(), 3);
        assert!((last_stroke.raw_points[0].pressure - 0.45).abs() < 1e-3);
        assert!((last_stroke.raw_points[2].pressure - 0.8).abs() < 1e-3);
    }

    fn collect_circles(shape: &egui::Shape, out: &mut Vec<egui::epaint::CircleShape>) {
        match shape {
            egui::Shape::Circle(c) => out.push(*c),
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    collect_circles(s, out);
                }
            }
            _ => {}
        }
    }

    fn run_editor_frame_with_hooks(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        time: f64,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 800.0))),
            time: Some(time),
            predicted_dt: 0.016,
            events,
            ..Default::default()
        };
        <DrawCraftApp as eframe::App>::raw_input_hook(app, ctx, &mut raw);
        ctx.run_ui(raw, |ui| app.show_ui(ui))
    }

    #[test]
    fn hover_cursor_stays_glued_to_pen_after_contact_ends_without_palm_or_mouse_jumps() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU64, Ordering};

        let ctx = egui::Context::default();
        let os_pen = Arc::new(AtomicU64::new(9));
        let os_pen_hook = Arc::clone(&os_pen);

        let mut app = setup_editor_app(&ctx);
        app.hooks.pen_pointer_id = Some(Box::new(move || {
            let v = os_pen_hook.load(Ordering::Relaxed);
            (v != u64::MAX).then_some(v)
        }));

        let dev = egui::TouchDeviceId(1);
        let pen_id = egui::TouchId(9);
        let palm_id = egui::TouchId(1);

        // Nominal hovering cursor radius (cur_p = 1.0).
        let pen_cfg = app.session.active_pen();
        let nominal_r = (pen_cfg.size * 0.5 * app.view.zoom).clamp(2.0, 120.0);

        // 1. Stylus draws a stroke with low pressure at lift transition.
        let out_draw = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.016,
            vec![
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id,
                    phase: egui::TouchPhase::Start,
                    pos: pos2(500.0, 300.0),
                    force: Some(0.45),
                },
                egui::Event::PointerMoved(pos2(500.0, 300.0)),
                egui::Event::PointerButton {
                    pos: pos2(500.0, 300.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id,
                    phase: egui::TouchPhase::Move,
                    pos: pos2(520.0, 310.0),
                    force: Some(0.15),
                },
                egui::Event::PointerMoved(pos2(520.0, 310.0)),
            ],
        );
        out_draw.drop_without_applying_deltas();
        assert!(app.session.live_stroke.is_some());

        // 2. Stylus contact ends while pen stays in OS hover proximity (os_pen == 9).
        let out_lift = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.032,
            vec![
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id,
                    phase: egui::TouchPhase::End,
                    pos: pos2(525.0, 312.0),
                    force: None,
                },
                egui::Event::PointerButton {
                    pos: pos2(525.0, 312.0),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerGone,
            ],
        );
        assert!(app.session.live_stroke.is_none());
        assert_eq!(app.session.doc.strokes.len(), 1);
        assert_eq!(
            ctx.input(|i| i.pointer.latest_pos()),
            Some(pos2(525.0, 312.0))
        );
        let mut lift_circles = Vec::new();
        for cs in &out_lift.shapes {
            collect_circles(&cs.shape, &mut lift_circles);
        }
        assert!(
            lift_circles
                .iter()
                .any(|c| c.center.distance(pos2(525.0, 312.0)) < 0.5
                    && (c.radius - nominal_r).abs() < 0.5),
            "expected stabilized nominal hover circle at lift position"
        );
        out_lift.drop_without_applying_deltas();

        // 3. After contact ends, palm touch + stale OS PointerMoved + hovering pen Move arrive together.
        let out_hover = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.048,
            vec![
                egui::Event::Touch {
                    device_id: dev,
                    id: palm_id,
                    phase: egui::TouchPhase::Start,
                    pos: pos2(250.0, 520.0),
                    force: None,
                },
                egui::Event::PointerMoved(pos2(250.0, 520.0)),
                egui::Event::PointerButton {
                    pos: pos2(250.0, 520.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id,
                    phase: egui::TouchPhase::Move,
                    pos: pos2(540.0, 325.0),
                    force: None,
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: palm_id,
                    phase: egui::TouchPhase::End,
                    pos: pos2(252.0, 522.0),
                    force: None,
                },
                egui::Event::PointerButton {
                    pos: pos2(252.0, 522.0),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerGone,
                egui::Event::PointerMoved(pos2(160.0, 180.0)),
            ],
        );
        assert_eq!(app.session.doc.strokes.len(), 1);
        assert!(app.session.live_stroke.is_none());
        assert_eq!(
            ctx.input(|i| i.pointer.latest_pos()),
            Some(pos2(540.0, 325.0))
        );
        let mut hover_circles = Vec::new();
        for cs in &out_hover.shapes {
            collect_circles(&cs.shape, &mut hover_circles);
        }
        assert!(
            hover_circles
                .iter()
                .any(|c| c.center.distance(pos2(540.0, 325.0)) < 0.5),
            "expected hover cursor ring at pen position"
        );
        assert!(
            !hover_circles
                .iter()
                .any(|c| c.center.distance(pos2(250.0, 520.0)) < 2.0
                    || c.center.distance(pos2(160.0, 180.0)) < 2.0),
            "hover cursor must never jump to palm or stale OS cursor position"
        );
        out_hover.drop_without_applying_deltas();

        // 4. Pen leaves OS proximity (os_pen -> None) -> hover cursor disappears cleanly, then mouse resumes.
        os_pen.store(u64::MAX, Ordering::Relaxed);
        let out_exit = run_editor_frame_with_hooks(&ctx, &mut app, 0.064, Vec::new());
        out_exit.drop_without_applying_deltas();
        assert_eq!(ctx.input(|i| i.pointer.latest_pos()), None);

        let out_mouse = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.25,
            vec![egui::Event::PointerMoved(pos2(610.0, 360.0))],
        );
        out_mouse.drop_without_applying_deltas();
        assert_eq!(
            ctx.input(|i| i.pointer.latest_pos()),
            Some(pos2(610.0, 360.0))
        );
    }

    fn collect_line_segments(shape: &egui::Shape, out: &mut Vec<[Pos2; 2]>) {
        match shape {
            egui::Shape::LineSegment { points, .. } => out.push(*points),
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    collect_line_segments(s, out);
                }
            }
            _ => {}
        }
    }

    fn count_meshes(shape: &egui::Shape) -> usize {
        match shape {
            egui::Shape::Mesh(_) => 1,
            egui::Shape::Vec(shapes) => shapes.iter().map(count_meshes).sum(),
            _ => 0,
        }
    }

    #[test]
    fn canvas_cursor_hides_os_arrow_and_shows_hand_while_panning() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);

        let out_hover = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.016,
            vec![egui::Event::PointerMoved(pos2(640.0, 400.0))],
        );
        assert_eq!(
            out_hover.platform_output.cursor_icon,
            egui::CursorIcon::None
        );
        out_hover.drop_without_applying_deltas();

        let out_space = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.032,
            vec![
                egui::Event::Key {
                    key: egui::Key::Space,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(pos2(645.0, 400.0)),
            ],
        );
        assert_eq!(
            out_space.platform_output.cursor_icon,
            egui::CursorIcon::Grab
        );
        out_space.drop_without_applying_deltas();

        let out_mid_down = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.048,
            vec![
                egui::Event::Key {
                    key: egui::Key::Space,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: pos2(645.0, 400.0),
                    button: egui::PointerButton::Middle,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        out_mid_down.drop_without_applying_deltas();

        let out_pan = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.064,
            vec![egui::Event::PointerMoved(pos2(685.0, 425.0))],
        );
        assert!(app.view.pan_dragging);
        assert_eq!(
            out_pan.platform_output.cursor_icon,
            egui::CursorIcon::Grabbing
        );
        out_pan.drop_without_applying_deltas();

        let out_mid_up = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.080,
            vec![egui::Event::PointerButton {
                pos: pos2(685.0, 425.0),
                button: egui::PointerButton::Middle,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        out_mid_up.drop_without_applying_deltas();

        crate::modal::open(&mut app, crate::modal::ActiveModal::Export);
        let out_modal = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.096,
            vec![egui::Event::PointerMoved(pos2(640.0, 400.0))],
        );
        assert_ne!(
            out_modal.platform_output.cursor_icon,
            egui::CursorIcon::None
        );
        out_modal.drop_without_applying_deltas();
    }

    #[test]
    fn fine_nib_draws_high_contrast_locator_ticks_and_same_frame_live_ink() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.session
            .execute(
                "page.setTemplate",
                &serde_json::json!({ "template": "Blank" }),
            )
            .unwrap();
        let active_kind = app.session.active_pen().kind;
        app.session
            .execute(
                "pen.setSettings",
                &serde_json::json!({
                    "kind": active_kind,
                    "size": 1.6,
                    "color": drawcraft_color::Color { r: 0.06, g: 0.06, b: 0.06, a: 0.15 },
                }),
            )
            .unwrap();

        let hover_pos = pos2(640.0, 400.0);
        let out_hover = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.016,
            vec![egui::Event::PointerMoved(hover_pos)],
        );
        let mut circles = Vec::new();
        let mut segments = Vec::new();
        for cs in &out_hover.shapes {
            collect_circles(&cs.shape, &mut circles);
            collect_line_segments(&cs.shape, &mut segments);
        }
        assert!(
            circles
                .iter()
                .any(|c| c.center.distance(hover_pos) < 0.5 && c.stroke.color.a() >= 210),
            "expected high-contrast reticle ring with minimum alpha >= 210"
        );
        let tick_count = segments
            .iter()
            .filter(|[a, b]| a.distance(hover_pos) < 16.0 && b.distance(hover_pos) < 16.0)
            .count();
        assert!(
            tick_count >= 8,
            "expected 4 locator ticks (halo + foreground) for fine nib, got {tick_count}"
        );
        out_hover.drop_without_applying_deltas();

        // Drawing input and live-ribbon rendering must run on the exact same frame.
        let out_draw = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.032,
            vec![
                egui::Event::PointerButton {
                    pos: pos2(520.0, 350.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(pos2(575.0, 375.0)),
            ],
        );
        assert!(app.session.live_stroke.is_some());
        let live_meshes: usize = out_draw
            .shapes
            .iter()
            .map(|cs| count_meshes(&cs.shape))
            .sum();
        assert!(
            live_meshes >= 1,
            "expected same-frame live stroke mesh without 1-frame delay"
        );
        out_draw.drop_without_applying_deltas();
    }

    #[test]
    fn canvas_render_cache_batches_dotted_paper_and_reuses_batches_across_hover_and_pan() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.session
            .execute(
                "page.setTemplate",
                &serde_json::json!({ "template": "Dotted" }),
            )
            .unwrap();

        // Commit one stroke across press+move+release.
        let out_press = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.016,
            vec![
                egui::Event::PointerButton {
                    pos: pos2(500.0, 350.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(pos2(560.0, 380.0)),
            ],
        );
        out_press.drop_without_applying_deltas();

        let out_rel = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.032,
            vec![egui::Event::PointerButton {
                pos: pos2(560.0, 380.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        out_rel.drop_without_applying_deltas();
        assert_eq!(app.session.doc.strokes.len(), 1);
        assert!(
            app.canvas_cache
                .paper
                .shapes
                .iter()
                .any(|s| count_meshes(s) > 0),
            "expected Dotted paper dots to batch into GPU mesh shapes"
        );
        assert_eq!(app.canvas_cache.committed.batches.len(), 1);
        let batch_ptr = Arc::as_ptr(&app.canvas_cache.committed.batches[0]);
        assert_eq!(app.canvas_cache.committed.translate_steps, 0);

        // Cursor hover alone must reuse cached batches with zero rebuild or translation.
        let out_hover = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.048,
            vec![egui::Event::PointerMoved(pos2(610.0, 395.0))],
        );
        out_hover.drop_without_applying_deltas();
        assert_eq!(
            Arc::as_ptr(&app.canvas_cache.committed.batches[0]),
            batch_ptr
        );
        assert_eq!(app.canvas_cache.committed.translate_steps, 0);

        // Middle-drag panning must incrementally translate cached paper & committed batches.
        let out_mid = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.064,
            vec![egui::Event::PointerButton {
                pos: pos2(610.0, 395.0),
                button: egui::PointerButton::Middle,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        out_mid.drop_without_applying_deltas();

        let pan_before = app.view.pan;
        let out_pan = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.080,
            vec![egui::Event::PointerMoved(pos2(650.0, 420.0))],
        );
        out_pan.drop_without_applying_deltas();
        assert_ne!(app.view.pan, pan_before);
        assert_eq!(app.canvas_cache.committed.translate_steps, 1);
        assert_eq!(app.canvas_cache.paper.translate_steps, 1);
        assert_eq!(app.canvas_cache.committed.rendered_pan, app.view.pan);
    }

    #[test]
    fn paired_pointer_events_do_not_truncate_tail_light_pressure_samples() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        let feed = app.stylus.feed.clone();
        let pen_id = egui::TouchId::from(77_u64);

        // Hardware feed sends two samples ending in very light 0.05 pressure while OS
        // also dispatches paired PointerMoved / PointerButton events on the same frame.
        feed.push(Some(crate::stylus::PenSample {
            pressure: 0.45,
            time_ms: Some(16.0),
            ..Default::default()
        }));
        feed.push(Some(crate::stylus::PenSample {
            pressure: 0.05,
            time_ms: Some(24.0),
            ..Default::default()
        }));
        let f1 = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.016,
            vec![
                egui::Event::Touch {
                    device_id: egui::TouchDeviceId(1),
                    id: pen_id,
                    phase: egui::TouchPhase::Start,
                    pos: pos2(520.0, 360.0),
                    force: Some(0.45),
                },
                egui::Event::PointerMoved(pos2(520.0, 360.0)),
                egui::Event::PointerButton {
                    pos: pos2(520.0, 360.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Touch {
                    device_id: egui::TouchDeviceId(1),
                    id: pen_id,
                    phase: egui::TouchPhase::Move,
                    pos: pos2(560.0, 360.0),
                    force: Some(0.05),
                },
                egui::Event::PointerMoved(pos2(560.0, 360.0)),
            ],
        );
        f1.drop_without_applying_deltas();

        let live = app
            .session
            .live_stroke
            .as_ref()
            .expect("active live stroke");
        assert_eq!(live.raw_points.len(), 2);
        assert!(
            (live.raw_points[1].pressure - 0.05).abs() < 1e-4,
            "second sample must preserve tail light pressure 0.05, got {}",
            live.raw_points[1].pressure
        );

        let f_end = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.032,
            vec![egui::Event::Touch {
                device_id: egui::TouchDeviceId(1),
                id: pen_id,
                phase: egui::TouchPhase::End,
                pos: pos2(560.0, 360.0),
                force: Some(0.05),
            }],
        );
        f_end.drop_without_applying_deltas();
        assert_eq!(app.session.doc.strokes.len(), 1);
    }

    fn collect_text_strings(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    collect_text_strings(s, out);
                }
            }
            _ => {}
        }
    }

    fn collect_rect_strokes(shape: &egui::Shape, out: &mut Vec<egui::epaint::RectShape>) {
        match shape {
            egui::Shape::Rect(r) => out.push(r.clone()),
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    collect_rect_strokes(s, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn text_tool_starts_on_single_click_without_popups_and_wraps_at_page_edge() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        crate::activate_select_tool(&mut app);
        app.view.text_insert_mode = true;
        app.view.text_size_draft = 24.0;
        app.view.text_line_spacing_draft = 1.4;
        app.view.text_letter_spacing_draft = 2.0;

        let xf = page_affine(&app);
        let start_page_x = f64::from(app.session.doc.width) - 180.0;
        let click_screen = xf.to_screen(Point::new(start_page_x, 220.0));
        let click_pos = pos2(click_screen.x as f32, click_screen.y as f32);
        let page_right_screen = xf
            .to_screen(Point::new(f64::from(app.session.doc.width), 220.0))
            .x as f32;

        // 1. Single press + release (no drag!) immediately activates inline typing even from Select tool.
        let out_press = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.016,
            vec![
                egui::Event::PointerMoved(click_pos),
                egui::Event::PointerButton {
                    pos: click_pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        out_press.drop_without_applying_deltas();
        assert!(app.view.text_editing);

        let out_release = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.032,
            vec![egui::Event::PointerButton {
                pos: click_pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        out_release.drop_without_applying_deltas();
        assert!(
            app.view.text_editing,
            "single click release must not drop inline text editing"
        );
        assert!(
            ctx.memory(|m| m.has_focus(egui::Id::unique("inline_page_text_editor"))),
            "inline text editor must keep keyboard focus after click release"
        );

        let accent = crate::theme::Tokens::for_theme(app.session.prefs.theme).accent;
        // 2. Type short text and check border expands dynamically without any popup buttons.
        let out_short = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.048,
            vec![egui::Event::Text("Hi".to_string())],
        );
        let mut short_rects = Vec::new();
        for cs in &out_short.shapes {
            collect_rect_strokes(&cs.shape, &mut short_rects);
        }
        let short_border = short_rects
            .iter()
            .find(|r| {
                r.stroke.color == accent
                    && r.stroke.width > 0.5
                    && r.fill == Color32::TRANSPARENT
                    && r.rect.contains(click_pos)
            })
            .cloned()
            .expect("expected dynamic accent border around short inline text");
        out_short.drop_without_applying_deltas();
        assert_eq!(app.view.text_draft, "Hi");

        // 3. Type long text across the remaining page width and verify wrapping inside page bounds.
        let out_long = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.064,
            vec![egui::Event::Text(
                " there this sentence should wrap cleanly inside the page margin".to_string(),
            )],
        );
        let mut texts = Vec::new();
        let mut long_rects = Vec::new();
        for cs in &out_long.shapes {
            collect_text_strings(&cs.shape, &mut texts);
            collect_rect_strokes(&cs.shape, &mut long_rects);
        }
        let long_border = long_rects
            .iter()
            .find(|r| {
                r.stroke.color == accent
                    && r.stroke.width > 0.5
                    && r.fill == Color32::TRANSPARENT
                    && r.rect.contains(click_pos)
            })
            .cloned()
            .expect("expected dynamic accent border around wrapped inline text");
        out_long.drop_without_applying_deltas();

        assert!(
            long_border.rect.width() > short_border.rect.width() + 20.0,
            "expected inline text border width to expand as user types (short={}, long={})",
            short_border.rect.width(),
            long_border.rect.width()
        );
        assert!(long_border.rect.height() > short_border.rect.height() + 8.0);
        assert!(
            long_border.rect.right() <= page_right_screen + 4.0,
            "wrapped inline text border ({}) must stay inside page right edge ({page_right_screen})",
            long_border.rect.right()
        );
        for forbidden in [
            "Done",
            "Cancel",
            "Click outside to finish.",
            "Edit on page",
            "Style",
        ] {
            assert!(
                !texts.iter().any(|t| t == forbidden),
                "unexpected floating text popup label {forbidden:?} in {texts:?}"
            );
        }

        // 4. Clicking another position on the page commits the first item and starts a new one.
        let second_screen = xf.to_screen(Point::new(140.0, 480.0));
        let second_pos = pos2(second_screen.x as f32, second_screen.y as f32);
        let out_next = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.080,
            vec![
                egui::Event::PointerMoved(second_pos),
                egui::Event::PointerButton {
                    pos: second_pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        out_next.drop_without_applying_deltas();
        assert_eq!(app.session.doc.texts.len(), 1);
        assert_eq!(app.session.doc.texts[0].line_spacing, 1.4);
        assert_eq!(app.session.doc.texts[0].letter_spacing, 2.0);
        assert!(app.view.text_editing);
        assert!(app.view.text_draft.is_empty());
    }

    #[test]
    fn text_selection_has_side_width_handles_without_height_handles_and_reflows_when_edited() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        crate::activate_select_tool(&mut app);

        app.session
            .execute(
                "text.insert",
                &serde_json::json!({
                    "text": "alpha beta gamma delta epsilon zeta",
                    "x": 100.0,
                    "y": 140.0,
                    "size": 24.0,
                }),
            )
            .unwrap();
        app.view.selected_object = Some(SelectedObject::Text(0));
        app.view.selected_objects = vec![SelectedObject::Text(0)];
        app.view.selected_text = Some(0);

        let initial_frame = run_editor_frame_with_hooks(&ctx, &mut app, 0.016, Vec::new());
        initial_frame.drop_without_applying_deltas();

        let xf = page_affine(&app);
        let painter = ctx.layer_painter(egui::LayerId::background());
        let item = app.session.doc.texts[0].as_ref().sanitized();
        let initial_geom =
            text_selection_geometry(&painter, app.session.doc.width, &item, xf, app.view.zoom);
        let initial_bounds = initial_geom.aabb();

        // Top and bottom midpoints must NOT expose height-resize handles; left/right midpoints MUST expose width handles.
        let top_mid = pos2(
            initial_bounds.center().x,
            initial_bounds.top() - SELECTION_SIDE_HANDLE_OFFSET,
        );
        let bottom_mid = pos2(
            initial_bounds.center().x,
            initial_bounds.bottom() + SELECTION_SIDE_HANDLE_OFFSET,
        );
        assert!(
            selection_handle_for_app(&app, &painter, xf, top_mid).is_none(),
            "top edge must not have a height resize handle"
        );
        assert!(
            selection_handle_for_app(&app, &painter, xf, bottom_mid).is_none(),
            "bottom edge must not have a height resize handle"
        );
        assert!(matches!(
            selection_handle_for_app(&app, &painter, xf, initial_geom.right_mid),
            Some(SelectionHandle::ResizeTextWidth {
                index: 0,
                from_left: false,
                cursor: egui::CursorIcon::ResizeHorizontal
            })
        ));
        assert!(matches!(
            selection_handle_for_app(&app, &painter, xf, initial_geom.left_mid),
            Some(SelectionHandle::ResizeTextWidth {
                index: 0,
                from_left: true,
                cursor: egui::CursorIcon::ResizeHorizontal
            })
        ));

        // Dragging the right width handle inward narrows the box, keeps font size & left edge fixed, and wraps taller.
        let drag_dx_screen = -140.0 * app.view.zoom;
        let drag_target = pos2(
            initial_geom.right_mid.x + drag_dx_screen,
            initial_geom.right_mid.y,
        );
        let f_drag = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.032,
            vec![
                egui::Event::PointerMoved(initial_geom.right_mid),
                egui::Event::PointerButton {
                    pos: initial_geom.right_mid,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(drag_target),
                egui::Event::PointerButton {
                    pos: drag_target,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        f_drag.drop_without_applying_deltas();

        let resized = app.session.doc.texts[0].as_ref().sanitized();
        let established_width = resized
            .max_width
            .expect("right width drag should establish explicit max_width");
        assert!(
            (resized.x - 100.0).abs() < 1e-4,
            "right width drag must keep left anchor x=100, got {}",
            resized.x
        );
        assert_eq!(resized.size, 24.0, "width resize must not scale font size");
        assert!(established_width < initial_geom.content_width_page as f32 - 100.0);

        let resized_geom =
            text_selection_geometry(&painter, app.session.doc.width, &resized, xf, app.view.zoom);
        assert!(
            resized_geom.box_size.y > initial_geom.box_size.y + 8.0,
            "narrowing text width must grow box height automatically (before={}, after={})",
            initial_geom.box_size.y,
            resized_geom.box_size.y
        );

        // Dragging the left width handle keeps the right edge fixed while shifting x and widening max_width.
        let right_edge_before = resized.x + f64::from(established_width);
        let left_drag_target = pos2(
            resized_geom.left_mid.x - 40.0 * app.view.zoom,
            resized_geom.left_mid.y,
        );
        let f_left = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.048,
            vec![
                egui::Event::PointerMoved(resized_geom.left_mid),
                egui::Event::PointerButton {
                    pos: resized_geom.left_mid,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(left_drag_target),
                egui::Event::PointerButton {
                    pos: left_drag_target,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        f_left.drop_without_applying_deltas();

        let after_left = app.session.doc.texts[0].as_ref().sanitized();
        let after_left_w = after_left.max_width.expect("max_width after left drag");
        assert!(after_left.x < resized.x - 20.0);
        assert!(
            ((after_left.x + f64::from(after_left_w)) - right_edge_before).abs() < 0.1,
            "left handle drag must anchor right edge"
        );

        // Editing the resized text box preserves the established width and grows vertically as more text is typed.
        let accent = crate::theme::Tokens::for_theme(app.session.prefs.theme).accent;
        start_editing_text_item(&mut app, 0, &after_left);
        assert_eq!(app.view.text_max_width_draft, Some(after_left_w));

        let edit_before = run_editor_frame_with_hooks(&ctx, &mut app, 0.064, Vec::new());
        let mut edit_rects_before = Vec::new();
        for cs in &edit_before.shapes {
            collect_rect_strokes(&cs.shape, &mut edit_rects_before);
        }
        let border_before = edit_rects_before
            .iter()
            .find(|r| {
                r.stroke.color == accent && r.stroke.width > 0.5 && r.fill == Color32::TRANSPARENT
            })
            .cloned()
            .expect("expected inline editor accent border before typing");
        edit_before.drop_without_applying_deltas();

        let edit_after = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.080,
            vec![egui::Event::Text(
                " theta iota kappa lambda mu nu xi omicron pi rho".to_string(),
            )],
        );
        let mut edit_rects_after = Vec::new();
        for cs in &edit_after.shapes {
            collect_rect_strokes(&cs.shape, &mut edit_rects_after);
        }
        let border_after = edit_rects_after
            .iter()
            .find(|r| {
                r.stroke.color == accent && r.stroke.width > 0.5 && r.fill == Color32::TRANSPARENT
            })
            .cloned()
            .expect("expected inline editor accent border after typing");
        edit_after.drop_without_applying_deltas();

        assert!(
            (border_after.rect.width() - border_before.rect.width()).abs() <= 2.0,
            "editing a width-constrained text box must preserve its width (before={}, after={})",
            border_before.rect.width(),
            border_after.rect.width()
        );
        assert!(
            border_after.rect.height() > border_before.rect.height() + 10.0,
            "editing a width-constrained text box must grow vertically as text wraps"
        );

        commit_pending_text(&mut app);
        assert_eq!(app.session.doc.texts[0].max_width, Some(after_left_w));
    }

    #[test]
    fn non_text_selection_exposes_all_side_stretch_handles_and_not_text_width() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        crate::activate_select_tool(&mut app);
        app.session
            .execute(
                "shape.insert",
                &serde_json::json!({
                    "kind": "Rectangle",
                    "x1": 120.0,
                    "y1": 160.0,
                    "x2": 320.0,
                    "y2": 280.0,
                }),
            )
            .unwrap();
        app.view.selected_object = Some(SelectedObject::Shape(0));
        app.view.selected_objects = vec![SelectedObject::Shape(0)];

        let out = run_editor_frame_with_hooks(&ctx, &mut app, 0.016, Vec::new());
        out.drop_without_applying_deltas();

        let xf = page_affine(&app);
        let painter = ctx.layer_painter(egui::LayerId::background());
        let bounds = selected_objects_bounds(&app, &painter, xf).expect("shape bounds");
        let frame_rect = bounds.expand(SELECTION_FRAME_PAD);

        let [(left_mid, _), (right_mid, _), (top_mid, _), (bottom_mid, _)] =
            non_text_side_handles(bounds);
        let corner_handles = non_text_corner_handles(bounds);
        let rotate_pos = non_text_rotation_handle_center(bounds);

        // Every corner square handle must sit strictly outside the selection frame (never inside bounds).
        for corner_center in corner_handles {
            let corner_rect =
                Rect::from_center_size(corner_center, Vec2::splat(SELECTION_CORNER_HANDLE_SIZE));
            assert!(
                !frame_rect.shrink(0.5).intersects(corner_rect),
                "corner handle {corner_rect:?} must sit outside frame {frame_rect:?}"
            );
        }

        assert!(matches!(
            selection_handle_for_app(&app, &painter, xf, right_mid),
            Some(SelectionHandle::Resize {
                resize_x: true,
                resize_y: false,
                proportional: false,
                cursor: egui::CursorIcon::ResizeHorizontal,
                ..
            })
        ));
        assert!(matches!(
            selection_handle_for_app(&app, &painter, xf, left_mid),
            Some(SelectionHandle::Resize {
                resize_x: true,
                resize_y: false,
                proportional: false,
                cursor: egui::CursorIcon::ResizeHorizontal,
                ..
            })
        ));
        assert!(matches!(
            selection_handle_for_app(&app, &painter, xf, top_mid),
            Some(SelectionHandle::Resize {
                resize_x: false,
                resize_y: true,
                proportional: false,
                cursor: egui::CursorIcon::ResizeVertical,
                ..
            })
        ));
        assert!(matches!(
            selection_handle_for_app(&app, &painter, xf, bottom_mid),
            Some(SelectionHandle::Resize {
                resize_x: false,
                resize_y: true,
                proportional: false,
                cursor: egui::CursorIcon::ResizeVertical,
                ..
            })
        ));
        assert!(matches!(
            selection_handle_for_app(&app, &painter, xf, corner_handles[3]),
            Some(SelectionHandle::Resize {
                resize_x: true,
                resize_y: true,
                proportional: false,
                cursor: egui::CursorIcon::ResizeNwSe,
                ..
            })
        ));
        assert!(matches!(
            selection_handle_for_app(&app, &painter, xf, rotate_pos),
            Some(SelectionHandle::Rotate { .. })
        ));
    }

    #[test]
    fn non_text_selection_stretches_on_all_sides_and_free_or_shift_corners() {
        fn drag_handle(
            ctx: &egui::Context,
            app: &mut DrawCraftApp,
            time: &mut f64,
            from: Pos2,
            to: Pos2,
            modifiers: egui::Modifiers,
        ) {
            *time += 0.016;
            let out = run_editor_frame_with_hooks(
                ctx,
                app,
                *time,
                vec![
                    egui::Event::PointerMoved(from),
                    egui::Event::PointerButton {
                        pos: from,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers,
                    },
                    egui::Event::PointerMoved(to),
                    egui::Event::PointerButton {
                        pos: to,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers,
                    },
                ],
            );
            out.drop_without_applying_deltas();
        }

        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        crate::activate_select_tool(&mut app);
        app.session
            .execute(
                "shape.insert",
                &serde_json::json!({
                    "kind": "Rectangle",
                    "x1": 140.0,
                    "y1": 180.0,
                    "x2": 340.0,
                    "y2": 300.0,
                }),
            )
            .unwrap();
        app.view.selected_object = Some(SelectedObject::Shape(0));
        app.view.selected_objects = vec![SelectedObject::Shape(0)];

        let mut time = 0.016;
        run_editor_frame_with_hooks(&ctx, &mut app, time, Vec::new())
            .drop_without_applying_deltas();

        let painter = ctx.layer_painter(egui::LayerId::background());
        let zoom = app.view.zoom;

        // 1. Top handle stretches upward while keeping bottom edge anchored.
        let b0 = selected_objects_bounds(&app, &painter, page_affine(&app)).unwrap();
        let top0 = non_text_side_handles(b0)[2].0;
        let s0 = app.session.doc.shapes[0].clone();
        drag_handle(
            &ctx,
            &mut app,
            &mut time,
            top0,
            pos2(top0.x, top0.y - 40.0 * zoom),
            egui::Modifiers::NONE,
        );
        let s1 = &app.session.doc.shapes[0];
        assert!(s1.y1 < s0.y1 - 25.0, "top drag must move y1 upward");
        assert!((s1.y2 - s0.y2).abs() < 1.5, "top drag must anchor y2");
        assert!((s1.x1 - s0.x1).abs() < 0.5 && (s1.x2 - s0.x2).abs() < 0.5);

        // 2. Bottom handle stretches downward while keeping top edge anchored.
        let b1 = selected_objects_bounds(&app, &painter, page_affine(&app)).unwrap();
        let bottom1 = non_text_side_handles(b1)[3].0;
        let y1_before = app.session.doc.shapes[0].y1;
        let y2_before = app.session.doc.shapes[0].y2;
        drag_handle(
            &ctx,
            &mut app,
            &mut time,
            bottom1,
            pos2(bottom1.x, bottom1.y + 35.0 * zoom),
            egui::Modifiers::NONE,
        );
        let s2 = &app.session.doc.shapes[0];
        assert!(
            s2.y2 > y2_before + 20.0,
            "bottom drag must move y2 downward"
        );
        assert!(
            (s2.y1 - y1_before).abs() < 1.5,
            "bottom drag must anchor y1"
        );

        // 3. Left handle stretches leftward while keeping right edge anchored.
        let b2 = selected_objects_bounds(&app, &painter, page_affine(&app)).unwrap();
        let left2 = non_text_side_handles(b2)[0].0;
        let x1_before = app.session.doc.shapes[0].x1;
        let x2_before = app.session.doc.shapes[0].x2;
        drag_handle(
            &ctx,
            &mut app,
            &mut time,
            left2,
            pos2(left2.x - 30.0 * zoom, left2.y),
            egui::Modifiers::NONE,
        );
        let s3 = &app.session.doc.shapes[0];
        assert!(s3.x1 < x1_before - 18.0, "left drag must move x1 leftward");
        assert!((s3.x2 - x2_before).abs() < 1.5, "left drag must anchor x2");

        // 4. Right handle stretches rightward while keeping left edge anchored.
        let b3 = selected_objects_bounds(&app, &painter, page_affine(&app)).unwrap();
        let right3 = non_text_side_handles(b3)[1].0;
        let x1_before = app.session.doc.shapes[0].x1;
        let x2_before = app.session.doc.shapes[0].x2;
        drag_handle(
            &ctx,
            &mut app,
            &mut time,
            right3,
            pos2(right3.x + 45.0 * zoom, right3.y),
            egui::Modifiers::NONE,
        );
        let s4 = &app.session.doc.shapes[0];
        assert!(
            s4.x2 > x2_before + 30.0,
            "right drag must move x2 rightward"
        );
        assert!((s4.x1 - x1_before).abs() < 1.5, "right drag must anchor x1");

        // 5. Corner drag without Shift stretches width and height independently.
        let b4 = selected_objects_bounds(&app, &painter, page_affine(&app)).unwrap();
        let rb4 = non_text_corner_handles(b4)[3];
        let w_before = s4.x2 - s4.x1;
        let h_before = s4.y2 - s4.y1;
        drag_handle(
            &ctx,
            &mut app,
            &mut time,
            rb4,
            pos2(rb4.x + 100.0 * zoom, rb4.y + 5.0 * zoom),
            egui::Modifiers::NONE,
        );
        let s5 = &app.session.doc.shapes[0];
        let x_ratio = (s5.x2 - s5.x1) / w_before;
        let y_ratio = (s5.y2 - s5.y1) / h_before;
        assert!(
            x_ratio > y_ratio + 0.15,
            "corner drag without Shift must stretch freely (x_ratio={x_ratio}, y_ratio={y_ratio})"
        );

        // 6. Corner drag with Shift preserves aspect ratio.
        let b5 = selected_objects_bounds(&app, &painter, page_affine(&app)).unwrap();
        let rb5 = non_text_corner_handles(b5)[3];
        let aspect_before = (s5.x2 - s5.x1) / (s5.y2 - s5.y1);
        drag_handle(
            &ctx,
            &mut app,
            &mut time,
            rb5,
            pos2(rb5.x + 80.0 * zoom, rb5.y - 20.0 * zoom),
            egui::Modifiers::SHIFT,
        );
        let s6 = &app.session.doc.shapes[0];
        let aspect_after = (s6.x2 - s6.x1) / (s6.y2 - s6.y1);
        assert!(
            (aspect_after - aspect_before).abs() < 0.05,
            "Shift+corner drag must keep aspect ratio (before={aspect_before}, after={aspect_after})"
        );
    }

    #[test]
    fn ctrl_and_shift_corner_resize_preserves_stroke_and_shape_path_width() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        crate::activate_select_tool(&mut app);

        let mut ball_pen = drawcraft_doc::PenSettings::ball_pen();
        ball_pen.size = 2.2;
        let stroke_res = app
            .session
            .execute(
                "stroke.draw",
                &serde_json::json!({
                    "pen": ball_pen,
                    "points": [
                        { "x": 160.0, "y": 160.0, "pressure": 0.5 },
                        { "x": 180.0, "y": 195.0, "pressure": 0.5 },
                        { "x": 200.0, "y": 230.0, "pressure": 0.5 },
                        { "x": 230.0, "y": 200.0, "pressure": 0.5 },
                        { "x": 260.0, "y": 170.0, "pressure": 0.5 },
                    ],
                }),
            )
            .unwrap();
        let stroke_id = StrokeId(stroke_res["strokeId"].as_u64().unwrap());

        app.session
            .execute(
                "shape.insert",
                &serde_json::json!({
                    "kind": "Rectangle",
                    "x1": 160.0,
                    "y1": 300.0,
                    "x2": 260.0,
                    "y2": 360.0,
                    "width": 2.2,
                }),
            )
            .unwrap();

        let painter = ctx.layer_painter(egui::LayerId::background());
        let zoom = app.view.zoom;
        let mut time = 0.016;

        let drag_corner = |app: &mut DrawCraftApp, time: &mut f64, modifiers: egui::Modifiers| {
            run_editor_frame_with_hooks(&ctx, app, *time, Vec::new())
                .drop_without_applying_deltas();
            let bounds = selected_objects_bounds(app, &painter, page_affine(app)).unwrap();
            let corner = non_text_corner_handles(bounds)[3];
            let target = pos2(corner.x + 120.0 * zoom, corner.y + 40.0 * zoom);
            *time += 0.016;
            run_editor_frame_with_hooks(
                &ctx,
                app,
                *time,
                vec![
                    egui::Event::PointerMoved(corner),
                    egui::Event::PointerButton {
                        pos: corner,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers,
                    },
                    egui::Event::PointerMoved(target),
                    egui::Event::PointerButton {
                        pos: target,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers,
                    },
                ],
            )
            .drop_without_applying_deltas();
        };

        // Ctrl+corner drag scales stroke dimensions proportionally without changing pen.size.
        app.view.selected_object = Some(SelectedObject::Stroke(stroke_id));
        app.view.selected_objects = vec![SelectedObject::Stroke(stroke_id)];
        let stroke_span_before = drawcraft_geom::Rect::from_point_iter(
            app.session.doc.strokes[0]
                .smoothed_points
                .iter()
                .map(|pt| pt.point()),
        );
        let stroke_aspect_before = stroke_span_before.w / stroke_span_before.h;
        drag_corner(&mut app, &mut time, egui::Modifiers::CTRL);
        let stroke_after = &app.session.doc.strokes[0];
        let stroke_span_after = drawcraft_geom::Rect::from_point_iter(
            stroke_after.smoothed_points.iter().map(|pt| pt.point()),
        );
        let stroke_aspect_after = stroke_span_after.w / stroke_span_after.h;
        assert!(
            stroke_span_after.w > stroke_span_before.w * 1.5,
            "Ctrl+corner drag must enlarge stroke dimensions"
        );
        assert!(
            (stroke_aspect_after - stroke_aspect_before).abs() < 0.01,
            "Ctrl+corner drag must preserve stroke aspect ratio (before={stroke_aspect_before}, after={stroke_aspect_after})"
        );
        assert!(
            (stroke_after.pen.size - 2.2).abs() < 1e-5,
            "Ctrl+corner drag must keep stroke path size at 2.2, got {}",
            stroke_after.pen.size
        );

        // Shift+corner drag scales shape dimensions proportionally without changing shape.width.
        app.view.selected_object = Some(SelectedObject::Shape(0));
        app.view.selected_objects = vec![SelectedObject::Shape(0)];
        let shape_before = app.session.doc.shapes[0].as_ref().clone();
        drag_corner(&mut app, &mut time, egui::Modifiers::SHIFT);
        let shape_after = &app.session.doc.shapes[0];
        assert!(
            (shape_after.x2 - shape_after.x1) > (shape_before.x2 - shape_before.x1) * 1.5,
            "Shift+corner drag must enlarge shape dimensions"
        );
        assert!(
            (shape_after.width - 2.2).abs() < 1e-5,
            "Shift+corner drag must keep shape path width at 2.2, got {}",
            shape_after.width
        );
    }

    #[test]
    fn lasso_drag_selects_enclosed_objects_and_excludes_outside() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        crate::activate_select_tool(&mut app);
        app.view.freeform_lasso_mode = true;
        app.session
            .execute(
                "shape.insert",
                &serde_json::json!({
                    "kind": "Rectangle",
                    "x1": 160.0,
                    "y1": 160.0,
                    "x2": 220.0,
                    "y2": 220.0,
                }),
            )
            .unwrap();
        app.session
            .execute(
                "shape.insert",
                &serde_json::json!({
                    "kind": "Ellipse",
                    "x1": 520.0,
                    "y1": 520.0,
                    "x2": 620.0,
                    "y2": 620.0,
                }),
            )
            .unwrap();

        let xf = page_affine(&app);
        let loop_pts = [
            Point::new(120.0, 120.0),
            Point::new(260.0, 120.0),
            Point::new(260.0, 260.0),
            Point::new(120.0, 260.0),
            Point::new(120.0, 140.0),
        ];
        let to_scr = |p: Point| {
            let s = xf.to_screen(p);
            pos2(s.x as f32, s.y as f32)
        };
        let mut events = vec![
            egui::Event::PointerMoved(to_scr(loop_pts[0])),
            egui::Event::PointerButton {
                pos: to_scr(loop_pts[0]),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        for p in &loop_pts[1..] {
            events.push(egui::Event::PointerMoved(to_scr(*p)));
        }
        let end = to_scr(loop_pts[loop_pts.len() - 1]);
        events.push(egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });

        let out = run_editor_frame_with_hooks(&ctx, &mut app, 0.25, events);
        out.drop_without_applying_deltas();
        assert_eq!(app.view.selected_objects, vec![SelectedObject::Shape(0)]);
    }

    #[test]
    fn multiframe_mouse_and_stylus_lasso_with_empty_initial_selection_selects_strokes() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        crate::activate_select_tool(&mut app);
        app.view.freeform_lasso_mode = true;

        let draw_stroke = |app: &mut DrawCraftApp, pts: &[(f64, f64)]| -> StrokeId {
            let json_pts: Vec<_> = pts
                .iter()
                .enumerate()
                .map(|(i, &(x, y))| {
                    serde_json::json!({
                        "x": x,
                        "y": y,
                        "pressure": 0.7,
                        "time_ms": (i as f64) * 16.0,
                    })
                })
                .collect();
            let res = app
                .session
                .execute(
                    "stroke.draw",
                    &serde_json::json!({
                        "kind": "FountainPen",
                        "points": json_pts,
                    }),
                )
                .unwrap();
            StrokeId(res["strokeId"].as_u64().unwrap())
        };

        let inside_id = draw_stroke(&mut app, &[(180.0, 180.0), (210.0, 200.0), (235.0, 215.0)]);
        let _outside_id = draw_stroke(&mut app, &[(520.0, 520.0), (570.0, 540.0), (610.0, 560.0)]);

        let xf = page_affine(&app);
        let to_scr = |x: f64, y: f64| {
            let s = xf.to_screen(Point::new(x, y));
            pos2(s.x as f32, s.y as f32)
        };
        let p0 = to_scr(130.0, 130.0);
        let p_tiny = pos2(p0.x + 1.5, p0.y + 1.0);
        let p1 = to_scr(280.0, 130.0);
        let p2 = to_scr(280.0, 270.0);
        let p3 = to_scr(130.0, 270.0);

        // Frame 1: pointer down on empty selection must survive end-of-frame outline rendering.
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.016,
            vec![
                egui::Event::PointerMoved(p0),
                egui::Event::PointerButton {
                    pos: p0,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
        .drop_without_applying_deltas();
        assert!(
            matches!(
                app.view.selection_drag.as_ref().map(|d| &d.mode),
                Some(crate::SelectionDragMode::Lasso { dragged: false, .. })
            ),
            "initial lasso press with no prior selection must persist across frames"
        );

        // Frame 2: sub-threshold micro-move (<4px) must also keep lasso drag alive.
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.032,
            vec![egui::Event::PointerMoved(p_tiny)],
        )
        .drop_without_applying_deltas();
        assert!(matches!(
            app.view.selection_drag.as_ref().map(|d| &d.mode),
            Some(crate::SelectionDragMode::Lasso { .. })
        ));

        // Frames 3-5: sweep loop across multiple frames around `inside_id`.
        for (idx, pos) in [p1, p2, p3].into_iter().enumerate() {
            run_editor_frame_with_hooks(
                &ctx,
                &mut app,
                0.048 + (idx as f64) * 0.016,
                vec![egui::Event::PointerMoved(pos)],
            )
            .drop_without_applying_deltas();
        }
        assert!(matches!(
            app.view.selection_drag.as_ref().map(|d| &d.mode),
            Some(crate::SelectionDragMode::Lasso {
                dragged: true,
                points,
                ..
            }) if points.len() >= 4
        ));

        // Frame 6: release commits lasso selection of the enclosed stroke.
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.096,
            vec![egui::Event::PointerButton {
                pos: p3,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
        .drop_without_applying_deltas();
        assert!(app.view.selection_drag.is_none());
        assert_eq!(
            app.view.selected_objects,
            vec![SelectedObject::Stroke(inside_id)]
        );

        // Now clear selection and verify multi-frame stylus Touch lasso works as well.
        app.view.clear_selection();
        let dev = egui::TouchDeviceId(1);
        let pen_id = egui::TouchId(77);
        let send_pen_frame = |app: &mut DrawCraftApp,
                              t: f64,
                              phase: egui::TouchPhase,
                              pos: Pos2,
                              force: Option<f32>,
                              btn: Option<bool>| {
            let mut evs = vec![
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id,
                    phase,
                    pos,
                    force,
                },
                egui::Event::PointerMoved(pos),
            ];
            if let Some(pressed) = btn {
                evs.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            run_editor_frame_with_hooks(&ctx, app, t, evs).drop_without_applying_deltas();
        };

        send_pen_frame(
            &mut app,
            0.200,
            egui::TouchPhase::Start,
            p0,
            Some(0.35),
            Some(true),
        );
        assert!(matches!(
            app.view.selection_drag.as_ref().map(|d| (&d.mode, d.touch_id)),
            Some((crate::SelectionDragMode::Lasso { .. }, Some(id))) if id == pen_id
        ));
        send_pen_frame(
            &mut app,
            0.216,
            egui::TouchPhase::Move,
            p1,
            Some(0.55),
            None,
        );
        send_pen_frame(
            &mut app,
            0.232,
            egui::TouchPhase::Move,
            p2,
            Some(0.55),
            None,
        );
        send_pen_frame(
            &mut app,
            0.248,
            egui::TouchPhase::Move,
            p3,
            Some(0.50),
            None,
        );
        send_pen_frame(
            &mut app,
            0.264,
            egui::TouchPhase::End,
            p3,
            Some(0.0),
            Some(false),
        );
        assert!(app.view.selection_drag.is_none());
        assert_eq!(
            app.view.selected_objects,
            vec![SelectedObject::Stroke(inside_id)]
        );
    }

    #[test]
    fn pencil_stroke_and_highlighter_only_eraser_workflow() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.session
            .execute("tool.select", &serde_json::json!({ "tool": "Pencil" }))
            .unwrap();
        let xf = page_affine(&app);
        let p0 = xf.to_screen(Point::new(150.0, 200.0));
        let p1 = xf.to_screen(Point::new(320.0, 200.0));
        let s0 = pos2(p0.x as f32, p0.y as f32);
        let s1 = pos2(p1.x as f32, p1.y as f32);

        let out = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.25,
            vec![
                egui::Event::PointerMoved(s0),
                egui::Event::PointerButton {
                    pos: s0,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(s1),
                egui::Event::PointerButton {
                    pos: s1,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        out.drop_without_applying_deltas();
        assert_eq!(app.session.doc.strokes.len(), 1);
        assert_eq!(app.session.doc.strokes[0].pen.kind, PenKind::Pencil);

        app.session
            .execute("tool.select", &serde_json::json!({ "tool": "Highlighter" }))
            .unwrap();
        let out = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.35,
            vec![
                egui::Event::PointerMoved(s0),
                egui::Event::PointerButton {
                    pos: s0,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(s1),
                egui::Event::PointerButton {
                    pos: s1,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        out.drop_without_applying_deltas();
        assert_eq!(app.session.doc.strokes.len(), 2);

        app.session
            .execute("tool.select", &serde_json::json!({ "tool": "Eraser" }))
            .unwrap();
        app.session
            .execute(
                "eraser.setSettings",
                &serde_json::json!({
                    "mode": "PrecisionEraser",
                    "size": 48.0,
                    "highlighter_only": true,
                }),
            )
            .unwrap();
        let mid_a = xf.to_screen(Point::new(200.0, 200.0));
        let mid_b = xf.to_screen(Point::new(260.0, 200.0));
        let pencil_id = app.session.doc.strokes[0].id;
        let out = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.45,
            vec![
                egui::Event::PointerMoved(pos2(mid_a.x as f32, mid_a.y as f32)),
                egui::Event::PointerButton {
                    pos: pos2(mid_a.x as f32, mid_a.y as f32),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(pos2(mid_b.x as f32, mid_b.y as f32)),
                egui::Event::PointerButton {
                    pos: pos2(mid_b.x as f32, mid_b.y as f32),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        out.drop_without_applying_deltas();
        assert!(
            app.session
                .doc
                .strokes
                .iter()
                .any(|s| s.id == pencil_id && s.pen.kind == PenKind::Pencil),
            "pencil stroke must stay intact when PrecisionEraser is in highlighter_only mode"
        );

        // Now test StrokeEraser with highlighter_only=true removes all remaining highlighter fragments while leaving Pencil intact.
        app.session
            .execute(
                "eraser.setSettings",
                &serde_json::json!({
                    "mode": "StrokeEraser",
                    "size": 48.0,
                    "highlighter_only": true,
                }),
            )
            .unwrap();
        let left_end = xf.to_screen(Point::new(155.0, 200.0));
        let right_end = xf.to_screen(Point::new(315.0, 200.0));
        let out = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.55,
            vec![
                egui::Event::PointerMoved(pos2(left_end.x as f32, left_end.y as f32)),
                egui::Event::PointerButton {
                    pos: pos2(left_end.x as f32, left_end.y as f32),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(pos2(right_end.x as f32, right_end.y as f32)),
                egui::Event::PointerButton {
                    pos: pos2(right_end.x as f32, right_end.y as f32),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        out.drop_without_applying_deltas();
        assert_eq!(app.session.doc.strokes.len(), 1);
        assert_eq!(app.session.doc.strokes[0].pen.kind, PenKind::Pencil);
    }

    #[test]
    fn draw_and_hold_snaps_circle_into_geometric_shape() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.session
            .execute("tool.select", &serde_json::json!({ "tool": "FountainPen" }))
            .unwrap();
        let xf = page_affine(&app);
        let center = Point::new(320.0, 320.0);
        let radius = 85.0_f64;
        let count = 36usize;
        let pts: Vec<Pos2> = (0..=count)
            .map(|i| {
                let ang = (i as f64 / count as f64) * std::f64::consts::TAU;
                let s = xf.to_screen(Point::new(
                    center.x + radius * ang.cos(),
                    center.y + radius * ang.sin(),
                ));
                pos2(s.x as f32, s.y as f32)
            })
            .collect();

        let mut events = vec![
            egui::Event::PointerMoved(pts[0]),
            egui::Event::PointerButton {
                pos: pts[0],
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        for p in &pts[1..] {
            events.push(egui::Event::PointerMoved(*p));
        }
        let out = run_editor_frame_with_hooks(&ctx, &mut app, 1.0, events);
        out.drop_without_applying_deltas();
        assert!(app.session.live_stroke.is_some());

        // Hold motionless for >0.72s to trigger shape recognition.
        let out = run_editor_frame_with_hooks(&ctx, &mut app, 1.85, Vec::new());
        out.drop_without_applying_deltas();
        assert!(
            app.session
                .live_stroke
                .as_ref()
                .is_some_and(|l| l.recognized_shape.is_some()),
            "expected Draw & Hold to snap circle before pointer release"
        );

        let last = *pts.last().unwrap();
        let out = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            1.90,
            vec![egui::Event::PointerButton {
                pos: last,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        out.drop_without_applying_deltas();
        assert!(app.session.doc.strokes.is_empty());
        assert_eq!(app.session.doc.shapes.len(), 1);
        assert_eq!(app.session.doc.shapes[0].kind, ShapeKind::Ellipse);
    }

    #[test]
    fn shape_fill_line_styles_and_concave_polygon_render_without_panics() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.view.shape_fill_enabled = true;
        app.view.shape_fill_opacity = 0.45;
        app.view.shape_line_style = drawcraft_doc::ShapeLineStyle::Dashed;
        let star_pts: Vec<Point> = (0..10)
            .map(|i| {
                let r = if i % 2 == 0 { 52.0 } else { 22.0 };
                let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * (i as f64 / 10.0);
                Point::new(256.0 + r * a.cos(), 256.0 + r * a.sin())
            })
            .collect();
        app.session
            .execute(
                "shape.insert",
                &serde_json::json!({
                    "kind": "Polygon",
                    "x1": 204.0,
                    "y1": 204.0,
                    "x2": 308.0,
                    "y2": 308.0,
                    "points": star_pts,
                }),
            )
            .unwrap();
        app.view.selected_objects = vec![SelectedObject::Shape(0)];
        assert_eq!(app.session.doc.shapes.len(), 1);
        assert_eq!(app.session.doc.shapes[0].kind, ShapeKind::Polygon);

        app.session
            .execute(
                "shape.update",
                &serde_json::json!({
                    "index": 0,
                    "fill_color": drawcraft_color::Color::from_rgba8(0x38, 0xBD, 0xF8, 128),
                    "line_style": "Dashed",
                }),
            )
            .unwrap();
        let out = run_editor_frame_with_hooks(&ctx, &mut app, 0.25, Vec::new());
        assert!(!out.shapes.is_empty());
        out.drop_without_applying_deltas();
    }

    #[test]
    fn copy_cut_paste_and_duplicate_work_across_pages_and_notebooks() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.session
            .execute(
                "shape.insert",
                &serde_json::json!({
                    "kind": "Rectangle",
                    "x1": 100.0,
                    "y1": 100.0,
                    "x2": 220.0,
                    "y2": 180.0,
                    "line_style": "Dotted",
                }),
            )
            .unwrap();
        app.session
            .execute(
                "text.insert",
                &serde_json::json!({
                    "text": "Shared note",
                    "x": 110.0,
                    "y": 210.0,
                    "alignment": "Center",
                }),
            )
            .unwrap();
        crate::activate_select_tool(&mut app);
        app.view.selected_objects = vec![SelectedObject::Shape(0), SelectedObject::Text(0)];

        assert!(crate::duplicate_selection(&mut app));
        assert_eq!(app.session.doc.shapes.len(), 2);
        assert_eq!(app.session.doc.texts.len(), 2);

        app.view.selected_objects = vec![SelectedObject::Shape(0), SelectedObject::Text(0)];
        assert!(crate::copy_selection(&mut app, Some(&ctx), false));

        // Paste onto newly added page 2.
        app.session
            .execute("page.add", &serde_json::json!({}))
            .unwrap();
        app.session
            .execute("page.select", &serde_json::json!({ "index": 1 }))
            .unwrap();
        assert!(crate::trigger_paste_on_page(&mut app, &[]));
        assert_eq!(app.session.doc.shapes.len(), 1);
        assert_eq!(app.session.doc.texts.len(), 1);
        assert_eq!(
            app.session.doc.shapes[0].line_style,
            drawcraft_doc::ShapeLineStyle::Dotted
        );

        // Paste onto a second notebook.
        app.session
            .execute(
                "notebook.create",
                &serde_json::json!({ "title": "Target NB" }),
            )
            .unwrap();
        assert!(crate::trigger_paste_on_page(&mut app, &[]));
        assert_eq!(app.session.doc.shapes.len(), 1);
        assert_eq!(app.session.doc.texts.len(), 1);
        assert_eq!(app.session.doc.texts[0].text, "Shared note");
        assert_eq!(app.session.doc.texts[0].alignment, TextAlignment::Center);

        // Cut removes selection from active page while preserving clipboard.
        app.view.selected_objects = vec![SelectedObject::Shape(0)];
        assert!(crate::copy_selection(&mut app, Some(&ctx), true));
        assert!(app.session.doc.shapes.is_empty());
        assert!(app.session.clipboard.is_some());
    }

    #[test]
    fn rich_text_spans_alignment_and_checkbox_click_toggle() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        crate::activate_select_tool(&mut app);
        app.session
            .execute(
                "text.insert",
                &serde_json::json!({
                    "text": "[ ] Buy milk\n- Bread",
                    "x": 160.0,
                    "y": 160.0,
                    "size": 24.0,
                    "spans": [
                        { "start": 4, "end": 12, "bold": true, "italic": false, "underline": true }
                    ]
                }),
            )
            .unwrap();

        // Warm frame so fonts are installed and checkbox rects can be hit-tested.
        let out = run_editor_frame_with_hooks(&ctx, &mut app, 0.25, Vec::new());
        out.drop_without_applying_deltas();

        let xf = page_affine(&app);
        let item = app.session.doc.texts[0].as_ref().sanitized();
        let doc_width = app.session.doc.width;
        let zoom = app.view.zoom;
        let measure_prefix_rect = |target: &TextItem, time: f64| -> Rect {
            let mut found = None;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
                time: Some(time),
                ..Default::default()
            };
            let measure = ctx.run_ui(raw, |ui| {
                let painter = ui.painter();
                let galley =
                    layout_text_item_galley(painter, doc_width, target, zoom, Color32::BLACK, None);
                let rects = collect_list_prefix_rects(&galley, &target.text);
                if let Some((_, _, r)) = rects.first() {
                    found = Some(*r);
                }
            });
            measure.drop_without_applying_deltas();
            found.expect("expected rendered checkbox prefix rect")
        };
        let r_before = measure_prefix_rect(&item, 0.30);
        let origin_s = xf.to_screen(Point::new(item.x, item.y));
        let origin = pos2(origin_s.x as f32, origin_s.y as f32);
        let checkbox_click = origin + r_before.center().to_vec2();

        let out = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.35,
            vec![
                egui::Event::PointerMoved(checkbox_click),
                egui::Event::PointerButton {
                    pos: checkbox_click,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: checkbox_click,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        out.drop_without_applying_deltas();
        assert_eq!(app.session.doc.texts[0].text, "[x] Buy milk\n- Bread");

        let checked_item = app.session.doc.texts[0].as_ref().sanitized();
        let r_after = measure_prefix_rect(&checked_item, 0.40);
        assert!(
            (r_after.center().x - r_before.center().x).abs() < 1e-4
                && (r_after.width() - r_before.width()).abs() < 1e-4,
            "checkbox prefix rect must not shift horizontally when toggled: before={r_before:?}, after={r_after:?}"
        );
    }

    #[test]
    fn elements_popover_renders_stamps_starter_and_saves_deletes_user_sticker() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.view.elements_popover_open = true;
        // Frame 0 initializes egui::Area state; frame 1 paints the popover layer.
        let warmup = run_editor_frame_with_hooks(&ctx, &mut app, 0.20, Vec::new());
        warmup.drop_without_applying_deltas();
        let out = run_editor_frame_with_hooks(&ctx, &mut app, 0.25, Vec::new());
        let mut rendered = String::new();
        let mut my_tab_pos = None;
        fn visit(
            s: &egui::Shape,
            buf: &mut String,
            find_label: &str,
            found_pos: &mut Option<Pos2>,
        ) {
            match s {
                egui::Shape::Text(t) => {
                    let txt = t.galley.text();
                    buf.push_str(txt);
                    buf.push('\n');
                    if txt == find_label {
                        *found_pos = Some(t.pos + t.galley.size() * 0.5);
                    }
                }
                egui::Shape::Vec(v) => v.iter().for_each(|c| visit(c, buf, find_label, found_pos)),
                _ => {}
            }
        }
        for c in &out.shapes {
            visit(&c.shape, &mut rendered, "My Stickers (0)", &mut my_tab_pos);
        }
        out.drop_without_applying_deltas();
        let expected_presets = format!(
            "App Presets ({})",
            drawcraft_doc::SavedElement::built_ins().len()
        );
        assert!(rendered.contains("Stickers & Elements"), "got:\n{rendered}");
        assert!(rendered.contains(&expected_presets), "got:\n{rendered}");
        assert!(rendered.contains("My Stickers (0)"), "got:\n{rendered}");
        assert!(rendered.contains("Reminder"), "got:\n{rendered}");
        assert!(rendered.contains("Idea Bulb"), "got:\n{rendered}");
        assert!(rendered.contains("Gold Star"), "got:\n{rendered}");

        // Click the My Stickers switch segment and confirm the popover stays open and switches tabs.
        let tab_pos = my_tab_pos.expect("expected My Stickers tab label position");
        let click_out = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.28,
            vec![
                egui::Event::PointerMoved(tab_pos),
                egui::Event::PointerButton {
                    pos: tab_pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: tab_pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        click_out.drop_without_applying_deltas();
        assert!(app.view.elements_popover_open);
        assert_eq!(app.view.stickers_tab, crate::StickersTab::Saved);
        app.view.stickers_tab = crate::StickersTab::Starter;

        let val = app
            .session
            .execute(
                "element.place",
                &serde_json::json!({ "id": "builtin_study_cat" }),
            )
            .unwrap();
        crate::select_stamped_result(&mut app, &val);
        assert!(!app.session.doc.images.is_empty());

        let targets = crate::toolbar::selected_targets_json(&app.view.selected_objects);
        let saved = app
            .session
            .execute(
                "element.save",
                &serde_json::json!({ "name": "My Study Cat", "targets": targets }),
            )
            .unwrap();
        let saved_id = saved["element"]["id"].as_str().unwrap().to_string();
        assert_eq!(app.session.prefs.saved_elements.len(), 1);

        // Switch to the My Stickers tab and confirm only custom saved stickers render there.
        app.view.stickers_tab = crate::StickersTab::Saved;
        app.view.elements_popover_open = true;
        let out2 = run_editor_frame_with_hooks(&ctx, &mut app, 0.35, Vec::new());
        let mut rendered2 = String::new();
        let mut unused_pos = None;
        for c in &out2.shapes {
            visit(&c.shape, &mut rendered2, "", &mut unused_pos);
        }
        out2.drop_without_applying_deltas();
        assert!(rendered2.contains("My Stickers (1)"), "got:\n{rendered2}");
        assert!(rendered2.contains("My Study Cat"), "got:\n{rendered2}");
        assert!(
            !rendered2.contains("Idea Bulb"),
            "starter presets should stay hidden on My Stickers tab:\n{rendered2}"
        );

        app.session
            .execute("element.delete", &serde_json::json!({ "id": saved_id }))
            .unwrap();
        assert!(app.session.prefs.saved_elements.is_empty());

        // With no custom stickers left, My Stickers tab renders the empty-state guidance.
        app.view.elements_popover_open = true;
        let out3 = run_editor_frame_with_hooks(&ctx, &mut app, 0.45, Vec::new());
        let mut rendered3 = String::new();
        for c in &out3.shapes {
            visit(&c.shape, &mut rendered3, "", &mut unused_pos);
        }
        out3.drop_without_applying_deltas();
        assert!(
            rendered3.contains("No custom stickers yet."),
            "got:\n{rendered3}"
        );
    }

    #[test]
    fn laser_pointer_draws_glow_trail_and_expires_without_modifying_doc() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        let rev_before = app.session.revision;
        app.session
            .execute(
                "tool.select",
                &serde_json::json!({ "tool": "LaserPointer" }),
            )
            .unwrap();
        let xf = page_affine(&app);
        let a = xf.to_screen(Point::new(140.0, 140.0));
        let b = xf.to_screen(Point::new(260.0, 220.0));
        let sa = pos2(a.x as f32, a.y as f32);
        let sb = pos2(b.x as f32, b.y as f32);

        let out = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            1.0,
            vec![
                egui::Event::PointerMoved(sa),
                egui::Event::PointerButton {
                    pos: sa,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(sb),
                egui::Event::PointerButton {
                    pos: sb,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        out.drop_without_applying_deltas();
        assert!(!app.view.laser_points.is_empty());

        let out = run_editor_frame_with_hooks(&ctx, &mut app, 4.0, Vec::new());
        out.drop_without_applying_deltas();
        assert!(app.view.laser_points.is_empty());
        assert!(app.session.doc.strokes.is_empty());
        assert!(app.session.doc.shapes.is_empty());
        assert!(!app.session.history.can_undo());
        assert_eq!(app.session.revision, rev_before);
    }

    #[test]
    fn laser_pointer_renders_continuous_ribbon_mesh_without_line_segments() {
        fn collect_meshes_info(shape: &egui::Shape, out: &mut Vec<(usize, usize)>) {
            match shape {
                egui::Shape::Mesh(m) => {
                    assert_eq!(m.indices.len() % 3, 0);
                    for &idx in &m.indices {
                        assert!((idx as usize) < m.vertices.len());
                    }
                    for v in &m.vertices {
                        assert!(v.pos.x.is_finite() && v.pos.y.is_finite());
                    }
                    out.push((m.vertices.len(), m.indices.len()));
                }
                egui::Shape::Vec(shapes) => {
                    for s in shapes {
                        collect_meshes_info(s, out);
                    }
                }
                _ => {}
            }
        }

        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.session
            .execute(
                "tool.select",
                &serde_json::json!({ "tool": "LaserPointer" }),
            )
            .unwrap();

        let base_out = run_editor_frame_with_hooks(&ctx, &mut app, 0.5, Vec::new());
        let mut base_lines = Vec::new();
        let mut base_meshes = Vec::new();
        for cs in &base_out.shapes {
            collect_line_segments(&cs.shape, &mut base_lines);
            collect_meshes_info(&cs.shape, &mut base_meshes);
        }
        base_out.drop_without_applying_deltas();

        // Draw a curved multi-point laser stroke + a second disconnected stroke.
        let xf = page_affine(&app);
        let to_pos = |px: f64, py: f64| {
            let s = xf.to_screen(Point::new(px, py));
            pos2(s.x as f32, s.y as f32)
        };
        let mut events = vec![
            egui::Event::PointerMoved(to_pos(120.0, 140.0)),
            egui::Event::PointerButton {
                pos: to_pos(120.0, 140.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        for step in 1..=12 {
            let t = f64::from(step) / 12.0;
            let px = 120.0 + 180.0 * t;
            let py = 140.0 + (t * std::f64::consts::PI).sin() * 55.0;
            events.push(egui::Event::PointerMoved(to_pos(px, py)));
        }
        events.push(egui::Event::PointerButton {
            pos: to_pos(300.0, 140.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        events.push(egui::Event::PointerButton {
            pos: to_pos(150.0, 260.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        events.push(egui::Event::PointerMoved(to_pos(260.0, 290.0)));
        events.push(egui::Event::PointerButton {
            pos: to_pos(260.0, 290.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });

        let trail_out = run_editor_frame_with_hooks(&ctx, &mut app, 1.0, events);
        let mut trail_lines = Vec::new();
        let mut trail_meshes = Vec::new();
        for cs in &trail_out.shapes {
            collect_line_segments(&cs.shape, &mut trail_lines);
            collect_meshes_info(&cs.shape, &mut trail_meshes);
        }
        trail_out.drop_without_applying_deltas();

        assert_eq!(
            trail_lines.len(),
            base_lines.len(),
            "laser trail must not emit segmented line_segment primitives"
        );
        assert!(
            trail_meshes.len() > base_meshes.len(),
            "expected continuous laser ribbon mesh(es)"
        );

        // Verify smoothing helper filters non-finite coordinates and preserves run endpoints.
        let run = vec![
            crate::LaserPoint {
                x: 100.0,
                y: 100.0,
                time_s: 0.9,
                stroke_break: true,
            },
            crate::LaserPoint {
                x: f64::NAN,
                y: 110.0,
                time_s: 0.92,
                stroke_break: false,
            },
            crate::LaserPoint {
                x: 130.0,
                y: 125.0,
                time_s: 0.95,
                stroke_break: false,
            },
            crate::LaserPoint {
                x: 180.0,
                y: 115.0,
                time_s: 1.0,
                stroke_break: false,
            },
        ];
        let smoothed = smooth_laser_run(&run, Affine2::IDENTITY, 1.0, 2.2);
        assert!(smoothed.len() > 3);
        assert_eq!(smoothed.first().unwrap().point(), Point::new(100.0, 100.0));
        assert_eq!(smoothed.last().unwrap().point(), Point::new(180.0, 115.0));
        assert!(smoothed.iter().all(|s| s.point().is_finite()));
    }

    #[test]
    fn inline_text_editor_clips_to_canvas_viewport_and_aligns_without_clamping_to_zero() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.view.pages_sidebar_open = true;
        app.view.text_insert_mode = true;
        run_editor_frame_with_hooks(&ctx, &mut app, 0.02, Vec::new())
            .drop_without_applying_deltas();

        // Zoom in to 181% around canvas center and click inside the visible page.
        let mut xf = page_affine(&app);
        xf.zoom_around(
            Point::new(780.0, 400.0),
            1.81 / f64::from(app.view.zoom),
            0.15,
            6.0,
        );
        app.view.zoom = xf.scale as f32;
        app.view.pan = [xf.offset_x, xf.offset_y];
        app.session.zoom = app.view.zoom;
        run_editor_frame_with_hooks(&ctx, &mut app, 0.04, Vec::new())
            .drop_without_applying_deltas();

        let click_pos = pos2(700.0, 300.0);
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.06,
            vec![
                egui::Event::PointerMoved(click_pos),
                egui::Event::PointerButton {
                    pos: click_pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: click_pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
        .drop_without_applying_deltas();
        assert!(app.view.text_editing);

        let sample = "3rdfwqfdqwdqw_visible_tail_extension_beyond_sidebar";
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.08,
            vec![egui::Event::Text(sample.to_string())],
        )
        .drop_without_applying_deltas();

        // Pan viewport right by 800px (so page origin shifts far left, x < 0 behind sidebar).
        app.view.pan[0] -= 800.0;
        let xf_panned = page_affine(&app);
        let expected_x = xf_panned
            .to_screen(Point::new(
                app.view.text_insert_position[0],
                app.view.text_insert_position[1],
            ))
            .x as f32;
        assert!(
            expected_x < 0.0,
            "expected panned text x < 0, got {expected_x}"
        );

        let out = run_editor_frame_with_hooks(&ctx, &mut app, 0.10, Vec::new());
        let mut found: Vec<(Pos2, Rect)> = Vec::new();
        fn visit_clipped(
            shape: &egui::Shape,
            clip: Rect,
            needle: &str,
            out: &mut Vec<(Pos2, Rect)>,
        ) {
            match shape {
                egui::Shape::Text(t) if t.galley.text().contains(needle) => {
                    out.push((t.pos, clip));
                }
                egui::Shape::Vec(v) => v.iter().for_each(|c| visit_clipped(c, clip, needle, out)),
                _ => {}
            }
        }
        for cs in &out.shapes {
            visit_clipped(&cs.shape, cs.clip_rect, sample, &mut found);
        }
        out.drop_without_applying_deltas();

        assert!(!found.is_empty(), "expected inline editor text shape");
        for (pos, clip) in found {
            assert!(
                (pos.x - expected_x).abs() < 2.0,
                "editor text x={:.1} drifted from true page coordinate expected_x={:.1}",
                pos.x,
                expected_x
            );
            assert!(
                clip.left() >= 150.0,
                "inline editor clip_rect.left()={:.1} overflowed into left sidebar",
                clip.left()
            );
        }

        // Center alignment without explicit max_width must match live and committed layout widths.
        app.view.pan = [40.0, 24.0];
        app.view.zoom = 1.0;
        app.session.zoom = 1.0;
        app.view.text_alignment_draft = TextAlignment::Center;
        app.view.text_max_width_draft = None;
        let out_center_live = run_editor_frame_with_hooks(&ctx, &mut app, 0.12, Vec::new());
        fn find_rect(shapes: &[egui::epaint::ClippedShape], needle: &str) -> Option<Rect> {
            fn visit(shape: &egui::Shape, needle: &str) -> Option<Rect> {
                match shape {
                    egui::Shape::Text(t) if t.galley.text().contains(needle) => {
                        Some(t.galley.rect.translate(t.pos.to_vec2()))
                    }
                    egui::Shape::Vec(v) => v.iter().find_map(|c| visit(c, needle)),
                    _ => None,
                }
            }
            shapes.iter().find_map(|cs| visit(&cs.shape, needle))
        }
        let live_center_rect = find_rect(&out_center_live.shapes, sample);
        out_center_live.drop_without_applying_deltas();
        commit_inline_text(&mut app);
        let out_center_committed = run_editor_frame_with_hooks(&ctx, &mut app, 0.14, Vec::new());
        let committed_center_rect = find_rect(&out_center_committed.shapes, sample);
        out_center_committed.drop_without_applying_deltas();
        let live_r = live_center_rect.expect("live center rect");
        let com_r = committed_center_rect.expect("committed center rect");
        assert!(
            (live_r.left() - com_r.left()).abs() < 2.0
                && (live_r.width() - com_r.width()).abs() < 2.0,
            "Center alignment jumped between live {live_r:?} and committed {com_r:?}"
        );
    }

    #[test]
    fn bold_monospace_uses_fixed_advance_distinct_from_bold_sans() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.install_rich_fonts_if_needed(&ctx);

        let measure_width = |font: TextFont, sample: &str| -> f32 {
            let mut item = TextItem {
                text: sample.to_string(),
                size: 24.0,
                font,
                ..TextItem::default()
            };
            item.toggle_span_style(0, sample.len(), TextStyleAttr::Bold);
            let mut width = 0.0_f32;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 200.0))),
                ..Default::default()
            };
            ctx.run_ui(raw, |ui| {
                let g = layout_text_item_galley(
                    ui.painter(),
                    app.session.doc.width,
                    &item,
                    1.0,
                    Color32::BLACK,
                    None,
                );
                width = g.size().x;
            })
            .drop_without_applying_deltas();
            width
        };

        let sans_i = measure_width(TextFont::Sans, "iiiiii");
        let sans_m = measure_width(TextFont::Sans, "mmmmmm");
        let mono_i = measure_width(TextFont::Monospace, "iiiiii");
        let mono_m = measure_width(TextFont::Monospace, "mmmmmm");

        assert!(
            sans_m > sans_i * 1.4,
            "expected proportional Bold Sans to vary in width (i={sans_i}, m={sans_m})"
        );
        assert!(
            (mono_m - mono_i).abs() <= 0.5,
            "expected Bold Monospace to have fixed glyph advance (i={mono_i}, m={mono_m})"
        );
        assert!(
            (mono_i - sans_i).abs() > 10.0,
            "expected Bold Monospace to differ visually from Bold Sans"
        );
    }

    #[test]
    fn inline_text_selection_persists_across_toolbar_controls_and_styles_spans_and_whole_boxes() {
        fn find_label_center(shapes: &[egui::epaint::ClippedShape], exact: &str) -> Option<Pos2> {
            fn visit(shape: &egui::Shape, exact: &str) -> Option<Pos2> {
                match shape {
                    egui::Shape::Text(t) if t.galley.text() == exact => {
                        Some(t.pos + t.galley.size() * 0.5)
                    }
                    egui::Shape::Text(t) if t.galley.text().contains(exact) => {
                        Some(t.pos + t.galley.size() * 0.5)
                    }
                    egui::Shape::Vec(v) => v.iter().find_map(|c| visit(c, exact)),
                    _ => None,
                }
            }
            shapes.iter().find_map(|cs| visit(&cs.shape, exact))
        }
        fn find_galley_rect(shapes: &[egui::epaint::ClippedShape], needle: &str) -> Option<Rect> {
            fn visit(shape: &egui::Shape, needle: &str) -> Option<Rect> {
                match shape {
                    egui::Shape::Text(t) if t.galley.text().contains(needle) => {
                        Some(Rect::from_min_size(t.pos, t.galley.size()))
                    }
                    egui::Shape::Vec(v) => v.iter().find_map(|c| visit(c, needle)),
                    _ => None,
                }
            }
            shapes.iter().find_map(|cs| visit(&cs.shape, needle))
        }

        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.view.text_insert_mode = true;
        run_editor_frame_with_hooks(&ctx, &mut app, 0.02, Vec::new())
            .drop_without_applying_deltas();

        let xf = page_affine(&app);
        let p_sc = xf.to_screen(Point::new(150.0, 220.0));
        let cp = pos2(p_sc.x as f32, p_sc.y as f32);
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.04,
            vec![
                egui::Event::PointerMoved(cp),
                egui::Event::PointerButton {
                    pos: cp,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: cp,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
        .drop_without_applying_deltas();

        let out_typed = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.06,
            vec![egui::Event::Text("hello world".to_string())],
        );
        let text_rect = find_galley_rect(&out_typed.shapes, "hello world").expect("draft galley");
        out_typed.drop_without_applying_deltas();

        // Drag-select "world" (second half of "hello world").
        let drag_start = pos2(text_rect.right() - 1.0, text_rect.center().y);
        let drag_end = pos2(
            text_rect.left() + text_rect.width() * 0.52,
            text_rect.center().y,
        );
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.08,
            vec![
                egui::Event::PointerMoved(drag_start),
                egui::Event::PointerButton {
                    pos: drag_start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
        .drop_without_applying_deltas();
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.10,
            vec![egui::Event::PointerMoved(drag_end)],
        )
        .drop_without_applying_deltas();
        let out_sel = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.12,
            vec![egui::Event::PointerButton {
                pos: drag_end,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let sel_after_drag = app.view.text_selection_bytes.expect("drag selection");
        assert!(
            sel_after_drag[0] < sel_after_drag[1] && sel_after_drag[1] == "hello world".len(),
            "expected tail selection, got {sel_after_drag:?}"
        );
        let font_btn = find_label_center(&out_sel.shapes, "Sans").expect("Font dropdown");
        let size_btn = find_label_center(&out_sel.shapes, "24").expect("Size DragValue");
        out_sel.drop_without_applying_deltas();

        // 1. Open Font ComboBox popup (press frame -> release frame -> popup settle frame):
        // selection must stay intact while popup is open.
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.14,
            vec![
                egui::Event::PointerMoved(font_btn),
                egui::Event::PointerButton {
                    pos: font_btn,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
        .drop_without_applying_deltas();
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.15,
            vec![egui::Event::PointerButton {
                pos: font_btn,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
        .drop_without_applying_deltas();
        let out_open_font = run_editor_frame_with_hooks(&ctx, &mut app, 0.16, Vec::new());
        let mono_opt = find_label_center(&out_open_font.shapes, "Monospace");
        out_open_font.drop_without_applying_deltas();
        let mono_opt = mono_opt.expect("Monospace option in popup");
        assert_eq!(app.view.text_selection_bytes, Some(sel_after_drag));

        // Pick Monospace in popup -> applies only to selected span, preserves selection!
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.17,
            vec![
                egui::Event::PointerMoved(mono_opt),
                egui::Event::PointerButton {
                    pos: mono_opt,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
        .drop_without_applying_deltas();
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.18,
            vec![egui::Event::PointerButton {
                pos: mono_opt,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
        .drop_without_applying_deltas();
        assert_eq!(app.view.text_font_draft, TextFont::Sans);
        assert_eq!(app.view.text_selection_bytes, Some(sel_after_drag));
        let snap = TextItem {
            text: app.view.text_draft.clone(),
            font: app.view.text_font_draft,
            size: app.view.text_size_draft,
            color: app.view.text_color_draft,
            spans: app.view.text_spans_draft.clone(),
            ..TextItem::default()
        };
        assert_eq!(
            snap.effective_font_across(sel_after_drag[0], sel_after_drag[1]),
            Some(TextFont::Monospace)
        );
        assert_eq!(snap.effective_font_across(0, "hello world".len()), None);

        // 2. Click Size DragValue, type 40 + Enter -> changes selected span size only, keeps selection!
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.19,
            vec![
                egui::Event::PointerMoved(size_btn),
                egui::Event::PointerButton {
                    pos: size_btn,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
        .drop_without_applying_deltas();
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.20,
            vec![egui::Event::PointerButton {
                pos: size_btn,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
        .drop_without_applying_deltas();
        assert_eq!(app.view.text_selection_bytes, Some(sel_after_drag));
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.22,
            vec![
                egui::Event::Text("40".to_string()),
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
        .drop_without_applying_deltas();
        assert_eq!(app.view.text_size_draft, 24.0);
        assert_eq!(app.view.text_selection_bytes, Some(sel_after_drag));

        // 3. Apply color to draft selection via toolbar helper & commit -> doc stores span overrides!
        let red = drawcraft_color::Color::from_rgba8(220, 38, 38, 255);
        crate::toolbar::apply_draft_text_color(&mut app, red);
        commit_inline_text(&mut app);
        let item = &app.session.doc.texts[0];
        assert_eq!(item.font, TextFont::Sans);
        assert_eq!(item.size, 24.0);
        assert_eq!(
            item.effective_font_across(sel_after_drag[0], sel_after_drag[1]),
            Some(TextFont::Monospace)
        );
        assert_eq!(
            item.effective_size_across(sel_after_drag[0], sel_after_drag[1]),
            Some(40.0)
        );
        assert_eq!(
            item.effective_color_across(sel_after_drag[0], sel_after_drag[1]),
            Some(red)
        );

        // 4. Whole-box Select-mode update clears conflicting span overrides & toolbar shows Mixed before.
        let out_mixed = {
            app.view.text_insert_mode = false;
            app.session
                .execute("tool.select", &serde_json::json!({ "tool": "select" }))
                .unwrap();
            app.view.selected_object = Some(SelectedObject::Text(0));
            app.view.selected_objects = vec![SelectedObject::Text(0)];
            app.view.selected_text = Some(0);
            run_editor_frame_with_hooks(&ctx, &mut app, 0.22, Vec::new())
        };
        assert!(
            find_label_center(&out_mixed.shapes, "Mixed").is_some(),
            "expected Mixed font label when spans differ"
        );
        out_mixed.drop_without_applying_deltas();

        app.session
            .execute(
                "text.update",
                &serde_json::json!({ "index": 0, "font": TextFont::Monospace }),
            )
            .unwrap();
        let unified = &app.session.doc.texts[0];
        assert_eq!(
            unified.effective_font_across(0, unified.text.len()),
            Some(TextFont::Monospace)
        );
    }

    #[test]
    fn selected_stroke_renders_corner_and_side_handles_outside_selection_frame() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        crate::activate_select_tool(&mut app);

        let mut next = (*app.session.doc).clone();
        let id = next.alloc_stroke_id();
        let samples = vec![
            StrokeSample::from_xy_pressure(180.0, 180.0, 0.7),
            StrokeSample::from_xy_pressure(280.0, 310.0, 0.75),
            StrokeSample::from_xy_pressure(210.0, 440.0, 0.7),
        ];
        next.strokes.push(std::sync::Arc::new(InkStroke::new(
            id,
            app.session.active_pen().clone(),
            samples.clone(),
            samples,
        )));
        next.sync_active_to_pages();
        app.session.doc = std::sync::Arc::new(next);
        set_selection(&mut app, vec![SelectedObject::Stroke(id)]);

        let out = run_editor_frame_with_hooks(&ctx, &mut app, 0.016, Vec::new());
        let mut rect_strokes = Vec::new();
        for cs in &out.shapes {
            collect_rect_strokes(&cs.shape, &mut rect_strokes);
        }
        out.drop_without_applying_deltas();

        let xf = page_affine(&app);
        let painter = ctx.layer_painter(egui::LayerId::background());
        let bounds = selected_objects_bounds(&app, &painter, xf).expect("stroke bounds");
        let frame_rect = bounds.expand(SELECTION_FRAME_PAD);
        let accent = crate::theme::Tokens::for_theme(app.session.prefs.theme).accent;

        for corner_center in non_text_corner_handles(bounds) {
            let corner_rect =
                Rect::from_center_size(corner_center, Vec2::splat(SELECTION_CORNER_HANDLE_SIZE));
            assert!(
                !frame_rect.shrink(0.5).intersects(corner_rect),
                "corner handle {corner_rect:?} must stay outside {frame_rect:?}"
            );
            assert!(
                rect_strokes.iter().any(|r| {
                    r.stroke.color == accent
                        && (r.rect.center() - corner_center).length() < 1.0
                        && (r.rect.width() - SELECTION_CORNER_HANDLE_SIZE).abs() < 1.0
                }),
                "expected rendered corner square handle at {corner_center:?}"
            );
        }
    }

    #[test]
    fn image_stretch_resets_to_pre_drag_aspect_when_ctrl_or_shift_held_mid_drag() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        crate::activate_select_tool(&mut app);

        let mut img = drawcraft_doc::SavedElement::built_ins()
            .into_iter()
            .next()
            .and_then(|el| el.images.into_iter().next())
            .expect("built-in sticker image");
        img.x = 180.0;
        img.y = 200.0;
        img.width = 120.0;
        img.height = 120.0;
        let mut next = (*app.session.doc).clone();
        next.images.push(std::sync::Arc::new(img));
        next.sync_active_to_pages();
        app.session.doc = std::sync::Arc::new(next);
        set_selection(&mut app, vec![SelectedObject::Image(0)]);

        run_editor_frame_with_hooks(&ctx, &mut app, 0.016, Vec::new())
            .drop_without_applying_deltas();

        let painter = ctx.layer_painter(egui::LayerId::background());
        let b0 = selected_objects_bounds(&app, &painter, page_affine(&app)).unwrap();
        let corner = non_text_corner_handles(b0)[3];
        let stretched_pos = pos2(corner.x + 120.0, corner.y - 30.0);

        // 1. Start corner resize without modifier and stretch horizontally + squash vertically.
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.032,
            vec![
                egui::Event::PointerMoved(corner),
                egui::Event::PointerButton {
                    pos: corner,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(stretched_pos),
            ],
        )
        .drop_without_applying_deltas();
        let img_stretched = &app.session.doc.images[0];
        let stretched_ratio = img_stretched.width / img_stretched.height;
        assert!(
            stretched_ratio > 1.8,
            "expected horizontally stretched image, got ratio={stretched_ratio}"
        );

        // 2. Press Ctrl mid-drag (even without moving pointer): must snap back to 1:1 pre-stretch aspect!
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.048,
            vec![egui::Event::ModifiersChanged(egui::Modifiers::CTRL)],
        )
        .drop_without_applying_deltas();
        let img_ctrl = &app.session.doc.images[0];
        let ctrl_ratio = img_ctrl.width / img_ctrl.height;
        assert!(
            (ctrl_ratio - 1.0).abs() < 1e-3,
            "holding Ctrl mid-drag must restore pre-stretch 1:1 ratio, got {ctrl_ratio}"
        );
        assert!(
            (img_ctrl.x - 180.0).abs() < 1e-3 && (img_ctrl.y - 200.0).abs() < 1e-3,
            "top-left pivot must stay anchored at (180, 200), got ({}, {})",
            img_ctrl.x,
            img_ctrl.y
        );

        // 3. Move pointer while holding Ctrl -> stays locked at 1:1.
        let diag_pos = pos2(stretched_pos.x + 40.0, stretched_pos.y + 40.0);
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.064,
            vec![egui::Event::PointerMoved(diag_pos)],
        )
        .drop_without_applying_deltas();
        let img_ctrl_move = &app.session.doc.images[0];
        assert!(
            ((img_ctrl_move.width / img_ctrl_move.height) - 1.0).abs() < 1e-3,
            "moving with Ctrl held must keep pre-stretch 1:1 ratio"
        );

        // 4. Release Ctrl without moving pointer -> resumes free stretch at current pointer.
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.080,
            vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)],
        )
        .drop_without_applying_deltas();
        let img_released = &app.session.doc.images[0];
        assert!(
            (img_released.width / img_released.height) > 1.5,
            "releasing Ctrl mid-drag should resume free stretch"
        );

        // 5. Hold Shift mid-drag -> also snaps cleanly back to pre-stretch 1:1 ratio and commit.
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.096,
            vec![
                egui::Event::ModifiersChanged(egui::Modifiers::SHIFT),
                egui::Event::PointerButton {
                    pos: diag_pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::SHIFT,
                },
            ],
        )
        .drop_without_applying_deltas();
        let img_final = &app.session.doc.images[0];
        assert!(
            ((img_final.width / img_final.height) - 1.0).abs() < 1e-3,
            "Shift mid-drag must also restore pre-stretch 1:1 ratio"
        );
    }

    #[test]
    fn smart_selection_prefers_text_and_inner_content_over_enclosing_shapes_and_backdrops() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        crate::activate_select_tool(&mut app);

        // Background image covering the cell area (bottom layer).
        let mut bg_img = drawcraft_doc::SavedElement::built_ins()
            .into_iter()
            .next()
            .and_then(|el| el.images.into_iter().next())
            .expect("built-in sticker image");
        bg_img.x = 80.0;
        bg_img.y = 80.0;
        bg_img.width = 460.0;
        bg_img.height = 360.0;
        let mut next = (*app.session.doc).clone();
        next.images.push(std::sync::Arc::new(bg_img));
        next.sync_active_to_pages();
        app.session.doc = std::sync::Arc::new(next);

        // Unfilled rectangle (Shape 0) enclosing text "Weekly\nReview" (Text 0).
        app.session
            .execute(
                "shape.insert",
                &serde_json::json!({
                    "kind": "Rectangle",
                    "x1": 140.0,
                    "y1": 140.0,
                    "x2": 380.0,
                    "y2": 280.0,
                    "curve": 8.0,
                }),
            )
            .unwrap();
        app.session
            .execute(
                "text.insert",
                &serde_json::json!({
                    "text": "Weekly\nReview",
                    "x": 180.0,
                    "y": 175.0,
                    "size": 24.0,
                }),
            )
            .unwrap();

        // Wide highlighter stroke across the text (Highlighter must not mask the text).
        let hl_pen = PenSettings {
            kind: PenKind::Highlighter,
            size: 28.0,
            color: drawcraft_color::Color::from_rgba8(250, 204, 21, 140),
            ..PenSettings::default()
        };
        let mut doc_mut = (*app.session.doc).clone();
        let hl_id = doc_mut.alloc_stroke_id();
        let hl_pts = vec![
            StrokeSample::from_xy_pressure(170.0, 195.0, 0.7),
            StrokeSample::from_xy_pressure(310.0, 195.0, 0.7),
        ];
        doc_mut.strokes.push(std::sync::Arc::new(InkStroke::new(
            hl_id,
            hl_pen,
            hl_pts.clone(),
            hl_pts,
        )));

        // Small ink stroke inside the rectangle below the text.
        let ink_id = doc_mut.alloc_stroke_id();
        let ink_pts = vec![
            StrokeSample::from_xy_pressure(190.0, 255.0, 0.7),
            StrokeSample::from_xy_pressure(280.0, 255.0, 0.7),
        ];
        doc_mut.strokes.push(std::sync::Arc::new(InkStroke::new(
            ink_id,
            app.session.active_pen().clone(),
            ink_pts.clone(),
            ink_pts,
        )));
        doc_mut.sync_active_to_pages();
        app.session.doc = std::sync::Arc::new(doc_mut);

        run_editor_frame_with_hooks(&ctx, &mut app, 0.016, Vec::new())
            .drop_without_applying_deltas();

        let xf = page_affine(&app);
        let to_scr = |x: f64, y: f64| {
            let s = xf.to_screen(Point::new(x, y));
            pos2(s.x as f32, s.y as f32)
        };
        let click = |ctx: &egui::Context, app: &mut DrawCraftApp, t: f64, p: Pos2| {
            let out = run_editor_frame_with_hooks(
                ctx,
                app,
                t,
                vec![
                    egui::Event::PointerMoved(p),
                    egui::Event::PointerButton {
                        pos: p,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos: p,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            out.drop_without_applying_deltas();
        };

        // 1. Clicking on "Weekly" inside the unfilled rectangle & over highlighter & image selects Text(0).
        click(&ctx, &mut app, 0.032, to_scr(198.0, 190.0));
        assert_eq!(app.view.selected_object, Some(SelectedObject::Text(0)));

        // 2. Clicking on the ink stroke inside the unfilled rectangle & over image selects Stroke(ink_id).
        click(&ctx, &mut app, 0.048, to_scr(235.0, 255.0));
        assert_eq!(
            app.view.selected_object,
            Some(SelectedObject::Stroke(ink_id))
        );

        // 3. Clicking on the highlighter outside the text selects Highlighter stroke.
        click(&ctx, &mut app, 0.064, to_scr(300.0, 195.0));
        assert_eq!(
            app.view.selected_object,
            Some(SelectedObject::Stroke(hl_id))
        );

        // 4. Clicking on the rectangle's outline border selects Shape(0).
        click(&ctx, &mut app, 0.080, to_scr(380.0, 210.0));
        assert_eq!(app.view.selected_object, Some(SelectedObject::Shape(0)));

        // 5. Make the enclosing rectangle FILLED (translucent card) -> clicking "Weekly" STILL picks Text(0)!
        app.session
            .execute(
                "shape.update",
                &serde_json::json!({
                    "index": 0,
                    "fill_color": { "r": 0.2, "g": 0.7, "b": 0.4, "a": 0.25 },
                }),
            )
            .unwrap();
        click(&ctx, &mut app, 0.096, to_scr(198.0, 190.0));
        assert_eq!(app.view.selected_object, Some(SelectedObject::Text(0)));

        // 6. Clicking empty filled interior away from text/strokes selects the filled Shape(0).
        click(&ctx, &mut app, 0.112, to_scr(350.0, 255.0));
        assert_eq!(app.view.selected_object, Some(SelectedObject::Shape(0)));

        // 7. Double-clicking "Weekly" while Shape(0) is selected opens inline text editing for Text(0)!
        let text_pt = to_scr(198.0, 190.0);
        let mut send_btn = |t: f64, pressed: bool| {
            run_editor_frame_with_hooks(
                &ctx,
                &mut app,
                t,
                vec![
                    egui::Event::PointerMoved(text_pt),
                    egui::Event::PointerButton {
                        pos: text_pt,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            )
            .drop_without_applying_deltas();
        };
        send_btn(0.600, true);
        send_btn(0.616, false);
        send_btn(0.640, true);
        send_btn(0.656, false);
        assert!(
            app.view.text_editing && app.view.text_edit_index == Some(0),
            "double-clicking text inside filled shape must edit Text(0)"
        );
        commit_inline_text(&mut app);

        // 8. Smaller nested filled shape created BEFORE larger filled shape still wins when clicked inside!
        app.session
            .execute(
                "shape.insert",
                &serde_json::json!({
                    "kind": "Rectangle",
                    "x1": 430.0,
                    "y1": 160.0,
                    "x2": 480.0,
                    "y2": 210.0,
                    "fill_color": { "r": 0.9, "g": 0.3, "b": 0.3, "a": 0.8 },
                }),
            )
            .unwrap();
        app.session
            .execute(
                "shape.insert",
                &serde_json::json!({
                    "kind": "Rectangle",
                    "x1": 400.0,
                    "y1": 120.0,
                    "x2": 580.0,
                    "y2": 320.0,
                    "fill_color": { "r": 0.2, "g": 0.4, "b": 0.9, "a": 0.3 },
                }),
            )
            .unwrap();
        click(&ctx, &mut app, 1.100, to_scr(455.0, 185.0));
        assert_eq!(
            app.view.selected_object,
            Some(SelectedObject::Shape(1)),
            "smaller inner shape must win over larger overlapping outer shape created later"
        );

        // 9. Wide max_width text box yields its empty trailing whitespace to shapes inside that margin.
        app.session
            .execute(
                "text.insert",
                &serde_json::json!({
                    "text": "Hi",
                    "x": 100.0,
                    "y": 500.0,
                    "size": 24.0,
                    "max_width": 360.0,
                }),
            )
            .unwrap();
        app.session
            .execute(
                "shape.insert",
                &serde_json::json!({
                    "kind": "Rectangle",
                    "x1": 340.0,
                    "y1": 502.0,
                    "x2": 390.0,
                    "y2": 532.0,
                    "fill_color": { "r": 0.1, "g": 0.6, "b": 0.8, "a": 0.9 },
                }),
            )
            .unwrap();
        click(&ctx, &mut app, 1.500, to_scr(365.0, 517.0));
        assert_eq!(
            app.view.selected_object,
            Some(SelectedObject::Shape(3)),
            "small shape sitting inside empty trailing margin of wide text box must be selected"
        );
        click(&ctx, &mut app, 1.900, to_scr(108.0, 514.0));
        assert_eq!(
            app.view.selected_object,
            Some(SelectedObject::Text(1)),
            "clicking directly on text glyphs must select Text(1)"
        );
    }

    #[test]
    fn lasso_starts_inside_shapes_and_images_and_selects_crossed_or_inner_items_cleanly() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        crate::activate_select_tool(&mut app);
        app.view.freeform_lasso_mode = true;

        // Large background image + unfilled rectangle + inner text + crossing line.
        let mut bg_img = drawcraft_doc::SavedElement::built_ins()
            .into_iter()
            .next()
            .and_then(|el| el.images.into_iter().next())
            .expect("built-in sticker image");
        bg_img.x = 60.0;
        bg_img.y = 60.0;
        bg_img.width = 520.0;
        bg_img.height = 420.0;
        let mut next = (*app.session.doc).clone();
        next.images.push(std::sync::Arc::new(bg_img));
        next.sync_active_to_pages();
        app.session.doc = std::sync::Arc::new(next);

        app.session
            .execute(
                "shape.insert",
                &serde_json::json!({
                    "kind": "Rectangle",
                    "x1": 100.0,
                    "y1": 100.0,
                    "x2": 420.0,
                    "y2": 360.0,
                }),
            )
            .unwrap();
        app.session
            .execute(
                "text.insert",
                &serde_json::json!({
                    "text": "Weekly\nReview",
                    "x": 180.0,
                    "y": 175.0,
                    "size": 24.0,
                }),
            )
            .unwrap();
        // Shape(1): a line far from center whose midpoint can be crossed by a lasso.
        app.session
            .execute(
                "shape.insert",
                &serde_json::json!({
                    "kind": "Line",
                    "x1": 450.0,
                    "y1": 120.0,
                    "x2": 450.0,
                    "y2": 420.0,
                }),
            )
            .unwrap();

        run_editor_frame_with_hooks(&ctx, &mut app, 0.016, Vec::new())
            .drop_without_applying_deltas();

        let xf = page_affine(&app);
        let to_scr = |x: f64, y: f64| {
            let s = xf.to_screen(Point::new(x, y));
            pos2(s.x as f32, s.y as f32)
        };
        let drag_lasso = |ctx: &egui::Context, app: &mut DrawCraftApp, t: f64, pts: &[Pos2]| {
            let mut events = vec![
                egui::Event::PointerMoved(pts[0]),
                egui::Event::PointerButton {
                    pos: pts[0],
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            for &p in &pts[1..] {
                events.push(egui::Event::PointerMoved(p));
            }
            events.push(egui::Event::PointerButton {
                pos: pts[pts.len() - 1],
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
            run_editor_frame_with_hooks(ctx, app, t, events).drop_without_applying_deltas();
        };

        // 1. Even with Shape(0) (unfilled rectangle) ALREADY selected, starting a lasso INSIDE it
        //    around "Weekly Review" must draw a Lasso and select ONLY Text(0) (not Shape(0) or Image(0))!
        set_selection(&mut app, vec![SelectedObject::Shape(0)]);
        let inner_loop = [
            to_scr(160.0, 160.0),
            to_scr(300.0, 160.0),
            to_scr(300.0, 260.0),
            to_scr(160.0, 260.0),
            to_scr(160.0, 175.0),
        ];
        drag_lasso(&ctx, &mut app, 0.032, &inner_loop);
        assert_eq!(
            app.view.selected_objects,
            vec![SelectedObject::Text(0)],
            "lasso strictly inside outer rectangle & image must select only enclosed text"
        );
        // Ensure outer rectangle was NOT moved by dragging inside it in lasso mode.
        assert!((app.session.doc.shapes[0].x1 - 100.0).abs() < 1e-6);

        // 2. Single-click (no drag) in Lasso mode on Shape(0)'s border selects Shape(0).
        let border_pt = to_scr(100.0, 200.0);
        drag_lasso(&ctx, &mut app, 0.048, &[border_pt, border_pt]);
        assert_eq!(app.view.selected_objects, vec![SelectedObject::Shape(0)]);

        // 3. Lasso crossing only the middle of Line Shape(1) (without enclosing endpoints!) selects Shape(1).
        set_selection(&mut app, Vec::new());
        let line_cross_loop = [
            to_scr(430.0, 250.0),
            to_scr(470.0, 250.0),
            to_scr(470.0, 290.0),
            to_scr(430.0, 290.0),
            to_scr(430.0, 260.0),
        ];
        drag_lasso(&ctx, &mut app, 0.064, &line_cross_loop);
        assert_eq!(
            app.view.selected_objects,
            vec![SelectedObject::Shape(1)],
            "lasso crossing line segment between endpoints must select the line"
        );

        // 4. Box marquee strictly inside outer unfilled Shape(0) (without a backdrop image under
        //    the press point, which Box mode would directly grab) selects only Text(0), not Shape(0).
        let mut without_bg = (*app.session.doc).clone();
        without_bg.images.clear();
        without_bg.sync_active_to_pages();
        app.session.doc = std::sync::Arc::new(without_bg);
        app.view.freeform_lasso_mode = false;
        set_selection(&mut app, Vec::new());
        let box_start = to_scr(160.0, 160.0);
        let box_end = to_scr(300.0, 260.0);
        drag_lasso(&ctx, &mut app, 0.080, &[box_start, box_end]);
        assert_eq!(
            app.view.selected_objects,
            vec![SelectedObject::Text(0)],
            "box marquee inside unfilled outer rectangle must select only enclosed text"
        );
    }

    fn assert_no_duplicate_polygon_vertices(shape: &GeometricShape) {
        for (pts, closed) in shape.sanitized().path_segments() {
            for pair in pts.windows(2) {
                let d2 = (pair[0].x - pair[1].x).powi(2) + (pair[0].y - pair[1].y).powi(2);
                assert!(d2 > 1e-6, "duplicate consecutive polygon vertex: {pts:?}");
            }
            if closed && pts.len() >= 2 {
                let first = pts[0];
                let last = pts[pts.len() - 1];
                let d2 = (first.x - last.x).powi(2) + (first.y - last.y).powi(2);
                assert!(d2 > 1e-6, "duplicate closing polygon vertex: {pts:?}");
            }
        }
    }

    fn assert_cached_galleys_reused(
        app: &DrawCraftApp,
        expected: &[Option<*const egui::Galley>],
        stage: &str,
    ) {
        for (idx, prev_ptr) in expected.iter().enumerate() {
            if let Some(expected_ptr) = prev_ptr {
                assert_eq!(
                    app.canvas_cache.texts.items[idx]
                        .galley
                        .as_ref()
                        .map(Arc::as_ptr),
                    Some(*expected_ptr),
                    "visible text galley {idx} should be reused on {stage}"
                );
            }
        }
    }

    fn cached_shape_mesh_ptrs(app: &DrawCraftApp) -> Vec<*const Mesh> {
        app.canvas_cache
            .shapes
            .shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Mesh(m) => Some(Arc::as_ptr(m)),
                _ => None,
            })
            .collect()
    }

    const SHOWCASE_FIXTURE_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../notebook-exemplo/00_Showcase_All_Examples.ncraft"
    );

    fn setup_synthetic_page_two_for_cache_tests(app: &mut DrawCraftApp) {
        app.session
            .execute("page.add", &serde_json::json!({}))
            .unwrap();
        app.session
            .execute("page.select", &serde_json::json!({ "index": 1 }))
            .unwrap();

        let mut next = (*app.session.doc).clone();
        next.shapes.push(Arc::new(GeometricShape {
            kind: drawcraft_doc::ShapeKind::Polygon,
            x1: 190.0,
            y1: 110.0,
            x2: 310.0,
            y2: 210.0,
            angle_degrees: 0.0,
            curve: 24.0,
            color: drawcraft_color::Color::from_rgba8(24, 64, 128, 255),
            width: 2.0,
            fill_color: Some(drawcraft_color::Color::from_rgba8(220, 235, 255, 180)),
            line_style: drawcraft_doc::ShapeLineStyle::Solid,
            points: vec![
                Point::new(190.0, 110.0),
                Point::new(310.0, 110.0),
                Point::new(310.0, 210.0),
                Point::new(190.0, 210.0),
            ],
        }));
        next.shapes.push(Arc::new(GeometricShape {
            kind: drawcraft_doc::ShapeKind::Rectangle,
            x1: 330.0,
            y1: 120.0,
            x2: 430.0,
            y2: 210.0,
            angle_degrees: 0.0,
            curve: 0.0,
            color: drawcraft_color::Color::from_rgba8(140, 82, 20, 255),
            width: 2.0,
            fill_color: Some(drawcraft_color::Color::from_rgba8(245, 225, 195, 180)),
            line_style: drawcraft_doc::ShapeLineStyle::Solid,
            points: Vec::new(),
        }));
        next.shapes.push(Arc::new(GeometricShape {
            kind: drawcraft_doc::ShapeKind::Line,
            x1: 190.0,
            y1: 255.0,
            x2: 430.0,
            y2: 275.0,
            angle_degrees: 0.0,
            curve: 0.0,
            color: drawcraft_color::Color::from_rgba8(32, 32, 32, 255),
            width: 2.0,
            fill_color: None,
            line_style: drawcraft_doc::ShapeLineStyle::Dashed,
            points: Vec::new(),
        }));
        // Off-screen shape exercises broadphase culling and `rect_contains_rect` pan checks.
        next.shapes.push(Arc::new(GeometricShape {
            kind: drawcraft_doc::ShapeKind::Rectangle,
            x1: 620.0,
            y1: 760.0,
            x2: 740.0,
            y2: 920.0,
            angle_degrees: 0.0,
            curve: 0.0,
            color: drawcraft_color::Color::from_rgba8(32, 32, 32, 255),
            width: 2.0,
            fill_color: None,
            line_style: drawcraft_doc::ShapeLineStyle::Solid,
            points: Vec::new(),
        }));
        next.sync_active_to_pages();
        app.session.doc = Arc::new(next);

        app.session
            .execute(
                "text.insert",
                &serde_json::json!({
                    "x": 200.0,
                    "y": 125.0,
                    "text": "Zoom Cache Heading",
                    "size": 18.0
                }),
            )
            .unwrap();
        app.session
            .execute(
                "text.insert",
                &serde_json::json!({
                    "x": 200.0,
                    "y": 185.0,
                    "text": "Reference link https://example.com",
                    "size": 16.0
                }),
            )
            .unwrap();

        let pen = app.session.active_pen().sanitized();
        let pts: Vec<_> = (0..12)
            .map(|i| StrokeSample::from_xy_pressure(200.0 + (i as f64) * 10.0, 290.0, 0.7))
            .collect();
        assert!(app.session.commit_raw_stroke(pen, pts).is_some());
    }

    fn assert_page_two_zoom_cache_reuse(ctx: &egui::Context, app: &mut DrawCraftApp) {
        // Check both synthetic large-radius rounding and every Page 2 polygon for
        // consecutive/closing duplicate endpoints emitted by `round_polygon`.
        let rounded_rect = GeometricShape {
            kind: drawcraft_doc::ShapeKind::Polygon,
            x1: 0.0,
            y1: 0.0,
            x2: 100.0,
            y2: 100.0,
            angle_degrees: 0.0,
            curve: 100.0,
            color: drawcraft_color::Color::from_rgba8(0, 0, 0, 255),
            width: 2.0,
            fill_color: Some(drawcraft_color::Color::from_rgba8(255, 0, 0, 128)),
            line_style: drawcraft_doc::ShapeLineStyle::Solid,
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(100.0, 0.0),
                Point::new(100.0, 100.0),
                Point::new(0.0, 100.0),
            ],
        };
        for item in
            std::iter::once(&rounded_rect).chain(app.session.doc.shapes.iter().map(AsRef::as_ref))
        {
            assert_no_duplicate_polygon_vertices(item);
        }

        // Prime Page 2 layout and settle camera before setting zoom = 2.85 with sidebar open.
        app.view.pages_sidebar_open = true;
        run_editor_frame(ctx, app, 0.016, Vec::new());

        app.view.zoom = 2.85;
        app.session.zoom = 2.85;
        app.view.pan = [-220.0, -180.0];
        app.view.motion.camera_glide = None;
        app.view.motion.fit_glide_requested = false;

        // 1. Warmup frame at zoom = 2.85 with stationary hover + Pages sidebar visible.
        run_editor_frame(
            ctx,
            app,
            0.032,
            vec![egui::Event::PointerMoved(pos2(720.0, 420.0))],
        );
        assert!(app.canvas_cache.shapes.key.is_some());
        assert!(!app.canvas_cache.shapes.shapes.is_empty());
        assert_eq!(app.canvas_cache.shapes.translate_steps, 0);
        assert!(app.canvas_cache.texts.key.is_some());
        assert!(!app.canvas_cache.texts.items.is_empty());

        let initial_mesh_ptrs = cached_shape_mesh_ptrs(app);
        assert!(!initial_mesh_ptrs.is_empty());

        let initial_galley_ptrs: Vec<Option<*const egui::Galley>> = app
            .canvas_cache
            .texts
            .items
            .iter()
            .map(|entry| entry.galley.as_ref().map(Arc::as_ptr))
            .collect();
        assert!(initial_galley_ptrs.iter().any(Option::is_some));

        // 2. Stationary Ctrl+hover frame exercises link_at_screen_pos without rebuilding caches.
        run_editor_frame(
            ctx,
            app,
            0.048,
            vec![
                egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
                egui::Event::PointerMoved(pos2(730.0, 425.0)),
            ],
        );

        assert_eq!(app.canvas_cache.shapes.translate_steps, 0);
        assert_eq!(cached_shape_mesh_ptrs(app), initial_mesh_ptrs);
        assert_cached_galleys_reused(app, &initial_galley_ptrs, "Ctrl+hover");

        // 3. Pan frame translates cached shapes and preserves laid-out text galleys.
        app.view.pan = [-260.0, -210.0];
        run_editor_frame(ctx, app, 0.064, Vec::new());
        assert!(app.canvas_cache.shapes.key.is_some());
        assert_eq!(app.canvas_cache.shapes.translate_steps, 1);
        assert_cached_galleys_reused(app, &initial_galley_ptrs, "pan");

        // 4. Active zoom preserves baked text galleys cleanly without mid-zoom re-layout, then
        //    rebakes crisp at the settled zoom after the settle delay elapses.
        app.view.zoom = 2.95;
        app.session.zoom = 2.95;
        run_editor_frame(ctx, app, 0.080, Vec::new());
        assert_eq!(
            app.canvas_cache.shapes.key.as_ref().map(|k| k.zoom_bits),
            Some(2.95_f32.to_bits())
        );
        assert_eq!(app.canvas_cache.shapes.translate_steps, 0);
        assert_eq!(app.canvas_cache.texts.last_zoom_bits, 2.95_f32.to_bits());
        assert_eq!(app.canvas_cache.texts.baked_zoom, 2.85);
        assert!(app.canvas_cache.texts.pending_zoom_rebake);
        assert_cached_galleys_reused(app, &initial_galley_ptrs, "active zoom");

        run_editor_frame(ctx, app, 0.170, Vec::new());
        assert_eq!(app.canvas_cache.texts.baked_zoom, 2.95);
        assert!(!app.canvas_cache.texts.pending_zoom_rebake);
    }

    #[test]
    fn showcase_page_two_zoomed_reuses_shape_text_and_stroke_caches() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        setup_synthetic_page_two_for_cache_tests(&mut app);
        assert_page_two_zoom_cache_reuse(&ctx, &mut app);

        if let Ok(raw) = std::fs::read(SHOWCASE_FIXTURE_PATH) {
            let fixture_ctx = egui::Context::default();
            let mut fixture_app = setup_editor_app(&fixture_ctx);
            fixture_app
                .session
                .load_notebook_bytes(&raw, Some("00_Showcase_All_Examples.ncraft"), true)
                .unwrap();
            fixture_app
                .session
                .execute("page.select", &serde_json::json!({ "index": 1 }))
                .unwrap();
            assert_page_two_zoom_cache_reuse(&fixture_ctx, &mut fixture_app);
        }
    }

    #[test]
    fn cpu_fallback_appending_single_ink_stroke_reuses_batch_prefix() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        assert!(!app.wgpu_enabled);
        let pen = app.session.active_pen().sanitized();
        let mk_pts = |y: f64| {
            (0..20)
                .map(|i| StrokeSample::from_xy_pressure(100.0 + (i as f64) * 6.0, y, 0.7))
                .collect::<Vec<_>>()
        };
        assert!(
            app.session
                .commit_raw_stroke(pen.clone(), mk_pts(150.0))
                .is_some()
        );
        run_editor_frame(&ctx, &mut app, 0.016, Vec::new());
        assert_eq!(app.canvas_cache.committed.batches.len(), 1);
        let batch_ptr_before = Arc::as_ptr(&app.canvas_cache.committed.batches[0]);
        let verts_before = app.canvas_cache.committed.batches[0].vertices.len();

        assert!(app.session.commit_raw_stroke(pen, mk_pts(260.0)).is_some());
        run_editor_frame(&ctx, &mut app, 0.032, Vec::new());
        assert_eq!(app.canvas_cache.committed.batches.len(), 1);
        assert_eq!(
            Arc::as_ptr(&app.canvas_cache.committed.batches[0]),
            batch_ptr_before
        );
        assert!(app.canvas_cache.committed.batches[0].vertices.len() > verts_before);
    }

    #[test]
    fn dashed_and_dotted_polylines_never_stagnate_at_fractional_widths() {
        for width in [1.5_f32 * 1.9, 1.5_f32 * 2.7, 1.0_f32 * 2.7, 0.5_f32] {
            for style in [
                drawcraft_doc::ShapeLineStyle::Dashed,
                drawcraft_doc::ShapeLineStyle::Dotted,
            ] {
                let mut out = Vec::new();
                emit_polyline_with_style(
                    &mut out,
                    &[pos2(10.0, 20.0), pos2(350.0, 42.0)],
                    false,
                    width,
                    Color32::BLACK,
                    style,
                );
                assert!(
                    out.len() <= 128,
                    "expected bounded segments for width {width} style {style:?}, got {}",
                    out.len()
                );
            }
        }
    }

    fn assert_shape_and_text_caches_persist_on_ink_commit(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
    ) {
        app.view.zoom = 1.90;
        app.session.zoom = 1.90;
        app.view.motion.camera_glide = None;
        app.view.motion.fit_glide_requested = false;
        run_editor_frame(ctx, app, 0.016, Vec::new());

        let initial_shape_ptrs = cached_shape_mesh_ptrs(app);
        assert!(!initial_shape_ptrs.is_empty());
        let initial_galley_ptrs: Vec<Option<*const egui::Galley>> = app
            .canvas_cache
            .texts
            .items
            .iter()
            .map(|entry| entry.galley.as_ref().map(Arc::as_ptr))
            .collect();
        assert!(initial_galley_ptrs.iter().any(Option::is_some));

        let pen = app.session.active_pen().sanitized();
        let pts: Vec<_> = (0..12)
            .map(|i| StrokeSample::from_xy_pressure(250.0 + (i as f64) * 5.0, 400.0, 0.6))
            .collect();
        assert!(app.session.commit_raw_stroke(pen, pts).is_some());
        run_editor_frame(ctx, app, 0.032, Vec::new());

        assert_eq!(cached_shape_mesh_ptrs(app), initial_shape_ptrs);
        assert_cached_galleys_reused(app, &initial_galley_ptrs, "ink commit");
    }

    #[test]
    fn shape_and_text_caches_persist_when_ink_strokes_change() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        setup_synthetic_page_two_for_cache_tests(&mut app);
        assert_shape_and_text_caches_persist_on_ink_commit(&ctx, &mut app);

        if let Ok(raw) = std::fs::read(SHOWCASE_FIXTURE_PATH) {
            let fixture_ctx = egui::Context::default();
            let mut fixture_app = setup_editor_app(&fixture_ctx);
            assert!(
                fixture_app
                    .session
                    .load_notebook_bytes(&raw, Some("00_Showcase_All_Examples.ncraft"), true)
                    .is_ok()
            );
            assert!(
                fixture_app
                    .session
                    .execute("page.select", &serde_json::json!({ "index": 1 }))
                    .is_ok()
            );
            assert_shape_and_text_caches_persist_on_ink_commit(&fixture_ctx, &mut fixture_app);
        }
    }

    #[test]
    fn zoom_anchor_and_pinch_preserve_cursor_page_point_without_drawing() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.view.zoom = 1.25;
        app.session.zoom = 1.25;
        app.view.pan = [-120.0, -90.0];
        app.view.motion.camera_glide = None;
        app.view.motion.fit_glide_requested = false;

        let cursor = pos2(680.0, 410.0);
        run_editor_frame(
            &ctx,
            &mut app,
            0.016,
            vec![egui::Event::PointerMoved(cursor)],
        );
        let expected_page =
            page_affine(&app).to_page(Point::new(f64::from(cursor.x), f64::from(cursor.y)));

        // Continuous Ctrl+MouseWheel zoom ticks must stay locked on the cursor's page coordinate
        // and keep `view.zoom` / `session.zoom` bit-exact across every intermediate frame.
        for step in 1..=6 {
            let t = 0.016 * ((step + 1) as f64);
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 800.0))),
                time: Some(t),
                predicted_dt: 0.016,
                events: vec![
                    egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
                    egui::Event::PointerMoved(cursor),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::vec2(0.0, 0.6),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Wheel,
                        modifiers: egui::Modifiers::CTRL,
                    },
                ],
                ..Default::default()
            };
            run_editor_frame_with_hooks(&ctx, &mut app, t, raw.events)
                .drop_without_applying_deltas();

            assert!(
                (app.view.zoom - app.session.zoom).abs() < 1e-6,
                "view/session zoom diverged at step {step}"
            );
            let drifted =
                page_affine(&app).to_page(Point::new(f64::from(cursor.x), f64::from(cursor.y)));
            assert!(
                (drifted.x - expected_page.x).abs() < 1e-3
                    && (drifted.y - expected_page.y).abs() < 1e-3,
                "cursor page anchor drifted on wheel step {step}: before={expected_page:?}, after={drifted:?}"
            );
        }
        assert!(app.view.zoom > 1.25, "expected Ctrl+Wheel to increase zoom");

        // Active two-finger pinch must zoom smoothly across Touch Start -> Move and never commit
        // stray pen strokes. (`TouchState` clears `previous` on the TouchPhase::Start frame so
        // zoom deltas begin on the subsequent Move frame.)
        let zoom_before_pinch = app.view.zoom;
        let pinch_center = pos2(600.0, 380.0);
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.140,
            vec![
                egui::Event::PointerMoved(pinch_center),
                egui::Event::Touch {
                    device_id: egui::TouchDeviceId(1),
                    id: egui::TouchId(1),
                    phase: egui::TouchPhase::Start,
                    pos: pos2(pinch_center.x - 40.0, pinch_center.y),
                    force: None,
                },
                egui::Event::Touch {
                    device_id: egui::TouchDeviceId(1),
                    id: egui::TouchId(2),
                    phase: egui::TouchPhase::Start,
                    pos: pos2(pinch_center.x + 40.0, pinch_center.y),
                    force: None,
                },
            ],
        )
        .drop_without_applying_deltas();
        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.156,
            vec![
                egui::Event::Touch {
                    device_id: egui::TouchDeviceId(1),
                    id: egui::TouchId(1),
                    phase: egui::TouchPhase::Move,
                    pos: pos2(pinch_center.x - 52.0, pinch_center.y),
                    force: None,
                },
                egui::Event::Touch {
                    device_id: egui::TouchDeviceId(1),
                    id: egui::TouchId(2),
                    phase: egui::TouchPhase::Move,
                    pos: pos2(pinch_center.x + 52.0, pinch_center.y),
                    force: None,
                },
            ],
        )
        .drop_without_applying_deltas();
        assert!(app.view.zoom > zoom_before_pinch);
        assert!(app.session.doc.strokes.is_empty());
        assert!(app.session.live_stroke.is_none());
    }

    #[test]
    fn zoom_tessellation_emits_unrounded_paper_ruling_and_stable_text_quads() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        assert!(
            app.session
                .execute(
                    "page.add",
                    &serde_json::json!({ "template": "RuledNarrow" })
                )
                .is_ok()
        );
        assert!(
            app.session
                .execute(
                    "text.insert",
                    &serde_json::json!({
                        "x": 140.0,
                        "y": 140.0,
                        "text": "Cardiac physiology & subpixel zoom invariance",
                        "size": 16.0
                    }),
                )
                .is_ok()
        );
        app.view.motion.camera_glide = None;
        app.view.motion.fit_glide_requested = false;
        app.view.pan = [60.0, 80.0];

        let mut text_min_ys = Vec::new();
        let mut ruled_ys = Vec::new();
        let mut initial_galley_ptrs = Vec::new();

        for (step, zoom) in [1.000_f32, 1.003, 1.006, 1.009, 1.012, 1.015, 1.018, 1.021]
            .into_iter()
            .enumerate()
        {
            app.view.zoom = zoom;
            app.session.zoom = zoom;
            let mut raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 800.0))),
                time: Some(0.016 * ((step + 1) as f64)),
                predicted_dt: 0.016,
                ..Default::default()
            };
            raw.viewports.insert(
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    native_pixels_per_point: Some(1.25),
                    ..Default::default()
                },
            );
            <DrawCraftApp as eframe::App>::raw_input_hook(&mut app, &ctx, &mut raw);
            let mut out = ctx.run_ui(raw, |ui| app.show_ui(ui));
            let ppp = out.pixels_per_point;
            assert!((ppp - 1.25).abs() < 1e-4);
            let shapes = std::mem::take(&mut out.shapes);
            out.drop_without_applying_deltas();

            if step == 0 {
                initial_galley_ptrs = app
                    .canvas_cache
                    .texts
                    .items
                    .iter()
                    .map(|entry| entry.galley.as_ref().map(Arc::as_ptr))
                    .collect();
                assert!(initial_galley_ptrs.iter().any(Option::is_some));
            } else {
                assert_eq!(app.canvas_cache.texts.baked_zoom, 1.000);
                assert_cached_galleys_reused(&app, &initial_galley_ptrs, "fractional zoom");
            }

            let xf = page_affine(&app);
            let page_left_approx = (xf.to_screen(Point::new(0.0, 96.0)).x) as f32;
            let rule_screen_y_approx = (xf.to_screen(Point::new(0.0, 96.0)).y) as f32;
            let text_top_approx = (xf.to_screen(Point::new(140.0, 140.0)).y) as f32;
            let text_left_approx = (xf.to_screen(Point::new(140.0, 140.0)).x) as f32;

            let primitives = ctx.tessellate(shapes, ppp);
            let mut step_ruled_y: Option<f32> = None;
            let mut step_text_min_y: Option<f32> = None;

            for prim in &primitives {
                if prim.clip_rect.min.y < 40.0 || prim.clip_rect.height() < 500.0 {
                    continue;
                }
                let egui::epaint::Primitive::Mesh(mesh) = &prim.primitive else {
                    continue;
                };
                for v in &mesh.vertices {
                    if v.color.a() == 0 {
                        continue;
                    }
                    // Left endpoint of the top horizontal ruled line at page y = 96.0 (no text or
                    // margin line starts at page x = 0.0, y = 96.0).
                    if (v.pos.x - page_left_approx).abs() < 4.0
                        && (v.pos.y - rule_screen_y_approx).abs() < 2.0
                    {
                        step_ruled_y = Some(match step_ruled_y {
                            Some(cur) => cur.min(v.pos.y),
                            None => v.pos.y,
                        });
                    }
                    // Ink-colored glyph quad vertices strictly inside the committed text box.
                    if v.pos.x >= text_left_approx - 2.0
                        && v.pos.x <= text_left_approx + 200.0
                        && (v.pos.y - text_top_approx).abs() < 20.0
                        && v.color.r() < 120
                    {
                        step_text_min_y = Some(match step_text_min_y {
                            Some(cur) => cur.min(v.pos.y),
                            None => v.pos.y,
                        });
                    }
                }
            }

            let ry = step_ruled_y.expect("expected tessellated ruled line vertices in canvas");
            let ty = step_text_min_y.expect("expected tessellated text vertices in canvas");
            ruled_ys.push(ry);
            text_min_ys.push(ty);
        }

        // Across uniform +0.003 zoom increments at 1.25 HiDPI, ruled line Y and text quad Y must
        // advance smoothly on every single step without pixel-snapping stalls (dy == 0) or jumps.
        let rule_deltas: Vec<f32> = ruled_ys.windows(2).map(|w| w[1] - w[0]).collect();
        let text_deltas: Vec<f32> = text_min_ys.windows(2).map(|w| w[1] - w[0]).collect();
        for (i, &dy) in rule_deltas.iter().enumerate() {
            assert!(
                (0.20..=0.35).contains(&dy),
                "ruled line Y delta jittered at step {i}: deltas={rule_deltas:?}"
            );
        }
        for (i, &dy) in text_deltas.iter().enumerate() {
            assert!(
                (0.32..=0.52).contains(&dy),
                "text quad Y delta jittered at step {i}: deltas={text_deltas:?}"
            );
        }

        // Once zoom holds still past the settle delay, text rebakes crisp at 1.021.
        let mut settle_raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 800.0))),
            time: Some(0.250),
            predicted_dt: 0.016,
            ..Default::default()
        };
        settle_raw.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                native_pixels_per_point: Some(1.25),
                ..Default::default()
            },
        );
        <DrawCraftApp as eframe::App>::raw_input_hook(&mut app, &ctx, &mut settle_raw);
        ctx.run_ui(settle_raw, |ui| app.show_ui(ui))
            .drop_without_applying_deltas();
        assert_eq!(app.canvas_cache.texts.baked_zoom, 1.021);
        assert!(!app.canvas_cache.texts.pending_zoom_rebake);
    }

    #[test]
    fn highlighter_self_overlap_stays_uniform_while_separate_passes_layer() {
        let mut hl_pen = PenSettings::highlighter();
        hl_pen.size = 24.0;
        hl_pen.opacity = 0.42;

        // Single folded-back continuous stroke: goes right along y=150, U-turns at x=240,
        // and passes back over itself along y=156 from x=240..80.
        let mut folded_pts = Vec::new();
        for i in 0..=16 {
            folded_pts.push(StrokeSample::from_xy_pressure(
                80.0 + (i as f64) * 10.0,
                150.0,
                0.5,
            ));
        }
        for i in 1..=8 {
            let theta = (i as f64 / 8.0) * std::f64::consts::PI;
            folded_pts.push(StrokeSample::from_xy_pressure(
                240.0 + theta.sin() * 8.0,
                153.0 - theta.cos() * 3.0,
                0.5,
            ));
        }
        for i in 1..=16 {
            folded_pts.push(StrokeSample::from_xy_pressure(
                240.0 - (i as f64) * 10.0,
                156.0,
                0.5,
            ));
        }

        let xf = Affine2::new(1.0, 0.0, 0.0);
        let nodes = evaluate_spine(&hl_pen, &folded_pts);
        let ribbon = build_stroke_ribbon(&hl_pen, &nodes, 0.75);
        let mesh1 = ribbon_to_egui_mesh(&ribbon, xf, false);
        let raw_union = mesh_screen_bounds(&mesh1).unwrap();
        let avail = Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 320.0));

        // 1. Single folded-back stroke must have uniform interior alpha (no self-summing).
        let (_, _, img_single) =
            build_committed_highlighter_image(std::slice::from_ref(&mesh1), raw_union, avail, 1.0);
        let img_single = img_single.expect("expected rasterized highlighter image");
        let expected_alpha = (0.42_f32 * 255.0).round() as u8;
        let max_alpha_single = img_single.pixels.iter().map(|c| c.a()).max().unwrap_or(0);
        assert!(
            (i16::from(max_alpha_single) - i16::from(expected_alpha)).abs() <= 1,
            "single self-overlapping highlighter summed alpha: max={max_alpha_single}, expected~={expected_alpha}"
        );

        // 2. Adding a second separate highlighter stroke crossing the first must layer (> single alpha).
        let cross_pts = (0..=16)
            .map(|i| StrokeSample::from_xy_pressure(160.0, 90.0 + (i as f64) * 8.0, 0.5))
            .collect::<Vec<_>>();
        let nodes2 = evaluate_spine(&hl_pen, &cross_pts);
        let ribbon2 = build_stroke_ribbon(&hl_pen, &nodes2, 0.75);
        let mesh2 = ribbon_to_egui_mesh(&ribbon2, xf, false);
        let union2 = raw_union.union(mesh_screen_bounds(&mesh2).unwrap());
        let (_, _, img_two) =
            build_committed_highlighter_image(&[mesh1, mesh2], union2, avail, 1.0);
        let img_two = img_two.expect("expected rasterized layered highlighter image");
        let max_alpha_layered = img_two.pixels.iter().map(|c| c.a()).max().unwrap_or(0);
        assert!(
            max_alpha_layered > expected_alpha + 25,
            "subsequent highlighter pass did not layer: layered={max_alpha_layered}, single={expected_alpha}"
        );
    }

    #[test]
    fn stylus_pan_canvas_hold_pans_without_drawing_and_updates_cursor() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let ctx = egui::Context::default();
        let sec_down = Arc::new(AtomicBool::new(false));
        let sec_hook = Arc::clone(&sec_down);
        let mut app = setup_editor_app(&ctx);
        app.hooks.secondary_barrel_down = Some(Box::new(move || sec_hook.load(Ordering::Relaxed)));
        app.session
            .execute(
                "stylus.setButtons",
                &serde_json::json!({ "secondary_barrel_button": "PanCanvasHold" }),
            )
            .unwrap();

        let initial_pan = app.view.pan;
        assert_eq!(app.session.doc.strokes.len(), 0);

        // Hold secondary stylus button while hovering -> cursor switches to Grab.
        sec_down.store(true, Ordering::Relaxed);
        let out_grab = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.016,
            vec![egui::Event::PointerMoved(pos2(600.0, 400.0))],
        );
        assert!(app.stylus.pan_held());
        assert_eq!(out_grab.platform_output.cursor_icon, egui::CursorIcon::Grab);
        out_grab.drop_without_applying_deltas();

        // Press and drag with Primary pointer while PanCanvasHold is active -> pans canvas without drawing ink.
        let out_down = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.032,
            vec![egui::Event::PointerButton {
                pos: pos2(600.0, 400.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        out_down.drop_without_applying_deltas();

        let out_drag = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.048,
            vec![egui::Event::PointerMoved(pos2(660.0, 445.0))],
        );
        assert!(app.view.pan_dragging);
        assert_eq!(
            out_drag.platform_output.cursor_icon,
            egui::CursorIcon::Grabbing
        );
        assert!((app.view.pan[0] - (initial_pan[0] + 60.0)).abs() < 0.5);
        assert!((app.view.pan[1] - (initial_pan[1] + 45.0)).abs() < 0.5);
        assert_eq!(app.session.doc.strokes.len(), 0);
        assert!(app.session.live_stroke.is_none());
        out_drag.drop_without_applying_deltas();

        // Release pointer and stylus button cleanly restores normal cursor without drawing.
        sec_down.store(false, Ordering::Relaxed);
        let out_up = run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.064,
            vec![egui::Event::PointerButton {
                pos: pos2(660.0, 445.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(!app.stylus.pan_held());
        assert!(!app.view.pan_dragging);
        assert_eq!(out_up.platform_output.cursor_icon, egui::CursorIcon::None);
        assert_eq!(app.session.doc.strokes.len(), 0);
        out_up.drop_without_applying_deltas();
    }

    #[test]
    fn web_ordered_pen_touch_events_commit_single_stroke_without_initial_dot() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        let dev = egui::TouchDeviceId(0);

        // Stroke 1: web eframe event ordering with stylus pressure (PointerMoved + PointerButton
        // emitted before Touch::Start, PointerMoved before Touch::Move, PointerButton(false) +
        // PointerGone before Touch::End).
        let pen_id_1 = egui::TouchId(11);
        let p0 = pos2(460.0, 320.0);
        let p1 = pos2(520.0, 345.0);
        let p2 = pos2(590.0, 370.0);

        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.016,
            vec![
                egui::Event::PointerMoved(p0),
                egui::Event::PointerButton {
                    pos: p0,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id_1,
                    phase: egui::TouchPhase::Start,
                    pos: p0,
                    force: Some(0.15),
                },
            ],
        )
        .drop_without_applying_deltas();
        assert_eq!(
            app.session.doc.strokes.len(),
            0,
            "touchstart must not commit a 1-point dot before drag"
        );
        assert!(app.session.live_stroke.is_some());

        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.032,
            vec![
                egui::Event::PointerMoved(p1),
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id_1,
                    phase: egui::TouchPhase::Move,
                    pos: p1,
                    force: Some(0.55),
                },
            ],
        )
        .drop_without_applying_deltas();
        assert_eq!(app.session.doc.strokes.len(), 0);

        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.048,
            vec![
                egui::Event::PointerButton {
                    pos: p2,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerGone,
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id_1,
                    phase: egui::TouchPhase::End,
                    pos: p2,
                    force: Some(0.05),
                },
            ],
        )
        .drop_without_applying_deltas();
        assert!(app.session.live_stroke.is_none());
        assert_eq!(
            app.session.doc.strokes.len(),
            1,
            "web stylus stroke must commit exactly one stroke with no stray starting dot"
        );
        assert_eq!(app.session.doc.strokes[0].raw_points.len(), 3);
        assert!((app.session.doc.strokes[0].raw_points[0].pressure - 0.15).abs() < 1e-3);

        // Stroke 2: web touchstart reporting force: Some(0.0) (non-pressure touch or initial zero
        // force before ramp-up) must also commit a single stroke with no stray starting dot.
        let pen_id_2 = egui::TouchId(12);
        let q0 = pos2(460.0, 420.0);
        let q1 = pos2(540.0, 445.0);

        run_editor_frame_with_hooks(
            &ctx,
            &mut app,
            0.080,
            vec![
                egui::Event::PointerMoved(q0),
                egui::Event::PointerButton {
                    pos: q0,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id_2,
                    phase: egui::TouchPhase::Start,
                    pos: q0,
                    force: Some(0.0),
                },
                egui::Event::PointerMoved(q1),
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id_2,
                    phase: egui::TouchPhase::Move,
                    pos: q1,
                    force: Some(0.0),
                },
                egui::Event::PointerButton {
                    pos: q1,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerGone,
                egui::Event::Touch {
                    device_id: dev,
                    id: pen_id_2,
                    phase: egui::TouchPhase::End,
                    pos: q1,
                    force: Some(0.0),
                },
            ],
        )
        .drop_without_applying_deltas();
        assert!(app.session.live_stroke.is_none());
        assert_eq!(app.session.doc.strokes.len(), 2);
        assert_eq!(app.session.doc.strokes[1].raw_points.len(), 2);
    }

    fn finger_drag_events(dev: egui::TouchDeviceId, from: Pos2, to: Pos2) -> Vec<egui::Event> {
        let id = egui::TouchId(77);
        let touch = |phase, pos| egui::Event::Touch {
            device_id: dev,
            id,
            phase,
            pos,
            force: Some(0.0),
        };
        vec![
            touch(egui::TouchPhase::Start, from),
            touch(egui::TouchPhase::Move, to),
            touch(egui::TouchPhase::End, to),
        ]
    }

    /// Events as eframe delivers them on iPadOS Safari: every touch (finger or Pencil) arrives as
    /// `PointerButton` + `Touch`, so pairing alone cannot tell them apart.
    fn safari_paired_drag(
        dev: egui::TouchDeviceId,
        id: u64,
        from: Pos2,
        to: Pos2,
        force: f32,
    ) -> Vec<egui::Event> {
        let touch = |phase, pos, f| egui::Event::Touch {
            device_id: dev,
            id: egui::TouchId(id),
            phase,
            pos,
            force: Some(f),
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![
            egui::Event::PointerMoved(from),
            button(from, true),
            touch(egui::TouchPhase::Start, from, force),
            egui::Event::PointerMoved(to),
            touch(egui::TouchPhase::Move, to, force),
            button(to, false),
            egui::Event::PointerGone,
            touch(egui::TouchPhase::End, to, force),
        ]
    }

    #[test]
    fn a_paired_finger_never_draws_after_the_pencil_was_seen_and_pans_instead() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.stylus.hardware_seen_in_session = true;
        app.view
            .study
            .set_touch_source(31, crate::TouchSource::Direct);
        let pan_before = app.view.pan;
        run_editor_frame(
            &ctx,
            &mut app,
            0.016,
            safari_paired_drag(
                egui::TouchDeviceId(0),
                31,
                pos2(450.0, 310.0),
                pos2(520.0, 360.0),
                0.0,
            ),
        );
        assert!(app.session.doc.strokes.is_empty());
        assert!(app.session.live_stroke.is_none());
        assert!(
            (app.view.pan[0] - pan_before[0]).abs() > 1.0
                || (app.view.pan[1] - pan_before[1]).abs() > 1.0,
            "the page followed the finger"
        );
    }

    #[test]
    fn a_paired_apple_pencil_touch_still_draws_with_pencil_only_mode_on() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.view
            .study
            .set_touch_source(32, crate::TouchSource::Stylus);
        app.stylus.hardware_seen_in_session = true;
        run_editor_frame(
            &ctx,
            &mut app,
            0.016,
            safari_paired_drag(
                egui::TouchDeviceId(0),
                32,
                pos2(450.0, 310.0),
                pos2(530.0, 340.0),
                0.5,
            ),
        );
        assert_eq!(app.session.doc.strokes.len(), 1, "the Pencil draws");
    }

    #[test]
    fn a_finger_after_pencil_contact_cannot_use_shape_select_or_eraser_tools() {
        for setup in 0..3 {
            let ctx = egui::Context::default();
            let mut app = setup_editor_app(&ctx);
            app.stylus.hardware_seen_in_session = true;
            app.view
                .study
                .set_touch_source(40 + setup, crate::TouchSource::Direct);
            match setup {
                0 => app.view.shape_insert_mode = true,
                1 => {
                    let _ = app
                        .session
                        .execute("tool.select", &serde_json::json!({ "tool": "Select" }));
                }
                _ => {
                    let _ = app
                        .session
                        .execute("tool.select", &serde_json::json!({ "tool": "Eraser" }));
                }
            }
            let shapes_before = app.session.doc.shapes.len();
            run_editor_frame(
                &ctx,
                &mut app,
                0.016,
                safari_paired_drag(
                    egui::TouchDeviceId(0),
                    40 + setup as u64,
                    pos2(450.0, 310.0),
                    pos2(540.0, 380.0),
                    0.0,
                ),
            );
            assert!(app.session.doc.strokes.is_empty(), "tool {setup}");
            assert_eq!(app.session.doc.shapes.len(), shapes_before, "tool {setup}");
            assert!(app.view.shape_drag_start.is_none(), "tool {setup}");
        }
    }

    #[test]
    fn the_finger_may_draw_when_the_user_enabled_finger_drawing() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.stylus.hardware_seen_in_session = true;
        let _ = app.session.execute(
            "prefs.setStudy",
            &serde_json::json!({ "finger_drawing": true }),
        );
        app.view
            .study
            .set_touch_source(33, crate::TouchSource::Direct);
        run_editor_frame(
            &ctx,
            &mut app,
            0.016,
            safari_paired_drag(
                egui::TouchDeviceId(0),
                33,
                pos2(450.0, 310.0),
                pos2(530.0, 340.0),
                0.0,
            ),
        );
        assert_eq!(app.session.doc.strokes.len(), 1);
    }

    #[test]
    fn pencil_only_mode_pans_with_one_finger_instead_of_drawing() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.stylus.hardware_seen_in_session = true;
        let dev = egui::TouchDeviceId(0);
        let pan_before = app.view.pan;
        run_editor_frame(
            &ctx,
            &mut app,
            0.016,
            finger_drag_events(dev, pos2(450.0, 310.0), pos2(500.0, 350.0)),
        );
        assert!(
            app.session.doc.strokes.is_empty(),
            "a finger never draws in Pencil-only mode"
        );
        assert!(app.session.live_stroke.is_none());
        assert!(
            (app.view.pan[0] - pan_before[0]).abs() > 1.0
                || (app.view.pan[1] - pan_before[1]).abs() > 1.0,
            "one finger pans the page"
        );
    }

    #[test]
    fn safari_synchronous_pointerdown_touch_frame_never_draws() {
        // Reproduces eframe's synchronous `runner.logic()` inside `pointerdown("touch")` on
        // iPadOS Safari, which delivers only `PointerButton(Primary, true)` before `touchstart`.
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.view.study.pointer_is_direct_touch = true;
        run_editor_frame(
            &ctx,
            &mut app,
            0.016,
            vec![
                egui::Event::PointerMoved(pos2(450.0, 310.0)),
                egui::Event::PointerButton {
                    pos: pos2(450.0, 310.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(pos2(510.0, 350.0)),
                egui::Event::PointerButton {
                    pos: pos2(510.0, 350.0),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert!(
            app.session.doc.strokes.is_empty(),
            "finger pointerdown must never draw when finger_drawing is off"
        );
        assert!(app.session.live_stroke.is_none());
    }

    #[test]
    fn direct_finger_touch_never_draws_even_before_first_apple_pencil_contact() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        // Fresh iPad session: no Pencil seen yet (`hardware_seen_in_session == false`).
        app.view
            .study
            .set_touch_source(91, crate::TouchSource::Direct);
        let pan_before = app.view.pan;
        run_editor_frame(
            &ctx,
            &mut app,
            0.016,
            safari_paired_drag(
                egui::TouchDeviceId(0),
                91,
                pos2(450.0, 310.0),
                pos2(520.0, 360.0),
                0.0,
            ),
        );
        assert!(
            app.session.doc.strokes.is_empty(),
            "finger must not draw on iPad even before first Pencil contact"
        );
        assert!(
            (app.view.pan[0] - pan_before[0]).abs() > 1.0
                || (app.view.pan[1] - pan_before[1]).abs() > 1.0,
            "finger pans the page instead"
        );
    }

    #[test]
    fn finger_drawing_preference_lets_a_finger_draw_again() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.stylus.hardware_seen_in_session = true;
        let _ = app.session.execute(
            "prefs.setStudy",
            &serde_json::json!({ "finger_drawing": true }),
        );
        let dev = egui::TouchDeviceId(0);
        run_editor_frame(
            &ctx,
            &mut app,
            0.016,
            finger_drag_events(dev, pos2(450.0, 310.0), pos2(530.0, 340.0)),
        );
        assert_eq!(
            app.session.doc.strokes.len(),
            1,
            "finger drawing is enabled"
        );
    }

    #[test]
    fn web_ordered_single_frame_and_cross_frame_pointer_touch_never_commit_initial_dot() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        let dev = egui::TouchDeviceId(0);

        // Case A: direct canvas input (without raw_input_hook) receiving web-ordered PointerButton
        // before Touch::Start with force: Some(0.0).
        let a0 = pos2(450.0, 310.0);
        let a1 = pos2(530.0, 340.0);
        run_editor_frame(
            &ctx,
            &mut app,
            0.016,
            vec![
                egui::Event::PointerMoved(a0),
                egui::Event::PointerButton {
                    pos: a0,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: egui::TouchId(21),
                    phase: egui::TouchPhase::Start,
                    pos: a0,
                    force: Some(0.0),
                },
                egui::Event::PointerMoved(a1),
                egui::Event::Touch {
                    device_id: dev,
                    id: egui::TouchId(21),
                    phase: egui::TouchPhase::Move,
                    pos: a1,
                    force: Some(0.4),
                },
                egui::Event::PointerButton {
                    pos: a1,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerGone,
                egui::Event::Touch {
                    device_id: dev,
                    id: egui::TouchId(21),
                    phase: egui::TouchPhase::End,
                    pos: a1,
                    force: Some(0.0),
                },
            ],
        );
        assert_eq!(app.session.doc.strokes.len(), 1);
        assert_eq!(app.session.doc.strokes[0].raw_points.len(), 2);

        // Case B: direct canvas input with web-ordered PointerButton before Touch::Start where
        // force is None — pointer owns the stroke without duplicating on Touch::Start.
        let b0 = pos2(450.0, 390.0);
        let b1 = pos2(530.0, 415.0);
        run_editor_frame(
            &ctx,
            &mut app,
            0.048,
            vec![
                egui::Event::PointerMoved(b0),
                egui::Event::PointerButton {
                    pos: b0,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: egui::TouchId(22),
                    phase: egui::TouchPhase::Start,
                    pos: b0,
                    force: None,
                },
                egui::Event::PointerMoved(b1),
                egui::Event::Touch {
                    device_id: dev,
                    id: egui::TouchId(22),
                    phase: egui::TouchPhase::Move,
                    pos: b1,
                    force: None,
                },
                egui::Event::PointerButton {
                    pos: b1,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerGone,
                egui::Event::Touch {
                    device_id: dev,
                    id: egui::TouchId(22),
                    phase: egui::TouchPhase::End,
                    pos: b1,
                    force: None,
                },
            ],
        );
        assert_eq!(app.session.doc.strokes.len(), 2);
        assert_eq!(app.session.doc.strokes[1].raw_points.len(), 2);

        // Case C: emulated PointerButton(true) arrives at the end of frame 1 and its paired
        // Touch::Start at the same coordinates arrives at the start of frame 2 — the unmoved
        // 1-point pointer stroke must be cancelled instead of committed as a dot.
        let c0 = pos2(450.0, 470.0);
        let c1 = pos2(550.0, 500.0);
        run_editor_frame(
            &ctx,
            &mut app,
            0.080,
            vec![
                egui::Event::PointerMoved(c0),
                egui::Event::PointerButton {
                    pos: c0,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert!(app.session.live_stroke.is_some());
        assert_eq!(app.session.doc.strokes.len(), 2);

        run_editor_frame(
            &ctx,
            &mut app,
            0.096,
            vec![
                egui::Event::Touch {
                    device_id: dev,
                    id: egui::TouchId(23),
                    phase: egui::TouchPhase::Start,
                    pos: c0,
                    force: Some(0.25),
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: egui::TouchId(23),
                    phase: egui::TouchPhase::Move,
                    pos: c1,
                    force: Some(0.6),
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: egui::TouchId(23),
                    phase: egui::TouchPhase::End,
                    pos: c1,
                    force: Some(0.1),
                },
            ],
        );
        assert!(app.session.live_stroke.is_none());
        assert_eq!(
            app.session.doc.strokes.len(),
            3,
            "cross-frame pointer-to-touch handoff must cancel the 1-point pointer stroke instead of committing a dot"
        );
        assert_eq!(app.session.doc.strokes[2].raw_points.len(), 2);
        assert!((app.session.doc.strokes[2].raw_points[0].pressure - 0.25).abs() < 1e-3);
    }

    #[test]
    fn zoomed_ruler_clamping_and_edge_snapping_work_near_page_corners() {
        let ctx = egui::Context::default();
        let mut app = setup_editor_app(&ctx);
        app.view.fit_requested = false;
        app.view.zoom = 3.0;
        let corner_center = Point::new(50.0, 60.0);
        app.view.pan = [640.0 - corner_center.x * 3.0, 400.0 - corner_center.y * 3.0];
        app.view.ruler_active = true;
        app.view.ruler_initialized = true;
        app.view.ruler_center = [corner_center.x, corner_center.y];
        app.view.ruler_angle = 0.0;
        app.view.ruler_length = 300.0;
        app.view.ruler_depth = 20.0;

        // A long ruler stays parked at the corner instead of being pushed away from it.
        let clamped = clamp_ruler_center(&app, corner_center);
        assert!((clamped.x - 50.0).abs() < 1e-6);
        assert!((clamped.y - 60.0).abs() < 1e-6);

        // Drawing along the ruler's upper edge (y = 60 - 10) snaps onto that edge.
        let xf = page_affine(&app);
        let p0 = xf.to_screen(Point::new(20.0, 50.0));
        let p1 = xf.to_screen(Point::new(90.0, 50.0));
        run_editor_frame(
            &ctx,
            &mut app,
            0.032,
            vec![
                egui::Event::PointerMoved(pos2(p0.x as f32, p0.y as f32)),
                egui::Event::PointerButton {
                    pos: pos2(p0.x as f32, p0.y as f32),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(pos2(p1.x as f32, p1.y as f32)),
                egui::Event::PointerButton {
                    pos: pos2(p1.x as f32, p1.y as f32),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(app.session.doc.strokes.len(), 1);
        for pt in app.session.doc.strokes[0].raw_points.iter() {
            assert!(
                (pt.y - 50.0).abs() < 1e-3,
                "expected ruler edge projection at y=50.0, got {}",
                pt.y
            );
        }
    }
}
