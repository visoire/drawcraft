//! Pen-physics spine evaluation: Fountain nib calligraphy, Ball pen, Brush tilt dynamics,
//! Highlighter chisel nib, velocity-based pressure simulation, and smooth start/end tapers.

use drawcraft_color::Color;
use drawcraft_doc::{PenKind, PenSettings, StrokeSample};
use drawcraft_geom::{Point, Vec2};

/// Evaluated geometry and ink parameters at one point along a stroke spine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpineNode {
    pub point: Point,
    pub radius: f32,
    pub alpha: f32,
    pub color: Color,
    pub angle_rad: f32,
    pub roundness: f32,
    pub direction_rad: f32,
    pub arc_length: f64,
}

#[inline]
fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    if (edge1 - edge0).abs() <= 1e-9 {
        return 1.0;
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn lerpf(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn compute_arc_lengths(samples: &[StrokeSample]) -> (Vec<f64>, f64) {
    let mut arcs = Vec::with_capacity(samples.len());
    let mut total = 0.0_f64;
    if let Some(first) = samples.first() {
        arcs.push(0.0);
        let mut prev = first.point();
        for s in samples.iter().skip(1) {
            let cur = s.point();
            total += prev.distance(cur);
            arcs.push(total);
            prev = cur;
        }
    }
    (arcs, total)
}

fn compute_velocities(samples: &[StrokeSample], arcs: &[f64]) -> Vec<f64> {
    let n = samples.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![0.6];
    }

    let mut seg_v: Vec<Option<f64>> = vec![None; n - 1];
    for i in 0..(n - 1) {
        let dt = samples[i + 1].time_ms - samples[i].time_ms;
        let dist = samples[i].point().distance(samples[i + 1].point());
        if dt.is_finite() && dt > 0.2 && dt < 200.0 {
            seg_v[i] = Some((dist / dt).clamp(0.02, 8.0));
        }
    }

    // Fill zero/missing dt gaps from nearest timed neighbor before falling back to spacing cadence.
    let mut last_valid = None;
    for slot in &mut seg_v {
        if let Some(v) = *slot {
            last_valid = Some(v);
        } else if let Some(v) = last_valid {
            *slot = Some(v);
        }
    }
    let mut next_valid = None;
    for slot in seg_v.iter_mut().rev() {
        if let Some(v) = *slot {
            next_valid = Some(v);
        } else if let Some(v) = next_valid {
            *slot = Some(v);
        }
    }

    let mut raw_v = vec![0.6_f64; n];
    for (i, dst) in raw_v.iter_mut().enumerate() {
        let seg_idx = if i == 0 { 0 } else { i - 1 };
        *dst = seg_v[seg_idx].unwrap_or_else(|| {
            let a = samples[seg_idx];
            let b = samples[(seg_idx + 1).min(n - 1)];
            let d = a.point().distance(b.point());
            (d / 12.0).clamp(0.05, 3.0)
        });
    }

    // Arc-length Gaussian smoothing (~10 px sigma, bounded +-30 px window in O(n)) so dense
    // stabilized micro-steps never introduce per-sample frame jitter or pinch tails.
    const SIGMA_PX: f64 = 10.0;
    const TWO_SIGMA_SQ: f64 = 2.0 * SIGMA_PX * SIGMA_PX;
    const RADIUS_PX: f64 = 30.0;
    let mut smooth_v = Vec::with_capacity(n);
    let mut lo = 0_usize;
    let mut hi = 0_usize;
    for i in 0..n {
        let arc_i = arcs[i];
        while lo < i && arc_i - arcs[lo] > RADIUS_PX {
            lo += 1;
        }
        while hi + 1 < n && arcs[hi + 1] - arc_i <= RADIUS_PX {
            hi += 1;
        }
        let mut num = 0.0_f64;
        let mut den = 0.0_f64;
        for j in lo..=hi {
            let d = arcs[j] - arc_i;
            let w = (-(d * d) / TWO_SIGMA_SQ).exp();
            num += w * raw_v[j];
            den += w;
        }
        smooth_v.push(if den > 1e-9 { num / den } else { raw_v[i] });
    }
    smooth_v
}

fn compute_pressures(samples: &[StrokeSample], arcs: &[f64]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    // Tablet pressure can briefly spike or dip for a single packet. Smooth it over a short
    // distance so those packets do not become pale marks or sharp width bumps in the ink.
    const SIGMA_PX: f64 = 2.5;
    const TWO_SIGMA_SQ: f64 = 2.0 * SIGMA_PX * SIGMA_PX;
    const RADIUS_PX: f64 = 7.5;
    let mut smooth = Vec::with_capacity(samples.len());
    let mut lo = 0_usize;
    let mut hi = 0_usize;
    for i in 0..samples.len() {
        let arc_i = arcs.get(i).copied().unwrap_or(0.0);
        while lo < i && arc_i - arcs.get(lo).copied().unwrap_or(arc_i) > RADIUS_PX {
            lo += 1;
        }
        while hi + 1 < samples.len()
            && arcs.get(hi + 1).copied().unwrap_or(arc_i) - arc_i <= RADIUS_PX
        {
            hi += 1;
        }

        let mut weighted = 0.0_f64;
        let mut total_weight = 0.0_f64;
        for (j, sample) in samples.iter().enumerate().take(hi + 1).skip(lo) {
            let distance = arcs.get(j).copied().unwrap_or(arc_i) - arc_i;
            let weight = (-(distance * distance) / TWO_SIGMA_SQ).exp();
            weighted += weight * f64::from(sample.sanitized().pressure);
            total_weight += weight;
        }
        let fallback = samples[i].sanitized().pressure;
        smooth.push(if total_weight > 1e-9 {
            (weighted / total_weight).clamp(0.0, 1.0) as f32
        } else {
            fallback
        });
    }
    smooth
}

fn segment_direction(samples: &[StrokeSample], idx: usize) -> Vec2 {
    let n = samples.len();
    if n < 2 {
        return Vec2::new(1.0, 0.0);
    }
    let a = if idx == 0 {
        samples[0].point()
    } else {
        samples[idx - 1].point()
    };
    let b = if idx + 1 >= n {
        samples[n - 1].point()
    } else {
        samples[idx + 1].point()
    };
    Vec2::from_points(a, b)
        .normalized()
        .or_else(|| {
            if idx + 1 < n {
                Vec2::from_points(samples[idx].point(), samples[idx + 1].point()).normalized()
            } else if idx > 0 {
                Vec2::from_points(samples[idx - 1].point(), samples[idx].point()).normalized()
            } else {
                None
            }
        })
        .unwrap_or(Vec2::new(1.0, 0.0))
}

fn taper_multiplier(arc: f64, total: f64, start_px: f32, end_px: f32) -> f32 {
    if total < 6.0 {
        return 1.0;
    }
    let s_taper = f64::from(start_px.max(0.0)).min(total * 0.35);
    let e_taper = f64::from(end_px.max(0.0)).min(total * 0.40);
    let start_f = if s_taper > 0.5 && arc < s_taper {
        0.18 + 0.82 * smoothstep(0.0, s_taper, arc)
    } else {
        1.0
    };
    let rem = (total - arc).max(0.0);
    let end_f = if e_taper > 0.5 && rem < e_taper {
        0.15 + 0.85 * smoothstep(0.0, e_taper, rem)
    } else {
        1.0
    };
    (start_f.min(end_f)) as f32
}

/// Evaluates pen dynamics across `samples` into [`SpineNode`]s ready for ribbon meshing or dab rendering.
pub fn evaluate_spine(pen: &PenSettings, samples: &[StrokeSample]) -> Vec<SpineNode> {
    if samples.is_empty() {
        return Vec::new();
    }
    let pen = pen.sanitized();
    let (arcs, total_len) = compute_arc_lengths(samples);
    let velocities = compute_velocities(samples, &arcs);
    let pressures = compute_pressures(samples, &arcs);

    // Mouse / trackpad streams report constant pressure 1.0 across all samples.
    let constant_mouse_pressure = samples.iter().all(|s| (s.pressure - 1.0).abs() < 1e-3);
    let simulate_velocity =
        pen.velocity_pressure_simulation && pen.pressure_enabled && constant_mouse_pressure;

    let base_radius = (pen.size * 0.5).max(0.25);
    let nib_rad = pen.nib_angle_deg.to_radians();

    let mut nodes = Vec::with_capacity(samples.len());
    for (i, &s) in samples.iter().enumerate() {
        let s = s.sanitized();
        let dir_vec = segment_direction(samples, i);
        let dir_rad = dir_vec.angle_rad() as f32;

        let raw_p = if simulate_velocity {
            let speed_t = (velocities[i] / 3.0).clamp(0.0, 1.0);
            let ease = speed_t * speed_t * (3.0 - 2.0 * speed_t);
            (0.94 - 0.34 * ease).clamp(0.60, 0.94) as f32
        } else {
            pressures.get(i).copied().unwrap_or(s.pressure)
        };
        let p_curved = if pen.pressure_enabled {
            pen.pressure_curve.eval(raw_p)
        } else {
            1.0
        };
        let opacity_p = if !pen.pressure_enabled {
            1.0
        } else if simulate_velocity {
            // Keep simulated mouse velocity from washing out ink.
            lerpf(0.75, 1.0, p_curved)
        } else {
            1.0
        };
        let color = if pen.pressure_enabled && !simulate_velocity {
            pressure_ink_color(&pen, p_curved)
        } else {
            pen.color.sanitized()
        };

        // Real stylus pressure streams dictate their own touchdown/lift-off shape directly;
        // synthetic distance tapers only apply when constant/non-pressure input is used.
        let taper = if pen.pressure_enabled && !constant_mouse_pressure {
            1.0
        } else {
            taper_multiplier(arcs[i], total_len, pen.start_taper_px, pen.end_taper_px)
        };
        let (radius, alpha, angle_rad, roundness) = match pen.kind {
            PenKind::FountainPen => {
                let p_scale = lerpf(pen.pressure_size_min, 1.0, p_curved);
                let r = (base_radius * p_scale * taper).max(0.2);
                let a = modulated_alpha(&pen, opacity_p, 1.0);
                let round = (1.0 - pen.nib_flatness * 0.65).clamp(0.2, 1.0);
                (r, a, nib_rad, round)
            }
            PenKind::BallPen => {
                let p_scale = lerpf(pen.pressure_size_min, 1.0, p_curved);
                let r = (base_radius * p_scale * taper).max(0.2);
                let a = modulated_alpha(&pen, opacity_p, 1.0);
                (r, a, 0.0, 1.0)
            }
            PenKind::BrushPen => {
                let tilt_mag = (f32::hypot(s.tilt_x, s.tilt_y) / 60.0).clamp(0.0, 1.0);
                let tilt_boost = 1.0 + 0.55 * tilt_mag * pen.tilt_sensitivity;
                let p_scale = lerpf(pen.pressure_size_min.min(0.15), 1.0, p_curved);
                let r = (base_radius * p_scale * tilt_boost * taper).max(0.2);
                let tilt_shade = 1.0 - 0.22 * tilt_mag * pen.tilt_sensitivity;
                let a = modulated_alpha(&pen, opacity_p, tilt_shade);
                let angle = if tilt_mag > 0.12 && pen.tilt_sensitivity > 0.05 {
                    s.tilt_y.atan2(s.tilt_x)
                } else {
                    nib_rad
                };
                let round = (1.0 - 0.55 * tilt_mag * pen.tilt_sensitivity - pen.nib_flatness * 0.3)
                    .clamp(0.25, 1.0);
                (r, a, angle, round)
            }
            PenKind::Highlighter => {
                let r = base_radius.max(0.5);
                let round = (1.0 - pen.nib_flatness.clamp(0.0, 0.85)).max(0.15);
                // Keep the marker's broad edge perpendicular to the stroke so its
                // flat start and end cuts do not inherit a diagonal chisel angle.
                (r, pen.opacity, dir_rad + std::f32::consts::FRAC_PI_2, round)
            }
            PenKind::Pencil => {
                let tilt_mag = (f32::hypot(s.tilt_x, s.tilt_y) / 60.0).clamp(0.0, 1.0);
                let tilt_boost = 1.0 + 1.15 * tilt_mag * pen.tilt_sensitivity;
                let p_scale = lerpf(pen.pressure_size_min, 1.0, p_curved);
                let r = (base_radius * p_scale * tilt_boost * taper).max(0.25);
                let tilt_shade = 1.0 - 0.34 * tilt_mag * pen.tilt_sensitivity;
                let min_op = if pen.pressure_opacity_min.is_finite() {
                    pen.pressure_opacity_min.clamp(0.15, 1.0)
                } else {
                    0.35
                };
                let press_alpha = lerpf(min_op, 1.0, opacity_p);
                let a = (pen.opacity * press_alpha * tilt_shade).clamp(0.05, 1.0);
                let angle = if tilt_mag > 0.12 && pen.tilt_sensitivity > 0.05 {
                    s.tilt_y.atan2(s.tilt_x)
                } else {
                    nib_rad
                };
                let round =
                    (1.0 - 0.55 * tilt_mag * pen.tilt_sensitivity - pen.nib_flatness * 0.25)
                        .clamp(0.25, 1.0);
                (r, a, angle, round)
            }
        };

        nodes.push(SpineNode {
            point: s.point(),
            radius,
            alpha,
            color,
            angle_rad,
            roundness,
            direction_rad: dir_rad,
            arc_length: arcs[i],
        });
    }
    nodes
}

/// Lightens the ink at low physical pressure while leaving stroke opacity unchanged.
fn pressure_ink_color(pen: &PenSettings, p_curved: f32) -> Color {
    let p = if p_curved.is_finite() {
        p_curved.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let min_strength = if pen.pressure_opacity_min.is_finite() {
        pen.pressure_opacity_min.clamp(0.68, 1.0)
    } else {
        0.68
    };
    // The square root gives light touches a visible response without keeping ordinary pressure pale.
    let strength = lerpf(min_strength, 1.0, p.sqrt());
    let base = pen.color.sanitized();
    Color::new(
        1.0 - (1.0 - base.r) * strength,
        1.0 - (1.0 - base.g) * strength,
        1.0 - (1.0 - base.b) * strength,
        base.a,
    )
}

fn modulated_alpha(pen: &PenSettings, opacity_p: f32, extra_scale: f32) -> f32 {
    let min_alpha = if pen.pressure_opacity_min.is_finite() {
        pen.pressure_opacity_min.clamp(0.0, 1.0)
    } else {
        0.16
    };
    let flow = lerpf(min_alpha, 1.0, opacity_p.clamp(0.0, 1.0));
    let base_opacity = if pen.opacity.is_finite() {
        pen.opacity.clamp(0.03, 1.0)
    } else {
        1.0
    };
    let scale = if extra_scale.is_finite() {
        extra_scale.clamp(0.0, 1.0)
    } else {
        1.0
    };
    (base_opacity * flow * scale).clamp(0.03, 1.0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn pressure_modulates_brush_radius_monotonically() {
        let pen = PenSettings::brush_pen();
        let samples = vec![
            StrokeSample {
                x: 0.0,
                y: 0.0,
                pressure: 0.1,
                time_ms: 0.0,
                ..Default::default()
            },
            StrokeSample {
                x: 50.0,
                y: 0.0,
                pressure: 0.5,
                time_ms: 16.0,
                ..Default::default()
            },
            StrokeSample {
                x: 100.0,
                y: 0.0,
                pressure: 1.0,
                time_ms: 32.0,
                ..Default::default()
            },
            StrokeSample {
                x: 150.0,
                y: 0.0,
                pressure: 1.0,
                time_ms: 48.0,
                ..Default::default()
            },
        ];
        let nodes = evaluate_spine(&pen, &samples);
        assert!(nodes[0].radius < nodes[1].radius);
        assert!(nodes[1].radius < nodes[2].radius);
    }

    #[test]
    fn highlighter_maintains_constant_opacity_and_chisel_nib() {
        let pen = PenSettings::highlighter();
        let samples = vec![
            StrokeSample::from_xy_pressure(0.0, 0.0, 0.2),
            StrokeSample::from_xy_pressure(80.0, 0.0, 0.9),
        ];
        let nodes = evaluate_spine(&pen, &samples);
        assert!((nodes[0].alpha - pen.opacity).abs() < 1e-6);
        assert!((nodes[1].alpha - pen.opacity).abs() < 1e-6);
        assert!(nodes[0].roundness < 0.5);
    }

    #[test]
    fn velocity_simulation_varies_mouse_stroke_width() {
        let pen = PenSettings::fountain_pen();
        let samples = vec![
            StrokeSample {
                x: 0.0,
                y: 0.0,
                pressure: 1.0,
                time_ms: 0.0,
                ..Default::default()
            },
            StrokeSample {
                x: 5.0,
                y: 0.0,
                pressure: 1.0,
                time_ms: 20.0,
                ..Default::default()
            },
            StrokeSample {
                x: 80.0,
                y: 0.0,
                pressure: 1.0,
                time_ms: 30.0,
                ..Default::default()
            },
            StrokeSample {
                x: 85.0,
                y: 0.0,
                pressure: 1.0,
                time_ms: 50.0,
                ..Default::default()
            },
        ];
        let nodes = evaluate_spine(&pen, &samples);
        // Fast middle segment should thin slightly vs slower endpoints (ignoring taper).
        assert_ne!(nodes[1].radius, nodes[2].radius);
    }

    #[test]
    fn light_pressure_lightens_ink_without_reducing_opacity() {
        let pens = [
            PenSettings::fountain_pen(),
            PenSettings::ball_pen(),
            PenSettings::brush_pen(),
        ];
        for pen in pens {
            let light = vec![
                StrokeSample::from_xy_pressure(0.0, 0.0, 0.05),
                StrokeSample::from_xy_pressure(50.0, 0.0, 0.05),
                StrokeSample::from_xy_pressure(100.0, 0.0, 0.05),
            ];
            let normal = vec![
                StrokeSample::from_xy_pressure(0.0, 0.0, 0.55),
                StrokeSample::from_xy_pressure(50.0, 0.0, 0.55),
                StrokeSample::from_xy_pressure(100.0, 0.0, 0.55),
            ];
            let firm = vec![
                StrokeSample::from_xy_pressure(0.0, 0.0, 0.95),
                StrokeSample::from_xy_pressure(50.0, 0.0, 0.95),
                StrokeSample::from_xy_pressure(100.0, 0.0, 0.95),
            ];
            let light_node = evaluate_spine(&pen, &light)[1];
            let normal_node = evaluate_spine(&pen, &normal)[1];
            let firm_node = evaluate_spine(&pen, &firm)[1];
            let brightness = |color: Color| color.r + color.g + color.b;
            assert!(
                (light_node.alpha - normal_node.alpha).abs() < 1e-6
                    && (normal_node.alpha - firm_node.alpha).abs() < 1e-6,
                "{:?} stylus pressure should not change opacity: {}, {}, {}",
                pen.kind,
                light_node.alpha,
                normal_node.alpha,
                firm_node.alpha
            );
            assert!(
                brightness(light_node.color) > brightness(normal_node.color)
                    && brightness(normal_node.color) > brightness(firm_node.color),
                "{:?} color should gain strength as pressure rises: {:?}, {:?}, {:?}",
                pen.kind,
                light_node.color,
                normal_node.color,
                firm_node.color
            );
            assert!(
                firm_node.alpha >= 0.85,
                "{:?} firm writing should remain opaque, got {}",
                pen.kind,
                firm_node.alpha
            );
        }
    }

    #[test]
    fn mouse_velocity_simulation_does_not_wash_out_ink_opacity() {
        let pen = PenSettings::fountain_pen();
        let samples = vec![
            StrokeSample {
                x: 0.0,
                y: 0.0,
                pressure: 1.0,
                time_ms: 0.0,
                ..Default::default()
            },
            StrokeSample {
                x: 100.0,
                y: 0.0,
                pressure: 1.0,
                time_ms: 16.0,
                ..Default::default()
            },
            StrokeSample {
                x: 110.0,
                y: 0.0,
                pressure: 1.0,
                time_ms: 48.0,
                ..Default::default()
            },
        ];
        let nodes = evaluate_spine(&pen, &samples);
        assert!(
            nodes[1].alpha >= 0.78,
            "fast mouse segment should stay crisp, got {}",
            nodes[1].alpha
        );
    }

    #[test]
    fn stylus_strokes_follow_pressure_without_synthetic_endpoint_tapers() {
        let mut pen = PenSettings::fountain_pen();
        assert_eq!(pen.start_taper_px, 0.0);
        assert_eq!(pen.end_taper_px, 0.0);
        // Even if an older saved pen carries legacy taper distances, real stylus pressure governs ends.
        pen.start_taper_px = 18.0;
        pen.end_taper_px = 18.0;

        let steady = vec![
            StrokeSample::from_xy_pressure(0.0, 0.0, 0.55),
            StrokeSample::from_xy_pressure(60.0, 0.0, 0.55),
            StrokeSample::from_xy_pressure(120.0, 0.0, 0.55),
            StrokeSample::from_xy_pressure(180.0, 0.0, 0.55),
        ];
        let nodes = evaluate_spine(&pen, &steady);
        assert!((nodes[0].radius - nodes[1].radius).abs() < 1e-4);
        assert!((nodes[3].radius - nodes[1].radius).abs() < 1e-4);

        // Actual light-to-firm-to-light stylus pressure drives natural tapering directly.
        let varied = vec![
            StrokeSample::from_xy_pressure(0.0, 0.0, 0.10),
            StrokeSample::from_xy_pressure(90.0, 0.0, 0.80),
            StrokeSample::from_xy_pressure(180.0, 0.0, 0.12),
        ];
        let vnodes = evaluate_spine(&pen, &varied);
        assert!(vnodes[0].radius < vnodes[1].radius * 0.7);
        assert!(vnodes[2].radius < vnodes[1].radius * 0.7);
    }

    #[test]
    fn comfortable_stylus_pressure_reaches_natural_width_and_rich_opacity() {
        for pen in [
            PenSettings::fountain_pen(),
            PenSettings::ball_pen(),
            PenSettings::brush_pen(),
        ] {
            let light_writing = vec![
                StrokeSample::from_xy_pressure(0.0, 0.0, 0.30),
                StrokeSample::from_xy_pressure(50.0, 0.0, 0.30),
            ];
            let nodes = evaluate_spine(&pen, &light_writing);
            assert!(
                nodes[0].alpha >= 0.85,
                "{:?} should render crisp rich ink at comfortable pressure 0.30, got {}",
                pen.kind,
                nodes[0].alpha
            );
            assert!(
                nodes[0].radius >= pen.size * 0.15,
                "{:?} should retain healthy stroke width at pressure 0.30, got radius {}",
                pen.kind,
                nodes[0].radius
            );
        }
    }
}
