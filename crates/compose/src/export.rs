//! Pure-Rust vector SVG and multi-page PDF 1.4 export built from page ruling geometry
//! and evaluated calligraphy/ink stroke outlines.

use std::fmt::Write;
use std::io::Write as _;
use std::sync::Arc;

use drawcraft_color::Color;
use drawcraft_doc::{
    Document, GeometricShape, InkStroke, NotebookPage, PageImage, PaperColor, PaperTemplate,
    PenKind, TextItem, base64_encode_bytes,
};
use drawcraft_geom::{Point, Vec2, distance_to_segment, rdp_indices};
use drawcraft_paint::outline::build_stroke_contour_with_flat_caps;
use drawcraft_paint::{SpineNode, evaluate_spine};
use flate2::{Compression, write::ZlibEncoder};

const OUTLINE_RDP_EPSILON: f64 = 0.12;
const BEZIER_STRAIGHT_TOL: f64 = 0.03;
const CIRCLE_BEZIER_KAPPA: f64 = 0.5522847498;

struct ExportPaperGeometry {
    lines: Vec<(f64, f64, f64, f64, f64, bool)>,
    dots: Vec<(f64, f64, f64)>,
    checkboxes: Vec<drawcraft_doc::PaperBox>,
    labels: Vec<drawcraft_doc::PaperLabel>,
}

fn color_hex(c: Color) -> String {
    let [r, g, b, _] = c.to_rgba8();
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn color_rgb_pdf(c: Color) -> String {
    let s = c.sanitized();
    format!("{:.4} {:.4} {:.4}", s.r, s.g, s.b)
}

fn xml_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().saturating_add(2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk.first().copied().unwrap_or(0);
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(a) << 16) | (u32::from(b) << 8) | u32::from(c);
        out.push(char::from(TABLE[((n >> 18) & 0x3f) as usize]));
        out.push(char::from(TABLE[((n >> 12) & 0x3f) as usize]));
        if chunk.len() > 1 {
            out.push(char::from(TABLE[((n >> 6) & 0x3f) as usize]));
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(TABLE[(n & 0x3f) as usize]));
        } else {
            out.push('=');
        }
    }
    out
}

fn build_paper_geometry(width: u32, height: u32, template: PaperTemplate) -> ExportPaperGeometry {
    let w = f64::from(width.clamp(64, 8192));
    let h = f64::from(height.clamp(64, 8192));
    let mut geom = ExportPaperGeometry {
        lines: Vec::new(),
        dots: Vec::new(),
        checkboxes: Vec::new(),
        labels: Vec::new(),
    };

    match template {
        PaperTemplate::Blank => {}
        PaperTemplate::RuledNarrow | PaperTemplate::RuledWide => {
            let spacing = if template == PaperTemplate::RuledNarrow {
                24.0
            } else {
                34.0
            };
            let mut y = 96.0;
            while y <= h - 48.0 {
                geom.lines.push((0.0, y, w, y, 1.0, false));
                y += spacing;
            }
            geom.lines.push((112.0, 0.0, 112.0, h, 1.2, true));
        }
        PaperTemplate::Grid => {
            let step = 24.0;
            let mut y = step;
            while y <= h - step {
                geom.lines.push((0.0, y, w, y, 1.0, false));
                y += step;
            }
            let mut x = step;
            while x <= w - step {
                geom.lines.push((x, 0.0, x, h, 1.0, false));
                x += step;
            }
        }
        PaperTemplate::Dotted => {
            let step = 24.0;
            let mut y = step;
            while y <= h - step {
                let mut x = step;
                while x <= w - step {
                    geom.dots.push((x, y, 1.25));
                    x += step;
                }
                y += step;
            }
        }
        PaperTemplate::Cornell => {
            let header_y = 110.0_f64.min(h * 0.25);
            let footer_y = (h - 200.0).max(header_y + 60.0);
            let cue_x = 280.0_f64.min(w / 3.0);
            geom.lines.push((0.0, header_y, w, header_y, 1.4, true));
            geom.lines.push((0.0, footer_y, w, footer_y, 1.4, true));
            geom.lines
                .push((cue_x, header_y, cue_x, footer_y, 1.4, true));
            let mut y = header_y + 28.0;
            while y <= footer_y - 14.0 {
                geom.lines.push((cue_x + 12.0, y, w, y, 1.0, false));
                y += 28.0;
            }
        }
        PaperTemplate::Isometric
        | PaperTemplate::MusicStaff
        | PaperTemplate::WeeklyPlanner
        | PaperTemplate::MonthlyPlanner
        | PaperTemplate::Checklist => {
            let shared = drawcraft_doc::paper_template_geometry(width, height, template);
            for line in shared.lines {
                geom.lines.push((
                    f64::from(line.x1),
                    f64::from(line.y1),
                    f64::from(line.x2),
                    f64::from(line.y2),
                    f64::from(line.stroke_width),
                    line.accent,
                ));
            }
            geom.checkboxes = shared.checkboxes;
            geom.labels = shared.labels;
        }
    }

    geom
}

fn stroke_spine_mean_alpha(stroke: &InkStroke, nodes: &[drawcraft_paint::SpineNode]) -> f32 {
    let base = stroke.pen.color.sanitized().a;
    if nodes.is_empty() {
        let flow = if stroke.pen.opacity.is_finite() {
            stroke.pen.opacity.clamp(0.03, 1.0)
        } else {
            1.0
        };
        return (base * flow).clamp(0.03, 1.0);
    }
    let mut weighted_sum = 0.0_f64;
    let mut total_len = 0.0_f64;
    for pair in nodes.windows(2) {
        let seg = (pair[1].arc_length - pair[0].arc_length).max(0.0);
        if seg.is_finite() && seg > 1e-6 {
            let mid_alpha = 0.5 * (pair[0].alpha as f64 + pair[1].alpha as f64);
            if mid_alpha.is_finite() {
                weighted_sum += mid_alpha * seg;
                total_len += seg;
            }
        }
    }
    let mean_node_alpha = if total_len > 1e-6 {
        (weighted_sum / total_len) as f32
    } else {
        let sum: f64 = nodes
            .iter()
            .map(|n| n.alpha as f64)
            .filter(|a| a.is_finite())
            .sum();
        let count = nodes.iter().filter(|n| n.alpha.is_finite()).count().max(1);
        (sum / count as f64) as f32
    };
    (base * mean_node_alpha).clamp(0.03, 1.0)
}

fn stroke_spine_mean_color(stroke: &InkStroke, nodes: &[drawcraft_paint::SpineNode]) -> Color {
    if nodes.is_empty() {
        return stroke.pen.color.sanitized();
    }

    let mut weighted = [0.0_f64; 3];
    let mut total_len = 0.0_f64;
    for pair in nodes.windows(2) {
        let seg = (pair[1].arc_length - pair[0].arc_length).max(0.0);
        if seg.is_finite() && seg > 1e-6 {
            weighted[0] += 0.5 * f64::from(pair[0].color.r + pair[1].color.r) * seg;
            weighted[1] += 0.5 * f64::from(pair[0].color.g + pair[1].color.g) * seg;
            weighted[2] += 0.5 * f64::from(pair[0].color.b + pair[1].color.b) * seg;
            total_len += seg;
        }
    }

    if total_len <= 1e-6 {
        let count = nodes.len() as f64;
        for node in nodes {
            weighted[0] += f64::from(node.color.r);
            weighted[1] += f64::from(node.color.g);
            weighted[2] += f64::from(node.color.b);
        }
        total_len = count.max(1.0);
    }

    let base = stroke.pen.color.sanitized();
    Color::new(
        (weighted[0] / total_len) as f32,
        (weighted[1] / total_len) as f32,
        (weighted[2] / total_len) as f32,
        base.a,
    )
    .sanitized()
}

fn fmt_coord(out: &mut String, val: f64) {
    if !val.is_finite() {
        out.push('0');
        return;
    }
    let rounded = (val * 100.0).round() / 100.0;
    let clean = if rounded.abs() < 0.005 { 0.0 } else { rounded };
    if (clean - clean.round()).abs() < 1e-6 {
        let _ = write!(out, "{}", clean.round() as i64);
        return;
    }
    let s = format!("{clean:.2}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    out.push_str(trimmed);
}

#[derive(Clone, Copy)]
enum PathSeg {
    Move(Point),
    Line(Point),
    Cubic(Point, Point, Point),
    Close,
}

fn push_ellipse_dot_segments(segs: &mut Vec<PathSeg>, node: &SpineNode) {
    let r = f64::from(node.radius.max(0.25));
    let round = f64::from(node.roundness.clamp(0.15, 1.0));
    let a = f64::from(node.angle_rad);
    let (sin_a, cos_a) = a.sin_cos();
    let u = Vec2::new(cos_a * r, sin_a * r);
    let v = Vec2::new(-sin_a * r * round, cos_a * r * round);
    let c = node.point;
    let k = CIRCLE_BEZIER_KAPPA;

    let pt = |ux: f64, vy: f64| Point::new(c.x + u.x * ux + v.x * vy, c.y + u.y * ux + v.y * vy);
    segs.push(PathSeg::Move(pt(1.0, 0.0)));
    segs.push(PathSeg::Cubic(pt(1.0, k), pt(k, 1.0), pt(0.0, 1.0)));
    segs.push(PathSeg::Cubic(pt(-k, 1.0), pt(-1.0, k), pt(-1.0, 0.0)));
    segs.push(PathSeg::Cubic(pt(-1.0, -k), pt(-k, -1.0), pt(0.0, -1.0)));
    segs.push(PathSeg::Cubic(pt(k, -1.0), pt(1.0, -k), pt(1.0, 0.0)));
    segs.push(PathSeg::Close);
}

fn clamp_tangent_vec(v: Vec2, max_len: f64) -> Vec2 {
    let len = (v.x * v.x + v.y * v.y).sqrt();
    if len <= max_len || len < 1e-9 || !len.is_finite() {
        v
    } else {
        let s = max_len / len;
        Vec2::new(v.x * s, v.y * s)
    }
}

fn is_sharp_corner(a: Point, b: Point, c: Point) -> bool {
    let Some(u) = Vec2::from_points(a, b).normalized() else {
        return true;
    };
    let Some(v) = Vec2::from_points(b, c).normalized() else {
        return true;
    };
    u.dot(v) < 0.72
}

fn append_smooth_closed_ring(segs: &mut Vec<PathSeg>, pts: &[Point]) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    segs.push(PathSeg::Move(pts[0]));
    if n == 2 {
        segs.push(PathSeg::Line(pts[1]));
        segs.push(PathSeg::Close);
        return;
    }
    for i in 0..n {
        let p0 = pts[(i + n - 1) % n];
        let p1 = pts[i];
        let p2 = pts[(i + 1) % n];
        let p3 = pts[(i + 2) % n];
        let chord = p1.distance(p2);
        if chord < 1e-6 || !chord.is_finite() {
            continue;
        }

        let max_h = chord * 0.40;
        let d1 = if is_sharp_corner(p0, p1, p2) {
            Vec2::ZERO
        } else {
            clamp_tangent_vec(Vec2::new((p2.x - p0.x) / 6.0, (p2.y - p0.y) / 6.0), max_h)
        };
        let d2 = if is_sharp_corner(p1, p2, p3) {
            Vec2::ZERO
        } else {
            clamp_tangent_vec(Vec2::new((p3.x - p1.x) / 6.0, (p3.y - p1.y) / 6.0), max_h)
        };

        let c1 = Point::new(p1.x + d1.x, p1.y + d1.y);
        let c2 = Point::new(p2.x - d2.x, p2.y - d2.y);
        let flat = distance_to_segment(c1, p1, p2).0 <= BEZIER_STRAIGHT_TOL
            && distance_to_segment(c2, p1, p2).0 <= BEZIER_STRAIGHT_TOL;
        if flat {
            segs.push(PathSeg::Line(p2));
        } else {
            segs.push(PathSeg::Cubic(c1, c2, p2));
        }
    }
    segs.push(PathSeg::Close);
}

fn build_stroke_path(stroke: &InkStroke) -> (Vec<PathSeg>, Color, f32) {
    let nodes = evaluate_spine(&stroke.pen, &stroke.smoothed_points);
    let color = stroke_spine_mean_color(stroke, &nodes);
    let alpha = stroke_spine_mean_alpha(stroke, &nodes);
    if nodes.is_empty() {
        return (Vec::new(), color, alpha);
    }
    if nodes.len() == 1 {
        let mut segs = Vec::with_capacity(6);
        push_ellipse_dot_segments(&mut segs, &nodes[0]);
        return (segs, color, alpha);
    }

    let contour =
        build_stroke_contour_with_flat_caps(&nodes, stroke.pen.kind == PenKind::Highlighter);
    if contour.len() < 3 {
        let mut segs = Vec::with_capacity(6);
        push_ellipse_dot_segments(&mut segs, &nodes[0]);
        return (segs, color, alpha);
    }

    let kept_idx = rdp_indices(&contour, OUTLINE_RDP_EPSILON);
    let mut kept: Vec<Point> = kept_idx.iter().map(|&i| contour[i]).collect();
    if kept.len() >= 2
        && let (Some(&first), Some(&last)) = (kept.first(), kept.last())
        && first.distance(last) < 1e-4
    {
        kept.pop();
    }
    if kept.len() < 3 {
        kept = contour;
    }

    let mut segs = Vec::with_capacity(kept.len() + 2);
    append_smooth_closed_ring(&mut segs, &kept);
    (segs, color, alpha)
}

fn append_svg_pattern_paper(
    out: &mut String,
    w: u32,
    h: u32,
    template: PaperTemplate,
    line_hex: &str,
    margin_hex: &str,
) {
    match template {
        PaperTemplate::Blank => {
            let _ = writeln!(out, "  <g id=\"paper-template\"/>");
        }
        PaperTemplate::Dotted => {
            let nx = w.saturating_sub(24) / 24;
            let ny = h.saturating_sub(24) / 24;
            if nx > 0 && ny > 0 {
                let tw = nx * 24;
                let th = ny * 24;
                let _ = writeln!(out, "  <defs>");
                let _ = writeln!(
                    out,
                    "    <pattern id=\"paper-dots\" x=\"12\" y=\"12\" width=\"24\" height=\"24\" patternUnits=\"userSpaceOnUse\">"
                );
                let _ = writeln!(
                    out,
                    "      <circle cx=\"12\" cy=\"12\" r=\"1.25\" fill=\"{line_hex}\"/>"
                );
                let _ = writeln!(out, "    </pattern>");
                let _ = writeln!(out, "  </defs>");
                let _ = writeln!(out, "  <g id=\"paper-template\">");
                let _ = writeln!(
                    out,
                    "    <rect x=\"12\" y=\"12\" width=\"{tw}\" height=\"{th}\" fill=\"url(#paper-dots)\"/>"
                );
                let _ = writeln!(out, "  </g>");
            } else {
                let _ = writeln!(out, "  <g id=\"paper-template\"/>");
            }
        }
        PaperTemplate::Grid => {
            let nx = w.saturating_sub(24) / 24;
            let ny = h.saturating_sub(24) / 24;
            let _ = writeln!(out, "  <defs>");
            let _ = writeln!(
                out,
                "    <pattern id=\"paper-grid-h\" x=\"0\" y=\"12\" width=\"24\" height=\"24\" patternUnits=\"userSpaceOnUse\">"
            );
            let _ = writeln!(
                out,
                "      <line x1=\"0\" y1=\"12\" x2=\"24\" y2=\"12\" stroke=\"{line_hex}\" stroke-width=\"1\"/>"
            );
            let _ = writeln!(out, "    </pattern>");
            let _ = writeln!(
                out,
                "    <pattern id=\"paper-grid-v\" x=\"12\" y=\"0\" width=\"24\" height=\"24\" patternUnits=\"userSpaceOnUse\">"
            );
            let _ = writeln!(
                out,
                "      <line x1=\"12\" y1=\"0\" x2=\"12\" y2=\"24\" stroke=\"{line_hex}\" stroke-width=\"1\"/>"
            );
            let _ = writeln!(out, "    </pattern>");
            let _ = writeln!(out, "  </defs>");
            let _ = writeln!(out, "  <g id=\"paper-template\">");
            if ny > 0 {
                let gh = ny * 24;
                let _ = writeln!(
                    out,
                    "    <rect x=\"0\" y=\"12\" width=\"{w}\" height=\"{gh}\" fill=\"url(#paper-grid-h)\"/>"
                );
            }
            if nx > 0 {
                let gw = nx * 24;
                let _ = writeln!(
                    out,
                    "    <rect x=\"12\" y=\"0\" width=\"{gw}\" height=\"{h}\" fill=\"url(#paper-grid-v)\"/>"
                );
            }
            let _ = writeln!(out, "  </g>");
        }
        PaperTemplate::RuledNarrow | PaperTemplate::RuledWide => {
            let spacing = if template == PaperTemplate::RuledNarrow {
                24u32
            } else {
                34u32
            };
            let half = f64::from(spacing) * 0.5;
            let pat_y = 96.0 - half;
            let count = h.saturating_sub(144) / spacing + u32::from(h >= 144);
            if count > 0 {
                let _ = writeln!(out, "  <defs>");
                let _ = writeln!(
                    out,
                    "    <pattern id=\"paper-ruled\" x=\"0\" y=\"{pat_y:.1}\" width=\"24\" height=\"{spacing}\" patternUnits=\"userSpaceOnUse\">"
                );
                let _ = writeln!(
                    out,
                    "      <line x1=\"0\" y1=\"{half:.1}\" x2=\"24\" y2=\"{half:.1}\" stroke=\"{line_hex}\" stroke-width=\"1\"/>"
                );
                let _ = writeln!(out, "    </pattern>");
                let _ = writeln!(out, "  </defs>");
            }
            let _ = writeln!(out, "  <g id=\"paper-template\">");
            if count > 0 {
                let rh = count * spacing;
                let _ = writeln!(
                    out,
                    "    <rect x=\"0\" y=\"{pat_y:.1}\" width=\"{w}\" height=\"{rh}\" fill=\"url(#paper-ruled)\"/>"
                );
            }
            let _ = writeln!(
                out,
                "    <line x1=\"112\" y1=\"0\" x2=\"112\" y2=\"{h}\" stroke=\"{margin_hex}\" stroke-width=\"1.2\"/>"
            );
            let _ = writeln!(out, "  </g>");
        }
        PaperTemplate::Cornell
        | PaperTemplate::Isometric
        | PaperTemplate::MusicStaff
        | PaperTemplate::WeeklyPlanner
        | PaperTemplate::MonthlyPlanner
        | PaperTemplate::Checklist => {
            let geom = build_paper_geometry(w, h, template);
            let mut body_d = String::new();
            let mut margin_d = String::new();
            for &(x1, y1, x2, y2, _, is_margin) in &geom.lines {
                let dst = if is_margin {
                    &mut margin_d
                } else {
                    &mut body_d
                };
                if !dst.is_empty() {
                    dst.push(' ');
                }
                dst.push('M');
                fmt_coord(dst, x1);
                dst.push(' ');
                fmt_coord(dst, y1);
                dst.push('L');
                fmt_coord(dst, x2);
                dst.push(' ');
                fmt_coord(dst, y2);
            }
            let _ = writeln!(out, "  <g id=\"paper-template\">");
            if !body_d.is_empty() {
                let _ = writeln!(
                    out,
                    "    <path d=\"{body_d}\" fill=\"none\" stroke=\"{line_hex}\" stroke-width=\"1\"/>"
                );
            }
            if !margin_d.is_empty() {
                let _ = writeln!(
                    out,
                    "    <path d=\"{margin_d}\" fill=\"none\" stroke=\"{margin_hex}\" stroke-width=\"1.4\"/>"
                );
            }
            for b in &geom.checkboxes {
                let _ = writeln!(
                    out,
                    "    <rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"none\" stroke=\"{margin_hex}\" stroke-width=\"1.25\"/>",
                    b.x, b.y, b.size, b.size
                );
            }
            for lbl in &geom.labels {
                let fill = if lbl.accent { margin_hex } else { line_hex };
                let esc = xml_escape(lbl.text);
                let _ = writeln!(
                    out,
                    "    <text x=\"{:.2}\" y=\"{:.2}\" font-family=\"sans-serif\" font-size=\"{:.1}\" font-weight=\"600\" dominant-baseline=\"hanging\" fill=\"{fill}\">{esc}</text>",
                    lbl.x, lbl.y, lbl.font_size
                );
            }
            let _ = writeln!(out, "  </g>");
        }
    }
}

fn append_svg_paper(out: &mut String, doc: &Document) {
    let w = doc.width.clamp(64, 8192);
    let h = doc.height.clamp(64, 8192);
    let bg = color_hex(doc.paper_color.background_color());
    let line_hex = color_hex(doc.paper_color.line_color());
    let margin_hex = color_hex(doc.paper_color.margin_color());

    let _ = writeln!(
        out,
        "  <rect x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\" fill=\"{bg}\"/>"
    );
    append_svg_pattern_paper(out, w, h, doc.paper_template, &line_hex, &margin_hex);
}

fn format_svg_path_data(segs: &[PathSeg]) -> String {
    let mut d = String::with_capacity(segs.len() * 20);
    for &seg in segs {
        match seg {
            PathSeg::Move(p) => {
                d.push('M');
                fmt_coord(&mut d, p.x);
                d.push(' ');
                fmt_coord(&mut d, p.y);
            }
            PathSeg::Line(p) => {
                d.push('L');
                fmt_coord(&mut d, p.x);
                d.push(' ');
                fmt_coord(&mut d, p.y);
            }
            PathSeg::Cubic(c1, c2, p) => {
                d.push('C');
                fmt_coord(&mut d, c1.x);
                d.push(' ');
                fmt_coord(&mut d, c1.y);
                d.push(' ');
                fmt_coord(&mut d, c2.x);
                d.push(' ');
                fmt_coord(&mut d, c2.y);
                d.push(' ');
                fmt_coord(&mut d, p.x);
                d.push(' ');
                fmt_coord(&mut d, p.y);
            }
            PathSeg::Close => d.push('Z'),
        }
    }
    d
}

fn append_svg_strokes(out: &mut String, doc: &Document) {
    let _ = writeln!(out, "  <g id=\"ink-strokes\">");
    let mut active_style: Option<([u8; 3], u16, bool)> = None;

    for stroke in &doc.strokes {
        let (segs, color, alpha) = build_stroke_path(stroke);
        if segs.is_empty() {
            continue;
        }
        let path_data = format_svg_path_data(&segs);
        let [r, g, b, _] = color.to_rgba8();
        let alpha_q = (alpha.clamp(0.03, 1.0) * 1000.0).round() as u16;
        let is_hi = stroke.pen.kind == PenKind::Highlighter;
        let style_key = ([r, g, b], alpha_q, is_hi);

        if active_style != Some(style_key) {
            if active_style.is_some() {
                let _ = writeln!(out, "    </g>");
            }
            let hex = color_hex(color);
            let opacity_attr = if alpha < 0.995 {
                format!(" fill-opacity=\"{alpha:.3}\"")
            } else {
                String::new()
            };
            let blend_attr = if is_hi {
                " style=\"mix-blend-mode:multiply\""
            } else {
                ""
            };
            let _ = writeln!(out, "    <g fill=\"{hex}\"{opacity_attr}{blend_attr}>");
            active_style = Some(style_key);
        }
        let _ = writeln!(out, "      <path d=\"{path_data}\"/>");
    }

    if active_style.is_some() {
        let _ = writeln!(out, "    </g>");
    }
    let _ = writeln!(out, "  </g>");
}

#[cfg(test)]
fn approx_export_char_advance(item: &TextItem, ch: char) -> f32 {
    let base = if item.font == drawcraft_doc::TextFont::Monospace {
        item.size * 0.62
    } else if ch == ' ' {
        item.size * 0.28
    } else if ch == '\t' {
        item.size * 1.12
    } else {
        item.size * 0.54
    };
    let letter = if ch == '\t' {
        item.letter_spacing * 4.0
    } else {
        item.letter_spacing
    };
    (base + letter).max(1.0)
}

#[cfg(test)]
fn wrapped_export_lines(doc_width: u32, item: &TextItem) -> Vec<String> {
    let max_width = item.effective_max_width(doc_width);
    drawcraft_doc::wrap_text_lines(&item.text, max_width, |ch| {
        approx_export_char_advance(item, ch)
    })
}

fn svg_font_family(font: drawcraft_doc::TextFont) -> &'static str {
    match font {
        drawcraft_doc::TextFont::Sans => "DrawCraftText, sans-serif",
        drawcraft_doc::TextFont::Monospace => "monospace",
    }
}

fn export_approx_char_w_for(ch: char, font: drawcraft_doc::TextFont, size: f32) -> f32 {
    if font == drawcraft_doc::TextFont::Monospace {
        size * 0.62
    } else if ch == ' ' {
        size * 0.28
    } else if ch == '\t' {
        size * 0.28 * 4.0
    } else {
        size * 0.54
    }
}

fn export_approx_text_w_for(
    text: &str,
    font: drawcraft_doc::TextFont,
    size: f32,
    letter_spacing: f32,
) -> f64 {
    let mut w = 0.0_f64;
    for ch in text.chars() {
        let adv = export_approx_char_w_for(ch, font, size);
        w += f64::from(adv + letter_spacing);
    }
    w
}

#[allow(clippy::too_many_arguments)]
fn append_svg_list_marker(
    out: &mut String,
    indent: &str,
    kind: drawcraft_doc::ListItemKind,
    item: &TextItem,
    line_top: f64,
    gutter_w: f64,
    color: drawcraft_color::Color,
    opacity: &str,
) {
    let col_hex = color_hex(color);
    match kind {
        drawcraft_doc::ListItemKind::Bullet => {
            let cx = item.x + gutter_w * 0.45;
            let cy = line_top + f64::from(item.size) * 0.55;
            let r = f64::from((item.size * 0.14).clamp(1.8, 6.0));
            let _ = writeln!(
                out,
                "{indent}<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{col_hex}\"{opacity}/>",
                fmt_number(cx),
                fmt_number(cy),
                fmt_number(r)
            );
        }
        drawcraft_doc::ListItemKind::Checkbox { checked } => {
            let bsz = f64::from((item.size * 0.68).clamp(8.0, 44.0));
            let bx = item.x + (gutter_w - bsz) * 0.4;
            let by = line_top + f64::from(item.size) * 0.16;
            let sw = (bsz * 0.11).max(1.1);
            if checked {
                let chk = if drawcraft_color::relative_luminance(color) > 0.45 {
                    "#141820"
                } else {
                    "#FFFFFF"
                };
                let _ = writeln!(
                    out,
                    "{indent}<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"2\" fill=\"{col_hex}\" stroke=\"{col_hex}\" stroke-width=\"{}\"/>",
                    fmt_number(bx),
                    fmt_number(by),
                    fmt_number(bsz),
                    fmt_number(bsz),
                    fmt_number(sw)
                );
                let _ = writeln!(
                    out,
                    "{indent}<polyline points=\"{},{} {},{} {},{}\" fill=\"none\" stroke=\"{chk}\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>",
                    fmt_number(bx + bsz * 0.22),
                    fmt_number(by + bsz * 0.54),
                    fmt_number(bx + bsz * 0.44),
                    fmt_number(by + bsz * 0.76),
                    fmt_number(bx + bsz * 0.80),
                    fmt_number(by + bsz * 0.28),
                    fmt_number((bsz * 0.14).max(1.2))
                );
            } else {
                let _ = writeln!(
                    out,
                    "{indent}<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"2\" fill=\"none\" stroke=\"{col_hex}\" stroke-width=\"{}\"/>",
                    fmt_number(bx),
                    fmt_number(by),
                    fmt_number(bsz),
                    fmt_number(bsz),
                    fmt_number(sw)
                );
            }
        }
    }
}

fn append_svg_text_run(
    out: &mut String,
    item: &TextItem,
    base_color: Color,
    has_primary_link: bool,
    run: &drawcraft_doc::StyledTextRun,
) {
    let ff = match run.font {
        Some(f) if f != item.font => format!(" font-family=\"{}\"", svg_font_family(f)),
        _ => String::new(),
    };
    let fz = match run.size {
        Some(sz) if (sz - item.size).abs() > 0.05 => {
            format!(
                " font-size=\"{}\"",
                fmt_number(f64::from(sz.clamp(6.0, 120.0)))
            )
        }
        _ => String::new(),
    };
    let fc = match run.color {
        Some(c) if c.sanitized() != base_color => {
            let sc = c.sanitized();
            let op = if (sc.a - base_color.a).abs() > 0.01 {
                format!(" fill-opacity=\"{:.3}\"", sc.a)
            } else {
                String::new()
            };
            format!(" fill=\"{}\"{op}", color_hex(sc))
        }
        _ => String::new(),
    };
    let fw = if run.bold { " font-weight=\"700\"" } else { "" };
    let fs = if run.italic {
        " font-style=\"italic\""
    } else {
        ""
    };
    let td = if run.underline && !has_primary_link {
        " text-decoration=\"underline\""
    } else {
        ""
    };
    let escaped = xml_escape(&run.text);
    if ff.is_empty()
        && fz.is_empty()
        && fc.is_empty()
        && fw.is_empty()
        && fs.is_empty()
        && td.is_empty()
    {
        out.push_str(&escaped);
    } else {
        let _ = write!(out, "<tspan{ff}{fz}{fc}{fw}{fs}{td}>{escaped}</tspan>");
    }
}

fn append_svg_text(out: &mut String, doc: &Document) {
    if doc.texts.is_empty() {
        return;
    }
    let font_reg = base64_encode(include_bytes!("../assets/fonts/Roboto-Regular.ttf"));
    let font_bold = base64_encode(include_bytes!("../assets/fonts/Roboto-Bold.ttf"));
    let font_italic = base64_encode(include_bytes!("../assets/fonts/Roboto-Italic.ttf"));
    let font_bold_italic = base64_encode(include_bytes!("../assets/fonts/Roboto-BoldItalic.ttf"));
    let license = xml_escape(include_str!("../assets/fonts/LICENSE"));
    let _ = writeln!(
        out,
        "  <metadata>Embedded Roboto font family license: {license}</metadata>"
    );
    let _ = writeln!(
        out,
        "  <defs><style>\
        @font-face{{font-family:DrawCraftText;font-style:normal;font-weight:400;src:url(data:font/ttf;base64,{font_reg}) format('truetype');}}\
        @font-face{{font-family:DrawCraftText;font-style:normal;font-weight:700;src:url(data:font/ttf;base64,{font_bold}) format('truetype');}}\
        @font-face{{font-family:DrawCraftText;font-style:italic;font-weight:400;src:url(data:font/ttf;base64,{font_italic}) format('truetype');}}\
        @font-face{{font-family:DrawCraftText;font-style:italic;font-weight:700;src:url(data:font/ttf;base64,{font_bold_italic}) format('truetype');}}\
        </style></defs>"
    );
    let _ = writeln!(out, "  <g id=\"page-text\">");
    for item in &doc.texts {
        let item = item.as_ref().sanitized();
        let color = item.color.sanitized();
        let font_family = svg_font_family(item.font);
        let opacity = if color.a < 0.995 {
            format!(" fill-opacity=\"{:.3}\"", color.a)
        } else {
            String::new()
        };
        let letter_spacing_attr = if item.letter_spacing.abs() > 0.01 {
            format!(
                " letter-spacing=\"{}px\"",
                fmt_number(f64::from(item.letter_spacing))
            )
        } else {
            String::new()
        };
        let has_rot = item.angle_degrees.abs() > f64::EPSILON;
        let primary_link = item.primary_link_url();
        let link_underline_attr = if primary_link.is_some() {
            " text-decoration=\"underline\""
        } else {
            ""
        };
        if let Some(url) = &primary_link {
            let escaped = xml_escape(url);
            let _ = writeln!(
                out,
                "    <a href=\"{escaped}\" target=\"_blank\" rel=\"noopener noreferrer\">"
            );
        }
        let indent = if primary_link.is_some() {
            "      "
        } else {
            "    "
        };
        if has_rot {
            let _ = writeln!(
                out,
                "{indent}<g transform=\"rotate({} {} {})\">",
                fmt_number(item.angle_degrees),
                fmt_number(item.x),
                fmt_number(item.y)
            );
        }
        let max_w = item.effective_max_width(doc.width);
        let lines =
            drawcraft_doc::wrap_styled_text_lines_with(&item, max_w, |ch, ch_font, ch_size| {
                export_approx_char_w_for(ch, ch_font, ch_size).max(1.0)
            });
        let gutter_w = f64::from((item.size * 1.15).max(14.0));
        let mut cur_top = item.y;

        for line in &lines {
            let line_max_size = line
                .runs
                .iter()
                .filter_map(|r| r.size)
                .fold(item.size, f32::max)
                .clamp(6.0, 120.0);
            let line_top = cur_top;
            let baseline = line_top + f64::from(line_max_size) * 0.86;
            cur_top += f64::from(line_max_size * item.line_spacing);
            let has_indent = line.list_kind.is_some() || line.is_continuation;
            let indent_w = if has_indent { gutter_w } else { 0.0 };
            let avail_w = (f64::from(max_w) - indent_w).max(24.0);
            let line_w: f64 = line
                .runs
                .iter()
                .map(|r| {
                    export_approx_text_w_for(
                        &r.text,
                        r.font.unwrap_or(item.font),
                        r.size.unwrap_or(item.size).clamp(6.0, 120.0),
                        item.letter_spacing,
                    )
                })
                .sum();
            let align_offset = match item.alignment {
                drawcraft_doc::TextAlignment::Left => 0.0,
                drawcraft_doc::TextAlignment::Center => ((avail_w - line_w) * 0.5).max(0.0),
                drawcraft_doc::TextAlignment::Right => (avail_w - line_w).max(0.0),
            };
            let content_x = item.x + indent_w + align_offset;

            if let Some(kind) = line.list_kind {
                append_svg_list_marker(
                    out, indent, kind, &item, line_top, gutter_w, color, &opacity,
                );
            }

            let mut run_x = content_x;
            for run in &line.runs {
                let run_font = run.font.unwrap_or(item.font);
                let run_size = run.size.unwrap_or(item.size).clamp(6.0, 120.0);
                let rw =
                    export_approx_text_w_for(&run.text, run_font, run_size, item.letter_spacing);
                if let Some(hl) = run.highlight
                    && hl.a > 0.0
                    && rw > 0.0
                {
                    let _ = writeln!(
                        out,
                        "{indent}<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"2\" fill=\"{}\" fill-opacity=\"{:.3}\"/>",
                        fmt_number(run_x - 1.0),
                        fmt_number(baseline - f64::from(run_size) * 0.78),
                        fmt_number(rw + 2.0),
                        fmt_number(f64::from(run_size) * 1.05),
                        color_hex(hl),
                        hl.a
                    );
                }
                run_x += rw;
            }

            let plain_only = line.runs.len() <= 1
                && line.runs.first().is_none_or(|r| {
                    !r.bold
                        && !r.italic
                        && !r.underline
                        && r.font.is_none_or(|f| f == item.font)
                        && r.size.is_none_or(|s| (s - item.size).abs() <= 0.05)
                        && r.color.is_none_or(|c| c.sanitized() == color)
                });
            let _ = write!(
                out,
                "{indent}<text x=\"{}\" y=\"{}\" font-size=\"{}\" font-family=\"{font_family}\" fill=\"{}\"{opacity}{letter_spacing_attr}{link_underline_attr}>",
                fmt_number(content_x),
                fmt_number(baseline),
                fmt_number(f64::from(item.size)),
                color_hex(color)
            );
            if plain_only {
                let body = line.runs.first().map_or("", |r| r.text.as_str());
                out.push_str(&xml_escape(body));
            } else {
                for run in &line.runs {
                    append_svg_text_run(out, &item, color, primary_link.is_some(), run);
                }
            }
            let _ = writeln!(out, "</text>");
        }
        if has_rot {
            let _ = writeln!(out, "{indent}</g>");
        }
        if primary_link.is_some() {
            let _ = writeln!(out, "    </a>");
        }
    }
    let _ = writeln!(out, "  </g>");
}

fn append_svg_shapes(out: &mut String, shapes: &[std::sync::Arc<GeometricShape>]) {
    if shapes.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "  <g id=\"geometric-shapes\" fill=\"none\" stroke-linecap=\"round\" stroke-linejoin=\"round\">"
    );
    for item in shapes {
        let shape = item.as_ref().sanitized();
        let mut path = String::new();
        for (points, closed) in shape.path_segments() {
            let Some(first) = points.first() else {
                continue;
            };
            if !path.is_empty() {
                path.push(' ');
            }
            let _ = write!(path, "M {} {}", fmt_number(first.x), fmt_number(first.y));
            for point in points.iter().skip(1) {
                let _ = write!(path, " L {} {}", fmt_number(point.x), fmt_number(point.y));
            }
            if closed {
                path.push_str(" Z");
            }
        }
        if path.is_empty() {
            continue;
        }
        let color = shape.color.sanitized();
        let opacity = if color.a < 0.995 {
            format!(" stroke-opacity=\"{:.3}\"", color.a)
        } else {
            String::new()
        };
        let fill_attr = if shape.is_closed()
            && let Some(fill) = shape.fill_color
            && fill.a > 0.0
        {
            format!(
                " fill=\"{}\" fill-opacity=\"{:.3}\"",
                color_hex(fill),
                fill.a
            )
        } else {
            " fill=\"none\"".to_string()
        };
        let dash_attr = match shape.line_style.dash_pattern(shape.width) {
            Some((d, g)) => format!(
                " stroke-dasharray=\"{} {}\"",
                fmt_number(f64::from(d)),
                fmt_number(f64::from(g))
            ),
            None => String::new(),
        };
        let _ = writeln!(
            out,
            "    <path d=\"{path}\"{fill_attr} stroke=\"{}\" stroke-width=\"{}\"{opacity}{dash_attr}/>",
            color_hex(color),
            fmt_number(f64::from(shape.width))
        );
    }
    let _ = writeln!(out, "  </g>");
}

fn append_svg_images(out: &mut String, images: &[Arc<PageImage>]) {
    if images.is_empty() {
        return;
    }
    let _ = writeln!(out, "  <g id=\"page-images\">");
    for img in images {
        let clean = img.sanitized();
        if clean.png_bytes.is_empty() {
            continue;
        }
        let b64 = base64_encode_bytes(&clean.png_bytes);
        let transform = if clean.angle_degrees.abs() > 1e-3 {
            let center = clean.center();
            format!(
                " transform=\"rotate({} {} {})\"",
                fmt_number(clean.angle_degrees),
                fmt_number(center.x),
                fmt_number(center.y),
            )
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "    <image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\" href=\"data:image/png;base64,{b64}\"{transform}/>",
            fmt_number(clean.x),
            fmt_number(clean.y),
            fmt_number(clean.width),
            fmt_number(clean.height),
        );
    }
    let _ = writeln!(out, "  </g>");
}

fn fmt_number(value: f64) -> String {
    let mut out = String::new();
    fmt_coord(&mut out, value);
    out
}

fn fit_pdf_export_pixels(w: u32, h: u32, max_dim: u32) -> (u32, u32) {
    let w = w.clamp(64, 8192);
    let h = h.clamp(64, 8192);
    let cap = max_dim.max(64);
    let max_side = w.max(h);
    if max_side <= cap {
        return (w, h);
    }
    let scale = f64::from(cap) / f64::from(max_side);
    let px_w = ((f64::from(w) * scale).round() as u32).clamp(1, cap);
    let px_h = ((f64::from(h) * scale).round() as u32).clamp(1, cap);
    (px_w, px_h)
}

fn write_svg_backdrop_image(out: &mut String, group_id: &str, w: u32, h: u32, rot: u8, b64: &str) {
    let rot = rot % 4;
    let wf = f64::from(w);
    let hf = f64::from(h);
    let (uw, uh) = if rot.is_multiple_of(2) {
        (wf, hf)
    } else {
        (hf, wf)
    };
    let ox = (wf - uw) * 0.5;
    let oy = (hf - uh) * 0.5;
    let transform_attr = if rot != 0 {
        let deg = u32::from(rot) * 90;
        let cx = wf * 0.5;
        let cy = hf * 0.5;
        format!(" transform=\"rotate({deg} {cx:.2} {cy:.2})\"")
    } else {
        String::new()
    };
    let _ = writeln!(out, "  <g id=\"{group_id}\">");
    let _ = writeln!(
        out,
        "    <image x=\"{ox:.2}\" y=\"{oy:.2}\" width=\"{uw:.2}\" height=\"{uh:.2}\"{transform_attr} preserveAspectRatio=\"none\" href=\"data:image/png;base64,{b64}\"/>"
    );
    let _ = writeln!(out, "  </g>");
}

fn append_svg_pdf_backdrop(out: &mut String, doc: &Document) -> bool {
    let w = doc.width.clamp(64, 8192);
    let h = doc.height.clamp(64, 8192);
    let rot = doc.backdrop_rotation % 4;

    if let Some(backdrop) = doc.pdf_backdrop {
        let _ = writeln!(
            out,
            "  <rect x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\" fill=\"#ffffff\"/>"
        );
        let Some(pdf_src) = doc.pdf_documents.get(backdrop.pdf_index) else {
            return true;
        };
        let (req_w, req_h) = if rot.is_multiple_of(2) {
            (w, h)
        } else {
            (h, w)
        };
        let (max_w, max_h) = fit_pdf_export_pixels(req_w, req_h, 2400);
        if let Ok((_, _, png_bytes)) = crate::pdf::render_pdf_page_png(
            Arc::clone(&pdf_src.bytes),
            backdrop.page_index,
            max_w,
            max_h,
        ) {
            let b64 = base64_encode_bytes(&png_bytes);
            write_svg_backdrop_image(out, "pdf-backdrop", w, h, rot, &b64);
        }
        return true;
    }

    if let Some(img_idx) = doc.custom_image_backdrop
        && let Some(tpl) = doc.image_templates.get(img_idx)
        && !tpl.png_bytes.is_empty()
    {
        let bg = color_hex(doc.paper_color.background_color());
        let _ = writeln!(
            out,
            "  <rect x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\" fill=\"{bg}\"/>"
        );
        let b64 = base64_encode_bytes(&tpl.png_bytes);
        write_svg_backdrop_image(out, "custom-template-backdrop", w, h, rot, &b64);
        return true;
    }

    false
}

/// Renders the active page of `doc` (or a page snapshot from `Document::document_for_page`)
/// into a standalone UTF-8 vector SVG document.
pub fn render_document_svg(doc: &Document) -> String {
    let w = doc.width.clamp(64, 8192);
    let h = doc.height.clamp(64, 8192);
    let title = xml_escape(&doc.title);

    let mut out = String::with_capacity(4096);
    let _ = writeln!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    let _ = writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">"
    );
    let _ = writeln!(out, "  <title>{title}</title>");
    if !append_svg_pdf_backdrop(&mut out, doc) {
        append_svg_paper(&mut out, doc);
    }
    append_svg_images(&mut out, &doc.images);
    append_svg_strokes(&mut out, doc);
    append_svg_text(&mut out, doc);
    append_svg_shapes(&mut out, &doc.shapes);
    out.push_str("</svg>\n");
    out
}

fn register_pdf_state(states: &mut Vec<(bool, u8)>, is_highlighter: bool, alpha: f32) -> usize {
    let q = (alpha.clamp(0.03, 1.0) * 255.0).round() as u8;
    let key = (is_highlighter, q);
    if let Some(pos) = states.iter().position(|&item| item == key) {
        return pos;
    }
    let idx = states.len();
    states.push(key);
    idx
}

fn pdf_compress_stream(raw: &[u8]) -> (&'static str, Vec<u8>) {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    if encoder.write_all(raw).is_ok()
        && let Ok(compressed) = encoder.finish()
        && compressed.len() < raw.len()
    {
        return ("/Filter /FlateDecode ", compressed);
    }
    ("", raw.to_vec())
}

fn push_pdf_pt(out: &mut String, p: Point) {
    fmt_coord(out, p.x);
    out.push(' ');
    fmt_coord(out, p.y);
}

fn append_pdf_path(out: &mut String, segs: &[PathSeg]) {
    for &seg in segs {
        match seg {
            PathSeg::Move(p) => {
                push_pdf_pt(out, p);
                out.push_str(" m ");
            }
            PathSeg::Line(p) => {
                push_pdf_pt(out, p);
                out.push_str(" l ");
            }
            PathSeg::Cubic(c1, c2, p) => {
                push_pdf_pt(out, c1);
                out.push(' ');
                push_pdf_pt(out, c2);
                out.push(' ');
                push_pdf_pt(out, p);
                out.push_str(" c ");
            }
            PathSeg::Close => out.push_str("h f\n"),
        }
    }
}

fn append_pdf_shape_paths(out: &mut String, segments: &[(Vec<Point>, bool)]) {
    for (points, closed) in segments {
        let Some(first) = points.first() else {
            continue;
        };
        push_pdf_pt(out, *first);
        out.push_str(" m ");
        for point in points.iter().skip(1) {
            push_pdf_pt(out, *point);
            out.push_str(" l ");
        }
        if *closed {
            out.push_str("h ");
        }
    }
}

fn append_pdf_shapes(
    out: &mut String,
    shapes: &[std::sync::Arc<GeometricShape>],
    states: &mut Vec<(bool, u8)>,
) {
    for item in shapes {
        let shape = item.as_ref().sanitized();
        let segments = shape.path_segments();
        if !segments.iter().any(|(points, _)| points.len() >= 2) {
            continue;
        }
        if shape.is_closed()
            && let Some(fill) = shape.fill_color
            && fill.a > 0.0
        {
            out.push_str("q\n");
            if fill.a < 0.995 {
                let state = register_pdf_state(states, false, fill.a);
                let _ = writeln!(out, "/GS{state} gs");
            }
            let _ = writeln!(out, "{} rg", color_rgb_pdf(fill));
            append_pdf_shape_paths(out, &segments);
            out.push_str("f\nQ\n");
        }
        out.push_str("q\n");
        if shape.color.a < 0.995 {
            let state = register_pdf_state(states, false, shape.color.a);
            let _ = writeln!(out, "/GS{state} gs");
        }
        let dash_cmd = match shape.line_style.dash_pattern(shape.width) {
            Some((d, g)) => format!("[{d:.2} {g:.2}] 0 d "),
            None => String::new(),
        };
        let _ = writeln!(
            out,
            "{} RG {:.3} w 1 J 1 j {dash_cmd}",
            color_rgb_pdf(shape.color),
            shape.width
        );
        append_pdf_shape_paths(out, &segments);
        out.push_str("S\nQ\n");
    }
}

fn winansi_byte(ch: char) -> Option<u8> {
    match ch {
        '\u{20AC}' => Some(0x80),
        '\u{201A}' => Some(0x82),
        '\u{0192}' => Some(0x83),
        '\u{201E}' => Some(0x84),
        '\u{2026}' => Some(0x85),
        '\u{2020}' => Some(0x86),
        '\u{2021}' => Some(0x87),
        '\u{02C6}' => Some(0x88),
        '\u{2030}' => Some(0x89),
        '\u{0160}' => Some(0x8A),
        '\u{2039}' => Some(0x8B),
        '\u{0152}' => Some(0x8C),
        '\u{017D}' => Some(0x8E),
        '\u{2018}' => Some(0x91),
        '\u{2019}' => Some(0x92),
        '\u{201C}' => Some(0x93),
        '\u{201D}' => Some(0x94),
        '\u{2022}' => Some(0x95),
        '\u{2013}' => Some(0x96),
        '\u{2014}' => Some(0x97),
        '\u{02DC}' => Some(0x98),
        '\u{2122}' => Some(0x99),
        '\u{0161}' => Some(0x9A),
        '\u{203A}' => Some(0x9B),
        '\u{0153}' => Some(0x9C),
        '\u{017E}' => Some(0x9E),
        '\u{0178}' => Some(0x9F),
        '\u{0020}'..='\u{007E}' => Some(ch as u8),
        '\u{00A0}'..='\u{00FF}' => Some(ch as u8),
        _ => None,
    }
}

fn append_pdf_literal_text(out: &mut String, text: &str) {
    for ch in text.chars() {
        let byte = winansi_byte(ch).unwrap_or(b'?');
        if (0x20..=0x7e).contains(&byte) && !matches!(byte, b'(' | b')' | b'\\') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "\\{byte:03o}");
        }
    }
}

fn pdf_font_resource(font: drawcraft_doc::TextFont, bold: bool, italic: bool) -> &'static str {
    match (font, bold, italic) {
        (drawcraft_doc::TextFont::Sans, false, false) => "/F1",
        (drawcraft_doc::TextFont::Monospace, false, false) => "/F2",
        (drawcraft_doc::TextFont::Sans, true, false) => "/F3",
        (drawcraft_doc::TextFont::Sans, false, true) => "/F4",
        (drawcraft_doc::TextFont::Sans, true, true) => "/F5",
        (drawcraft_doc::TextFont::Monospace, true, false) => "/F6",
        (drawcraft_doc::TextFont::Monospace, false, true) => "/F7",
        (drawcraft_doc::TextFont::Monospace, true, true) => "/F8",
    }
}

fn append_pdf_text(
    out: &mut String,
    page_width: u32,
    text_items: &[std::sync::Arc<TextItem>],
    states: &mut Vec<(bool, u8)>,
) {
    for item in text_items {
        let item = item.as_ref().sanitized();
        let color = item.color.sanitized();
        let state = (color.a < 0.995).then(|| register_pdf_state(states, false, color.a));
        let max_w = item.effective_max_width(page_width);
        let lines =
            drawcraft_doc::wrap_styled_text_lines_with(&item, max_w, |ch, ch_font, ch_size| {
                export_approx_char_w_for(ch, ch_font, ch_size).max(1.0)
            });
        let gutter_w = f64::from((item.size * 1.15).max(14.0));
        let has_rot = item.angle_degrees.abs() > f64::EPSILON;

        out.push_str("q\n");
        if has_rot {
            let (sin, cos) = item.angle_degrees.to_radians().sin_cos();
            let _ = writeln!(
                out,
                "1 0 0 1 {:.3} {:.3} cm {:.6} {:.6} {:.6} {:.6} 0 0 cm 1 0 0 1 {:.3} {:.3} cm",
                item.x, item.y, cos, sin, -sin, cos, -item.x, -item.y
            );
        }

        let mut cur_top = item.y;
        for line in &lines {
            let line_max_size = line
                .runs
                .iter()
                .filter_map(|r| r.size)
                .fold(item.size, f32::max)
                .clamp(6.0, 120.0);
            let line_top = cur_top;
            let baseline = line_top + f64::from(line_max_size) * 0.86;
            cur_top += f64::from(line_max_size * item.line_spacing);
            let has_indent = line.list_kind.is_some() || line.is_continuation;
            let indent_w = if has_indent { gutter_w } else { 0.0 };
            let avail_w = (f64::from(max_w) - indent_w).max(24.0);
            let line_w: f64 = line
                .runs
                .iter()
                .map(|r| {
                    export_approx_text_w_for(
                        &r.text,
                        r.font.unwrap_or(item.font),
                        r.size.unwrap_or(item.size).clamp(6.0, 120.0),
                        item.letter_spacing,
                    )
                })
                .sum();
            let align_offset = match item.alignment {
                drawcraft_doc::TextAlignment::Left => 0.0,
                drawcraft_doc::TextAlignment::Center => ((avail_w - line_w) * 0.5).max(0.0),
                drawcraft_doc::TextAlignment::Right => (avail_w - line_w).max(0.0),
            };
            let content_x = item.x + indent_w + align_offset;

            if let Some(kind) = line.list_kind {
                out.push_str("q\n");
                if let Some(idx) = state {
                    let _ = writeln!(out, "/GS{idx} gs");
                }
                let rgb = color_rgb_pdf(color);
                match kind {
                    drawcraft_doc::ListItemKind::Bullet => {
                        let r = f64::from((item.size * 0.14).clamp(1.8, 6.0));
                        let bx = item.x + gutter_w * 0.45 - r;
                        let by = line_top + f64::from(item.size) * 0.55 - r;
                        let _ = writeln!(
                            out,
                            "{rgb} rg {bx:.2} {by:.2} {:.2} {:.2} re f",
                            r * 2.0,
                            r * 2.0
                        );
                    }
                    drawcraft_doc::ListItemKind::Checkbox { checked } => {
                        let bsz = f64::from((item.size * 0.68).clamp(8.0, 44.0));
                        let bx = item.x + (gutter_w - bsz) * 0.4;
                        let by = line_top + f64::from(item.size) * 0.16;
                        let sw = (bsz * 0.11).max(1.1);
                        if checked {
                            let chk_rgb = if drawcraft_color::relative_luminance(color) > 0.45 {
                                "0.08 0.09 0.12"
                            } else {
                                "1.0 1.0 1.0"
                            };
                            let _ =
                                writeln!(out, "{rgb} rg {bx:.2} {by:.2} {bsz:.2} {bsz:.2} re f");
                            let _ = writeln!(
                                out,
                                "{chk_rgb} RG {:.2} w 1 J 1 j {:.2} {:.2} m {:.2} {:.2} l {:.2} {:.2} l S",
                                (bsz * 0.14).max(1.2),
                                bx + bsz * 0.22,
                                by + bsz * 0.54,
                                bx + bsz * 0.44,
                                by + bsz * 0.76,
                                bx + bsz * 0.80,
                                by + bsz * 0.28
                            );
                        } else {
                            let _ = writeln!(
                                out,
                                "{rgb} RG {sw:.2} w {bx:.2} {by:.2} {bsz:.2} {bsz:.2} re S"
                            );
                        }
                    }
                }
                out.push_str("Q\n");
            }

            let mut run_x = content_x;
            for run in &line.runs {
                let run_font = run.font.unwrap_or(item.font);
                let run_size = run.size.unwrap_or(item.size).clamp(6.0, 120.0);
                let run_color = run.color.map_or(color, |c| c.sanitized());
                let run_state =
                    (run_color.a < 0.995).then(|| register_pdf_state(states, false, run_color.a));
                let rw =
                    export_approx_text_w_for(&run.text, run_font, run_size, item.letter_spacing);
                if let Some(hl) = run.highlight
                    && hl.a > 0.0
                    && rw > 0.0
                {
                    out.push_str("q\n");
                    if hl.a < 0.995 {
                        let h_state = register_pdf_state(states, false, hl.a);
                        let _ = writeln!(out, "/GS{h_state} gs");
                    }
                    let _ = writeln!(
                        out,
                        "{} rg {:.2} {:.2} {:.2} {:.2} re f\nQ",
                        color_rgb_pdf(hl),
                        run_x - 1.0,
                        baseline - f64::from(run_size) * 0.78,
                        rw + 2.0,
                        f64::from(run_size) * 1.05
                    );
                }
                if !run.text.is_empty() {
                    out.push_str("q\n");
                    if let Some(idx) = run_state {
                        let _ = writeln!(out, "/GS{idx} gs");
                    }
                    let _ = writeln!(out, "{} rg", color_rgb_pdf(run_color));
                    let res = pdf_font_resource(run_font, run.bold, run.italic);
                    let _ = write!(out, "BT {res} {run_size:.2} Tf ");
                    if item.letter_spacing.abs() > 0.01 {
                        let _ = write!(out, "{:.2} Tc ", item.letter_spacing);
                    }
                    let _ = write!(out, "1 0 0 -1 ");
                    fmt_coord(out, run_x);
                    out.push(' ');
                    fmt_coord(out, baseline);
                    out.push_str(" Tm (");
                    append_pdf_literal_text(out, &run.text);
                    out.push_str(") Tj ET\n");
                    if run.underline && rw > 0.0 {
                        let uy = baseline + f64::from((run_size * 0.14).max(1.5));
                        let lw = f64::from((run_size * 0.075).max(1.0));
                        let _ = writeln!(
                            out,
                            "{} RG {lw:.2} w {:.2} {:.2} m {:.2} {:.2} l S",
                            color_rgb_pdf(run_color),
                            run_x,
                            uy,
                            run_x + rw,
                            uy
                        );
                    }
                    out.push_str("Q\n");
                }
                run_x += rw;
            }
        }
        out.push_str("Q\n");
    }
}

fn append_pdf_paper_dots(out: &mut String, dots: &[(f64, f64, f64)], color: Color) {
    if dots.is_empty() {
        return;
    }
    let _ = writeln!(out, "{} rg", color_rgb_pdf(color));
    for &(cx, cy, r) in dots {
        fmt_coord(out, cx - r);
        out.push(' ');
        fmt_coord(out, cy - r);
        out.push(' ');
        let d = r * 2.0;
        fmt_coord(out, d);
        out.push(' ');
        fmt_coord(out, d);
        out.push_str(" re f\n");
    }
}

fn build_pdf_paper_xobject_stream(
    w: u32,
    h: u32,
    template: PaperTemplate,
    paper_color: PaperColor,
) -> Vec<u8> {
    let w_f = f64::from(w);
    let h_f = f64::from(h);
    let pt_h = h_f * 0.5;
    let mut out = String::with_capacity(2048);
    let _ = writeln!(out, "0.5 0 0 -0.5 0 {pt_h:.3} cm");
    let _ = writeln!(
        out,
        "{} rg 0 0 {w_f:.2} {h_f:.2} re f",
        color_rgb_pdf(paper_color.background_color())
    );

    let geom = build_paper_geometry(w, h, template);
    let line_rgb = color_rgb_pdf(paper_color.line_color());
    let margin_rgb = color_rgb_pdf(paper_color.margin_color());
    for &(x1, y1, x2, y2, lw, is_margin) in &geom.lines {
        let rgb = if is_margin { &margin_rgb } else { &line_rgb };
        let _ = write!(out, "{rgb} RG {lw:.2} w ");
        fmt_coord(&mut out, x1);
        out.push(' ');
        fmt_coord(&mut out, y1);
        out.push_str(" m ");
        fmt_coord(&mut out, x2);
        out.push(' ');
        fmt_coord(&mut out, y2);
        out.push_str(" l S\n");
    }
    append_pdf_paper_dots(&mut out, &geom.dots, paper_color.line_color());
    if !geom.checkboxes.is_empty() {
        let _ = writeln!(out, "{margin_rgb} RG 1.25 w");
        for b in &geom.checkboxes {
            let _ = writeln!(
                out,
                "{:.2} {:.2} {:.2} {:.2} re S",
                b.x, b.y, b.size, b.size
            );
        }
    }
    if !geom.labels.is_empty() {
        for lbl in &geom.labels {
            let rgb = if lbl.accent { &margin_rgb } else { &line_rgb };
            let baseline_y = lbl.y + lbl.font_size * 0.86;
            let _ = write!(
                out,
                "q {rgb} rg BT /F1 {:.2} Tf 1 0 0 -1 {:.2} {:.2} Tm (",
                lbl.font_size, lbl.x, baseline_y
            );
            append_pdf_literal_text(&mut out, lbl.text);
            out.push_str(") Tj ET Q\n");
        }
    }
    out.into_bytes()
}

struct PdfImageEntry {
    xobj_name: String,
    obj_id: usize,
    smask_obj_id: Option<usize>,
    width: u32,
    height: u32,
    rgb_bytes: Vec<u8>,
    alpha_bytes: Option<Vec<u8>>,
    page_image: PageImage,
}

fn prepare_pdf_page_images(page: &NotebookPage, next_obj_id: &mut usize) -> Vec<PdfImageEntry> {
    let mut entries = Vec::new();
    for (idx, image) in page.images.iter().enumerate() {
        let clean = image.sanitized();
        if clean.png_bytes.is_empty() {
            continue;
        }
        let Ok(decoded) = image::load_from_memory(&clean.png_bytes) else {
            continue;
        };
        let rgba = decoded.to_rgba8();
        let w = rgba.width();
        let h = rgba.height();
        if w == 0 || h == 0 {
            continue;
        }
        let raw = rgba.as_raw();
        let pixel_count = (w as usize).saturating_mul(h as usize);
        let mut rgb_bytes = Vec::with_capacity(pixel_count.saturating_mul(3));
        let mut has_transparency = false;
        let mut alpha_bytes = Vec::with_capacity(pixel_count);
        for px in raw.as_chunks::<4>().0 {
            rgb_bytes.extend_from_slice(&px[0..3]);
            alpha_bytes.push(px[3]);
            if px[3] < 255 {
                has_transparency = true;
            }
        }
        let obj_id = *next_obj_id;
        *next_obj_id += 1;
        let smask_obj_id = if has_transparency {
            let smask_id = *next_obj_id;
            *next_obj_id += 1;
            Some(smask_id)
        } else {
            None
        };
        entries.push(PdfImageEntry {
            xobj_name: format!("Im{idx}"),
            obj_id,
            smask_obj_id,
            width: w,
            height: h,
            rgb_bytes,
            alpha_bytes: has_transparency.then_some(alpha_bytes),
            page_image: clean,
        });
    }
    entries
}

fn append_pdf_images(out: &mut String, entries: &[PdfImageEntry]) {
    for entry in entries {
        let img = &entry.page_image;
        let center = img.center();
        let (sin, cos) = img.angle_degrees.to_radians().sin_cos();
        let w = img.width;
        let h = img.height;
        let a = w * cos;
        let b = w * sin;
        let c = h * sin;
        let d = -h * cos;
        let e = center.x - 0.5 * w * cos - 0.5 * h * sin;
        let f = center.y - 0.5 * w * sin + 0.5 * h * cos;
        let _ = writeln!(
            out,
            "q {a:.4} {b:.4} {c:.4} {d:.4} {e:.4} {f:.4} cm /{} Do Q",
            entry.xobj_name
        );
    }
}

fn emit_pdf_image_objects(pdf: &mut Vec<u8>, offsets: &mut [usize], entries: &[PdfImageEntry]) {
    for entry in entries {
        if let (Some(smask_id), Some(alpha)) = (entry.smask_obj_id, &entry.alpha_bytes) {
            offsets[smask_id] = pdf.len();
            let smask_extra = format!(
                "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 ",
                entry.width, entry.height
            );
            append_pdf_stream_object(pdf, smask_id, &smask_extra, alpha);
        }
        offsets[entry.obj_id] = pdf.len();
        let smask_ref = entry
            .smask_obj_id
            .map_or(String::new(), |id| format!("/SMask {id} 0 R "));
        let img_extra = format!(
            "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 {smask_ref}",
            entry.width, entry.height
        );
        append_pdf_stream_object(pdf, entry.obj_id, &img_extra, &entry.rgb_bytes);
    }
}

fn build_pdf_backdrop_entry(
    page: &NotebookPage,
    rendered: crate::pdf::RenderedPdfPage,
    obj_id: usize,
) -> PdfImageEntry {
    let mut rgb_bytes = Vec::with_capacity(rendered.rgba.len() / 4 * 3);
    for px in rendered.rgba.as_chunks::<4>().0 {
        rgb_bytes.extend_from_slice(&px[..3]);
    }
    let page_w = f64::from(page.width.clamp(64, 8192));
    let page_h = f64::from(page.height.clamp(64, 8192));
    PdfImageEntry {
        xobj_name: "Bg0".to_string(),
        obj_id,
        smask_obj_id: None,
        width: rendered.width,
        height: rendered.height,
        rgb_bytes,
        alpha_bytes: None,
        page_image: PageImage {
            x: 0.0,
            y: 0.0,
            width: page_w,
            height: page_h,
            angle_degrees: 0.0,
            pixel_width: rendered.width,
            pixel_height: rendered.height,
            png_bytes: Arc::new(Vec::new()),
        },
    }
}

fn render_grouped_pdf_backdrops(
    doc: &Document,
    pages: &[NotebookPage],
    next_obj_id: &mut usize,
) -> Vec<Option<PdfImageEntry>> {
    let mut entries: Vec<Option<PdfImageEntry>> =
        std::iter::repeat_with(|| None).take(pages.len()).collect();
    if !doc.pdf_documents.is_empty() {
        struct PdfRenderRequest {
            doc_page_idx: usize,
            pdf_page: usize,
            px_w: u32,
            px_h: u32,
            rot: u8,
        }
        let mut by_pdf: Vec<Vec<PdfRenderRequest>> =
            (0..doc.pdf_documents.len()).map(|_| Vec::new()).collect();
        for (doc_page_idx, page) in pages.iter().enumerate() {
            if let Some(backdrop) = page.pdf_backdrop
                && let Some(bucket) = by_pdf.get_mut(backdrop.pdf_index)
            {
                let rot = page.backdrop_rotation % 4;
                let (req_w, req_h) = if rot.is_multiple_of(2) {
                    (page.width, page.height)
                } else {
                    (page.height, page.width)
                };
                let (px_w, px_h) = fit_pdf_export_pixels(req_w, req_h, 2400);
                bucket.push(PdfRenderRequest {
                    doc_page_idx,
                    pdf_page: backdrop.page_index,
                    px_w,
                    px_h,
                    rot,
                });
            }
        }

        for (pdf_idx, items) in by_pdf.into_iter().enumerate() {
            if items.is_empty() {
                continue;
            }
            let Some(pdf_src) = doc.pdf_documents.get(pdf_idx) else {
                continue;
            };
            let requests: Vec<(usize, u32, u32)> =
                items.iter().map(|r| (r.pdf_page, r.px_w, r.px_h)).collect();
            let Ok(rendered_pages) =
                crate::pdf::render_pdf_batch_exact_rgba(Arc::clone(&pdf_src.bytes), &requests)
            else {
                continue;
            };
            for (req, rendered_opt) in items.into_iter().zip(rendered_pages) {
                let doc_page_idx = req.doc_page_idx;
                let rot = req.rot;
                if let Some(mut rendered) = rendered_opt {
                    if !rot.is_multiple_of(4) {
                        let (rot_rgba, rw, rh) = crate::rotate_rgba_quarter_turns(
                            &rendered.rgba,
                            rendered.width,
                            rendered.height,
                            rot,
                        );
                        rendered.rgba = rot_rgba;
                        rendered.width = rw;
                        rendered.height = rh;
                    }
                    let obj_id = *next_obj_id;
                    *next_obj_id += 1;
                    entries[doc_page_idx] = Some(build_pdf_backdrop_entry(
                        &pages[doc_page_idx],
                        rendered,
                        obj_id,
                    ));
                }
            }
        }
    }

    for (doc_page_idx, page) in pages.iter().enumerate() {
        if entries[doc_page_idx].is_some() {
            continue;
        }
        let Some(img_idx) = page.custom_image_backdrop else {
            continue;
        };
        let Some(tpl) = doc.image_templates.get(img_idx) else {
            continue;
        };
        let Ok(decoded) = image::load_from_memory(&tpl.png_bytes) else {
            continue;
        };
        let rgba_img = decoded.to_rgba8();
        let (w, h) = (rgba_img.width(), rgba_img.height());
        if w == 0 || h == 0 {
            continue;
        }
        let rot = page.backdrop_rotation % 4;
        let (raw, out_w, out_h) = if rot == 0 {
            (rgba_img.into_raw(), w, h)
        } else {
            crate::rotate_rgba_quarter_turns(rgba_img.as_raw(), w, h, rot)
        };
        let obj_id = *next_obj_id;
        *next_obj_id += 1;
        let pixel_count = (out_w as usize).saturating_mul(out_h as usize);
        let mut rgb_bytes = Vec::with_capacity(pixel_count.saturating_mul(3));
        let mut alpha_bytes = Vec::with_capacity(pixel_count);
        let mut has_transparency = false;
        for px in raw.as_chunks::<4>().0 {
            rgb_bytes.extend_from_slice(&px[0..3]);
            alpha_bytes.push(px[3]);
            if px[3] < 255 {
                has_transparency = true;
            }
        }
        let smask_obj_id = if has_transparency {
            let id = *next_obj_id;
            *next_obj_id += 1;
            Some(id)
        } else {
            None
        };
        entries[doc_page_idx] = Some(PdfImageEntry {
            xobj_name: "Bg0".to_string(),
            obj_id,
            smask_obj_id,
            width: out_w,
            height: out_h,
            rgb_bytes,
            alpha_bytes: has_transparency.then_some(alpha_bytes),
            page_image: PageImage {
                x: 0.0,
                y: 0.0,
                width: f64::from(page.width.clamp(64, 8192)),
                height: f64::from(page.height.clamp(64, 8192)),
                angle_degrees: 0.0,
                pixel_width: out_w,
                pixel_height: out_h,
                png_bytes: Arc::new(Vec::new()),
            },
        });
    }

    entries
}

fn prepare_pdf_page_entries(
    doc: &Document,
    pages: &[NotebookPage],
    next_obj_id: &mut usize,
) -> Vec<Vec<PdfImageEntry>> {
    let backdrops = render_grouped_pdf_backdrops(doc, pages, next_obj_id);
    let mut out = Vec::with_capacity(pages.len());
    for (page, backdrop_opt) in pages.iter().zip(backdrops) {
        let mut page_entries = Vec::new();
        if let Some(bg) = backdrop_opt {
            page_entries.push(bg);
        }
        page_entries.extend(prepare_pdf_page_images(page, next_obj_id));
        out.push(page_entries);
    }
    out
}

fn build_pdf_page_stream(
    page: &NotebookPage,
    paper_xobj_name: &str,
    images: &[PdfImageEntry],
) -> (Vec<u8>, Vec<(bool, u8)>) {
    let h = page.height.clamp(64, 8192);
    let pt_h = f64::from(h) * 0.5;

    let mut out = String::with_capacity(2048);
    if !images.first().is_some_and(|e| e.xobj_name == "Bg0") {
        let _ = writeln!(out, "/{paper_xobj_name} Do");
    }
    let _ = writeln!(out, "q 0.5 0 0 -0.5 0 {pt_h:.3} cm");
    append_pdf_images(&mut out, images);

    let mut states = Vec::new();
    let mut active_gs: Option<usize> = None;
    let mut active_rgb: Option<[u8; 3]> = None;

    for stroke in &page.strokes {
        let (segs, color, alpha) = build_stroke_path(stroke);
        if segs.is_empty() {
            continue;
        }
        let is_hi = stroke.pen.kind == PenKind::Highlighter;
        let needs_gs = is_hi || alpha < 0.995;
        let next_gs = needs_gs.then(|| register_pdf_state(&mut states, is_hi, alpha));

        if next_gs != active_gs {
            if active_gs.is_some() {
                out.push_str("Q\n");
                active_rgb = None;
            }
            if let Some(gs_idx) = next_gs {
                let _ = writeln!(out, "q /GS{gs_idx} gs");
                active_rgb = None;
            }
            active_gs = next_gs;
        }

        let [r, g, b, _] = color.to_rgba8();
        let rgb_key = [r, g, b];
        if active_rgb != Some(rgb_key) {
            let _ = writeln!(out, "{} rg", color_rgb_pdf(color));
            active_rgb = Some(rgb_key);
        }
        append_pdf_path(&mut out, &segs);
    }

    if active_gs.is_some() {
        out.push_str("Q\n");
    }
    append_pdf_text(&mut out, page.width, &page.texts, &mut states);
    append_pdf_shapes(&mut out, &page.shapes, &mut states);
    out.push_str("Q\n");
    (out.into_bytes(), states)
}

fn format_pdf_resources(
    states: &[(bool, u8)],
    paper_xobj_name: &str,
    paper_obj_id: usize,
    images: &[PdfImageEntry],
) -> String {
    let mut res = format!("<< /XObject << /{paper_xobj_name} {paper_obj_id} 0 R");
    for entry in images {
        let _ = write!(res, " /{} {} 0 R", entry.xobj_name, entry.obj_id);
    }
    res.push_str(" >>");
    if !states.is_empty() {
        res.push_str(" /ExtGState <<");
        for (idx, &(is_hi, alpha_q)) in states.iter().enumerate() {
            let alpha = f32::from(alpha_q) / 255.0;
            let bm = if is_hi { "/Multiply" } else { "/Normal" };
            let _ = write!(
                res,
                " /GS{idx} << /Type /ExtGState /ca {alpha:.3} /CA {alpha:.3} /BM {bm} >>"
            );
        }
        res.push_str(" >>");
    }
    res.push_str(" /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> /F2 << /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >> /F3 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >> /F4 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Oblique /Encoding /WinAnsiEncoding >> /F5 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica-BoldOblique /Encoding /WinAnsiEncoding >> /F6 << /Type /Font /Subtype /Type1 /BaseFont /Courier-Bold /Encoding /WinAnsiEncoding >> /F7 << /Type /Font /Subtype /Type1 /BaseFont /Courier-Oblique /Encoding /WinAnsiEncoding >> /F8 << /Type /Font /Subtype /Type1 /BaseFont /Courier-BoldOblique /Encoding /WinAnsiEncoding >> >>");
    res.push_str(" >>");
    res
}

fn append_pdf_stream_object(pdf: &mut Vec<u8>, obj_id: usize, dict_extra: &str, raw_stream: &[u8]) {
    let (filter_entry, encoded) = pdf_compress_stream(raw_stream);
    let header = format!(
        "{obj_id} 0 obj\n<< {dict_extra}{filter_entry}/Length {} >>\nstream\n",
        encoded.len()
    );
    pdf.extend_from_slice(header.as_bytes());
    pdf.extend_from_slice(&encoded);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
}

#[derive(Clone, Copy, PartialEq)]
struct PaperTemplateKey {
    w: u32,
    h: u32,
    template: PaperTemplate,
    color: PaperColor,
}

/// Exports every page in `doc` as a self-contained pure-Rust multi-page PDF 1.4 byte vector
/// with shared paper Form XObjects, smooth Cubic Bezier stroke outlines, and FlateDecode streams.
pub fn export_notebook_pdf(doc: &Document) -> Vec<u8> {
    let pages = doc.resolved_pages();
    export_notebook_pdf_slice(doc, &pages)
}

/// Exports only the requested 0-based `page_indices` from `doc` as a multi-page PDF 1.4 byte vector.
/// Out-of-bounds or duplicate indices are ignored; empty selections fall back to all pages.
pub fn export_notebook_pdf_pages(doc: &Document, page_indices: &[usize]) -> Vec<u8> {
    let all_pages = doc.resolved_pages();
    if page_indices.is_empty() {
        return export_notebook_pdf_slice(doc, &all_pages);
    }
    let mut seen = std::collections::HashSet::new();
    let mut selected = Vec::with_capacity(page_indices.len().min(all_pages.len()));
    for &idx in page_indices {
        if idx < all_pages.len() && seen.insert(idx) {
            selected.push(all_pages[idx].clone());
        }
    }
    if selected.is_empty() {
        return export_notebook_pdf_slice(doc, &all_pages);
    }
    export_notebook_pdf_slice(doc, &selected)
}

fn export_notebook_pdf_slice(doc: &Document, pages: &[NotebookPage]) -> Vec<u8> {
    let page_count = pages.len();

    let mut template_keys: Vec<PaperTemplateKey> = Vec::new();
    let mut page_template_idx = Vec::with_capacity(page_count);
    for page in pages {
        let key = PaperTemplateKey {
            w: page.width.clamp(64, 8192),
            h: page.height.clamp(64, 8192),
            template: page.paper_template,
            color: page.paper_color,
        };
        let idx = if let Some(pos) = template_keys.iter().position(|k| *k == key) {
            pos
        } else {
            let pos = template_keys.len();
            template_keys.push(key);
            pos
        };
        page_template_idx.push(idx);
    }

    let first_template_obj = 3usize;
    let first_page_obj = first_template_obj + template_keys.len();
    let mut next_extra_obj = first_page_obj + page_count * 2;
    let page_images = prepare_pdf_page_entries(doc, pages, &mut next_extra_obj);
    let total_objs = next_extra_obj - 1;
    let mut offsets = vec![0usize; total_objs + 1];

    let mut pdf = Vec::with_capacity(page_count * 2048);
    pdf.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n%DrawCraft\n");

    offsets[1] = pdf.len();
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    let mut kids = String::new();
    for i in 0..page_count {
        let page_obj_id = first_page_obj + i * 2;
        if i > 0 {
            kids.push(' ');
        }
        let _ = write!(kids, "{page_obj_id} 0 R");
    }
    offsets[2] = pdf.len();
    let pages_obj =
        format!("2 0 obj\n<< /Type /Pages /Kids [{kids}] /Count {page_count} >>\nendobj\n");
    pdf.extend_from_slice(pages_obj.as_bytes());

    for (t_idx, key) in template_keys.iter().enumerate() {
        let obj_id = first_template_obj + t_idx;
        offsets[obj_id] = pdf.len();
        let pt_w = f64::from(key.w) * 0.5;
        let pt_h = f64::from(key.h) * 0.5;
        let stream = build_pdf_paper_xobject_stream(key.w, key.h, key.template, key.color);
        let extra = format!(
            "/Type /XObject /Subtype /Form /FormType 1 /BBox [0 0 {pt_w:.2} {pt_h:.2}] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> >> >> "
        );
        append_pdf_stream_object(&mut pdf, obj_id, &extra, &stream);
    }

    for (i, page) in pages.iter().enumerate() {
        let page_obj_id = first_page_obj + i * 2;
        let content_obj_id = page_obj_id + 1;
        let t_idx = page_template_idx[i];
        let paper_obj_id = first_template_obj + t_idx;
        let paper_name = format!("P{t_idx}");

        let (stream, states) = build_pdf_page_stream(page, &paper_name, &page_images[i]);
        let resources = format_pdf_resources(&states, &paper_name, paper_obj_id, &page_images[i]);
        let pt_w = f64::from(page.width.clamp(64, 8192)) * 0.5;
        let pt_h = f64::from(page.height.clamp(64, 8192)) * 0.5;

        offsets[page_obj_id] = pdf.len();
        let page_dict = format!(
            "{page_obj_id} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pt_w:.2} {pt_h:.2}] /Resources {resources} /Contents {content_obj_id} 0 R >>\nendobj\n"
        );
        pdf.extend_from_slice(page_dict.as_bytes());

        offsets[content_obj_id] = pdf.len();
        append_pdf_stream_object(&mut pdf, content_obj_id, "", &stream);
    }

    for entries in &page_images {
        emit_pdf_image_objects(&mut pdf, &mut offsets, entries);
    }

    let xref_offset = pdf.len();
    let xref_head = format!("xref\n0 {}\n0000000000 65535 f \n", total_objs + 1);
    pdf.extend_from_slice(xref_head.as_bytes());
    for &off in offsets.iter().skip(1) {
        let entry = format!("{off:010} 00000 n \n");
        pdf.extend_from_slice(entry.as_bytes());
    }
    let trailer = format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
        total_objs + 1
    );
    pdf.extend_from_slice(trailer.as_bytes());

    pdf
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use drawcraft_doc::{InkStroke, NotebookPreset, PaperTemplate, PenSettings, StrokeSample};

    use super::*;

    fn sample_stroke(doc: &mut Document, pen: PenSettings) -> Arc<InkStroke> {
        let id = doc.alloc_stroke_id();
        let pts = vec![
            StrokeSample::from_xy_pressure(100.0, 200.0, 0.35),
            StrokeSample::from_xy_pressure(180.0, 220.0, 0.8),
            StrokeSample::from_xy_pressure(260.0, 250.0, 0.55),
        ];
        Arc::new(InkStroke::new(id, pen, pts.clone(), pts))
    }

    #[test]
    fn svg_export_contains_paper_and_strokes() {
        let mut doc = NotebookPreset::EngineeringGrid.create_document();
        let s1 = sample_stroke(&mut doc, PenSettings::fountain_pen());
        let s2 = sample_stroke(&mut doc, PenSettings::highlighter());
        doc.strokes.push(s1);
        doc.strokes.push(s2);

        let svg = render_document_svg(&doc);
        assert!(svg.starts_with("<?xml version=\"1.0\""));
        assert!(svg.contains("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.contains("id=\"paper-template\""));
        assert!(svg.contains("mix-blend-mode:multiply"));
        assert!(svg.trim_end().ends_with("</svg>"));
    }

    #[test]
    fn pdf_export_generates_valid_multipage_document() {
        let mut doc = NotebookPreset::EverydayLined.create_document();
        let s1 = sample_stroke(&mut doc, PenSettings::fountain_pen());
        doc.strokes.push(s1);

        doc.add_page(Some(PaperTemplate::Dotted), None);
        let s2 = sample_stroke(&mut doc, PenSettings::highlighter());
        doc.strokes.push(s2);

        let bytes = export_notebook_pdf(&doc);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("/Count 2"));
        assert!(text.contains("/Type /Page"));
        assert!(text.contains("/BM /Multiply"));
        assert!(text.contains("xref"));
        assert!(text.trim_end().ends_with("%%EOF"));
    }

    #[test]
    fn light_pressure_stroke_exports_with_lighter_opaque_color() {
        let mut doc = NotebookPreset::EverydayLined.create_document();
        let pen = PenSettings::fountain_pen();
        let light_id = doc.alloc_stroke_id();
        let light_pts = vec![
            StrokeSample::from_xy_pressure(100.0, 200.0, 0.05),
            StrokeSample::from_xy_pressure(220.0, 200.0, 0.05),
        ];
        doc.strokes.push(Arc::new(InkStroke::new(
            light_id,
            pen.clone(),
            light_pts.clone(),
            light_pts,
        )));

        let svg = render_document_svg(&doc);
        assert!(
            !svg.contains("fill-opacity=\""),
            "pressure should not reduce the SVG stroke opacity"
        );
        assert!(!svg.contains(format!("fill=\"{}\"", color_hex(pen.color)).as_str()));

        let pdf = String::from_utf8_lossy(&export_notebook_pdf(&doc)).into_owned();
        assert!(
            !pdf.contains("/ca "),
            "pressure should not reduce the PDF stroke opacity"
        );
        assert!(!pdf.contains(format!("{} rg", color_rgb_pdf(pen.color)).as_str()));
    }

    #[test]
    fn dotted_svg_uses_compact_pattern_and_bezier_curves() {
        let mut doc = NotebookPreset::EverydayLined.create_document();
        doc.paper_template = PaperTemplate::Dotted;
        if let Some(page) = doc.pages.get_mut(doc.current_page) {
            page.paper_template = PaperTemplate::Dotted;
        }
        let id = doc.alloc_stroke_id();
        let pts: Vec<StrokeSample> = (0..60)
            .map(|i| {
                let t = f64::from(i) * 5.0;
                StrokeSample::from_xy_pressure(100.0 + t, 200.0 + (t * 0.04).sin() * 40.0, 0.55)
            })
            .collect();
        doc.strokes.push(Arc::new(InkStroke::new(
            id,
            PenSettings::fountain_pen(),
            pts.clone(),
            pts,
        )));

        let svg = render_document_svg(&doc);
        assert!(svg.contains("<pattern id=\"paper-dots\""));
        assert!(svg.contains("fill=\"url(#paper-dots)\""));
        assert!(
            svg.contains('C'),
            "curved stroke SVG path should use Cubic Beziers"
        );
        assert!(
            svg.len() < 8_000,
            "dotted SVG + 60-sample stroke should be compact, got {} bytes",
            svg.len()
        );
    }

    #[test]
    fn multipage_pdf_shares_form_xobject_and_uses_flate_compression() {
        let mut doc = NotebookPreset::EverydayLined.create_document();
        doc.paper_template = PaperTemplate::Dotted;
        if let Some(page) = doc.pages.get_mut(doc.current_page) {
            page.paper_template = PaperTemplate::Dotted;
        }
        for _ in 0..9 {
            doc.add_page(Some(PaperTemplate::Dotted), None);
        }
        let s1 = sample_stroke(&mut doc, PenSettings::fountain_pen());
        doc.strokes.push(s1);

        let pdf_bytes = export_notebook_pdf(&doc);
        let text = String::from_utf8_lossy(&pdf_bytes);
        assert!(text.contains("/Filter /FlateDecode"));
        assert!(text.contains("/Subtype /Form"));
        // Only one Form XObject emitted for 10 pages sharing identical Dotted paper.
        assert_eq!(text.matches("/Subtype /Form").count(), 1);
        assert!(
            pdf_bytes.len() < 25_000,
            "10-page dotted PDF should be tiny, got {} bytes",
            pdf_bytes.len()
        );
    }

    #[test]
    fn sharp_corner_and_hairpin_svg_and_pdf_exports_remain_bounded() {
        let mut doc = NotebookPreset::EverydayLined.create_document();
        let mut pen = PenSettings::ball_pen();
        pen.size = 10.0;
        let pts = vec![
            StrokeSample::from_xy_pressure(50.0, 200.0, 0.7),
            StrokeSample::from_xy_pressure(160.0, 200.0, 0.7),
            StrokeSample::from_xy_pressure(60.0, 170.0, 0.7),
        ];
        let id = doc.alloc_stroke_id();
        let stroke = InkStroke::new(id, pen.clone(), pts.clone(), pts);
        let (segs, _, _) = build_stroke_path(&stroke);
        assert!(segs.len() >= 6);
        let max_x = segs
            .iter()
            .flat_map(|seg| match *seg {
                PathSeg::Move(p) | PathSeg::Line(p) => vec![p.x],
                PathSeg::Cubic(a, b, c) => vec![a.x, b.x, c.x],
                PathSeg::Close => Vec::new(),
            })
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            max_x <= 160.0 + f64::from(pen.size) * 0.5 + 0.5,
            "sharp V-bend exported outline spiked past round join: max_x={max_x}"
        );

        doc.strokes.push(Arc::new(stroke));
        let svg = render_document_svg(&doc);
        assert!(svg.contains("<path"));
        let pdf = export_notebook_pdf(&doc);
        assert!(pdf.starts_with(b"%PDF-1.4"));
    }

    #[test]
    fn svg_and_pdf_export_embed_page_images_with_optional_alpha_mask() {
        let mut doc = Document::new(400, 500);
        let png_bytes = drawcraft_raster::png_encode(
            2,
            2,
            &[
                255, 0, 0, 255, // opaque red
                0, 255, 0, 128, // semi-transparent green
                0, 0, 255, 255, // opaque blue
                255, 255, 0, 64, // semi-transparent yellow
            ],
        )
        .unwrap();
        doc.images.push(Arc::new(PageImage {
            x: 50.0,
            y: 80.0,
            width: 120.0,
            height: 90.0,
            angle_degrees: 30.0,
            pixel_width: 2,
            pixel_height: 2,
            png_bytes: Arc::new(png_bytes),
        }));

        let svg = render_document_svg(&doc);
        assert!(svg.contains("id=\"page-images\""));
        assert!(svg.contains("data:image/png;base64,"));
        assert!(svg.contains("transform=\"rotate(30"));

        let pdf_bytes = export_notebook_pdf(&doc);
        let pdf_text = String::from_utf8_lossy(&pdf_bytes);
        assert!(pdf_text.contains("/Subtype /Image"));
        assert!(pdf_text.contains("/ColorSpace /DeviceRGB"));
        assert!(pdf_text.contains("/SMask"));
        assert!(pdf_text.contains("/Im0 "));

        let pages = doc.resolved_pages();
        let mut next_id = 6;
        let entries = prepare_pdf_page_images(&pages[0], &mut next_id);
        let (raw_stream, _) = build_pdf_page_stream(&pages[0], "P0", &entries);
        let stream_text = String::from_utf8_lossy(&raw_stream);
        assert!(stream_text.contains("/Im0 Do Q"));
    }

    #[test]
    fn svg_and_pdf_export_include_pdf_backdrop() {
        let objects = [
            b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".as_slice(),
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".as_slice(),
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << >> >>\nendobj\n".as_slice(),
            b"4 0 obj\n<< /Length 28 >>\nstream\n0 1 0 rg 0 0 100 100 re f\nendstream\nendobj\n".as_slice(),
        ];
        let mut raw_pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0usize];
        for obj in objects {
            offsets.push(raw_pdf.len());
            raw_pdf.extend_from_slice(obj);
        }
        let xref = raw_pdf.len();
        raw_pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for off in &offsets[1..] {
            raw_pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        raw_pdf.extend_from_slice(
            format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );

        let mut doc = Document::new(200, 200);
        doc.title = "PDF Export Test".to_string();
        doc.pdf_documents.push(drawcraft_doc::EmbeddedPdf {
            name: "green.pdf".to_string(),
            bytes: Arc::new(raw_pdf),
        });
        doc.pdf_backdrop = Some(drawcraft_doc::PdfPageBackdrop {
            pdf_index: 0,
            page_index: 0,
        });
        doc.sync_active_to_pages();
        doc.add_page(None, None);
        doc.select_page(0);

        let svg = render_document_svg(&doc);
        assert!(svg.contains("id=\"pdf-backdrop\""));
        assert!(svg.contains("data:image/png;base64,"));

        let exported_pdf = export_notebook_pdf(&doc);
        let text = String::from_utf8_lossy(&exported_pdf);
        assert!(text.contains("/Bg0"));
        assert!(text.contains("/Type /XObject /Subtype /Image"));
    }

    #[test]
    fn svg_export_wraps_text_according_to_user_max_width() {
        let mut doc = Document::new(800, 1000);
        doc.texts.push(Arc::new(TextItem {
            text: "alpha beta gamma delta".to_string(),
            x: 40.0,
            y: 80.0,
            size: 20.0,
            max_width: Some(80.0),
            ..TextItem::default()
        }));

        let lines = wrapped_export_lines(doc.width, &doc.texts[0]);
        assert!(
            lines.len() >= 3,
            "expected text with max_width=80 to wrap across multiple lines, got {lines:?}"
        );
        let svg = render_document_svg(&doc);
        assert_eq!(svg.matches("<text ").count(), lines.len());
    }

    #[test]
    fn svg_export_wraps_linked_text_in_anchor_and_underlines() {
        let mut doc = Document::new(800, 1000);
        doc.texts.push(Arc::new(TextItem {
            text: "Open Rust docs".to_string(),
            x: 40.0,
            y: 80.0,
            size: 20.0,
            link_url: Some("https://doc.rust-lang.org".to_string()),
            ..TextItem::default()
        }));

        let svg = render_document_svg(&doc);
        assert!(svg.contains("<a href=\"https://doc.rust-lang.org\""));
        assert!(svg.contains("text-decoration=\"underline\""));
        assert!(svg.contains("</a>"));
    }

    #[test]
    fn svg_and_pdf_export_support_new_templates_custom_backdrop_rotation_and_subset() {
        use drawcraft_doc::{CustomPaperTemplate, CustomTemplateKind};

        let mut doc = Document::new(800, 1000);
        doc.paper_template = PaperTemplate::WeeklyPlanner;
        doc.add_page(Some(PaperTemplate::Checklist), None);
        doc.add_page(Some(PaperTemplate::MonthlyPlanner), None);
        doc.select_page(0);

        let svg_weekly = render_document_svg(&doc);
        assert!(svg_weekly.contains("WEEKLY PLANNER"));

        doc.select_page(1);
        let svg_checklist = render_document_svg(&doc);
        assert!(svg_checklist.contains("CHECKLIST"));
        assert!(svg_checklist.contains("<rect "));

        let png = drawcraft_raster::png_encode(2, 2, &[200, 100, 50, 255].repeat(4)).unwrap();
        let custom = CustomPaperTemplate {
            id: "tpl_bg".to_string(),
            name: "Bg".to_string(),
            kind: CustomTemplateKind::Image,
            width: 800,
            height: 1000,
            pdf_page_index: 0,
            bytes: Arc::new(png),
        };
        doc.apply_custom_template_to_page(2, &custom).unwrap();
        assert!(doc.rotate_page_quarter_turns(2, 1));
        let page2 = doc.document_for_page(2).unwrap();
        let svg_rot = render_document_svg(&page2);
        assert!(svg_rot.contains("custom-template-backdrop"));
        assert!(svg_rot.contains("transform=\"rotate(90 "));

        let subset_pdf = export_notebook_pdf_pages(&doc, &[0, 2]);
        let pdf_text = String::from_utf8_lossy(&subset_pdf);
        assert!(pdf_text.contains("/Count 2"));
        assert!(pdf_text.contains("/Bg0"));
    }

    #[test]
    fn cpu_svg_and_pdf_export_render_pencil_polygon_fills_and_rich_text() {
        use drawcraft_color::Color;
        use drawcraft_doc::{
            GeometricShape, PenKind, Point, ShapeKind, ShapeLineStyle, TextAlignment, TextItem,
            TextSpan,
        };

        let mut doc = Document::new(800, 1000);
        let s = sample_stroke(&mut doc, PenSettings::pencil());
        assert_eq!(s.pen.kind, PenKind::Pencil);
        doc.strokes.push(s);

        doc.shapes.push(Arc::new(GeometricShape {
            kind: ShapeKind::Polygon,
            fill_color: Some(Color::from_rgba8(0x28, 0x66, 0xE8, 96)),
            line_style: ShapeLineStyle::Dashed,
            points: vec![
                Point::new(120.0, 140.0),
                Point::new(320.0, 140.0),
                Point::new(360.0, 280.0),
                Point::new(100.0, 280.0),
            ],
            color: Color::from_rgba8(0x28, 0x66, 0xE8, 255),
            width: 3.0,
            ..GeometricShape::default()
        }));

        doc.texts.push(Arc::new(TextItem {
            text: "Bold Italic\n[x] Done".to_string(),
            x: 100.0,
            y: 350.0,
            size: 24.0,
            alignment: TextAlignment::Center,
            max_width: Some(300.0),
            spans: vec![
                TextSpan {
                    start: 0,
                    end: 4,
                    bold: true,
                    italic: false,
                    underline: true,
                    highlight: Some(Color::from_rgba8(0xFF, 0xEE, 0x58, 180)),
                    font: Some(drawcraft_doc::TextFont::Monospace),
                    size: Some(30.0),
                    color: Some(Color::from_rgba8(0xDC, 0x26, 0x26, 255)),
                },
                TextSpan {
                    start: 5,
                    end: 11,
                    bold: false,
                    italic: true,
                    underline: false,
                    highlight: None,
                    ..TextSpan::default()
                },
            ],
            ..TextItem::default()
        }));
        doc.sync_active_to_pages();

        let png_surface = crate::render_document(&doc, None);
        assert!(png_surface.pixel(220, 200)[2] > 0.70);

        let svg = render_document_svg(&doc);
        assert!(svg.contains("stroke-dasharray="));
        assert!(svg.contains("fill-opacity="));
        assert!(svg.contains("font-weight=\"700\""));
        assert!(svg.contains("font-style=\"italic\""));
        assert!(svg.contains("font-family=\"monospace\""));
        assert!(svg.contains("font-size=\"30\""));
        assert!(svg.contains("fill=\"#DC2626\""));

        let pdf = export_notebook_pdf(&doc);
        let parsed =
            hayro::hayro_syntax::Pdf::new(Arc::new(pdf)).expect("generated PDF should parse");
        assert_eq!(parsed.pages().len(), 1);
    }
}
