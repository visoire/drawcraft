//! L2 handwriting & drawing engine for DrawCraft.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod dynamics;
pub mod eraser;
pub mod outline;
pub mod render;
pub mod shape_recognition;
pub mod spline;
pub mod stabilizer;

pub use dynamics::{SpineNode, evaluate_spine};
pub use eraser::{erase_along_path, erase_along_path_filtered, stroke_intersects_circle};
pub use outline::{
    RibbonMesh, RibbonVertex, ScalableRibbonMesh, ScalableRibbonVertex, build_stroke_ribbon,
    build_stroke_ribbon_scalable,
};
pub use render::{graphite_grain_factor, render_stroke_to_surface};
pub use shape_recognition::recognize_drawn_shape;
pub use spline::{
    densify_linear, predict_live_tip, resample_adaptive_catmull_rom,
    resample_centripetal_catmull_rom,
};
pub use stabilizer::{StrokeStabilizer, apply_streamline};

use drawcraft_doc::{PenKind, PenSettings, SmoothingConfig, StabilizerMode, StrokeSample};
use drawcraft_geom::distance_to_segment;

/// Post-processes stabilizer output into a smooth, curve-interpolated spine without distorting
/// tight stabilized paths or deliberate sharp corners.
pub fn finalize_smoothed_samples(
    stabilized: &[StrokeSample],
    smoothing: &SmoothingConfig,
) -> Vec<StrokeSample> {
    if stabilized.is_empty() {
        return Vec::new();
    }
    let cfg = smoothing.sanitized();
    let faired = apply_streamline(stabilized, cfg.streamline_amount);
    if faired.len() < 2 {
        return faired;
    }
    if cfg.stabilizer_mode == StabilizerMode::SimpleDirect {
        return densify_linear(&faired, 3.5);
    }
    if cfg.catmull_rom_subdivision {
        resample_centripetal_catmull_rom(&faired, cfg.spline_tension, 2.5)
    } else {
        resample_adaptive_catmull_rom(&faired, 3.0)
    }
}

/// Runs the active pen stabilizer and smooth curve subdivision over raw pointer samples.
pub fn smooth_stroke_samples(
    raw: &[StrokeSample],
    smoothing: &SmoothingConfig,
    zoom: f32,
) -> Vec<StrokeSample> {
    if raw.is_empty() {
        return Vec::new();
    }
    let cfg = smoothing.sanitized();
    let mut stab = StrokeStabilizer::new(&cfg, zoom);
    let mut stabilized = Vec::with_capacity(raw.len());
    for &s in raw {
        stab.push(s.sanitized(), &mut stabilized);
    }
    stab.finish(&mut stabilized);

    if stabilized.is_empty() {
        stabilized.push(raw[0].sanitized());
    }

    finalize_smoothed_samples(&stabilized, &cfg)
}

/// Evaluates smoothed samples and returns both smoothed samples and evaluated spine nodes for a pen.
pub fn build_pen_spine(
    pen: &PenSettings,
    raw: &[StrokeSample],
    zoom: f32,
) -> (Vec<StrokeSample>, Vec<SpineNode>) {
    let densified = smooth_stroke_samples(raw, &pen.smoothing, zoom);
    let smoothed = simplify_committed_spine(&densified, pen);
    let spine = evaluate_spine(pen, &smoothed);
    (smoothed, spine)
}

fn mark_taper_zones(samples: &[StrokeSample], pen: &PenSettings, keep: &mut [bool]) {
    let n = samples.len();
    let mut arcs = vec![0.0_f64; n];
    for i in 1..n {
        arcs[i] = arcs[i - 1] + samples[i - 1].point().distance(samples[i].point());
    }
    let total_len = arcs.last().copied().unwrap_or(0.0);
    let start_t = f64::from(pen.start_taper_px.max(0.0));
    let end_t = f64::from(pen.end_taper_px.max(0.0));
    for (i, &arc) in arcs.iter().enumerate() {
        if (start_t > 0.0 && arc <= start_t + 2.5)
            || (end_t > 0.0 && arc >= (total_len - end_t - 2.5).max(0.0))
        {
            keep[i] = true;
        }
    }
}

fn sample_deviation_exceeds(
    s: StrokeSample,
    a: StrokeSample,
    b: StrokeSample,
    check_tilt: bool,
    check_vel: bool,
) -> bool {
    let (dist, t_proj) = distance_to_segment(s.point(), a.point(), b.point());
    if !dist.is_finite() || dist > 0.10 {
        return true;
    }
    let tf = t_proj as f32;
    let p_interp = a.pressure + (b.pressure - a.pressure) * tf;
    if (s.pressure - p_interp).abs() > 0.008 {
        return true;
    }
    if check_tilt {
        let tx_i = a.tilt_x + (b.tilt_x - a.tilt_x) * tf;
        let ty_i = a.tilt_y + (b.tilt_y - a.tilt_y) * tf;
        if (s.tilt_x - tx_i).abs() > 1.5 || (s.tilt_y - ty_i).abs() > 1.5 {
            return true;
        }
    }
    if check_vel {
        let dt_ab = b.time_ms - a.time_ms;
        if dt_ab > 0.5 && a.point().distance(b.point()) > 0.5 {
            let t_interp = a.time_ms + dt_ab * t_proj;
            if ((s.time_ms - t_interp) / dt_ab).abs() > 0.12 {
                return true;
            }
        }
    }
    false
}

/// Removes visually redundant interior samples from a densified stroke spine while preserving
/// shape (`<= 0.10 px`), pressure (`<= 0.008`), tilt, simulated velocity, and start/end tapers.
pub fn simplify_committed_spine(samples: &[StrokeSample], pen: &PenSettings) -> Vec<StrokeSample> {
    let n = samples.len();
    if n <= 3 {
        return samples.to_vec();
    }
    let pen = pen.sanitized();
    let constant_mouse_pressure = samples.iter().all(|s| (s.pressure - 1.0).abs() < 1e-3);
    let check_vel =
        pen.velocity_pressure_simulation && pen.pressure_enabled && constant_mouse_pressure;
    let check_tilt =
        matches!(pen.kind, PenKind::BrushPen | PenKind::Pencil) && pen.tilt_sensitivity > 0.05;

    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    mark_taper_zones(samples, &pen, &mut keep);

    // Seed stack with each contiguous unkept run between pre-marked boundary/taper points.
    let mut stack: Vec<(usize, usize)> = Vec::with_capacity(64);
    let mut prev = 0usize;
    for (idx, &k) in keep.iter().enumerate().skip(1) {
        if k {
            if idx > prev + 1 {
                stack.push((prev, idx));
            }
            prev = idx;
        }
    }

    while let Some((start, end)) = stack.pop() {
        if end <= start + 1 {
            continue;
        }
        let a = samples[start];
        let b = samples[end];
        let mut best_score = 0.0_f64;
        let mut best_idx = start;

        for (i, &s) in samples.iter().enumerate().take(end).skip(start + 1) {
            if sample_deviation_exceeds(s, a, b, check_tilt, check_vel) {
                let (dist, t_proj) = distance_to_segment(s.point(), a.point(), b.point());
                let p_err = f64::from(
                    (s.pressure - (a.pressure + (b.pressure - a.pressure) * t_proj as f32)).abs(),
                );
                let score = dist + p_err * 20.0 + 0.01;
                if score > best_score {
                    best_score = score;
                    best_idx = i;
                }
            }
        }

        if best_idx > start && best_idx < end {
            keep[best_idx] = true;
            if stack.len() < 4096 {
                stack.push((start, best_idx));
                stack.push((best_idx, end));
            }
        }
    }

    samples
        .iter()
        .enumerate()
        .filter_map(|(i, &s)| keep[i].then_some(s))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn simplify_committed_spine_compacts_straight_lines_and_keeps_tapers() {
        let pen = PenSettings::fountain_pen();
        let line = densify_linear(
            &[
                StrokeSample::from_xy_pressure(50.0, 100.0, 0.7),
                StrokeSample::from_xy_pressure(650.0, 100.0, 0.7),
            ],
            4.0,
        );
        assert!(line.len() > 100);
        let compact = simplify_committed_spine(&line, &pen);
        assert!(
            compact.len() < 20,
            "expected compact straight line with taper zones, got {} samples (was {})",
            compact.len(),
            line.len()
        );
        assert_eq!(
            compact.first().unwrap().point(),
            line.first().unwrap().point()
        );
        assert_eq!(
            compact.last().unwrap().point(),
            line.last().unwrap().point()
        );
    }

    #[test]
    fn simplify_committed_spine_preserves_curve_fidelity_and_pressure() {
        let pen = PenSettings::fountain_pen();
        let mut pts = Vec::new();
        for i in 0..160 {
            let t = f64::from(i) * 2.5;
            pts.push(StrokeSample::from_xy_pressure(
                100.0 + t,
                250.0 + (t * 0.03).sin() * 45.0,
                0.25 + 0.65 * (f32::from(i as u16) / 159.0),
            ));
        }
        let compact = simplify_committed_spine(&pts, &pen);
        assert!(
            compact.len() < pts.len() * 3 / 4,
            "expected meaningful curve point reduction, got {} -> {}",
            pts.len(),
            compact.len()
        );
        assert!(compact.len() >= 10);
    }
}
