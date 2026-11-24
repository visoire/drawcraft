//! Stroke Eraser and Precision Eraser (segment splitting) engines.

use std::sync::Arc;

use drawcraft_doc::{EraserMode, InkStroke, StrokeId, StrokeSample};
use drawcraft_geom::{Point, Rect, distance_to_segment};

fn path_bounds(path: &[(Point, f64)]) -> Rect {
    let mut r = Rect::from_point_iter(path.iter().map(|(p, _)| *p));
    let max_rad = path
        .iter()
        .map(|(_, rad)| if rad.is_finite() { rad.max(0.0) } else { 0.0 })
        .fold(0.0_f64, f64::max);
    r = r.inflated(max_rad + 2.0);
    r
}

/// Checks if any segment of `stroke` intersects the circle `(center, radius)`.
pub fn stroke_intersects_circle(stroke: &InkStroke, center: Point, radius: f64) -> bool {
    if !(center.is_finite() && radius.is_finite() && radius > 0.0) {
        return false;
    }
    let test_rect = Rect::from_points(center, center).inflated(radius);
    if !stroke.bounds.intersects(test_rect) {
        return false;
    }
    let pts = &stroke.smoothed_points;
    if pts.is_empty() {
        return false;
    }
    let stroke_r = f64::from(stroke.pen.size * 0.5);
    let hit_r = radius + stroke_r;
    if pts.len() == 1 {
        return pts[0].point().distance(center) <= hit_r;
    }
    pts.windows(2).any(|w| {
        let (d, _) = distance_to_segment(center, w[0].point(), w[1].point());
        d <= hit_r
    })
}

fn sample_hit_by_path(pt: Point, path: &[(Point, f64)], stroke_r: f64) -> bool {
    for &(c, r) in path {
        if pt.distance(c) <= r + stroke_r {
            return true;
        }
    }
    for w in path.windows(2) {
        let (a, ra) = w[0];
        let (b, rb) = w[1];
        let (d, t) = distance_to_segment(pt, a, b);
        let r_interp = ra + (rb - ra) * t;
        if d <= r_interp + stroke_r {
            return true;
        }
    }
    false
}

fn segment_hit_by_path(a: Point, b: Point, path: &[(Point, f64)], stroke_r: f64) -> bool {
    if sample_hit_by_path(a, path, stroke_r) || sample_hit_by_path(b, path, stroke_r) {
        return true;
    }
    for &(c, r) in path {
        let (d, _) = distance_to_segment(c, a, b);
        if d <= r + stroke_r {
            return true;
        }
    }
    false
}

fn split_stroke_precision(
    stroke: &InkStroke,
    path: &[(Point, f64)],
    next_id: &mut u64,
) -> (Vec<Arc<InkStroke>>, bool) {
    let pts = &stroke.smoothed_points;
    if pts.is_empty() {
        return (Vec::new(), true);
    }
    let stroke_r = f64::from(stroke.pen.size * 0.35);

    // Subdivide any long segments slightly so precision erasing feels crisp even on sparse strokes.
    let mut dense: Vec<StrokeSample> = Vec::with_capacity(pts.len() * 2);
    dense.push(pts[0]);
    for w in pts.windows(2) {
        let d = w[0].point().distance(w[1].point());
        if d > 4.0 {
            let n = (d / 3.0).ceil().clamp(1.0, 32.0) as usize;
            for k in 1..=n {
                let t = k as f64 / n as f64;
                let tf = t as f32;
                dense.push(StrokeSample {
                    x: w[0].x + (w[1].x - w[0].x) * t,
                    y: w[0].y + (w[1].y - w[0].y) * t,
                    pressure: w[0].pressure + (w[1].pressure - w[0].pressure) * tf,
                    tilt_x: w[0].tilt_x + (w[1].tilt_x - w[0].tilt_x) * tf,
                    tilt_y: w[0].tilt_y + (w[1].tilt_y - w[0].tilt_y) * tf,
                    rotation: w[0].rotation,
                    time_ms: w[0].time_ms + (w[1].time_ms - w[0].time_ms) * t,
                });
            }
        } else {
            dense.push(w[1]);
        }
    }

    let kept_flags: Vec<bool> = dense
        .iter()
        .map(|s| !sample_hit_by_path(s.point(), path, stroke_r))
        .collect();

    if kept_flags.iter().all(|&k| k) {
        // Also test segment midpoints against eraser circles.
        let any_seg_hit = dense
            .windows(2)
            .any(|w| segment_hit_by_path(w[0].point(), w[1].point(), path, stroke_r));
        if !any_seg_hit {
            return (vec![Arc::new(stroke.clone())], false);
        }
    }

    let mut runs: Vec<Vec<StrokeSample>> = Vec::new();
    let mut current: Vec<StrokeSample> = Vec::new();
    for (i, &keep) in kept_flags.iter().enumerate() {
        if keep {
            if let Some(prev) = current.last()
                && segment_hit_by_path(prev.point(), dense[i].point(), path, stroke_r)
            {
                runs.push(std::mem::take(&mut current));
            }
            current.push(dense[i]);
        } else if !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }

    if runs.len() == 1 && runs[0].len() == dense.len() {
        return (vec![Arc::new(stroke.clone())], false);
    }

    let mut out = Vec::with_capacity(runs.len());
    for (idx, run) in runs.into_iter().enumerate() {
        let id = if idx == 0 {
            stroke.id
        } else {
            let allocated = StrokeId(*next_id);
            *next_id = next_id.saturating_add(1);
            allocated
        };
        let mut piece_pen = stroke.pen.clone();
        // Split pieces keep natural smooth ends without re-applying long original start/end tapers excessively.
        piece_pen.start_taper_px = piece_pen.start_taper_px.min(4.0);
        piece_pen.end_taper_px = piece_pen.end_taper_px.min(4.0);
        out.push(Arc::new(InkStroke::new(id, piece_pen, run.clone(), run)));
    }
    (out, true)
}

/// Applies Stroke Eraser or Precision Eraser along `path` (`[(center, radius)]`).
/// Returns `(updated_strokes, modified)`.
pub fn erase_along_path(
    strokes: &[Arc<InkStroke>],
    path: &[(Point, f64)],
    mode: EraserMode,
    next_id: &mut u64,
) -> (Vec<Arc<InkStroke>>, bool) {
    erase_along_path_filtered(strokes, path, mode, false, next_id)
}

/// Applies the active eraser mode along `path`, optionally restricting erasure to highlighter strokes only.
pub fn erase_along_path_filtered(
    strokes: &[Arc<InkStroke>],
    path: &[(Point, f64)],
    mode: EraserMode,
    highlighter_only: bool,
    next_id: &mut u64,
) -> (Vec<Arc<InkStroke>>, bool) {
    if strokes.is_empty() || path.is_empty() {
        return (strokes.to_vec(), false);
    }
    let clean_path: Vec<(Point, f64)> = path
        .iter()
        .filter(|(p, r)| p.is_finite() && r.is_finite() && *r > 0.0)
        .copied()
        .collect();
    if clean_path.is_empty() {
        return (strokes.to_vec(), false);
    }

    let p_bounds = path_bounds(&clean_path);
    let mut out = Vec::with_capacity(strokes.len());
    let mut changed = false;

    for stroke in strokes {
        if highlighter_only && stroke.pen.kind != drawcraft_doc::PenKind::Highlighter {
            out.push(Arc::clone(stroke));
            continue;
        }
        if !stroke.bounds.intersects(p_bounds) {
            out.push(Arc::clone(stroke));
            continue;
        }
        match mode {
            EraserMode::StrokeEraser => {
                let hit = clean_path
                    .iter()
                    .any(|&(c, r)| stroke_intersects_circle(stroke, c, r));
                if hit {
                    changed = true;
                } else {
                    out.push(Arc::clone(stroke));
                }
            }
            EraserMode::PrecisionEraser => {
                let (pieces, piece_changed) = split_stroke_precision(stroke, &clean_path, next_id);
                if piece_changed {
                    changed = true;
                    out.extend(pieces);
                } else {
                    out.push(Arc::clone(stroke));
                }
            }
        }
    }

    (out, changed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use drawcraft_doc::PenSettings;

    fn sample_line() -> Arc<InkStroke> {
        let pts: Vec<StrokeSample> = (0..=20)
            .map(|i| StrokeSample::from_xy_pressure(f64::from(i) * 10.0, 50.0, 0.8))
            .collect();
        Arc::new(InkStroke::new(
            StrokeId(1),
            PenSettings::ball_pen(),
            pts.clone(),
            pts,
        ))
    }

    #[test]
    fn stroke_eraser_deletes_entire_touched_stroke() {
        let strokes = vec![sample_line()];
        let mut next_id = 2;
        let (res, changed) = erase_along_path(
            &strokes,
            &[(Point::new(100.0, 52.0), 8.0)],
            EraserMode::StrokeEraser,
            &mut next_id,
        );
        assert!(changed);
        assert!(res.is_empty());
    }

    #[test]
    fn precision_eraser_splits_stroke_into_two_clean_halves() {
        let strokes = vec![sample_line()];
        let mut next_id = 2;
        let (res, changed) = erase_along_path(
            &strokes,
            &[(Point::new(100.0, 50.0), 15.0)],
            EraserMode::PrecisionEraser,
            &mut next_id,
        );
        assert!(changed);
        assert_eq!(res.len(), 2);
        assert!(res[0].smoothed_points.last().unwrap().x < 86.0);
        assert!(res[1].smoothed_points.first().unwrap().x > 114.0);
    }
}
