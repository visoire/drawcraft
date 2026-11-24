//! Deterministic CPU dab rasterizer for strokes (the oracle renderer).

use drawcraft_color::{BlendMode, Color as InkColor};
use drawcraft_doc::{PenKind, PenSettings, StrokeSample};
use drawcraft_geom::{IRect, Point, Vec2};
use drawcraft_raster::Surface;

use crate::dynamics::{SpineNode, evaluate_spine};
use crate::spline::densify_linear;

#[inline]
fn lerp_node(a: &SpineNode, b: &SpineNode, t: f64) -> SpineNode {
    let tf = t as f32;
    let dr = (b.angle_rad - a.angle_rad + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    SpineNode {
        point: a.point.lerp(b.point, t),
        radius: a.radius + (b.radius - a.radius) * tf,
        alpha: a.alpha + (b.alpha - a.alpha) * tf,
        color: InkColor::new(
            a.color.r + (b.color.r - a.color.r) * tf,
            a.color.g + (b.color.g - a.color.g) * tf,
            a.color.b + (b.color.b - a.color.b) * tf,
            a.color.a + (b.color.a - a.color.a) * tf,
        ),
        angle_rad: a.angle_rad + dr * tf,
        roundness: a.roundness + (b.roundness - a.roundness) * tf,
        direction_rad: a.direction_rad,
        arc_length: a.arc_length + (b.arc_length - a.arc_length) * t,
    }
}

#[inline]
fn dab_coverage(dx: f64, dy: f64, node: &SpineNode, hardness: f32) -> f32 {
    let r = f64::from(node.radius.max(0.2));
    let round = f64::from(node.roundness.clamp(0.15, 1.0));
    let (sin_a, cos_a) = f64::from(node.angle_rad).sin_cos();
    let lx = dx * cos_a + dy * sin_a;
    let ly = (-dx * sin_a + dy * cos_a) / round;
    let dist = (lx * lx + ly * ly).sqrt();

    let edge = (0.85_f64 + (1.0 - f64::from(hardness.clamp(0.0, 1.0))) * r * 0.65).max(0.65);
    let inner = (r - edge * 0.5).max(0.0);
    if dist <= inner {
        1.0
    } else if dist >= inner + edge {
        0.0
    } else {
        let t = (dist - inner) / edge;
        // Smooth cubic step from 1.0 to 0.0
        (1.0 - t * t * (3.0 - 2.0 * t)) as f32
    }
}

fn stamp_dab(
    cov_buf: &mut [f32],
    color_buf: &mut [[u8; 3]],
    bounds: IRect,
    node: &SpineNode,
    hardness: f32,
    start_clip: Option<(Point, Vec2)>,
    end_clip: Option<(Point, Vec2)>,
) {
    let reach = f64::from(node.radius.max(0.2)) + 2.0;
    let min_x = (node.point.x - reach).floor() as i32;
    let min_y = (node.point.y - reach).floor() as i32;
    let max_x = (node.point.x + reach).ceil() as i32;
    let max_y = (node.point.y + reach).ceil() as i32;

    let x0 = min_x.max(bounds.x);
    let y0 = min_y.max(bounds.y);
    let x1 = max_x.min(bounds.right());
    let y1 = max_y.min(bounds.bottom());
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    let w = bounds.w as usize;
    let color = node.color.sanitized();
    let color_rgb = [
        (color.r * 255.0).round() as u8,
        (color.g * 255.0).round() as u8,
        (color.b * 255.0).round() as u8,
    ];
    for py in y0..y1 {
        let cy = f64::from(py) + 0.5;
        let row_off = ((py - bounds.y) as usize) * w;
        for px in x0..x1 {
            let cx = f64::from(px) + 0.5;
            let sample_point = Point::new(cx, cy);
            let start_coverage = start_clip.map_or(1.0, |(origin, tangent)| {
                (Vec2::from_points(origin, sample_point).dot(tangent) + 0.5).clamp(0.0, 1.0) as f32
            });
            let end_coverage = end_clip.map_or(1.0, |(origin, tangent)| {
                (0.5 - Vec2::from_points(origin, sample_point).dot(tangent)).clamp(0.0, 1.0) as f32
            });
            let cov = dab_coverage(cx - node.point.x, cy - node.point.y, node, hardness)
                * node.alpha
                * start_coverage
                * end_coverage;
            if cov > 0.0 {
                let idx = row_off + ((px - bounds.x) as usize);
                if let Some(slot) = cov_buf.get_mut(idx)
                    && cov >= *slot
                {
                    *slot = cov;
                    if let Some(color_slot) = color_buf.get_mut(idx) {
                        *color_slot = color_rgb;
                    }
                }
            }
        }
    }
}

/// Rasterizes a stroke onto `surface` via an isolated stroke coverage buffer (preventing
/// self-overlap darkening on semi-transparent highlighters) and returns the damaged pixel bounds.
/// Computes a deterministic paper-tooth graphite multiplier (`0.42..=1.0`) for `PenKind::Pencil`.
#[must_use]
pub fn graphite_grain_factor(x: f32, y: f32) -> f32 {
    let xi = if x.is_finite() {
        (x * 0.5).floor() as i32 as u32
    } else {
        0
    };
    let yi = if y.is_finite() {
        (y * 0.5).floor() as i32 as u32
    } else {
        0
    };
    let fine = hash2d(xi, yi);
    let coarse = hash2d(xi >> 1, yi >> 1);
    let combined = fine * 0.68 + coarse * 0.32;
    0.42 + combined * 0.58
}

#[inline]
fn hash2d(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    (h & 0xFFFF) as f32 / 65535.0
}

pub fn render_stroke_to_surface(
    surface: &mut Surface,
    pen: &PenSettings,
    samples: &[StrokeSample],
) -> IRect {
    let dense = densify_linear(samples, 3.0);
    let nodes = evaluate_spine(pen, &dense);
    if nodes.is_empty() {
        return IRect::EMPTY;
    }
    let pen = pen.sanitized();

    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for n in &nodes {
        let pad = f64::from(n.radius) + 3.0;
        min_x = min_x.min(n.point.x - pad);
        min_y = min_y.min(n.point.y - pad);
        max_x = max_x.max(n.point.x + pad);
        max_y = max_y.max(n.point.y + pad);
    }

    let doc_rect = IRect::new(0, 0, surface.width() as i32, surface.height() as i32);
    let raw_bounds = IRect::from_bounds(
        min_x.floor() as i32,
        min_y.floor() as i32,
        max_x.ceil() as i32,
        max_y.ceil() as i32,
    );
    let bounds = raw_bounds.intersection(doc_rect);
    if bounds.is_empty() {
        return IRect::EMPTY;
    }

    let len = (bounds.w as usize).saturating_mul(bounds.h as usize);
    // Guard against absurd offscreen allocations.
    if len > 16_000_000 {
        return IRect::EMPTY;
    }
    let mut cov_buf = vec![0.0_f32; len];
    let mut color_buf = vec![[0_u8; 3]; len];

    let flat_caps = pen.kind == PenKind::Highlighter && nodes.len() > 1;
    let start_tangent = flat_caps.then(|| {
        Vec2::new(
            f64::from(nodes[0].direction_rad).cos(),
            f64::from(nodes[0].direction_rad).sin(),
        )
    });
    let end_tangent = flat_caps.then(|| {
        let last = nodes[nodes.len() - 1];
        Vec2::new(
            f64::from(last.direction_rad).cos(),
            f64::from(last.direction_rad).sin(),
        )
    });
    let start_cut = start_tangent.map(|tangent| (nodes[0].point, tangent));
    let last_node = nodes[nodes.len() - 1];
    let end_cut = end_tangent.map(|tangent| (last_node.point, tangent));
    let start_cut_depth = f64::from(nodes[0].radius.max(0.2)) + 2.0;
    let end_cut_depth = f64::from(last_node.radius.max(0.2)) + 2.0;

    stamp_dab(
        &mut cov_buf,
        &mut color_buf,
        bounds,
        &nodes[0],
        pen.hardness,
        start_cut,
        end_cut.filter(|_| last_node.arc_length <= end_cut_depth),
    );
    for pair in nodes.windows(2) {
        let a = pair[0];
        let b = pair[1];
        let dist = a.point.distance(b.point);
        let min_r = f64::from(a.radius.min(b.radius).max(0.5));
        let step = (min_r * 0.25).clamp(0.35, 2.0);
        let steps = (dist / step).ceil().clamp(1.0, 4096.0) as usize;
        for k in 1..=steps {
            let t = k as f64 / steps as f64;
            let q = lerp_node(&a, &b, t);
            let clip_start = start_cut.filter(|_| q.arc_length <= start_cut_depth);
            let clip_end = end_cut.filter(|_| last_node.arc_length - q.arc_length <= end_cut_depth);
            stamp_dab(
                &mut cov_buf,
                &mut color_buf,
                bounds,
                &q,
                pen.hardness,
                clip_start,
                clip_end,
            );
        }
    }

    let mode = match pen.kind {
        PenKind::Highlighter => BlendMode::Multiply,
        PenKind::FountainPen | PenKind::BallPen | PenKind::BrushPen | PenKind::Pencil => {
            BlendMode::Normal
        }
    };

    let is_pencil = pen.kind == PenKind::Pencil;
    let w = bounds.w as usize;
    for ly in 0..(bounds.h as usize) {
        let py = bounds.y + ly as i32;
        let row_off = ly * w;
        for lx in 0..w {
            let mut alpha = cov_buf[row_off + lx];
            if alpha > 0.0 {
                let px = bounds.x + lx as i32;
                if is_pencil {
                    let grain = graphite_grain_factor(px as f32, py as f32);
                    // Heavy pressure fills paper valleys slightly more while keeping graphite tooth.
                    let filled_grain = grain + (1.0 - grain) * (alpha * 0.35).clamp(0.0, 0.45);
                    alpha *= filled_grain;
                }
                let color = color_buf[row_off + lx];
                let src = [
                    f32::from(color[0]) / 255.0,
                    f32::from(color[1]) / 255.0,
                    f32::from(color[2]) / 255.0,
                    alpha.clamp(0.0, 1.0),
                ];
                surface.blend_pixel(px, py, src, mode);
            }
        }
    }

    bounds
}

/// Evaluates whether `pt` is within `radius` of a rendered dab center (used for tests).
pub fn point_near_spine(nodes: &[SpineNode], pt: Point) -> Option<f64> {
    nodes
        .iter()
        .map(|n| n.point.distance(pt))
        .min_by(f64::total_cmp)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use drawcraft_color::Color;

    #[test]
    fn highlighter_renders_uniformly_without_overlap_darkening_and_preserves_black_ink() {
        let mut s = Surface::new(160, 80, Color::WHITE);
        let mut black_pen = PenSettings::ball_pen();
        black_pen.color = Color::BLACK;
        black_pen.size = 6.0;
        render_stroke_to_surface(
            &mut s,
            &black_pen,
            &[
                StrokeSample::from_xy_pressure(80.0, 10.0, 1.0),
                StrokeSample::from_xy_pressure(80.0, 70.0, 1.0),
            ],
        );

        let hl = PenSettings::highlighter();
        render_stroke_to_surface(
            &mut s,
            &hl,
            &[
                StrokeSample::from_xy_pressure(20.0, 40.0, 1.0),
                StrokeSample::from_xy_pressure(140.0, 40.0, 1.0),
            ],
        );

        // Black ink underneath yellow highlighter must stay black (Multiply blend).
        let center_ink = s.pixel_u8(80, 40);
        assert!(
            center_ink[0] < 10 && center_ink[1] < 10 && center_ink[2] < 10,
            "{center_ink:?}"
        );

        // Highlighter pixels on white paper should be uniform along the stroke.
        let p_left = s.pixel_u8(40, 40);
        let p_right = s.pixel_u8(120, 40);
        assert_eq!(p_left, p_right);
        assert!(p_left[0] > 230 && p_left[1] > 220 && p_left[2] < 200);
    }

    #[test]
    fn light_pressure_stroke_renders_lighter_without_fading_on_white_paper() {
        let mut s = Surface::new(160, 80, Color::WHITE);
        let pen = PenSettings::fountain_pen();

        render_stroke_to_surface(
            &mut s,
            &pen,
            &[
                StrokeSample::from_xy_pressure(20.0, 20.0, 0.05),
                StrokeSample::from_xy_pressure(140.0, 20.0, 0.05),
            ],
        );
        render_stroke_to_surface(
            &mut s,
            &pen,
            &[
                StrokeSample::from_xy_pressure(20.0, 55.0, 0.95),
                StrokeSample::from_xy_pressure(140.0, 55.0, 0.95),
            ],
        );

        let light_center = s.pixel_u8(80, 20);
        let firm_center = s.pixel_u8(80, 55);
        // Light pressure changes the ink color toward white while leaving its opacity intact.
        assert!(firm_center[0] < 40, "firm={firm_center:?}");
        assert!(
            light_center[0] < 240,
            "light pressure should retain a readable ink color: {light_center:?}"
        );
        assert!(
            light_center[0] > firm_center[0] + 30,
            "light={light_center:?} firm={firm_center:?}"
        );
    }
}
