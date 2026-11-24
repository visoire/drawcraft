//! Multi-algorithm stroke stabilization and Taubin StreamLine fairing.

use std::collections::VecDeque;
use std::f64::consts::PI;

use drawcraft_doc::{
    BufferAveraging, BufferPreprocessor, SmoothingConfig, StabilizerMode, StrokeSample,
};
use drawcraft_geom::{Point, distance_to_segment};

pub const MAX_STRING_PX: f64 = 80.0;
pub const SMOOTHING_STEP_MS: f64 = 16.0;

#[inline]
fn lerp_angle_deg(a: f32, b: f32, t: f32) -> f32 {
    let dr = (b - a + 540.0).rem_euclid(360.0) - 180.0;
    (a + dr * t).rem_euclid(360.0)
}

#[inline]
fn lerp_sample(a: &StrokeSample, b: &StrokeSample, t: f64) -> StrokeSample {
    let tt = t.clamp(0.0, 1.0);
    let tf = tt as f32;
    StrokeSample {
        x: a.x + (b.x - a.x) * tt,
        y: a.y + (b.y - a.y) * tt,
        pressure: a.pressure + (b.pressure - a.pressure) * tf,
        tilt_x: a.tilt_x + (b.tilt_x - a.tilt_x) * tf,
        tilt_y: a.tilt_y + (b.tilt_y - a.tilt_y) * tf,
        rotation: lerp_angle_deg(a.rotation, b.rotation, tf),
        time_ms: a.time_ms + (b.time_ms - a.time_ms) * tt,
    }
}

#[inline]
fn pull_sample(brush: &StrokeSample, target: &StrokeSample, factor: f64) -> StrokeSample {
    let mut s = lerp_sample(brush, target, factor);
    s.time_ms = target.time_ms;
    s
}

#[inline]
fn sample_differs(a: &StrokeSample, b: &StrokeSample) -> bool {
    (a.x - b.x).abs() + (a.y - b.y).abs() > 0.02 || (a.pressure - b.pressure).abs() > 1e-3
}

/// 1D adaptive low-pass 1€ Filter (Casiez et al., 2012).
#[derive(Clone, Debug)]
struct OneEuroFilter1D {
    initialized: bool,
    x_prev: f64,
    dx_prev: f64,
}

impl OneEuroFilter1D {
    fn new() -> Self {
        Self {
            initialized: false,
            x_prev: 0.0,
            dx_prev: 0.0,
        }
    }

    #[inline]
    fn alpha(dt_s: f64, cutoff_hz: f64) -> f64 {
        let tau = 1.0 / (2.0 * PI * cutoff_hz.max(1e-3));
        1.0 / (1.0 + tau / dt_s.max(1e-4))
    }

    fn filter(&mut self, value: f64, dt_s: f64, min_cutoff_hz: f64, beta: f64) -> f64 {
        if !value.is_finite() {
            return self.x_prev;
        }
        if !self.initialized {
            self.initialized = true;
            self.x_prev = value;
            self.dx_prev = 0.0;
            return value;
        }
        let dt = dt_s.clamp(1.0 / 1000.0, 0.25);
        let dx = (value - self.x_prev) / dt;
        let a_d = Self::alpha(dt, 1.0);
        let dx_hat = self.dx_prev + a_d * (dx - self.dx_prev);
        let cutoff = min_cutoff_hz + beta * dx_hat.abs();
        let a = Self::alpha(dt, cutoff);
        let x_hat = self.x_prev + a * (value - self.x_prev);
        self.x_prev = x_hat;
        self.dx_prev = dx_hat;
        x_hat
    }
}

/// 2D + pressure/tilt adaptive 1€ Stabilizer.
#[derive(Clone, Debug)]
pub struct OneEuroStabilizer {
    min_cutoff: f64,
    beta: f64,
    fx: OneEuroFilter1D,
    fy: OneEuroFilter1D,
    fp: OneEuroFilter1D,
    ftx: OneEuroFilter1D,
    fty: OneEuroFilter1D,
    last_time_ms: Option<f64>,
    last_raw: Option<StrokeSample>,
    last_filtered: Option<StrokeSample>,
    catch_up_on_end: bool,
}

impl OneEuroStabilizer {
    pub fn new(strength: f32, catch_up_on_end: bool) -> Self {
        let s = f64::from(if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            0.45
        });
        // Strength 0 -> min_cutoff=8.0 Hz, beta=0.08; Strength 1 -> min_cutoff=0.4 Hz, beta=0.005.
        let min_cutoff = 8.0 * (0.05_f64).powf(s);
        let beta = 0.08 * (0.0625_f64).powf(s);
        Self {
            min_cutoff,
            beta,
            fx: OneEuroFilter1D::new(),
            fy: OneEuroFilter1D::new(),
            fp: OneEuroFilter1D::new(),
            ftx: OneEuroFilter1D::new(),
            fty: OneEuroFilter1D::new(),
            last_time_ms: None,
            last_raw: None,
            last_filtered: None,
            catch_up_on_end,
        }
    }

    pub fn push(&mut self, sample: StrokeSample, out: &mut Vec<StrokeSample>) {
        let p = sample.sanitized();
        let dt_ms = match self.last_time_ms {
            Some(prev_t) if p.time_ms > prev_t => (p.time_ms - prev_t).clamp(1.0, 200.0),
            Some(_) if self.last_raw.is_some() => 4.0,
            _ => SMOOTHING_STEP_MS,
        };
        self.last_time_ms = Some(p.time_ms);
        self.last_raw = Some(p);

        let dt_s = dt_ms / 1000.0;
        let x = self.fx.filter(p.x, dt_s, self.min_cutoff, self.beta);
        let y = self.fy.filter(p.y, dt_s, self.min_cutoff, self.beta);
        let pressure = self
            .fp
            .filter(
                f64::from(p.pressure),
                dt_s,
                self.min_cutoff * 1.5,
                self.beta * 2.0,
            )
            .clamp(0.0, 1.0) as f32;
        let tilt_x = self
            .ftx
            .filter(f64::from(p.tilt_x), dt_s, self.min_cutoff, self.beta)
            .clamp(-90.0, 90.0) as f32;
        let tilt_y = self
            .fty
            .filter(f64::from(p.tilt_y), dt_s, self.min_cutoff, self.beta)
            .clamp(-90.0, 90.0) as f32;

        let filtered = StrokeSample {
            x,
            y,
            pressure,
            tilt_x,
            tilt_y,
            rotation: p.rotation,
            time_ms: p.time_ms,
        };
        self.last_filtered = Some(filtered);
        out.push(filtered);
    }

    pub fn finish(&mut self, out: &mut Vec<StrokeSample>) {
        if !self.catch_up_on_end {
            return;
        }
        if let (Some(filtered), Some(raw)) = (self.last_filtered, self.last_raw)
            && sample_differs(&filtered, &raw)
        {
            let mid = lerp_sample(&filtered, &raw, 0.55);
            out.push(mid);
            out.push(raw);
            self.last_filtered = Some(raw);
        }
    }
}

/// Pulled-string (virtual tether) stabilizer.
#[derive(Clone, Debug)]
pub struct PulledStringStabilizer {
    string_len: f64,
    catch_up_on_end: bool,
    pos: Option<StrokeSample>,
    last_input: Option<StrokeSample>,
}

impl PulledStringStabilizer {
    pub fn new(strength: f32, zoom: f32, adjust_for_zoom: bool, catch_up_on_end: bool) -> Self {
        let s = f64::from(if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            0.45
        });
        let z = f64::from(if zoom.is_finite() {
            zoom.max(0.05)
        } else {
            1.0
        });
        let len = if adjust_for_zoom {
            (s * MAX_STRING_PX) / z
        } else {
            s * MAX_STRING_PX
        };
        Self {
            string_len: len,
            catch_up_on_end,
            pos: None,
            last_input: None,
        }
    }

    pub fn push(&mut self, sample: StrokeSample, out: &mut Vec<StrokeSample>) {
        let p = sample.sanitized();
        self.last_input = Some(p);
        let Some(pos) = self.pos else {
            self.pos = Some(p);
            out.push(p);
            return;
        };
        if self.string_len <= 1e-4 {
            self.pos = Some(p);
            out.push(p);
            return;
        }
        let dist = pos.point().distance(p.point());
        if dist > self.string_len {
            let k = (dist - self.string_len) / dist;
            let next = pull_sample(&pos, &p, k);
            self.pos = Some(next);
            out.push(next);
        }
    }

    pub fn tether(&self) -> Option<(Point, Point)> {
        let (pos, last) = (self.pos?, self.last_input?);
        if (pos.x - last.x).abs() + (pos.y - last.y).abs() > 0.5 {
            Some((pos.point(), last.point()))
        } else {
            None
        }
    }

    pub fn finish(&mut self, out: &mut Vec<StrokeSample>) {
        if !self.catch_up_on_end {
            return;
        }
        if let (Some(pos), Some(last)) = (self.pos, self.last_input)
            && sample_differs(&pos, &last)
        {
            for step in 1..=4 {
                out.push(lerp_sample(&pos, &last, f64::from(step) / 4.0));
            }
            self.pos = Some(last);
        }
    }
}

/// Time-normalized Exponential Moving Average stabilizer with stationary & end catch-up.
#[derive(Clone, Debug)]
pub struct EmaStabilizer {
    amount: f64,
    catch_up: bool,
    catch_up_on_end: bool,
    pos: Option<StrokeSample>,
    last_input: Option<StrokeSample>,
}

impl EmaStabilizer {
    pub fn new(strength: f32, catch_up: bool, catch_up_on_end: bool) -> Self {
        let s = f64::from(if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            0.45
        });
        Self {
            amount: s.min(0.95),
            catch_up,
            catch_up_on_end,
            pos: None,
            last_input: None,
        }
    }

    pub fn push(&mut self, sample: StrokeSample, out: &mut Vec<StrokeSample>) {
        let p = sample.sanitized();
        let prev = self.last_input.replace(p);
        let Some(pos) = self.pos else {
            self.pos = Some(p);
            out.push(p);
            return;
        };
        let a = 1.0 - self.amount;
        let still = prev.is_some_and(|q| q.x == p.x && q.y == p.y);
        if still && !self.catch_up {
            return;
        }
        let dt = prev.map_or(0.0, |q| p.time_ms - q.time_ms);
        let (iters, step) = if dt > 0.0 && dt.is_finite() {
            let steps = dt / SMOOTHING_STEP_MS;
            let iters = steps.ceil().clamp(1.0, 64.0);
            (
                iters as usize,
                1.0 - (1.0 - a).powf(steps.min(64.0) / iters),
            )
        } else {
            (1, a)
        };
        let mut cur = pos;
        for _ in 0..iters {
            cur = pull_sample(&cur, &p, step);
            out.push(cur);
        }
        self.pos = Some(cur);
    }

    pub fn is_lagging(&self) -> bool {
        match (self.catch_up, self.pos, self.last_input) {
            (true, Some(pos), Some(last)) => sample_differs(&pos, &last),
            _ => false,
        }
    }

    pub fn finish(&mut self, out: &mut Vec<StrokeSample>) {
        if !self.catch_up_on_end {
            return;
        }
        if let (Some(pos), Some(last)) = (self.pos, self.last_input)
            && sample_differs(&pos, &last)
        {
            out.push(lerp_sample(&pos, &last, 0.5));
            out.push(last);
            self.pos = Some(last);
        }
    }
}

/// Causal 5-tap moving Gaussian kernel stabilizer.
#[derive(Clone, Debug)]
pub struct GaussianStabilizer {
    strength: f64,
    catch_up_on_end: bool,
    window: VecDeque<StrokeSample>,
    last_emitted: Option<StrokeSample>,
}

impl GaussianStabilizer {
    const WEIGHTS: [f64; 5] = [0.06, 0.14, 0.22, 0.28, 0.30];

    pub fn new(strength: f32, catch_up_on_end: bool) -> Self {
        let s = f64::from(if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            0.45
        });
        Self {
            strength: s,
            catch_up_on_end,
            window: VecDeque::with_capacity(5),
            last_emitted: None,
        }
    }

    pub fn push(&mut self, sample: StrokeSample, out: &mut Vec<StrokeSample>) {
        let p = sample.sanitized();
        if self.window.len() >= 5 {
            self.window.pop_front();
        }
        self.window.push_back(p);

        if self.window.len() == 1 {
            self.last_emitted = Some(p);
            out.push(p);
            return;
        }

        let offset = 5 - self.window.len();
        let mut w_sum = 0.0;
        let (mut x, mut y, mut pr, mut tx, mut ty) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (idx, s) in self.window.iter().enumerate() {
            let w = Self::WEIGHTS[offset + idx];
            w_sum += w;
            x += s.x * w;
            y += s.y * w;
            pr += f64::from(s.pressure) * w;
            tx += f64::from(s.tilt_x) * w;
            ty += f64::from(s.tilt_y) * w;
        }
        let inv = if w_sum > 1e-9 { 1.0 / w_sum } else { 1.0 };
        let g = StrokeSample {
            x: x * inv,
            y: y * inv,
            pressure: (pr * inv).clamp(0.0, 1.0) as f32,
            tilt_x: (tx * inv).clamp(-90.0, 90.0) as f32,
            tilt_y: (ty * inv).clamp(-90.0, 90.0) as f32,
            rotation: p.rotation,
            time_ms: p.time_ms,
        };
        let blended = lerp_sample(&p, &g, self.strength);
        self.last_emitted = Some(blended);
        out.push(blended);
    }

    pub fn finish(&mut self, out: &mut Vec<StrokeSample>) {
        if !self.catch_up_on_end {
            return;
        }
        if let (Some(emitted), Some(last)) = (self.last_emitted, self.window.back().copied())
            && sample_differs(&emitted, &last)
        {
            out.push(lerp_sample(&emitted, &last, 0.5));
            out.push(last);
            self.last_emitted = Some(last);
        }
    }
}

/// Taubin ($\lambda / \mu$) StreamLine fairing that removes hand wobble while preserving loop volume
/// and keeping stroke endpoints anchored.
pub fn apply_streamline(points: &[StrokeSample], amount: f32) -> Vec<StrokeSample> {
    let amt = f64::from(if amount.is_finite() {
        amount.clamp(0.0, 1.0)
    } else {
        0.0
    });
    if points.len() < 3 || amt <= 1e-4 {
        return points.to_vec();
    }

    let passes = (1.0 + amt * 4.0).round() as usize;
    let lambda = 0.45 * amt;
    let mu = -0.44 * amt;

    let mut cur = points.to_vec();
    let mut tmp = cur.clone();

    for _ in 0..passes {
        taubin_step(&cur, &mut tmp, lambda);
        taubin_step(&tmp, &mut cur, mu);
    }
    cur
}

fn taubin_step(src: &[StrokeSample], dst: &mut [StrokeSample], weight: f64) {
    let n = src.len();
    dst.copy_from_slice(src);
    for i in 1..(n - 1) {
        let prev = src[i - 1];
        let mid = src[i];
        let next = src[i + 1];
        let avg_x = 0.5 * (prev.x + next.x);
        let avg_y = 0.5 * (prev.y + next.y);
        let avg_p = 0.5 * (prev.pressure + next.pressure);
        let wf = weight as f32;
        dst[i].x = mid.x + weight * (avg_x - mid.x);
        dst[i].y = mid.y + weight * (avg_y - mid.y);
        dst[i].pressure = (mid.pressure + wf * (avg_p - mid.pressure)).clamp(0.0, 1.0);
    }
}

const MAX_VELOCITY_GAUSSIAN_WINDOW: usize = 256;
const MIN_MOTION_THRESHOLD: f64 = 0.3;
const SPLINE_FLATNESS_TOLERANCE: f64 = 1.0001;
const MAX_SPLINE_DEPTH: u8 = 10;
const MAX_SPLINE_SEGMENT_LEN: f64 = 4.0;
const MAX_SPLINE_PRESSURE_STEP: f32 = 0.06;
const MAX_PAINT_PRESSURE_STEP: f32 = 0.08;

#[inline]
fn midpoint(a: Point, b: Point) -> Point {
    Point::new(0.5 * (a.x + b.x), 0.5 * (a.y + b.y))
}

fn subdivide_cubic_segment(
    ctrl: [Point; 4],
    t_range: [f64; 2],
    ends: (&StrokeSample, &StrokeSample),
    min_knot: f64,
    depth: u8,
    out: &mut Vec<StrokeSample>,
) {
    let [p0, p1, p2, p3] = ctrl;
    let [t0, t1] = t_range;
    let l = p0.distance(p3);
    let l1 = p0.distance(p1);
    let l2 = p1.distance(p2);
    let l3 = p2.distance(p3);
    let p_start = ends.0.pressure + (ends.1.pressure - ends.0.pressure) * (t0 as f32);
    let p_end = ends.0.pressure + (ends.1.pressure - ends.0.pressure) * (t1 as f32);
    let is_flat = (l1 + l2 + l3) <= SPLINE_FLATNESS_TOLERANCE * l;
    let small_dp = (p_end - p_start).abs() <= MAX_SPLINE_PRESSURE_STEP;
    let is_leaf = depth >= MAX_SPLINE_DEPTH
        || l < min_knot
        || (depth >= 1 && is_flat && small_dp && l <= MAX_SPLINE_SEGMENT_LEN);

    if is_leaf {
        let is_end = (t1 - 1.0).abs() < 1e-9;
        let sample = if is_end {
            *ends.1
        } else {
            let mut s = lerp_sample(ends.0, ends.1, t1);
            s.x = p3.x;
            s.y = p3.y;
            s
        };
        let min_sep = if is_end { 1e-6 } else { 1e-4 };
        if out
            .last()
            .is_none_or(|prev| prev.point().distance(p3) >= min_sep)
        {
            out.push(sample);
        }
        return;
    }

    let p01 = midpoint(p0, p1);
    let p12 = midpoint(p1, p2);
    let p23 = midpoint(p2, p3);
    let p012 = midpoint(p01, p12);
    let p123 = midpoint(p12, p23);
    let p0123 = midpoint(p012, p123);
    let tm = 0.5 * (t0 + t1);

    subdivide_cubic_segment(
        [p0, p01, p012, p0123],
        [t0, tm],
        ends,
        min_knot,
        depth + 1,
        out,
    );
    subdivide_cubic_segment(
        [p0123, p123, p23, p3],
        [tm, t1],
        ends,
        min_knot,
        depth + 1,
        out,
    );
}

/// Circular-buffer input stabilizer (Arithmetic Mean or Velocity-weighted Gaussian,
/// optional Deadzone / Inertia preprocessor, and C1 quadratic spline stroke finalization).
#[derive(Clone, Debug)]
pub struct CircularBufferStabilizer {
    averaging: BufferAveraging,
    preprocessor: BufferPreprocessor,
    buffer_size: usize,
    min_motion_threshold: f64,
    two_sigma_sq: f64,
    deadzone_radius: f64,
    cusp_detection: bool,
    one_minus_drag: f64,
    mass: f64,
    finalize_stroke: bool,
    initialized: bool,
    arith_buf: VecDeque<StrokeSample>,
    vel_buf: VecDeque<(StrokeSample, f64)>,
    last_timestamp_ms: f64,
    last_event: StrokeSample,
    last_stabilized: StrokeSample,
    last_live_event: StrokeSample,
    last_live_dir: (f64, f64),
    has_live_dir: bool,
    speed: (f64, f64),
}

impl CircularBufferStabilizer {
    pub fn new(cfg: &SmoothingConfig) -> Self {
        Self::new_with_zoom(cfg, 1.0)
    }

    pub fn new_with_zoom(cfg: &SmoothingConfig, zoom: f32) -> Self {
        let clean = cfg.sanitized();
        let sigma = f64::from(clean.buffer_sigma).clamp(0.05, 5.0);
        let z = f64::from(if zoom.is_finite() {
            zoom.clamp(0.5, 6.0)
        } else {
            1.0
        })
        .max(1.0);
        Self {
            averaging: clean.buffer_averaging,
            preprocessor: clean.buffer_preprocessor,
            buffer_size: usize::from(clean.buffer_size).clamp(2, 100),
            min_motion_threshold: MIN_MOTION_THRESHOLD / z,
            two_sigma_sq: 2.0 * sigma * sigma,
            deadzone_radius: f64::from(clean.buffer_deadzone_radius).clamp(0.1, 50.0),
            cusp_detection: clean.buffer_cusp_detection,
            one_minus_drag: 1.0 - f64::from(clean.buffer_drag).clamp(0.0, 1.0),
            mass: f64::from(clean.buffer_mass).clamp(1.0, 30.0),
            finalize_stroke: clean.buffer_finalize_stroke,
            initialized: false,
            arith_buf: VecDeque::new(),
            vel_buf: VecDeque::new(),
            last_timestamp_ms: 0.0,
            last_event: StrokeSample::default(),
            last_stabilized: StrokeSample::default(),
            last_live_event: StrokeSample::default(),
            last_live_dir: (0.0, 0.0),
            has_live_dir: false,
            speed: (0.0, 0.0),
        }
    }

    pub fn push(&mut self, sample: StrokeSample, out: &mut Vec<StrokeSample>) {
        let s = sample.sanitized();
        if !self.initialized {
            self.record_first(s, out);
            return;
        }
        self.last_event = s;
        match self.preprocessor {
            BufferPreprocessor::None => self.average_and_paint(s, out),
            BufferPreprocessor::Deadzone => self.push_deadzone(s, out),
            BufferPreprocessor::Inertia => self.push_inertia(s, out),
        }
    }

    pub fn finish(&mut self, out: &mut Vec<StrokeSample>) {
        if !self.initialized || !self.finalize_stroke {
            return;
        }
        let is_active = self.averaging != BufferAveraging::None
            || self.preprocessor != BufferPreprocessor::None;
        if !is_active {
            return;
        }
        self.rebalance_pressures(out);
        let target = self.final_target_event();
        self.quadratic_spline_to(target, out);
    }

    fn record_first(&mut self, s: StrokeSample, out: &mut Vec<StrokeSample>) {
        self.initialized = true;
        self.last_event = s;
        self.last_stabilized = s;
        self.last_live_event = s;
        self.last_live_dir = (0.0, 0.0);
        self.has_live_dir = false;
        self.speed = (0.0, 0.0);
        self.last_timestamp_ms = s.time_ms;
        match self.averaging {
            BufferAveraging::ArithmeticMean => {
                self.arith_buf.clear();
                self.arith_buf.resize(self.buffer_size, s);
            }
            BufferAveraging::VelocityGaussian => {
                self.vel_buf.clear();
                self.vel_buf.push_front((s, 0.0));
            }
            BufferAveraging::None => {}
        }
        out.push(s);
    }

    fn push_deadzone(&mut self, s: StrokeSample, out: &mut Vec<StrokeSample>) {
        let dx = s.x - self.last_stabilized.x;
        let dy = s.y - self.last_stabilized.y;
        let norm = dx.hypot(dy);
        if norm <= self.deadzone_radius {
            let center = StrokeSample {
                time_ms: s.time_ms,
                ..self.last_stabilized
            };
            self.reset_buffer(center);
            return;
        }
        if self.cusp_detection
            && self.has_live_dir
            && (dx * self.last_live_dir.0 + dy * self.last_live_dir.1) < 0.0
        {
            self.handle_deadzone_cusp(s, out);
            return;
        }
        let ratio = self.deadzone_radius / norm;
        self.last_live_event = s;
        self.last_live_dir = (dx, dy);
        self.has_live_dir = true;
        let adjusted = StrokeSample {
            x: s.x - ratio * dx,
            y: s.y - ratio * dy,
            ..s
        };
        self.average_and_paint(adjusted, out);
    }

    fn handle_deadzone_cusp(&mut self, s: StrokeSample, out: &mut Vec<StrokeSample>) {
        let cusp_tip = self.last_live_event;
        self.quadratic_spline_to(cusp_tip, out);

        let diff_x = s.x - cusp_tip.x;
        let diff_y = s.y - cusp_tip.y;
        let diff_norm = diff_x.hypot(diff_y);
        if diff_norm > 1e-6 {
            let coeff = self.deadzone_radius / diff_norm;
            let step_x = coeff * diff_x;
            let step_y = coeff * diff_y;
            self.last_live_dir = (step_x, step_y);
            self.has_live_dir = true;
            let coeff_f32 = coeff.clamp(0.0, 1.0) as f32;
            let back = StrokeSample {
                x: s.x - step_x,
                y: s.y - step_y,
                pressure: (coeff_f32 * cusp_tip.pressure + (1.0 - coeff_f32) * s.pressure)
                    .clamp(0.0, 1.0),
                ..s
            };
            self.last_stabilized = back;
            self.paint_to(back, out);
        }
        self.last_live_event = s;
        let center = StrokeSample {
            time_ms: s.time_ms,
            ..self.last_stabilized
        };
        self.reset_buffer(center);
    }

    fn push_inertia(&mut self, s: StrokeSample, out: &mut Vec<StrokeSample>) {
        let accel_x = (s.x - self.last_stabilized.x) / self.mass;
        let accel_y = (s.y - self.last_stabilized.y) / self.mass;
        let vx = self.one_minus_drag * self.speed.0 + accel_x;
        let vy = self.one_minus_drag * self.speed.1 + accel_y;
        if vx.is_finite() && vy.is_finite() {
            self.speed = (vx, vy);
        } else {
            self.speed = (0.0, 0.0);
        }
        let ev = StrokeSample {
            x: self.last_stabilized.x + self.speed.0,
            y: self.last_stabilized.y + self.speed.1,
            ..s
        }
        .sanitized();
        self.average_and_paint(ev, out);
    }

    fn average_and_paint(&mut self, ev: StrokeSample, out: &mut Vec<StrokeSample>) {
        let smoothed = match self.averaging {
            BufferAveraging::ArithmeticMean => self.average_arithmetic(ev),
            BufferAveraging::VelocityGaussian => self.average_velocity_gaussian(ev),
            BufferAveraging::None => ev,
        };
        self.last_stabilized = smoothed;
        self.paint_to(smoothed, out);
    }

    fn average_arithmetic(&mut self, ev: StrokeSample) -> StrokeSample {
        if self.arith_buf.len() >= self.buffer_size {
            self.arith_buf.pop_back();
        }
        self.arith_buf.push_front(ev);
        let n = self.arith_buf.len().max(1) as f64;
        let mut sum_x = 0.0_f64;
        let mut sum_y = 0.0_f64;
        let mut sum_p = 0.0_f64;
        let mut sum_tx = 0.0_f64;
        let mut sum_ty = 0.0_f64;
        for item in &self.arith_buf {
            sum_x += item.x;
            sum_y += item.y;
            sum_p += f64::from(item.pressure);
            sum_tx += f64::from(item.tilt_x);
            sum_ty += f64::from(item.tilt_y);
        }
        StrokeSample {
            x: sum_x / n,
            y: sum_y / n,
            pressure: ((sum_p / n) as f32).clamp(0.0, 1.0),
            tilt_x: ((sum_tx / n) as f32).clamp(-90.0, 90.0),
            tilt_y: ((sum_ty / n) as f32).clamp(-90.0, 90.0),
            rotation: ev.rotation,
            time_ms: ev.time_ms,
        }
    }

    fn average_velocity_gaussian(&mut self, ev: StrokeSample) -> StrokeSample {
        let vel = if let Some(&(prev, _)) = self.vel_buf.front() {
            let dt = (ev.time_ms - self.last_timestamp_ms).max(1.0);
            (ev.x - prev.x).hypot(ev.y - prev.y) / dt
        } else {
            0.0
        };
        self.vel_buf.push_front((ev, vel));
        if self.vel_buf.len() > MAX_VELOCITY_GAUSSIAN_WINDOW {
            self.vel_buf.truncate(MAX_VELOCITY_GAUSSIAN_WINDOW);
        }
        self.last_timestamp_ms = ev.time_ms;

        let mut sum_x = 0.0_f64;
        let mut sum_y = 0.0_f64;
        let mut sum_p = 0.0_f64;
        let mut sum_tx = 0.0_f64;
        let mut sum_ty = 0.0_f64;
        let mut sum_weights = 0.0_f64;
        let mut sum_vel = 0.0_f64;
        let mut keep_len = self.vel_buf.len();

        for (idx, &(sample, v)) in self.vel_buf.iter().enumerate() {
            let weight = (-(sum_vel * sum_vel) / self.two_sigma_sq).exp();
            if weight < 0.01 {
                keep_len = idx;
                break;
            }
            sum_vel += v;
            sum_x += weight * sample.x;
            sum_y += weight * sample.y;
            sum_p += weight * f64::from(sample.pressure);
            sum_tx += weight * f64::from(sample.tilt_x);
            sum_ty += weight * f64::from(sample.tilt_y);
            sum_weights += weight;
        }
        self.vel_buf.truncate(keep_len.max(1));

        if sum_weights <= 1e-12 {
            return ev;
        }
        StrokeSample {
            x: sum_x / sum_weights,
            y: sum_y / sum_weights,
            pressure: ((sum_p / sum_weights) as f32).clamp(0.0, 1.0),
            tilt_x: ((sum_tx / sum_weights) as f32).clamp(-90.0, 90.0),
            tilt_y: ((sum_ty / sum_weights) as f32).clamp(-90.0, 90.0),
            rotation: ev.rotation,
            time_ms: ev.time_ms,
        }
    }

    fn reset_buffer(&mut self, ev: StrokeSample) {
        match self.averaging {
            BufferAveraging::ArithmeticMean => {
                let differs = self.arith_buf.back().is_none_or(|b| sample_differs(b, &ev));
                if differs {
                    self.arith_buf.clear();
                    self.arith_buf.resize(self.buffer_size, ev);
                }
            }
            BufferAveraging::VelocityGaussian => {
                let differs = self.vel_buf.len() != 1
                    || (self.last_timestamp_ms - ev.time_ms).abs() > 1e-6
                    || self
                        .vel_buf
                        .front()
                        .is_none_or(|(f, _)| sample_differs(f, &ev));
                if differs {
                    self.vel_buf.clear();
                    self.vel_buf.push_front((ev, 0.0));
                    self.last_timestamp_ms = ev.time_ms;
                }
            }
            BufferAveraging::None => {}
        }
    }

    fn paint_to(&mut self, ev: StrokeSample, out: &mut Vec<StrokeSample>) {
        let Some(&last) = out.last() else {
            out.push(ev);
            return;
        };
        let dist = last.point().distance(ev.point());
        if dist < self.min_motion_threshold {
            // If pressure builds right at touchdown before nib motion crosses threshold,
            // thicken the touchdown point in place instead of creating micro-duplicates.
            if out.len() == 1
                && ev.pressure > last.pressure
                && let Some(first) = out.first_mut()
            {
                first.pressure = ev.pressure;
            }
            return;
        }
        let dp = (ev.pressure - last.pressure).abs();
        if dp > MAX_PAINT_PRESSURE_STEP && dist >= self.min_motion_threshold * 2.0 {
            let by_pressure = (dp / MAX_PAINT_PRESSURE_STEP).ceil() as usize;
            let by_dist = (dist / self.min_motion_threshold).floor() as usize;
            let steps = by_pressure.min(by_dist).clamp(1, 16);
            for i in 1..steps {
                out.push(lerp_sample(&last, &ev, i as f64 / steps as f64));
            }
        }
        out.push(ev);
    }

    fn final_target_event(&self) -> StrokeSample {
        if self.preprocessor != BufferPreprocessor::None {
            return self.last_event;
        }
        match self.averaging {
            BufferAveraging::ArithmeticMean => {
                self.arith_buf.front().copied().unwrap_or(self.last_event)
            }
            BufferAveraging::VelocityGaussian => self
                .vel_buf
                .front()
                .map(|(s, _)| *s)
                .unwrap_or(self.last_event),
            BufferAveraging::None => self.last_event,
        }
    }

    fn rebalance_pressures(&self, out: &mut [StrokeSample]) {
        if self.preprocessor == BufferPreprocessor::None || out.len() < 3 {
            return;
        }
        let n = out.len();
        if let (Some(&p_prev), Some(p_second_last)) = (out.get(n - 3), out.get(n - 2)) {
            let avg = ((p_prev.pressure + p_second_last.pressure) * 0.5).clamp(0.0, 1.0);
            if let Some(slot) = out.get_mut(n - 2) {
                slot.pressure = avg;
            }
        }
    }

    fn quadratic_spline_to(&mut self, target: StrokeSample, out: &mut Vec<StrokeSample>) {
        let len = out.len();
        if len == 0 {
            return;
        }
        if len == 1 {
            if out[0].point().distance(target.point()) >= 0.05 {
                out.push(target);
            } else if target.pressure > out[0].pressure {
                out[0].pressure = target.pressure;
            }
            return;
        }
        let (Some(&a), Some(&b)) = (out.get(len - 2), out.get(len - 1)) else {
            return;
        };
        let v_ab = (b.x - a.x, b.y - a.y);
        let v_bc = (target.x - b.x, target.y - b.y);
        let sq_bc = v_bc.0 * v_bc.0 + v_bc.1 * v_bc.1;
        let norm_bc = sq_bc.sqrt();
        let norm_ab = v_ab.0.hypot(v_ab.1);
        if norm_bc < 1e-6 {
            return;
        }
        if norm_ab < 1e-6 {
            out.push(target);
            return;
        }

        let dot = v_ab.0 * v_bc.0 + v_ab.1 * v_bc.1;
        let mut b_adj = b;
        let mut target_adj = target;
        // Extrapolate realistic cadence when target shares b's frame timestamp (avoids dt == 0 velocity spikes).
        if target_adj.time_ms <= b_adj.time_ms + 0.05 {
            let dt_ab = (b.time_ms - a.time_ms).clamp(1.0, 48.0);
            let pace_dt = if a.time_ms > 0.0 && b.time_ms > a.time_ms {
                (dt_ab * (norm_bc / norm_ab.max(0.25))).clamp(1.0, 48.0)
            } else {
                (norm_bc * 1.5).clamp(1.0, 48.0)
            };
            target_adj.time_ms = b_adj.time_ms + pace_dt;
        }
        // Cushion steep single-packet lift-off pressure cliffs right at stroke release.
        if target_adj.pressure < b_adj.pressure * 0.4 && norm_bc > 0.2 {
            target_adj.pressure =
                (b_adj.pressure * 0.65 + target_adj.pressure * 0.35).clamp(0.0, 1.0);
        }

        let (fp, sp) = if dot > 1e-6 {
            let cos_theta = (dot / (norm_ab * norm_bc)).clamp(0.0, 1.0);
            let max_reach = norm_bc * (0.25 + 0.75 * cos_theta);
            let distance = (sq_bc * norm_ab / (2.0 * dot)).min(max_reach);
            let coeff = norm_bc * 0.5 + distance;
            let denom = norm_ab + coeff;
            if denom > 1e-9 {
                let new_p = ((coeff * f64::from(a.pressure)
                    + norm_ab * f64::from(target_adj.pressure))
                    / denom) as f32;
                b_adj.pressure = new_p.clamp(0.0, 1.0);
                if let Some(last) = out.last_mut() {
                    last.pressure = b_adj.pressure;
                }
            }
            let qx = b.x + (v_ab.0 / norm_ab) * distance;
            let qy = b.y + (v_ab.1 / norm_ab) * distance;
            (
                Point::new(
                    b.x + (qx - b.x) * (2.0 / 3.0),
                    b.y + (qy - b.y) * (2.0 / 3.0),
                ),
                Point::new(
                    target_adj.x + (qx - target_adj.x) * (2.0 / 3.0),
                    target_adj.y + (qy - target_adj.y) * (2.0 / 3.0),
                ),
            )
        } else {
            // Sharp reversal at lift-off: connect linearly to target instead of overshooting forward.
            (
                Point::new(b.x + v_bc.0 * (1.0 / 3.0), b.y + v_bc.1 * (1.0 / 3.0)),
                Point::new(b.x + v_bc.0 * (2.0 / 3.0), b.y + v_bc.1 * (2.0 / 3.0)),
            )
        };

        subdivide_cubic_segment(
            [b.point(), fp, sp, target_adj.point()],
            [0.0, 1.0],
            (&b_adj, &target_adj),
            self.min_motion_threshold,
            0,
            out,
        );
    }
}

const MAX_WOBBLE_WINDOW_SAMPLES: usize = 16;
const WOBBLE_TIMEOUT_MS: f64 = 28.0;
const WOBBLE_SPEED_FLOOR_PX_MS: f64 = 0.10;
const WOBBLE_SPEED_CEILING_PX_MS: f64 = 0.80;
const MODELER_STEP_MS: f64 = 1000.0 / 180.0;
const MAX_MODELER_SUBSTEPS: usize = 24;
const MAX_RAW_PATH_WINDOW: usize = 32;
const MAX_SETTLE_STEPS: usize = 18;

/// Three-stage physical stroke modeler:
/// 1) time-windowed velocity wobble smoother,
/// 2) 180 Hz damped spring-mass Euler position modeler, and
/// 3) arc-projected stylus state interpolation.
#[derive(Clone, Debug)]
pub struct ModeledSpringStabilizer {
    wobble_window: VecDeque<(StrokeSample, f64)>,
    raw_path: VecDeque<StrokeSample>,
    v_floor: f64,
    v_ceiling: f64,
    min_step_dist: f64,
    spring_k: f64,
    drag_k: f64,
    catch_up_on_end: bool,
    sim_time_ms: f64,
    last_raw_time_ms: f64,
    anchor: Option<Point>,
    pos: Option<Point>,
    vel: (f64, f64),
    last_raw: Option<StrokeSample>,
    last_emitted_time_ms: f64,
}

impl ModeledSpringStabilizer {
    pub fn new(strength: f32, zoom: f32, adjust_for_zoom: bool, catch_up_on_end: bool) -> Self {
        let s = f64::from(if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            0.55
        });
        let z = if adjust_for_zoom && zoom.is_finite() {
            f64::from(zoom.clamp(0.25, 8.0))
        } else {
            1.0
        };
        Self {
            wobble_window: VecDeque::with_capacity(MAX_WOBBLE_WINDOW_SAMPLES),
            raw_path: VecDeque::with_capacity(MAX_RAW_PATH_WINDOW),
            v_floor: WOBBLE_SPEED_FLOOR_PX_MS / z,
            v_ceiling: WOBBLE_SPEED_CEILING_PX_MS / z,
            min_step_dist: 0.25 / z,
            spring_k: 0.075 + 0.085 * (1.0 - s),
            drag_k: 0.56,
            catch_up_on_end,
            sim_time_ms: 0.0,
            last_raw_time_ms: 0.0,
            anchor: None,
            pos: None,
            vel: (0.0, 0.0),
            last_raw: None,
            last_emitted_time_ms: 0.0,
        }
    }

    pub fn push(&mut self, sample: StrokeSample, out: &mut Vec<StrokeSample>) {
        let p = sample.sanitized();
        if self.raw_path.len() >= MAX_RAW_PATH_WINDOW {
            self.raw_path.pop_front();
        }
        self.raw_path.push_back(p);

        if self.pos.is_none() {
            let t0 = if p.time_ms.is_finite() {
                p.time_ms
            } else {
                0.0
            };
            self.sim_time_ms = t0;
            self.last_raw_time_ms = p.time_ms;
            self.wobble_window.clear();
            self.wobble_window.push_back((p, t0));
            self.anchor = Some(p.point());
            self.pos = Some(p.point());
            self.vel = (0.0, 0.0);
            self.last_raw = Some(p);
            self.last_emitted_time_ms = p.time_ms;
            out.push(p);
            return;
        }

        let dt_ms = if p.time_ms > self.last_raw_time_ms {
            (p.time_ms - self.last_raw_time_ms).clamp(2.0, 120.0)
        } else {
            8.0
        };
        self.last_raw_time_ms = p.time_ms;
        self.sim_time_ms += dt_ms;

        self.wobble_window.push_back((p, self.sim_time_ms));
        while self.wobble_window.len() > 2
            && self
                .wobble_window
                .front()
                .is_some_and(|(_, t)| self.sim_time_ms - *t > WOBBLE_TIMEOUT_MS)
        {
            self.wobble_window.pop_front();
        }
        while self.wobble_window.len() > MAX_WOBBLE_WINDOW_SAMPLES {
            self.wobble_window.pop_front();
        }

        let anchor_cur = self.compute_wobble_anchor(p);
        let anchor_prev = self.anchor.unwrap_or(p.point());
        let mut cur_pos = self.pos.unwrap_or(p.point());
        let mut cur_vel = self.vel;

        let substeps = (dt_ms / MODELER_STEP_MS)
            .ceil()
            .clamp(1.0, MAX_MODELER_SUBSTEPS as f64) as usize;
        let base_t = self.last_emitted_time_ms;
        for j in 1..=substeps {
            let u = j as f64 / substeps as f64;
            let a_j = anchor_prev.lerp(anchor_cur, u);
            let vx = (1.0 - self.drag_k) * cur_vel.0 + self.spring_k * (a_j.x - cur_pos.x);
            let vy = (1.0 - self.drag_k) * cur_vel.1 + self.spring_k * (a_j.y - cur_pos.y);
            if !(vx.is_finite() && vy.is_finite()) {
                break;
            }
            cur_vel = (vx, vy);
            cur_pos = Point::new(cur_pos.x + vx, cur_pos.y + vy);

            let t_j = if p.time_ms > base_t {
                base_t + (p.time_ms - base_t) * u
            } else {
                base_t + dt_ms * u
            };
            self.emit_modeled_point(cur_pos, t_j, p, out);
        }

        self.anchor = Some(anchor_cur);
        self.pos = Some(cur_pos);
        self.vel = cur_vel;
        self.last_raw = Some(p);
    }

    pub fn finish(&mut self, out: &mut Vec<StrokeSample>) {
        if !self.catch_up_on_end {
            return;
        }
        let Some(last_raw) = self.last_raw else {
            return;
        };
        let mut cur_pos = self.pos.unwrap_or(last_raw.point());
        let mut cur_vel = self.vel;
        for _ in 0..MAX_SETTLE_STEPS {
            if cur_pos.distance(last_raw.point()) <= 0.15 {
                break;
            }
            let vx = (1.0 - self.drag_k) * cur_vel.0 + self.spring_k * (last_raw.x - cur_pos.x);
            let vy = (1.0 - self.drag_k) * cur_vel.1 + self.spring_k * (last_raw.y - cur_pos.y);
            if !(vx.is_finite() && vy.is_finite()) {
                break;
            }
            cur_vel = (vx, vy);
            cur_pos = Point::new(cur_pos.x + vx, cur_pos.y + vy);
            let next_t = self.last_emitted_time_ms + MODELER_STEP_MS;
            self.emit_modeled_point(cur_pos, next_t, last_raw, out);
        }

        if out
            .last()
            .is_none_or(|prev| prev.point().distance(last_raw.point()) >= 0.05)
        {
            let mut end_sample = last_raw;
            if let Some(&prev) = out.last()
                && end_sample.pressure < prev.pressure * 0.4
                && prev.point().distance(end_sample.point()) > 0.2
            {
                end_sample.pressure =
                    (prev.pressure * 0.65 + end_sample.pressure * 0.35).clamp(0.0, 1.0);
            }
            if end_sample.time_ms <= self.last_emitted_time_ms {
                end_sample.time_ms = self.last_emitted_time_ms + 1.0;
            }
            out.push(end_sample);
            self.last_emitted_time_ms = end_sample.time_ms;
        }
        self.pos = Some(last_raw.point());
        self.vel = (0.0, 0.0);
    }

    fn compute_wobble_anchor(&self, raw: StrokeSample) -> Point {
        if self.wobble_window.len() < 2 {
            return raw.point();
        }
        let mut sum_dist = 0.0_f64;
        let mut sum_dt = 0.0_f64;
        let mut sum_wx = 0.0_f64;
        let mut sum_wy = 0.0_f64;
        let iter_a = self.wobble_window.iter();
        let iter_b = self.wobble_window.iter().skip(1);
        for ((a, ta), (b, tb)) in iter_a.zip(iter_b) {
            let seg_dt = (*tb - *ta).max(0.5);
            sum_dist += a.point().distance(b.point());
            sum_dt += seg_dt;
            sum_wx += 0.5 * (a.x + b.x) * seg_dt;
            sum_wy += 0.5 * (a.y + b.y) * seg_dt;
        }
        if sum_dt <= 1e-9 {
            return raw.point();
        }
        let avg_pos = Point::new(sum_wx / sum_dt, sum_wy / sum_dt);
        let speed = sum_dist / sum_dt;
        let denom = (self.v_ceiling - self.v_floor).max(1e-6);
        let alpha = ((speed - self.v_floor) / denom).clamp(0.0, 1.0);
        avg_pos.lerp(raw.point(), alpha)
    }

    fn project_stylus_state(
        &self,
        pos: Point,
        time_ms: f64,
        fallback: StrokeSample,
    ) -> StrokeSample {
        let len = self.raw_path.len();
        if len == 0 {
            return StrokeSample {
                x: pos.x,
                y: pos.y,
                time_ms,
                ..fallback
            }
            .sanitized();
        }
        if len == 1 {
            return StrokeSample {
                x: pos.x,
                y: pos.y,
                time_ms,
                ..self.raw_path[0]
            }
            .sanitized();
        }
        let mut best_dist = f64::INFINITY;
        let mut best_a = self.raw_path[0];
        let mut best_b = self.raw_path[1];
        let mut best_t = 0.0_f64;
        for i in 0..(len - 1) {
            let a = self.raw_path[i];
            let b = self.raw_path[i + 1];
            let (dist, t) = distance_to_segment(pos, a.point(), b.point());
            if dist < best_dist {
                best_dist = dist;
                best_a = a;
                best_b = b;
                best_t = t;
            }
        }
        let mut projected = lerp_sample(&best_a, &best_b, best_t);
        projected.x = pos.x;
        projected.y = pos.y;
        projected.time_ms = time_ms;
        projected.sanitized()
    }

    fn emit_modeled_point(
        &mut self,
        pos: Point,
        time_ms: f64,
        fallback: StrokeSample,
        out: &mut Vec<StrokeSample>,
    ) {
        let candidate = self.project_stylus_state(pos, time_ms, fallback);
        if let Some(&last) = out.last() {
            let dist = last.point().distance(candidate.point());
            if dist < self.min_step_dist {
                if out.len() == 1
                    && candidate.pressure > last.pressure
                    && let Some(first) = out.first_mut()
                {
                    first.pressure = candidate.pressure;
                }
                return;
            }
        }
        self.last_emitted_time_ms = candidate.time_ms;
        out.push(candidate);
    }
}

/// Sliding 4-knot centripetal Catmull-Rom to C1 cubic Bézier path builder with
/// 3-tap knot pre-filtering and adaptive flatness subdivision.
#[derive(Clone, Debug)]
pub struct CurvedBezierStabilizer {
    strength: f64,
    min_knot_dist: f64,
    min_subdiv_step: f64,
    catch_up_on_end: bool,
    knots: VecDeque<StrokeSample>,
    prev_raw: Option<StrokeSample>,
    pending_raw: Option<StrokeSample>,
    last_input: Option<StrokeSample>,
    emitted_opening: bool,
}

impl CurvedBezierStabilizer {
    pub fn new(strength: f32, zoom: f32, adjust_for_zoom: bool, catch_up_on_end: bool) -> Self {
        let s = f64::from(if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            0.65
        });
        let z = if adjust_for_zoom && zoom.is_finite() {
            f64::from(zoom.clamp(0.25, 8.0))
        } else {
            1.0
        };
        Self {
            strength: s,
            min_knot_dist: 1.6 / z,
            min_subdiv_step: (0.45 / z).clamp(0.25, 2.0),
            catch_up_on_end,
            knots: VecDeque::with_capacity(5),
            prev_raw: None,
            pending_raw: None,
            last_input: None,
            emitted_opening: false,
        }
    }

    pub fn push(&mut self, sample: StrokeSample, out: &mut Vec<StrokeSample>) {
        let p = sample.sanitized();
        self.last_input = Some(p);
        if self.knots.is_empty() {
            self.knots.push_back(p);
            self.prev_raw = Some(p);
            out.push(p);
            return;
        }

        let anchor_raw = self.pending_raw.or(self.prev_raw).unwrap_or(p);
        if anchor_raw.point().distance(p.point()) < self.min_knot_dist {
            if out.len() == 1 && self.pending_raw.is_none() && p.pressure > out[0].pressure {
                out[0].pressure = p.pressure;
                if let Some(k0) = self.knots.front_mut() {
                    k0.pressure = p.pressure;
                }
                if let Some(r0) = self.prev_raw.as_mut() {
                    r0.pressure = p.pressure;
                }
            }
            return;
        }

        if let (Some(r_prev), Some(r_mid)) = (self.prev_raw, self.pending_raw) {
            let filtered = Self::three_tap_filter(r_prev, r_mid, p, self.strength);
            self.knots.push_back(filtered);
            self.prev_raw = Some(r_mid);
            self.pending_raw = Some(p);

            if !self.emitted_opening && self.knots.len() == 3 {
                let (k0, k1, k2) = (self.knots[0], self.knots[1], self.knots[2]);
                let p_before = Point::new(2.0 * k0.x - k1.x, 2.0 * k0.y - k1.y);
                self.emit_bezier_span(p_before, &k0, &k1, k2.point(), out);
                self.emitted_opening = true;
            } else if self.knots.len() >= 4 {
                let (k0, k1, k2, k3) = (self.knots[0], self.knots[1], self.knots[2], self.knots[3]);
                if !self.emitted_opening {
                    let p_before = Point::new(2.0 * k0.x - k1.x, 2.0 * k0.y - k1.y);
                    self.emit_bezier_span(p_before, &k0, &k1, k2.point(), out);
                    self.emitted_opening = true;
                }
                self.emit_bezier_span(k0.point(), &k1, &k2, k3.point(), out);
                self.knots.pop_front();
            }
        } else {
            self.pending_raw = Some(p);
        }
    }

    pub fn finish(&mut self, out: &mut Vec<StrokeSample>) {
        if let Some(pend) = self.pending_raw.take() {
            self.knots.push_back(pend);
        }
        if self.catch_up_on_end
            && let Some(last) = self.last_input
            && self
                .knots
                .back()
                .is_some_and(|b| b.point().distance(last.point()) >= 0.2)
        {
            self.knots.push_back(last);
        }

        if self.knots.len() == 2 && !self.emitted_opening {
            let (k0, k1) = (self.knots[0], self.knots[1]);
            let p0 = k0.point();
            let p1 = k1.point();
            if p0.distance(p1) >= 1e-4 {
                subdivide_cubic_segment(
                    [p0, p0.lerp(p1, 1.0 / 3.0), p0.lerp(p1, 2.0 / 3.0), p1],
                    [0.0, 1.0],
                    (&k0, &k1),
                    self.min_subdiv_step,
                    0,
                    out,
                );
            }
            self.emitted_opening = true;
        } else if self.knots.len() >= 3 {
            if !self.emitted_opening {
                let (k0, k1, k2) = (self.knots[0], self.knots[1], self.knots[2]);
                let p_before = Point::new(2.0 * k0.x - k1.x, 2.0 * k0.y - k1.y);
                self.emit_bezier_span(p_before, &k0, &k1, k2.point(), out);
                self.emitted_opening = true;
            }
            while self.knots.len() >= 4 {
                let (k0, k1, k2, k3) = (self.knots[0], self.knots[1], self.knots[2], self.knots[3]);
                self.emit_bezier_span(k0.point(), &k1, &k2, k3.point(), out);
                self.knots.pop_front();
            }
            if self.knots.len() == 3 {
                let (ka, kb, kc) = (self.knots[0], self.knots[1], self.knots[2]);
                let p_after = Point::new(2.0 * kc.x - kb.x, 2.0 * kc.y - kb.y);
                self.emit_bezier_span(ka.point(), &kb, &kc, p_after, out);
                self.knots.pop_front();
            }
        }

        if self.catch_up_on_end
            && let Some(last) = self.last_input
            && out
                .last()
                .is_some_and(|prev| prev.point().distance(last.point()) >= 0.05)
        {
            out.push(last);
        }
    }

    fn three_tap_filter(
        a: StrokeSample,
        b: StrokeSample,
        c: StrokeSample,
        strength: f64,
    ) -> StrokeSample {
        let avg_x = 0.25 * a.x + 0.50 * b.x + 0.25 * c.x;
        let avg_y = 0.25 * a.y + 0.50 * b.y + 0.25 * c.y;
        let avg_p = 0.25 * a.pressure + 0.50 * b.pressure + 0.25 * c.pressure;
        let blend = (0.85 * strength).clamp(0.0, 0.85);
        let mut filtered = b;
        filtered.x = b.x + (avg_x - b.x) * blend;
        filtered.y = b.y + (avg_y - b.y) * blend;
        filtered.pressure = (b.pressure + (avg_p - b.pressure) * (blend as f32)).clamp(0.0, 1.0);
        filtered.sanitized()
    }

    fn catmull_to_bezier_controls(p0: Point, p1: Point, p2: Point, p3: Point) -> [Point; 4] {
        let chord = p1.distance(p2);
        if chord < 1e-6 {
            return [p1, p1, p2, p2];
        }
        let d01 = p0.distance(p1).sqrt().max(1e-4);
        let d12 = chord.sqrt().max(1e-4);
        let d23 = p2.distance(p3).sqrt().max(1e-4);

        let m1x = d12 * ((p1.x - p0.x) / d01 - (p2.x - p0.x) / (d01 + d12) + (p2.x - p1.x) / d12);
        let m1y = d12 * ((p1.y - p0.y) / d01 - (p2.y - p0.y) / (d01 + d12) + (p2.y - p1.y) / d12);

        let m2x = d12 * ((p2.x - p1.x) / d12 + (p3.x - p2.x) / d23 - (p3.x - p1.x) / (d12 + d23));
        let m2y = d12 * ((p2.y - p1.y) / d12 + (p3.y - p2.y) / d23 - (p3.y - p1.y) / (d12 + d23));

        let max_handle = 0.45 * chord;
        let mut h1 = (m1x / 3.0, m1y / 3.0);
        let h1_len = h1.0.hypot(h1.1);
        if h1_len > max_handle && h1_len > 1e-9 {
            let s = max_handle / h1_len;
            h1 = (h1.0 * s, h1.1 * s);
        }
        let mut h2 = (m2x / 3.0, m2y / 3.0);
        let h2_len = h2.0.hypot(h2.1);
        if h2_len > max_handle && h2_len > 1e-9 {
            let s = max_handle / h2_len;
            h2 = (h2.0 * s, h2.1 * s);
        }
        [
            p1,
            Point::new(p1.x + h1.0, p1.y + h1.1),
            Point::new(p2.x - h2.0, p2.y - h2.1),
            p2,
        ]
    }

    fn emit_bezier_span(
        &self,
        p0: Point,
        k1: &StrokeSample,
        k2: &StrokeSample,
        p3: Point,
        out: &mut Vec<StrokeSample>,
    ) {
        let p1 = k1.point();
        let p2 = k2.point();
        if p1.distance(p2) < 1e-4 {
            return;
        }
        let ctrl = Self::catmull_to_bezier_controls(p0, p1, p2, p3);
        subdivide_cubic_segment(ctrl, [0.0, 1.0], (k1, k2), self.min_subdiv_step, 0, out);
    }
}

/// Low-latency direct segment builder with sub-pixel noise filtering and pressure interpolation.
#[derive(Clone, Debug)]
pub struct SimpleDirectStabilizer {
    min_dist: f64,
    last_emitted: Option<StrokeSample>,
    last_input: Option<StrokeSample>,
}

impl SimpleDirectStabilizer {
    pub fn new(zoom: f32, adjust_for_zoom: bool) -> Self {
        let z = if adjust_for_zoom && zoom.is_finite() {
            f64::from(zoom.clamp(0.25, 8.0))
        } else {
            1.0
        };
        Self {
            min_dist: 0.15 / z,
            last_emitted: None,
            last_input: None,
        }
    }

    pub fn push(&mut self, sample: StrokeSample, out: &mut Vec<StrokeSample>) {
        let p = sample.sanitized();
        self.last_input = Some(p);
        let Some(prev) = self.last_emitted else {
            self.last_emitted = Some(p);
            out.push(p);
            return;
        };
        let dist = prev.point().distance(p.point());
        if dist < self.min_dist {
            if out.len() == 1
                && p.pressure > prev.pressure
                && let Some(first) = out.first_mut()
            {
                first.pressure = p.pressure;
                self.last_emitted = Some(*first);
            }
            return;
        }
        let dp = (p.pressure - prev.pressure).abs();
        if dp > MAX_PAINT_PRESSURE_STEP && dist >= self.min_dist * 2.0 {
            let by_p = (dp / MAX_PAINT_PRESSURE_STEP).ceil() as usize;
            let by_d = (dist / self.min_dist).floor() as usize;
            let steps = by_p.min(by_d).clamp(1, 16);
            for i in 1..steps {
                out.push(lerp_sample(&prev, &p, i as f64 / steps as f64));
            }
        }
        self.last_emitted = Some(p);
        out.push(p);
    }

    pub fn finish(&mut self, out: &mut Vec<StrokeSample>) {
        let (Some(emitted), Some(last)) = (self.last_emitted, self.last_input) else {
            return;
        };
        if emitted.point().distance(last.point()) >= 0.05 {
            out.push(last);
            self.last_emitted = Some(last);
        } else if out.len() == 1
            && last.pressure > emitted.pressure
            && let Some(first) = out.first_mut()
        {
            first.pressure = last.pressure;
            self.last_emitted = Some(*first);
        }
    }
}

/// Unified incremental stabilizer dispatching the active [`StabilizerMode`].
#[derive(Clone, Debug)]
pub enum StrokeStabilizer {
    None,
    OneEuro(OneEuroStabilizer),
    PulledString(PulledStringStabilizer),
    Ema(EmaStabilizer),
    Gaussian(GaussianStabilizer),
    CircularBuffer(CircularBufferStabilizer),
    ModeledSpring(ModeledSpringStabilizer),
    CurvedBezier(CurvedBezierStabilizer),
    SimpleDirect(SimpleDirectStabilizer),
}

impl StrokeStabilizer {
    pub fn new(cfg: &SmoothingConfig, zoom: f32) -> Self {
        let cfg = cfg.sanitized();
        match cfg.stabilizer_mode {
            StabilizerMode::None => Self::None,
            StabilizerMode::OneEuro => Self::OneEuro(OneEuroStabilizer::new(
                cfg.stabilizer_strength,
                cfg.catch_up_on_end,
            )),
            StabilizerMode::PulledString => Self::PulledString(PulledStringStabilizer::new(
                cfg.stabilizer_strength,
                zoom,
                cfg.adjust_for_zoom,
                cfg.catch_up_on_end,
            )),
            StabilizerMode::ExponentialMovingAverage => Self::Ema(EmaStabilizer::new(
                cfg.stabilizer_strength,
                cfg.catch_up,
                cfg.catch_up_on_end,
            )),
            StabilizerMode::GaussianWindow => Self::Gaussian(GaussianStabilizer::new(
                cfg.stabilizer_strength,
                cfg.catch_up_on_end,
            )),
            StabilizerMode::CircularBuffer => {
                Self::CircularBuffer(CircularBufferStabilizer::new_with_zoom(&cfg, zoom))
            }
            StabilizerMode::ModeledSpring => Self::ModeledSpring(ModeledSpringStabilizer::new(
                cfg.stabilizer_strength,
                zoom,
                cfg.adjust_for_zoom,
                cfg.catch_up_on_end,
            )),
            StabilizerMode::CurvedBezier => Self::CurvedBezier(CurvedBezierStabilizer::new(
                cfg.stabilizer_strength,
                zoom,
                cfg.adjust_for_zoom,
                cfg.catch_up_on_end,
            )),
            StabilizerMode::SimpleDirect => {
                Self::SimpleDirect(SimpleDirectStabilizer::new(zoom, cfg.adjust_for_zoom))
            }
        }
    }

    pub fn push(&mut self, sample: StrokeSample, out: &mut Vec<StrokeSample>) {
        match self {
            Self::None => out.push(sample.sanitized()),
            Self::OneEuro(s) => s.push(sample, out),
            Self::PulledString(s) => s.push(sample, out),
            Self::Ema(s) => s.push(sample, out),
            Self::Gaussian(s) => s.push(sample, out),
            Self::CircularBuffer(s) => s.push(sample, out),
            Self::ModeledSpring(s) => s.push(sample, out),
            Self::CurvedBezier(s) => s.push(sample, out),
            Self::SimpleDirect(s) => s.push(sample, out),
        }
    }

    pub fn finish(&mut self, out: &mut Vec<StrokeSample>) {
        match self {
            Self::None => {}
            Self::OneEuro(s) => s.finish(out),
            Self::PulledString(s) => s.finish(out),
            Self::Ema(s) => s.finish(out),
            Self::Gaussian(s) => s.finish(out),
            Self::CircularBuffer(s) => s.finish(out),
            Self::ModeledSpring(s) => s.finish(out),
            Self::CurvedBezier(s) => s.finish(out),
            Self::SimpleDirect(s) => s.finish(out),
        }
    }

    pub fn string_tether(&self) -> Option<(Point, Point)> {
        match self {
            Self::PulledString(s) => s.tether(),
            _ => None,
        }
    }

    pub fn is_lagging(&self) -> bool {
        match self {
            Self::Ema(s) => s.is_lagging(),
            _ => false,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn jitter_wave() -> Vec<StrokeSample> {
        (0..40)
            .map(|i| {
                let y = if i % 2 == 0 { -6.0 } else { 6.0 };
                StrokeSample {
                    x: f64::from(i) * 5.0,
                    y: 100.0 + y,
                    pressure: 0.7,
                    time_ms: f64::from(i) * 8.0,
                    ..Default::default()
                }
            })
            .collect()
    }

    #[test]
    fn all_stabilizer_modes_reduce_high_frequency_jitter() {
        let raw = jitter_wave();
        let raw_dev: f64 = raw.iter().map(|p| (p.y - 100.0).abs()).sum();
        for mode in [
            StabilizerMode::OneEuro,
            StabilizerMode::PulledString,
            StabilizerMode::ExponentialMovingAverage,
            StabilizerMode::GaussianWindow,
            StabilizerMode::CircularBuffer,
            StabilizerMode::ModeledSpring,
            StabilizerMode::CurvedBezier,
        ] {
            let cfg = SmoothingConfig {
                stabilizer_mode: mode,
                stabilizer_strength: 0.7,
                ..Default::default()
            };
            let mut stab = StrokeStabilizer::new(&cfg, 1.0);
            let mut out = Vec::new();
            for &s in &raw {
                stab.push(s, &mut out);
            }
            stab.finish(&mut out);
            assert!(!out.is_empty());
            let dev: f64 = out.iter().map(|p| (p.y - 100.0).abs()).sum::<f64>() / out.len() as f64;
            let raw_mean = raw_dev / raw.len() as f64;
            assert!(
                dev < raw_mean,
                "{mode:?} did not smooth jitter: {dev} >= {raw_mean}"
            );
        }
    }

    #[test]
    fn pulled_string_holds_until_tether_exceeded_and_catches_up() {
        let cfg = SmoothingConfig {
            stabilizer_mode: StabilizerMode::PulledString,
            stabilizer_strength: 0.5, // 40 px
            catch_up_on_end: true,
            ..Default::default()
        };
        let mut stab = StrokeStabilizer::new(&cfg, 1.0);
        let mut out = Vec::new();
        stab.push(StrokeSample::from_xy_pressure(0.0, 0.0, 0.8), &mut out);
        stab.push(StrokeSample::from_xy_pressure(20.0, 0.0, 0.8), &mut out);
        assert_eq!(out.len(), 1, "still inside 40px tether");
        assert!(stab.string_tether().is_some());

        stab.push(StrokeSample::from_xy_pressure(60.0, 0.0, 0.8), &mut out);
        assert_eq!(out.len(), 2);
        assert!((out[1].x - 20.0).abs() < 1e-6);

        stab.finish(&mut out);
        assert!((out.last().unwrap().x - 60.0).abs() < 1e-6);
    }

    #[test]
    fn taubin_streamline_preserves_endpoints_and_circle_radius() {
        let n = 48;
        let radius = 80.0;
        let mut pts = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let theta = 2.0 * PI * (i as f64 / n as f64);
            let wobble = if i == 0 || i == n {
                0.0
            } else if i % 2 == 0 {
                4.0
            } else {
                -4.0
            };
            let r = radius + wobble;
            pts.push(StrokeSample::from_xy_pressure(
                200.0 + r * theta.cos(),
                200.0 + r * theta.sin(),
                0.8,
            ));
        }
        let faired = apply_streamline(&pts, 0.8);
        assert_eq!(
            faired.first().unwrap().point(),
            pts.first().unwrap().point()
        );
        assert_eq!(faired.last().unwrap().point(), pts.last().unwrap().point());

        let mean_r: f64 = faired
            .iter()
            .map(|p| p.point().distance(Point::new(200.0, 200.0)))
            .sum::<f64>()
            / faired.len() as f64;
        assert!(
            (mean_r - radius).abs() < 3.0,
            "Taubin shrunk loop radius to {mean_r}"
        );
    }

    #[test]
    fn arithmetic_mean_buffer_10_prefills_and_finalizes_smoothly_to_endpoint() {
        let cfg = SmoothingConfig::default();
        let mut stab = StrokeStabilizer::new(&cfg, 1.0);
        let mut out = Vec::new();
        stab.push(StrokeSample::from_xy_pressure(0.0, 0.0, 0.5), &mut out);
        stab.push(StrokeSample::from_xy_pressure(20.0, 0.0, 0.6), &mut out);
        // Buffer size 10 prefilled with 0.0 => 9 copies of 0.0 + 1 copy of 20.0 => mean x = 2.0.
        assert_eq!(out.len(), 2);
        assert!(
            (out[1].x - 2.0).abs() < 1e-6,
            "expected 2.0, got {}",
            out[1].x
        );

        for i in 2..=30 {
            stab.push(
                StrokeSample::from_xy_pressure(f64::from(i) * 10.0, 0.0, 0.7),
                &mut out,
            );
        }
        let pre_finish_x = out.last().unwrap().x;
        assert!(pre_finish_x < 300.0, "expected buffer lag before finalize");

        stab.finish(&mut out);
        let last = out.last().unwrap();
        assert!(
            (last.x - 300.0).abs() < 1e-6,
            "finalize should reach endpoint 300.0, got {}",
            last.x
        );
        assert!(out.len() > 31, "finalize spline should subdivide tail gap");
    }

    #[test]
    fn deadzone_and_velocity_gaussian_modes_stabilize_without_panicking() {
        let raw = jitter_wave();
        for (averaging, preprocessor) in [
            (
                BufferAveraging::ArithmeticMean,
                BufferPreprocessor::Deadzone,
            ),
            (BufferAveraging::VelocityGaussian, BufferPreprocessor::None),
            (
                BufferAveraging::VelocityGaussian,
                BufferPreprocessor::Inertia,
            ),
            (BufferAveraging::None, BufferPreprocessor::Deadzone),
        ] {
            let cfg = SmoothingConfig {
                stabilizer_mode: StabilizerMode::CircularBuffer,
                buffer_averaging: averaging,
                buffer_preprocessor: preprocessor,
                buffer_deadzone_radius: 3.0,
                buffer_cusp_detection: true,
                buffer_finalize_stroke: true,
                ..Default::default()
            };
            let mut stab = StrokeStabilizer::new(&cfg, 1.0);
            let mut out = Vec::new();
            for &s in &raw {
                stab.push(s, &mut out);
            }
            stab.finish(&mut out);
            assert!(!out.is_empty());
            assert!(
                out.iter()
                    .all(|p| p.x.is_finite() && p.y.is_finite() && p.pressure.is_finite())
            );
        }

        // Hostile non-finite / edge parameters must stay safe.
        let hostile_cfg = SmoothingConfig {
            stabilizer_mode: StabilizerMode::CircularBuffer,
            buffer_size: 0,
            buffer_sigma: f32::NAN,
            buffer_deadzone_radius: 0.0,
            buffer_drag: 2.0,
            buffer_mass: -5.0,
            ..Default::default()
        };
        let mut stab = StrokeStabilizer::new(&hostile_cfg, 1.0);
        let mut out = Vec::new();
        stab.push(
            StrokeSample::from_xy_pressure(f64::NAN, 10.0, 0.5),
            &mut out,
        );
        stab.push(
            StrokeSample::from_xy_pressure(50.0, f64::INFINITY, 0.5),
            &mut out,
        );
        stab.finish(&mut out);
        assert!(
            out.iter()
                .all(|p| p.x.is_finite() && p.y.is_finite() && p.pressure.is_finite())
        );
    }

    #[test]
    fn arithmetic_buffer_filters_micro_motion_updates_first_pressure_and_avoids_reversal_loop() {
        let cfg = SmoothingConfig::default();
        let mut stab = StrokeStabilizer::new(&cfg, 1.0);
        let mut out = Vec::new();
        stab.push(StrokeSample::from_xy_pressure(10.0, 10.0, 0.2), &mut out);
        // Mean shift = 2.0 / 20 = 0.1 < 0.3 threshold -> pressure updates touchdown point in place.
        stab.push(StrokeSample::from_xy_pressure(12.0, 10.0, 0.8), &mut out);
        assert_eq!(out.len(), 1);
        assert!(out[0].pressure > 0.2);

        // Move rightward then sharply reverse left at the end without overshooting past last painted.
        let mut stab2 = StrokeStabilizer::new(&cfg, 1.0);
        let mut out2 = Vec::new();
        for i in 0..15 {
            stab2.push(
                StrokeSample::from_xy_pressure(f64::from(i) * 10.0, 50.0, 0.6),
                &mut out2,
            );
        }
        let max_before_finish = out2.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
        // Sudden reversal back behind the current painted tail.
        for x in [120.0, 80.0, 40.0] {
            stab2.push(StrokeSample::from_xy_pressure(x, 50.0, 0.6), &mut out2);
        }
        let peak_x = out2.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
        stab2.finish(&mut out2);
        let max_after = out2.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
        assert!(max_after <= peak_x + 1e-4 && peak_x >= max_before_finish);
        assert!((out2.last().unwrap().x - 40.0).abs() < 1e-6);
    }

    #[test]
    fn none_averaging_and_none_preprocessor_passes_through_without_finalizing() {
        let cfg = SmoothingConfig {
            stabilizer_mode: StabilizerMode::CircularBuffer,
            buffer_averaging: BufferAveraging::None,
            buffer_preprocessor: BufferPreprocessor::None,
            buffer_finalize_stroke: true,
            ..Default::default()
        };
        let mut stab = StrokeStabilizer::new(&cfg, 1.0);
        let mut out = Vec::new();
        stab.push(StrokeSample::from_xy_pressure(0.0, 0.0, 0.5), &mut out);
        stab.push(StrokeSample::from_xy_pressure(10.0, 5.0, 0.5), &mut out);
        stab.push(StrokeSample::from_xy_pressure(25.0, 5.0, 0.5), &mut out);
        let before_finish = out.len();
        stab.finish(&mut out);
        assert_eq!(out.len(), before_finish);
        assert!((out.last().unwrap().x - 25.0).abs() < 1e-6);
    }

    #[test]
    fn inertia_integrates_smoothly_even_across_sub_threshold_steps() {
        let cfg = SmoothingConfig {
            stabilizer_mode: StabilizerMode::CircularBuffer,
            buffer_averaging: BufferAveraging::None,
            buffer_preprocessor: BufferPreprocessor::Inertia,
            buffer_drag: 0.4,
            buffer_mass: 10.0,
            buffer_finalize_stroke: false,
            ..Default::default()
        };
        let mut stab = StrokeStabilizer::new(&cfg, 1.0);
        let mut out = Vec::new();
        stab.push(StrokeSample::from_xy_pressure(0.0, 0.0, 0.7), &mut out);
        // Step by 1.0 each event: initial spring step is 1.0/10 = 0.1 (< 0.3 threshold),
        // yet last_stabilized must keep integrating so position advances smoothly.
        for i in 1..=20 {
            stab.push(
                StrokeSample::from_xy_pressure(f64::from(i), 0.0, 0.7),
                &mut out,
            );
        }
        assert!(
            out.len() > 2,
            "inertia should accumulate velocity and advance"
        );
        assert!(out.last().unwrap().x > 5.0 && out.last().unwrap().x < 20.0);
    }

    #[test]
    fn modeled_spring_curved_bezier_and_simple_direct_converge_and_resist_hostile_inputs() {
        use drawcraft_doc::StabilizationPreset;

        for &preset in StabilizationPreset::ALL {
            let cfg = preset.to_smoothing_config();
            let mut stab = StrokeStabilizer::new(&cfg, 1.0);
            let mut out = Vec::new();
            for i in 0..24 {
                let t = f64::from(i) / 23.0;
                let s = StrokeSample {
                    x: 10.0 + t * 220.0,
                    y: 120.0 + (t * std::f64::consts::TAU).sin() * 25.0,
                    pressure: 0.25 + 0.65 * (t as f32),
                    tilt_x: 10.0,
                    tilt_y: -5.0,
                    rotation: 0.0,
                    time_ms: f64::from(i) * 12.0,
                };
                stab.push(s, &mut out);
            }
            stab.finish(&mut out);
            assert!(out.len() >= 10, "{preset:?} produced too few samples");
            let first = out.first().unwrap();
            let last = out.last().unwrap();
            assert!((first.x - 10.0).abs() < 1e-4 && (first.y - 120.0).abs() < 1e-4);
            assert!((last.x - 230.0).abs() < 1e-3 && (last.y - 120.0).abs() < 1e-3);
            assert!(last.pressure > 0.75);
            assert!(
                out.iter()
                    .all(|p| p.x.is_finite() && p.y.is_finite() && p.pressure.is_finite())
            );

            // Short 2-point stroke and hostile NaN/Inf resilience.
            let mut short_stab = StrokeStabilizer::new(&cfg, f32::NAN);
            let mut short_out = Vec::new();
            short_stab.push(
                StrokeSample::from_xy_pressure(f64::NAN, 15.0, 0.4),
                &mut short_out,
            );
            short_stab.push(
                StrokeSample::from_xy_pressure(45.0, f64::INFINITY, 0.8),
                &mut short_out,
            );
            short_stab.finish(&mut short_out);
            assert!(!short_out.is_empty());
            assert!(
                short_out
                    .iter()
                    .all(|p| p.x.is_finite() && p.y.is_finite() && p.pressure.is_finite())
            );
        }

        // Verify CurvedBezier handles 3-point strokes smoothly and idempotently on double finish().
        let curved_cfg = StabilizationPreset::CurvedBezier.to_smoothing_config();
        let mut c_stab = StrokeStabilizer::new(&curved_cfg, 1.0);
        let mut c_out = Vec::new();
        c_stab.push(StrokeSample::from_xy_pressure(0.0, 0.0, 0.5), &mut c_out);
        c_stab.push(StrokeSample::from_xy_pressure(30.0, 20.0, 0.6), &mut c_out);
        c_stab.push(StrokeSample::from_xy_pressure(60.0, 0.0, 0.7), &mut c_out);
        c_stab.finish(&mut c_out);
        let count_after_first_finish = c_out.len();
        c_stab.finish(&mut c_out);
        assert_eq!(c_out.len(), count_after_first_finish);
        assert!((c_out.last().unwrap().x - 60.0).abs() < 1e-6);

        // Verify SimpleDirect preserves exact polyline vertices without spline rounding.
        let direct_cfg = StabilizationPreset::SimpleDirect.to_smoothing_config();
        let raw_corner = vec![
            StrokeSample::from_xy_pressure(0.0, 0.0, 0.6),
            StrokeSample::from_xy_pressure(50.0, 0.0, 0.6),
            StrokeSample::from_xy_pressure(50.0, 50.0, 0.6),
        ];
        let direct_out = crate::smooth_stroke_samples(&raw_corner, &direct_cfg, 1.0);
        assert!(
            direct_out
                .iter()
                .any(|p| (p.x - 50.0).abs() < 1e-6 && p.y.abs() < 1e-6),
            "SimpleDirect should preserve sharp corner vertex (50, 0)"
        );
    }
}
