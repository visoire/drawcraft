//! Freehand "Draw & Hold" shape recognition from raw stroke samples.

use std::f64::consts::PI;

use drawcraft_color::Color;
use drawcraft_doc::{GeometricShape, ShapeKind, StrokeSample};
use drawcraft_geom::{Point, distance_to_segment, rdp_indices};

/// Evaluates `samples` and, when the stroke clearly resembles an arrow, closed circle,
/// ellipse, rectangle, triangle, polygon, or straight line (when `include_line` is `true`),
/// returns a clean [`GeometricShape`].
pub fn recognize_drawn_shape(
    samples: &[StrokeSample],
    color: Color,
    width: f32,
    include_line: bool,
) -> Option<GeometricShape> {
    let pts = filter_finite_points(samples);
    if pts.len() < 6 {
        return None;
    }

    let (min_pt, max_pt) = points_bbox(&pts)?;
    let bbox_w = max_pt.x - min_pt.x;
    let bbox_h = max_pt.y - min_pt.y;
    let diag = bbox_w.hypot(bbox_h);
    let total_len = polyline_length(&pts);
    if !diag.is_finite() || diag < 14.0 || total_len < 28.0 {
        return None;
    }

    let first = pts[0];
    let last = pts[pts.len() - 1];
    let closure_gap = first.distance(last);
    let close_thresh = (diag * 0.34).max(32.0);
    let is_closed = closure_gap <= close_thresh && bbox_w >= 14.0 && bbox_h >= 14.0;

    let clean_color = color.sanitized();
    let clean_width = if width.is_finite() {
        width.clamp(0.5, 64.0)
    } else {
        2.5
    };

    if !is_closed
        && let Some(arrow) = try_recognize_arrow(&pts, total_len, clean_color, clean_width)
    {
        return Some(arrow);
    }

    if is_closed
        && let Some(closed_shape) = try_recognize_closed_shape(&pts, diag, clean_color, clean_width)
    {
        return Some(closed_shape);
    }

    if include_line
        && !is_closed
        && let Some(line) = try_recognize_line(&pts, total_len, clean_color, clean_width)
    {
        return Some(line);
    }

    None
}

fn filter_finite_points(samples: &[StrokeSample]) -> Vec<Point> {
    let mut out = Vec::with_capacity(samples.len());
    for s in samples {
        if !s.x.is_finite() || !s.y.is_finite() {
            continue;
        }
        let p = Point::new(s.x, s.y);
        if out
            .last()
            .is_none_or(|prev: &Point| prev.distance(p) >= 0.75)
        {
            out.push(p);
        }
    }
    out
}

fn points_bbox(pts: &[Point]) -> Option<(Point, Point)> {
    let first = *pts.first()?;
    let mut min_x = first.x;
    let mut min_y = first.y;
    let mut max_x = first.x;
    let mut max_y = first.y;
    for p in pts.iter().skip(1) {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }
    Some((Point::new(min_x, min_y), Point::new(max_x, max_y)))
}

fn polyline_length(pts: &[Point]) -> f64 {
    pts.windows(2).map(|w| w[0].distance(w[1])).sum()
}

fn try_recognize_line(
    pts: &[Point],
    total_len: f64,
    color: Color,
    width: f32,
) -> Option<GeometricShape> {
    let start = *pts.first()?;
    let end = *pts.last()?;
    let chord = start.distance(end);
    if chord < 20.0 || total_len > chord * 1.12 {
        return None;
    }
    let max_dev = pts
        .iter()
        .map(|&p| distance_to_segment(p, start, end).0)
        .fold(0.0_f64, f64::max);
    if max_dev > (chord * 0.055).max(7.0) {
        return None;
    }
    Some(
        GeometricShape {
            kind: ShapeKind::Line,
            x1: start.x,
            y1: start.y,
            x2: end.x,
            y2: end.y,
            color,
            width,
            ..GeometricShape::default()
        }
        .sanitized(),
    )
}

fn try_recognize_arrow(
    pts: &[Point],
    total_len: f64,
    color: Color,
    width: f32,
) -> Option<GeometricShape> {
    let start = pts[0];
    let mut best_tip_idx = None;
    let mut best_dist = 0.0_f64;
    for (idx, p) in pts.iter().enumerate() {
        let d = start.distance(*p);
        if d > best_dist {
            best_dist = d;
            best_tip_idx = Some(idx);
        }
    }
    let tip_idx = best_tip_idx?;
    if tip_idx < 3 || tip_idx + 2 >= pts.len() {
        return None;
    }
    let tip = pts[tip_idx];
    let shaft_len = start.distance(tip);
    if shaft_len < (total_len * 0.45).max(28.0) {
        return None;
    }

    // Shaft before the tip must be reasonably straight.
    let shaft_dev = pts[..=tip_idx]
        .iter()
        .map(|&p| distance_to_segment(p, start, tip).0)
        .fold(0.0_f64, f64::max);
    if shaft_dev > (shaft_len * 0.085).max(7.5) {
        return None;
    }

    // Inspect barb stroke after tip: must move back along reverse shaft at an oblique angle.
    let tail = &pts[tip_idx..];
    let barb_len = polyline_length(tail);
    if barb_len < (shaft_len * 0.12).max(10.0) || barb_len > shaft_len * 0.88 {
        return None;
    }

    let u_shaft_x = (tip.x - start.x) / shaft_len;
    let u_shaft_y = (tip.y - start.y) / shaft_len;
    let mut max_perp = 0.0_f64;
    let mut max_back = 0.0_f64;
    for &p in tail {
        let vx = p.x - tip.x;
        let vy = p.y - tip.y;
        let forward = vx * u_shaft_x + vy * u_shaft_y;
        let perp = (vx * u_shaft_y - vy * u_shaft_x).abs();
        if forward > (shaft_len * 0.12).max(8.0) {
            return None;
        }
        max_back = max_back.max(-forward);
        max_perp = max_perp.max(perp);
    }

    // Genuine arrow barbs flare outward laterally while stepping backward from the tip.
    if max_perp < (shaft_len * 0.065).max(6.0)
        || max_back < (shaft_len * 0.08).max(7.0)
        || max_back > shaft_len * 0.58
    {
        return None;
    }

    Some(
        GeometricShape {
            kind: ShapeKind::Arrow,
            x1: start.x,
            y1: start.y,
            x2: tip.x,
            y2: tip.y,
            color,
            width,
            ..GeometricShape::default()
        }
        .sanitized(),
    )
}

fn try_recognize_closed_shape(
    raw_pts: &[Point],
    diag: f64,
    color: Color,
    width: f32,
) -> Option<GeometricShape> {
    let mut closed_ring = raw_pts.to_vec();
    if let (Some(&first), Some(&last)) = (closed_ring.first(), closed_ring.last())
        && first.distance(last) > 1.0
    {
        closed_ring.push(first);
    }

    let perim = polyline_length(&closed_ring);
    let area = polygon_signed_area(&closed_ring).abs();
    if perim < 32.0 || area < 220.0 {
        return None;
    }
    let compactness = (4.0 * PI * area) / (perim * perim);
    if compactness < 0.24 {
        return None;
    }

    let uniform = resample_closed_ring(&closed_ring, 64);
    if uniform.len() < 16 {
        return None;
    }

    let corners = extract_cyclic_corners(&uniform, diag);
    let poly_err = corners
        .as_ref()
        .map(|c| mean_polygon_edge_error(&uniform, c) / diag)
        .unwrap_or(f64::INFINITY);

    let ellipse_fit = fit_ellipse_pca(&uniform);

    // 3 corners -> upright Triangle or exact 3-vertex Polygon
    if let Some(ref c) = corners
        && c.len() == 3
        && poly_err <= 0.048
    {
        return Some(build_triangle_or_polygon(c, color, width));
    }

    // 4 corners -> oriented Rectangle when near right angles, otherwise 4-vertex Polygon
    if let Some(ref c) = corners
        && c.len() == 4
        && poly_err <= 0.048
    {
        if let Some(rect) = try_build_oriented_rectangle(c, color, width) {
            return Some(rect);
        }
        return Some(build_polygon_shape(c, color, width));
    }

    // Smooth Ellipse / Circle check (compare errors in matching radial-normalized units!)
    if let Some(fit) = ellipse_fit {
        let mean_r = ((fit.rx + fit.ry) * 0.5).max(1.0);
        let poly_r_err = corners
            .as_ref()
            .map(|c| mean_polygon_edge_error(&uniform, c) / mean_r)
            .unwrap_or(f64::INFINITY);
        let sharp_corners = corners
            .as_ref()
            .map(|c| count_sharp_vertices(c, 48.0))
            .unwrap_or(0);
        let strongly_polygonal = corners.as_ref().is_some_and(|c| {
            (5..=7).contains(&c.len())
                && sharp_corners >= c.len()
                && fit.radial_rms > 0.025
                && poly_r_err < fit.radial_rms * 0.45
        });

        if !strongly_polygonal && fit.radial_rms <= 0.145 && fit.radial_max <= 0.30 {
            let ratio = fit.rx.max(fit.ry) / fit.rx.min(fit.ry).max(1e-6);
            if ratio <= 1.20 {
                let r = (fit.rx + fit.ry) * 0.5;
                return Some(
                    GeometricShape {
                        kind: ShapeKind::Ellipse,
                        x1: fit.cx - r,
                        y1: fit.cy - r,
                        x2: fit.cx + r,
                        y2: fit.cy + r,
                        angle_degrees: 0.0,
                        color,
                        width,
                        ..GeometricShape::default()
                    }
                    .sanitized(),
                );
            }
            let snapped_angle = snap_right_angle(fit.angle_deg);
            return Some(
                GeometricShape {
                    kind: ShapeKind::Ellipse,
                    x1: fit.cx - fit.rx,
                    y1: fit.cy - fit.ry,
                    x2: fit.cx + fit.rx,
                    y2: fit.cy + fit.ry,
                    angle_degrees: snapped_angle,
                    color,
                    width,
                    ..GeometricShape::default()
                }
                .sanitized(),
            );
        }
    }

    // 5..=10 vertex polygon
    if let Some(ref c) = corners
        && (5..=10).contains(&c.len())
        && poly_err <= 0.055
    {
        return Some(build_polygon_shape(c, color, width));
    }

    None
}

fn polygon_signed_area(closed_ring: &[Point]) -> f64 {
    let mut sum = 0.0_f64;
    for win in closed_ring.windows(2) {
        sum += win[0].x * win[1].y - win[1].x * win[0].y;
    }
    sum * 0.5
}

fn resample_closed_ring(closed_ring: &[Point], count: usize) -> Vec<Point> {
    if closed_ring.len() < 3 || count < 8 {
        return Vec::new();
    }
    let mut cum = Vec::with_capacity(closed_ring.len());
    cum.push(0.0_f64);
    for win in closed_ring.windows(2) {
        let prev = *cum.last().unwrap_or(&0.0);
        cum.push(prev + win[0].distance(win[1]));
    }
    let total = *cum.last().unwrap_or(&0.0);
    if total <= 1e-6 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(count);
    let mut seg = 0usize;
    for i in 0..count {
        let target = total * (i as f64 / count as f64);
        while seg + 1 < closed_ring.len() && cum[seg + 1] < target {
            seg += 1;
        }
        let d0 = cum[seg];
        let d1 = cum.get(seg + 1).copied().unwrap_or(d0);
        let span = (d1 - d0).max(1e-9);
        let t = ((target - d0) / span).clamp(0.0, 1.0);
        let a = closed_ring[seg];
        let b = closed_ring.get(seg + 1).copied().unwrap_or(a);
        out.push(Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t));
    }
    out
}

fn extract_cyclic_corners(uniform: &[Point], diag: f64) -> Option<Vec<Point>> {
    if uniform.len() < 8 {
        return None;
    }
    let cx = uniform.iter().map(|p| p.x).sum::<f64>() / uniform.len() as f64;
    let cy = uniform.iter().map(|p| p.y).sum::<f64>() / uniform.len() as f64;
    let center = Point::new(cx, cy);

    // Rotate starting index to the point farthest from centroid so seam lands on a real vertex.
    let mut start_idx = 0usize;
    let mut best_d = -1.0_f64;
    for (idx, p) in uniform.iter().enumerate() {
        let d = p.distance(center);
        if d > best_d {
            best_d = d;
            start_idx = idx;
        }
    }

    let mut rotated = Vec::with_capacity(uniform.len() + 1);
    rotated.extend_from_slice(&uniform[start_idx..]);
    rotated.extend_from_slice(&uniform[..start_idx]);
    rotated.push(rotated[0]);

    let eps = (diag * 0.052).clamp(5.0, 18.0);
    let indices = rdp_indices(&rotated, eps);
    let mut corners: Vec<Point> = indices
        .into_iter()
        .filter_map(|i| rotated.get(i).copied())
        .collect();
    if corners.len() >= 2 && corners[0].distance(corners[corners.len() - 1]) < 1.0 {
        corners.pop();
    }

    // Merge consecutive tiny edges and drop near-collinear seam/noise vertices.
    let min_edge = (diag * 0.08).max(10.0);
    let mut changed = true;
    while changed && corners.len() >= 3 {
        changed = false;
        let mut i = 0usize;
        while i < corners.len() && corners.len() >= 3 {
            let n = corners.len();
            let prev = corners[(i + n - 1) % n];
            let curr = corners[i];
            let next = corners[(i + 1) % n];
            if curr.distance(next) < min_edge {
                corners.remove((i + 1) % n);
                changed = true;
                break;
            }
            if vertex_turn_angle_deg(prev, curr, next) < 22.0 {
                corners.remove(i);
                changed = true;
                break;
            }
            i += 1;
        }
    }

    if corners.len() >= 3 {
        Some(corners)
    } else {
        None
    }
}

fn vertex_turn_angle_deg(prev: Point, curr: Point, next: Point) -> f64 {
    let ax = curr.x - prev.x;
    let ay = curr.y - prev.y;
    let bx = next.x - curr.x;
    let by = next.y - curr.y;
    let la = ax.hypot(ay);
    let lb = bx.hypot(by);
    if la <= 1e-6 || lb <= 1e-6 {
        return 0.0;
    }
    let dot = ((ax * bx + ay * by) / (la * lb)).clamp(-1.0, 1.0);
    dot.acos().to_degrees()
}

fn interior_angle_deg(prev: Point, curr: Point, next: Point) -> f64 {
    let ax = prev.x - curr.x;
    let ay = prev.y - curr.y;
    let bx = next.x - curr.x;
    let by = next.y - curr.y;
    let la = ax.hypot(ay);
    let lb = bx.hypot(by);
    if la <= 1e-6 || lb <= 1e-6 {
        return 0.0;
    }
    let dot = ((ax * bx + ay * by) / (la * lb)).clamp(-1.0, 1.0);
    dot.acos().to_degrees()
}

fn count_sharp_vertices(corners: &[Point], min_turn_deg: f64) -> usize {
    let n = corners.len();
    if n < 3 {
        return 0;
    }
    (0..n)
        .filter(|&i| {
            let prev = corners[(i + n - 1) % n];
            let curr = corners[i];
            let next = corners[(i + 1) % n];
            vertex_turn_angle_deg(prev, curr, next) >= min_turn_deg
        })
        .count()
}

fn mean_polygon_edge_error(samples: &[Point], corners: &[Point]) -> f64 {
    let n = corners.len();
    if n < 3 || samples.is_empty() {
        return f64::INFINITY;
    }
    let mut sum = 0.0_f64;
    for &p in samples {
        let mut best = f64::INFINITY;
        for i in 0..n {
            let a = corners[i];
            let b = corners[(i + 1) % n];
            best = best.min(distance_to_segment(p, a, b).0);
        }
        sum += best;
    }
    sum / samples.len() as f64
}

fn build_polygon_shape(corners: &[Point], color: Color, width: f32) -> GeometricShape {
    GeometricShape {
        kind: ShapeKind::Polygon,
        points: corners.to_vec(),
        color,
        width,
        ..GeometricShape::default()
    }
    .sanitized()
}

fn build_triangle_or_polygon(corners: &[Point], color: Color, width: f32) -> GeometricShape {
    if corners.len() == 3
        && let Some((min_p, max_p)) = points_bbox(corners)
    {
        let w = (max_p.x - min_p.x).max(1.0);
        let h = (max_p.y - min_p.y).max(1.0);
        // If one edge sits near bottom-horizontal and top vertex sits near horizontal center,
        // emit canonical upright Triangle for crisp corner-rounding & handle parity.
        let mut sorted = corners.to_vec();
        sorted.sort_by(|a, b| a.y.total_cmp(&b.y));
        let top = sorted[0];
        let b1 = sorted[1];
        let b2 = sorted[2];
        let bottom_tilt_ok = (b1.y - b2.y).abs() <= h * 0.14;
        let top_centered = (top.x - (min_p.x + max_p.x) * 0.5).abs() <= w * 0.16;
        let base_wide = (b1.x - b2.x).abs() >= w * 0.75;
        if bottom_tilt_ok && top_centered && base_wide {
            return GeometricShape {
                kind: ShapeKind::Triangle,
                x1: min_p.x,
                y1: min_p.y,
                x2: max_p.x,
                y2: max_p.y,
                color,
                width,
                ..GeometricShape::default()
            }
            .sanitized();
        }
    }
    build_polygon_shape(corners, color, width)
}

fn try_build_oriented_rectangle(
    corners: &[Point],
    color: Color,
    width: f32,
) -> Option<GeometricShape> {
    if corners.len() != 4 {
        return None;
    }
    for i in 0..4 {
        let prev = corners[(i + 3) % 4];
        let curr = corners[i];
        let next = corners[(i + 1) % 4];
        let ang = interior_angle_deg(prev, curr, next);
        if !(60.0..=120.0).contains(&ang) {
            return None;
        }
    }

    // Average orientation of opposite edges folded modulo 90 degrees.
    let e0x = corners[1].x - corners[0].x;
    let e0y = corners[1].y - corners[0].y;
    let mut raw_deg = e0y.atan2(e0x).to_degrees();
    while raw_deg > 45.0 {
        raw_deg -= 90.0;
    }
    while raw_deg < -45.0 {
        raw_deg += 90.0;
    }
    let angle_deg = if raw_deg.abs() <= 9.0 { 0.0 } else { raw_deg };

    let cx = corners.iter().map(|p| p.x).sum::<f64>() * 0.25;
    let cy = corners.iter().map(|p| p.y).sum::<f64>() * 0.25;
    let rad = angle_deg.to_radians();
    let (sin, cos) = rad.sin_cos();

    let mut min_u = f64::INFINITY;
    let mut max_u = f64::NEG_INFINITY;
    let mut min_v = f64::INFINITY;
    let mut max_v = f64::NEG_INFINITY;
    for p in corners {
        let dx = p.x - cx;
        let dy = p.y - cy;
        let u = dx * cos + dy * sin;
        let v = -dx * sin + dy * cos;
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
    }
    let half_w = (max_u - min_u) * 0.5;
    let half_h = (max_v - min_v) * 0.5;
    if half_w < 6.0 || half_h < 6.0 {
        return None;
    }
    let shift_u = (min_u + max_u) * 0.5;
    let shift_v = (min_v + max_v) * 0.5;
    let final_cx = cx + shift_u * cos - shift_v * sin;
    let final_cy = cy + shift_u * sin + shift_v * cos;

    Some(
        GeometricShape {
            kind: ShapeKind::Rectangle,
            x1: final_cx - half_w,
            y1: final_cy - half_h,
            x2: final_cx + half_w,
            y2: final_cy + half_h,
            angle_degrees: angle_deg,
            color,
            width,
            ..GeometricShape::default()
        }
        .sanitized(),
    )
}

struct EllipseFit {
    cx: f64,
    cy: f64,
    rx: f64,
    ry: f64,
    angle_deg: f64,
    radial_rms: f64,
    radial_max: f64,
}

fn fit_ellipse_pca(pts: &[Point]) -> Option<EllipseFit> {
    if pts.len() < 12 {
        return None;
    }
    let n = pts.len() as f64;
    let cx = pts.iter().map(|p| p.x).sum::<f64>() / n;
    let cy = pts.iter().map(|p| p.y).sum::<f64>() / n;

    let mut cxx = 0.0_f64;
    let mut cyy = 0.0_f64;
    let mut cxy = 0.0_f64;
    for p in pts {
        let dx = p.x - cx;
        let dy = p.y - cy;
        cxx += dx * dx;
        cyy += dy * dy;
        cxy += dx * dy;
    }
    cxx /= n;
    cyy /= n;
    cxy /= n;

    let trace = cxx + cyy;
    let diff = cxx - cyy;
    let disc = (diff * diff + 4.0 * cxy * cxy).sqrt();
    let lambda1 = (trace + disc) * 0.5;
    let lambda2 = (trace - disc) * 0.5;
    if lambda1 <= 1.0 || lambda2 <= 1.0 {
        return None;
    }
    let angle_rad = if cxy.abs() < 1e-9 && diff.abs() < 1e-9 {
        0.0
    } else {
        0.5 * (2.0 * cxy).atan2(diff)
    };
    let mut rx = (2.0 * lambda1).sqrt();
    let mut ry = (2.0 * lambda2).sqrt();
    if rx < 5.0 || ry < 5.0 {
        return None;
    }

    let (sin, cos) = angle_rad.sin_cos();
    let mut scale_sum = 0.0_f64;
    for p in pts {
        let dx = p.x - cx;
        let dy = p.y - cy;
        let u = (dx * cos + dy * sin) / rx;
        let v = (-dx * sin + dy * cos) / ry;
        scale_sum += u.hypot(v);
    }
    let radial_scale = scale_sum / n;
    if radial_scale.is_finite() && radial_scale > 0.25 {
        rx *= radial_scale;
        ry *= radial_scale;
    }

    let mut err_sq_sum = 0.0_f64;
    let mut err_max = 0.0_f64;
    for p in pts {
        let dx = p.x - cx;
        let dy = p.y - cy;
        let u = (dx * cos + dy * sin) / rx;
        let v = (-dx * sin + dy * cos) / ry;
        let r_norm = u.hypot(v);
        let err = (r_norm - 1.0).abs();
        err_sq_sum += err * err;
        err_max = err_max.max(err);
    }

    Some(EllipseFit {
        cx,
        cy,
        rx,
        ry,
        angle_deg: angle_rad.to_degrees(),
        radial_rms: (err_sq_sum / n).sqrt(),
        radial_max: err_max,
    })
}

fn snap_right_angle(deg: f64) -> f64 {
    let mut a = (deg + 180.0).rem_euclid(360.0) - 180.0;
    for target in [-180.0, -90.0, 0.0, 90.0, 180.0] {
        if (a - target).abs() <= 9.0 {
            a = if (target - 180.0).abs() < 1e-6 || (target + 180.0).abs() < 1e-6 {
                0.0
            } else {
                target
            };
            break;
        }
    }
    a
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sample(x: f64, y: f64) -> StrokeSample {
        StrokeSample {
            x,
            y,
            pressure: 0.5,
            tilt_x: 0.0,
            tilt_y: 0.0,
            rotation: 0.0,
            time_ms: 0.0,
        }
    }

    #[test]
    fn recognizes_circle_and_ellipse_cleanly() {
        let circle: Vec<StrokeSample> = (0..=48)
            .map(|i| {
                let a = std::f64::consts::TAU * (i as f64 / 48.0);
                sample(200.0 + 60.0 * a.cos(), 220.0 + 60.0 * a.sin())
            })
            .collect();
        let recognized = recognize_drawn_shape(&circle, Color::BLACK, 2.5, false)
            .expect("circle must recognize");
        assert_eq!(recognized.kind, ShapeKind::Ellipse);
        assert!(((recognized.x2 - recognized.x1) - 120.0).abs() < 6.0);
        assert!(((recognized.y2 - recognized.y1) - 120.0).abs() < 6.0);

        let ellipse: Vec<StrokeSample> = (0..=48)
            .map(|i| {
                let a = std::f64::consts::TAU * (i as f64 / 48.0);
                sample(300.0 + 110.0 * a.cos(), 300.0 + 45.0 * a.sin())
            })
            .collect();
        let recognized_e = recognize_drawn_shape(&ellipse, Color::BLACK, 2.5, false)
            .expect("ellipse must recognize");
        assert_eq!(recognized_e.kind, ShapeKind::Ellipse);
        assert!((recognized_e.x2 - recognized_e.x1) > (recognized_e.y2 - recognized_e.y1) * 1.8);
    }

    #[test]
    fn recognizes_rectangle_triangle_arrow_and_pentagon() {
        let mut rect = Vec::new();
        for i in 0..=10 {
            rect.push(sample(100.0 + i as f64 * 14.0, 100.0));
        }
        for i in 1..=10 {
            rect.push(sample(240.0, 100.0 + i as f64 * 9.0));
        }
        for i in 1..=10 {
            rect.push(sample(240.0 - i as f64 * 14.0, 190.0));
        }
        for i in 1..=10 {
            rect.push(sample(100.0, 190.0 - i as f64 * 9.0));
        }
        let res_rect = recognize_drawn_shape(&rect, Color::BLACK, 3.0, false).unwrap();
        assert_eq!(res_rect.kind, ShapeKind::Rectangle);

        let mut tri = Vec::new();
        let v = [
            (200.0, 100.0),
            (280.0, 220.0),
            (120.0, 220.0),
            (200.0, 100.0),
        ];
        for w in v.windows(2) {
            for s in 0..10 {
                let t = s as f64 / 10.0;
                tri.push(sample(
                    w[0].0 + (w[1].0 - w[0].0) * t,
                    w[0].1 + (w[1].1 - w[0].1) * t,
                ));
            }
        }
        tri.push(sample(200.0, 100.0));
        let res_tri = recognize_drawn_shape(&tri, Color::BLACK, 3.0, false).unwrap();
        assert_eq!(res_tri.kind, ShapeKind::Triangle);

        let mut arrow = Vec::new();
        for i in 0..=16 {
            arrow.push(sample(100.0 + i as f64 * 10.0, 300.0));
        }
        for i in 1..=5 {
            arrow.push(sample(260.0 - i as f64 * 6.0, 300.0 - i as f64 * 6.0));
        }
        let res_arrow = recognize_drawn_shape(&arrow, Color::BLACK, 3.0, false).unwrap();
        assert_eq!(res_arrow.kind, ShapeKind::Arrow);

        let mut pent = Vec::new();
        let verts: Vec<(f64, f64)> = (0..=5)
            .map(|i| {
                let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * (i as f64 / 5.0);
                (300.0 + 70.0 * a.cos(), 300.0 + 70.0 * a.sin())
            })
            .collect();
        for w in verts.windows(2) {
            for s in 0..8 {
                let t = s as f64 / 8.0;
                pent.push(sample(
                    w[0].0 + (w[1].0 - w[0].0) * t,
                    w[0].1 + (w[1].1 - w[0].1) * t,
                ));
            }
        }
        pent.push(sample(verts[0].0, verts[0].1));
        let res_poly = recognize_drawn_shape(&pent, Color::BLACK, 3.0, false).unwrap();
        assert_eq!(res_poly.kind, ShapeKind::Polygon);
        assert_eq!(res_poly.points.len(), 5);
    }

    #[test]
    fn rejects_ordinary_writing_and_hostile_nan() {
        let mut scribble = Vec::new();
        for i in 0..40 {
            let x = 100.0 + i as f64 * 6.0;
            let y = 200.0 + ((i as f64) * 0.9).sin() * 20.0;
            scribble.push(sample(x, y));
        }
        assert!(recognize_drawn_shape(&scribble, Color::BLACK, 2.0, false).is_none());

        let hostile = vec![sample(f64::NAN, f64::INFINITY); 12];
        assert!(recognize_drawn_shape(&hostile, Color::BLACK, 2.0, true).is_none());
    }
}
