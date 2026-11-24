//! Centripetal Catmull-Rom curve subdivision and damped kinematic tip prediction.

use drawcraft_doc::StrokeSample;
use drawcraft_geom::{Point, Vec2, catmull_rom_centripetal_with_tension};

#[inline]
fn lerp_angle_deg(a: f32, b: f32, t: f32) -> f32 {
    let dr = (b - a + 540.0).rem_euclid(360.0) - 180.0;
    (a + dr * t).rem_euclid(360.0)
}

#[inline]
fn phantom_before(p0: Point, p1: Point) -> Point {
    Point::new(2.0 * p0.x - p1.x, 2.0 * p0.y - p1.y)
}

#[inline]
fn interpolate_sample(a: StrokeSample, b: StrokeSample, pos: Point, u: f64) -> StrokeSample {
    let uf = u as f32;
    StrokeSample {
        x: pos.x,
        y: pos.y,
        pressure: a.pressure + (b.pressure - a.pressure) * uf,
        tilt_x: a.tilt_x + (b.tilt_x - a.tilt_x) * uf,
        tilt_y: a.tilt_y + (b.tilt_y - a.tilt_y) * uf,
        rotation: lerp_angle_deg(a.rotation, b.rotation, uf),
        time_ms: a.time_ms + (b.time_ms - a.time_ms) * u,
    }
}

/// Subdivides sparse linear segments so start/end tapers and pen dynamics resolve accurately on
/// straight segments or 2-point polylines without curving corners.
pub fn densify_linear(points: &[StrokeSample], max_step_px: f64) -> Vec<StrokeSample> {
    if points.len() < 2 {
        return points.to_vec();
    }
    const MAX_DENSIFIED_SAMPLES: usize = 100_000;
    let step = if max_step_px.is_finite() {
        max_step_px.clamp(1.0, 64.0)
    } else {
        4.0
    };
    let mut out = Vec::with_capacity(points.len());
    out.push(points[0].sanitized());
    for pair in points.windows(2) {
        let a = pair[0].sanitized();
        let b = pair[1].sanitized();
        let dist = a.point().distance(b.point());
        let remaining = MAX_DENSIFIED_SAMPLES.saturating_sub(out.len());
        if remaining == 0 {
            break;
        }
        let subdivs = (dist / step).ceil().clamp(1.0, 128.0) as usize;
        let subdivs = subdivs.min(remaining).max(1);
        for k in 1..=subdivs {
            if k == subdivs {
                out.push(b);
            } else {
                let u = k as f64 / subdivs as f64;
                let pos = a.point().lerp(b.point(), u);
                out.push(interpolate_sample(a, b, pos, u));
            }
        }
    }
    out
}

/// Resamples a polyline through a Centripetal Catmull-Rom ($\alpha = 0.5$) spline with
/// adaptive chord-length subdivision, preventing cusps in tight handwriting loops.
pub fn resample_centripetal_catmull_rom(
    points: &[StrokeSample],
    tension: f32,
    min_step_px: f64,
) -> Vec<StrokeSample> {
    resample_catmull_rom_inner(points, tension, min_step_px, false, 0.0)
}

/// Smooths sparse curve chords via corner-safe Centripetal Catmull-Rom while keeping already-dense
/// stabilized segments exact and preserving sharp deliberate corners without overshoot ears.
pub fn resample_adaptive_catmull_rom(
    points: &[StrokeSample],
    max_step_px: f64,
) -> Vec<StrokeSample> {
    resample_catmull_rom_inner(points, 0.5, max_step_px, true, 3.5)
}

fn resample_catmull_rom_inner(
    points: &[StrokeSample],
    tension: f32,
    min_step_px: f64,
    preserve_corners: bool,
    linear_dense_below_px: f64,
) -> Vec<StrokeSample> {
    if points.len() < 2 {
        return points.to_vec();
    }

    // Filter near-coincident consecutive samples while preserving start and end.
    let mut dedup: Vec<StrokeSample> = Vec::with_capacity(points.len());
    for (i, &s) in points.iter().enumerate() {
        let s = s.sanitized();
        let is_last = i + 1 == points.len();
        if let Some(prev) = dedup.last()
            && !is_last
            && prev.point().distance(s.point()) < 0.2
        {
            continue;
        }
        dedup.push(s);
    }
    if dedup.len() < 2 {
        return points.to_vec();
    }
    // If last point coincides with previous, keep exact position without zero-span loop issues.
    if dedup.len() == 2 && dedup[0].point().distance(dedup[1].point()) < 1e-4 {
        return dedup;
    }

    let step = if min_step_px.is_finite() {
        min_step_px.clamp(0.5, 25.0)
    } else {
        2.5
    };
    let tens = f64::from(if tension.is_finite() {
        tension.clamp(0.0, 1.0)
    } else {
        0.5
    });

    let n = dedup.len();
    let mut out = Vec::with_capacity(n * 3);
    out.push(dedup[0]);

    for i in 0..(n - 1) {
        let a = dedup[i];
        let b = dedup[i + 1];
        let p1 = a.point();
        let p2 = b.point();
        let dist = p1.distance(p2);

        if dist <= linear_dense_below_px {
            out.push(b);
            continue;
        }

        let mut p0 = if i == 0 {
            phantom_before(p1, p2)
        } else {
            dedup[i - 1].point()
        };
        let mut p3 = if i + 2 >= n {
            phantom_before(p2, p1)
        } else {
            dedup[i + 2].point()
        };

        if preserve_corners && let Some(t_cur) = Vec2::from_points(p1, p2).normalized() {
            if i > 0
                && let Some(t_prev) = Vec2::from_points(p0, p1).normalized()
                && t_prev.dot(t_cur) < 0.50
            {
                p0 = phantom_before(p1, p2);
            }
            if i + 2 < n
                && let Some(t_next) = Vec2::from_points(p2, p3).normalized()
                && t_cur.dot(t_next) < 0.50
            {
                p3 = phantom_before(p2, p1);
            }
        }

        let subdivs = (dist / step).ceil().clamp(1.0, 24.0) as usize;
        for k in 1..=subdivs {
            let u = k as f64 / subdivs as f64;
            let pos = if k == subdivs {
                p2
            } else {
                catmull_rom_centripetal_with_tension(p0, p1, p2, p3, u, tens)
            };
            out.push(interpolate_sample(a, b, pos, u));
        }
    }
    out
}

/// Projects a damped kinematic lookahead tip sample from the trailing stroke velocity so live
/// ink tracks fast stylus motion with minimal visual latency.
pub fn predict_live_tip(smoothed: &[StrokeSample], prediction_ms: f32) -> Option<StrokeSample> {
    if !prediction_ms.is_finite() || prediction_ms <= 0.1 || smoothed.len() < 2 {
        return None;
    }
    let pred_ms = f64::from(prediction_ms.clamp(0.0, 32.0));
    let n = smoothed.len();
    let last = smoothed[n - 1];
    let prev = smoothed[n - 2];

    let dt = last.time_ms - prev.time_ms;
    if !(dt.is_finite() && dt > 0.5 && dt < 120.0) {
        return None;
    }
    let vel = Vec2::from_points(prev.point(), last.point()).scale(1.0 / dt);
    let speed = vel.len();
    if !(speed.is_finite() && speed > 0.05) {
        return None;
    }

    // Damped projection clamped to 18 px max to prevent overshoot on sudden direction changes.
    let mut offset = vel.scale(pred_ms * 0.65);
    let len = offset.len();
    if len > 18.0 {
        offset = offset.scale(18.0 / len);
    }
    Some(
        StrokeSample {
            x: last.x + offset.x,
            y: last.y + offset.y,
            time_ms: last.time_ms + pred_ms,
            ..last
        }
        .sanitized(),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn catmull_rom_subdivides_and_preserves_endpoints() {
        let pts = vec![
            StrokeSample::from_xy_pressure(0.0, 0.0, 0.2),
            StrokeSample::from_xy_pressure(20.0, 30.0, 0.6),
            StrokeSample::from_xy_pressure(40.0, 0.0, 0.9),
        ];
        let sub = resample_centripetal_catmull_rom(&pts, 0.5, 4.0);
        assert!(sub.len() > pts.len());
        assert_eq!(sub.first().unwrap().point(), pts.first().unwrap().point());
        assert_eq!(sub.last().unwrap().point(), pts.last().unwrap().point());
        assert!(sub.iter().all(|s| s.point().is_finite()));
    }

    #[test]
    fn kinematic_prediction_caps_overshoot() {
        let pts = vec![
            StrokeSample {
                x: 0.0,
                y: 0.0,
                time_ms: 0.0,
                ..Default::default()
            },
            StrokeSample {
                x: 100.0,
                y: 0.0,
                time_ms: 4.0,
                ..Default::default()
            },
        ];
        let pred = predict_live_tip(&pts, 20.0).unwrap();
        assert!((pred.x - 118.0).abs() < 1e-6);
        assert_eq!(pred.y, 0.0);
    }
}
