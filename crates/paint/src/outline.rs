//! Anti-aliased variable-width triangle ribbon meshing and closed contour extraction.

use std::f64::consts::PI;

use drawcraft_doc::{PenKind, PenSettings};
use drawcraft_geom::{Point, Vec2};

use crate::dynamics::SpineNode;

const MIN_STEP_PX: f64 = 1e-2;
const MIN_RADIUS_PX: f64 = 1e-2;
const MAX_PRUNE_SIN_ALPHA: f64 = 0.94;
const MAX_SIN_ALPHA: f64 = 0.96;
const MIN_ROUNDNESS: f64 = 0.05;
const CLEAN_MITER_MIN_DOT: f64 = -0.75;
const CLEAN_MITER_MAX_RADIUS_MULT: f64 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RibbonVertex {
    pub pos: [f32; 2],
    pub rgba: [f32; 4],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RibbonMesh {
    pub vertices: Vec<RibbonVertex>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScalableRibbonVertex {
    pub pos: [f32; 2],
    pub aa_offset: [f32; 2],
    pub rgba: [f32; 4],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScalableRibbonMesh {
    pub vertices: Vec<ScalableRibbonVertex>,
    pub indices: Vec<u32>,
}

#[inline]
fn elliptical_half_width_parts(basis: NibBasis, radius: f64, n: Vec2) -> f64 {
    let b = radius * basis.roundness;
    (radius * n.dot(basis.axis)).hypot(b * n.dot(basis.ortho))
}

#[inline]
fn ellipse_contact_offset_parts(basis: NibBasis, radius: f64, n: Vec2) -> Vec2 {
    let b = radius * basis.roundness;
    let nx = n.dot(basis.axis);
    let ny = n.dot(basis.ortho);
    let h = (radius * nx).hypot(b * ny);
    if !h.is_finite() || h <= 1e-12 {
        return Vec2::ZERO;
    }
    let cx = (radius * radius * nx) / h;
    let cy = (b * b * ny) / h;
    basis.axis.scale(cx) + basis.ortho.scale(cy)
}

/// Support function (half-width) of an elliptical nib along `normal`.
#[inline]
pub fn elliptical_half_width(node: &SpineNode, normal: Vec2) -> f64 {
    let Some(n) = normal.normalized() else {
        return 0.0;
    };
    let basis = NibBasis::from_node(node);
    let radius = sanitize_radius(f64::from(node.radius));
    elliptical_half_width_parts(basis, radius, n)
}

/// Exact boundary offset from the nib center whose outward normal aligns with `normal`.
#[inline]
pub fn ellipse_contact_offset(node: &SpineNode, normal: Vec2) -> Vec2 {
    let Some(n) = normal.normalized() else {
        return Vec2::ZERO;
    };
    let basis = NibBasis::from_node(node);
    let radius = sanitize_radius(f64::from(node.radius));
    ellipse_contact_offset_parts(basis, radius, n)
}

#[derive(Clone, Copy, Debug)]
struct NibBasis {
    axis: Vec2,
    ortho: Vec2,
    roundness: f64,
}

impl NibBasis {
    #[inline]
    fn from_node(node: &SpineNode) -> Self {
        let angle = if node.angle_rad.is_finite() {
            f64::from(node.angle_rad)
        } else {
            0.0
        };
        let roundness = if node.roundness.is_finite() {
            f64::from(node.roundness).clamp(MIN_ROUNDNESS, 1.0)
        } else {
            1.0
        };
        let (sin, cos) = angle.sin_cos();
        Self {
            axis: Vec2::new(cos, sin),
            ortho: Vec2::new(-sin, cos),
            roundness,
        }
    }

    #[inline]
    fn matches(self, other: Self) -> bool {
        // Treat smoothly changing brush tilt as one local nib frame. Switching to
        // world-space bitangents for tiny orientation changes creates false turns
        // at every sample and overlapping triangles along otherwise straight strokes.
        (self.roundness - other.roundness).abs() < 0.01 && self.axis.dot(other.axis) > 0.999
    }

    #[inline]
    fn to_circle(self, v: Vec2) -> Vec2 {
        Vec2::new(v.dot(self.axis), v.dot(self.ortho) / self.roundness)
    }

    #[inline]
    fn contact_offset_from_circle(self, u_c: Vec2, radius: f64) -> Vec2 {
        self.axis.scale(u_c.x * radius) + self.ortho.scale(u_c.y * self.roundness * radius)
    }

    #[inline]
    fn world_normal_from_circle(self, u_c: Vec2) -> Vec2 {
        (self.axis.scale(u_c.x) + self.ortho.scale(u_c.y / self.roundness))
            .normalized()
            .unwrap_or(u_c)
    }
}

#[derive(Clone, Copy)]
struct CleanNode {
    point: Point,
    radius: f64,
    basis: NibBasis,
    alpha: f32,
    rgb: [f32; 3],
}

impl CleanNode {
    #[inline]
    fn contact_and_normal_at_circle_angle(self, phi_c: f64) -> (Point, Vec2) {
        let (sin, cos) = phi_c.sin_cos();
        let u_c = Vec2::new(cos, sin);
        let offset = self.basis.contact_offset_from_circle(u_c, self.radius);
        let normal = self.basis.world_normal_from_circle(u_c);
        (offset_point(self.point, offset), normal)
    }

    #[inline]
    fn contact_and_normal_at_world_angle(self, phi_w: f64) -> (Point, Vec2) {
        let (sin, cos) = phi_w.sin_cos();
        let normal = Vec2::new(cos, sin);
        let offset = ellipse_contact_offset_parts(self.basis, self.radius, normal);
        (offset_point(self.point, offset), normal)
    }
}

#[derive(Clone, Copy)]
struct SegmentFrame {
    l0: Point,
    l1: Point,
    r0: Point,
    r1: Point,
    n_left: Vec2,
    n_right: Vec2,
    t_c: Vec2,
    u_left_c: Vec2,
    u_right_c: Vec2,
    alpha: f64,
    uniform_nib: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy)]
struct JointArc {
    side: Side,
    radius: f64,
    start_phi: f64,
    delta: f64,
    use_circle_space: bool,
}

#[derive(Clone, Copy)]
enum JointKind {
    Straight,
    CleanMiter {
        outer_arc: JointArc,
        inner_side: Side,
        miter_point: Point,
        miter_normal: Vec2,
    },
    CapJoin {
        left_arc: Option<JointArc>,
        right_arc: Option<JointArc>,
    },
}

#[inline]
fn sanitize_radius(r: f64) -> f64 {
    if r.is_finite() {
        r.max(MIN_RADIUS_PX)
    } else {
        MIN_RADIUS_PX
    }
}

#[inline]
fn offset_point(p: Point, v: Vec2) -> Point {
    Point::new(p.x + v.x, p.y + v.y)
}

#[inline]
fn cross2(a: Vec2, b: Vec2) -> f64 {
    a.x * b.y - a.y * b.x
}

#[inline]
fn node_span_and_dr(a: CleanNode, b: CleanNode) -> (f64, f64) {
    let seg_vec = Vec2::from_points(a.point, b.point);
    if a.basis.matches(b.basis) {
        (a.basis.to_circle(seg_vec).len(), b.radius - a.radius)
    } else {
        let dist = seg_vec.len();
        let Some(t) = seg_vec.normalized() else {
            return (0.0, b.radius - a.radius);
        };
        let n = t.perp();
        let h_a = elliptical_half_width_parts(a.basis, a.radius, n);
        let h_b = elliptical_half_width_parts(b.basis, b.radius, n);
        (dist, h_b - h_a)
    }
}

fn clean_spine_nodes(nodes: &[SpineNode]) -> Vec<CleanNode> {
    let mut deduped: Vec<CleanNode> = Vec::with_capacity(nodes.len());
    for node in nodes {
        if !node.point.is_finite() {
            continue;
        }
        let clean = CleanNode {
            point: node.point,
            radius: sanitize_radius(f64::from(node.radius)),
            basis: NibBasis::from_node(node),
            alpha: if node.alpha.is_finite() {
                node.alpha.clamp(0.0, 1.0)
            } else {
                0.0
            },
            rgb: {
                let color = node.color.sanitized();
                [color.r, color.g, color.b]
            },
        };
        if let Some(last) = deduped.last_mut()
            && last.point.distance(clean.point) < MIN_STEP_PX
        {
            last.radius = last.radius.max(clean.radius);
            last.alpha = last.alpha.max(clean.alpha);
            last.rgb = clean.rgb;
            continue;
        }
        deduped.push(clean);
    }

    // Remove disks completely swallowed inside an adjacent neighbor's nib ellipse so every
    // consecutive pair has real external bitangents without inverted cones or gap cracks.
    let mut cleaned: Vec<CleanNode> = Vec::with_capacity(deduped.len());
    for cur in deduped {
        while let Some(&prev) = cleaned.last() {
            let (dist, dr) = node_span_and_dr(prev, cur);
            if dr >= MAX_PRUNE_SIN_ALPHA * dist {
                cleaned.pop();
            } else {
                break;
            }
        }
        if let Some(&prev) = cleaned.last() {
            let (dist, dr) = node_span_and_dr(prev, cur);
            if -dr >= MAX_PRUNE_SIN_ALPHA * dist {
                continue;
            }
        }
        cleaned.push(cur);
    }
    cleaned
}

fn segment_frame(a: CleanNode, b: CleanNode) -> Option<SegmentFrame> {
    let seg_vec = Vec2::from_points(a.point, b.point);
    let len = seg_vec.len();
    if !len.is_finite() || len < MIN_STEP_PX {
        return None;
    }
    let uniform_nib = a.basis.matches(b.basis);
    if uniform_nib {
        let d_c = a.basis.to_circle(seg_vec);
        let len_c = d_c.len();
        if !len_c.is_finite() || len_c <= 1e-6 {
            return None;
        }
        let t_c = d_c.scale(1.0 / len_c);
        let n_c = t_c.perp();
        let sin_alpha = ((a.radius - b.radius) / len_c).clamp(-MAX_SIN_ALPHA, MAX_SIN_ALPHA);
        let cos_alpha = (1.0 - sin_alpha * sin_alpha).max(0.0).sqrt();
        let u_left_c = n_c.scale(cos_alpha) + t_c.scale(sin_alpha);
        let u_right_c = n_c.scale(-cos_alpha) + t_c.scale(sin_alpha);
        Some(SegmentFrame {
            l0: offset_point(
                a.point,
                a.basis.contact_offset_from_circle(u_left_c, a.radius),
            ),
            l1: offset_point(
                b.point,
                b.basis.contact_offset_from_circle(u_left_c, b.radius),
            ),
            r0: offset_point(
                a.point,
                a.basis.contact_offset_from_circle(u_right_c, a.radius),
            ),
            r1: offset_point(
                b.point,
                b.basis.contact_offset_from_circle(u_right_c, b.radius),
            ),
            n_left: a.basis.world_normal_from_circle(u_left_c),
            n_right: a.basis.world_normal_from_circle(u_right_c),
            t_c,
            u_left_c,
            u_right_c,
            alpha: sin_alpha.asin(),
            uniform_nib: true,
        })
    } else {
        let t = seg_vec.scale(1.0 / len);
        let n = t.perp();
        let h_a = elliptical_half_width_parts(a.basis, a.radius, n);
        let h_b = elliptical_half_width_parts(b.basis, b.radius, n);
        let sin_alpha = ((h_a - h_b) / len).clamp(-MAX_SIN_ALPHA, MAX_SIN_ALPHA);
        let cos_alpha = (1.0 - sin_alpha * sin_alpha).max(0.0).sqrt();
        let u_left = n.scale(cos_alpha) + t.scale(sin_alpha);
        let u_right = n.scale(-cos_alpha) + t.scale(sin_alpha);
        Some(SegmentFrame {
            l0: offset_point(
                a.point,
                ellipse_contact_offset_parts(a.basis, a.radius, u_left),
            ),
            l1: offset_point(
                b.point,
                ellipse_contact_offset_parts(b.basis, b.radius, u_left),
            ),
            r0: offset_point(
                a.point,
                ellipse_contact_offset_parts(a.basis, a.radius, u_right),
            ),
            r1: offset_point(
                b.point,
                ellipse_contact_offset_parts(b.basis, b.radius, u_right),
            ),
            n_left: u_left,
            n_right: u_right,
            t_c: t,
            u_left_c: u_left,
            u_right_c: u_right,
            alpha: sin_alpha.asin(),
            uniform_nib: false,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn intersect_inner_edges(
    a0: Point,
    a1: Point,
    b0: Point,
    b1: Point,
    joint_center: Point,
    joint_radius: f64,
    inner_norm_in: Vec2,
    inner_norm_out: Vec2,
) -> Option<(Point, Vec2)> {
    let d_a = Vec2::from_points(a0, a1);
    let d_b = Vec2::from_points(b0, b1);
    let len_a = d_a.len();
    let len_b = d_b.len();
    if len_a < MIN_STEP_PX || len_b < MIN_STEP_PX {
        return None;
    }
    let denom = d_a.y * d_b.x - d_a.x * d_b.y;
    if denom.abs() <= 1e-4 * len_a * len_b {
        return None;
    }
    let w = Vec2::from_points(a0, b0);
    let s = (w.y * d_b.x - w.x * d_b.y) / denom;
    let u = (d_a.x * w.y - d_a.y * w.x) / denom;
    if !(0.52..=1.00).contains(&s) || !(0.00..=0.48).contains(&u) {
        return None;
    }
    let x = Point::new(a0.x + s * d_a.x, a0.y + s * d_a.y);
    if !x.is_finite() || x.distance(joint_center) > joint_radius * CLEAN_MITER_MAX_RADIUS_MULT {
        return None;
    }
    let from_center = Vec2::from_points(joint_center, x);
    if from_center.dot(inner_norm_in) <= 0.0 || from_center.dot(inner_norm_out) <= 0.0 {
        return None;
    }
    let avg = (inner_norm_in + inner_norm_out)
        .normalized()
        .unwrap_or(inner_norm_in);
    let miter_scale = (1.0 / avg.dot(inner_norm_in).max(0.55)).min(1.65);
    Some((x, avg.scale(miter_scale)))
}

fn make_arc(
    side: Side,
    node: CleanNode,
    in_seg: &SegmentFrame,
    delta: f64,
    use_circle_space: bool,
) -> JointArc {
    let start_dir = match (use_circle_space, side) {
        (true, Side::Left) => in_seg.u_left_c,
        (true, Side::Right) => in_seg.u_right_c,
        (false, Side::Left) => in_seg.n_left,
        (false, Side::Right) => in_seg.n_right,
    };
    JointArc {
        side,
        radius: node.radius,
        start_phi: start_dir.angle_rad(),
        delta,
        use_circle_space,
    }
}

fn evaluate_joint(
    node: CleanNode,
    in_seg: &mut SegmentFrame,
    out_seg: &mut SegmentFrame,
) -> JointKind {
    let use_circle_space = in_seg.uniform_nib && out_seg.uniform_nib;
    let (dot, theta_spine) = if use_circle_space {
        let d = in_seg.t_c.dot(out_seg.t_c).clamp(-1.0, 1.0);
        let c = cross2(in_seg.t_c, out_seg.t_c);
        (d, c.atan2(d))
    } else {
        let t_in = in_seg.n_right.perp();
        let t_out = out_seg.n_right.perp();
        let d = t_in.dot(t_out).clamp(-1.0, 1.0);
        let c = cross2(t_in, t_out);
        (d, c.atan2(d))
    };

    let d_alpha = out_seg.alpha - in_seg.alpha;
    let delta_left = theta_spine - d_alpha;
    let delta_right = theta_spine + d_alpha;
    let left_convex = delta_left < -1e-3;
    let right_convex = delta_right > 1e-3;

    if !left_convex && !right_convex && delta_left.abs() <= 1e-3 && delta_right.abs() <= 1e-3 {
        return JointKind::Straight;
    }

    if left_convex && !right_convex {
        let outer_arc = make_arc(Side::Left, node, in_seg, delta_left, use_circle_space);
        if dot > CLEAN_MITER_MIN_DOT
            && let Some((miter_point, miter_normal)) = intersect_inner_edges(
                in_seg.r0,
                in_seg.r1,
                out_seg.r0,
                out_seg.r1,
                node.point,
                node.radius,
                in_seg.n_right,
                out_seg.n_right,
            )
            && cross2(
                Vec2::from_points(miter_point, in_seg.l1),
                Vec2::from_points(miter_point, out_seg.l0),
            ) < -1e-6
        {
            in_seg.r1 = miter_point;
            out_seg.r0 = miter_point;
            return JointKind::CleanMiter {
                outer_arc,
                inner_side: Side::Right,
                miter_point,
                miter_normal,
            };
        }
        return JointKind::CapJoin {
            left_arc: Some(outer_arc),
            right_arc: None,
        };
    }

    if right_convex && !left_convex {
        let outer_arc = make_arc(Side::Right, node, in_seg, delta_right, use_circle_space);
        if dot > CLEAN_MITER_MIN_DOT
            && let Some((miter_point, miter_normal)) = intersect_inner_edges(
                in_seg.l0,
                in_seg.l1,
                out_seg.l0,
                out_seg.l1,
                node.point,
                node.radius,
                in_seg.n_left,
                out_seg.n_left,
            )
            && cross2(
                Vec2::from_points(miter_point, in_seg.r1),
                Vec2::from_points(miter_point, out_seg.r0),
            ) > 1e-6
        {
            in_seg.l1 = miter_point;
            out_seg.l0 = miter_point;
            return JointKind::CleanMiter {
                outer_arc,
                inner_side: Side::Left,
                miter_point,
                miter_normal,
            };
        }
        return JointKind::CapJoin {
            left_arc: None,
            right_arc: Some(outer_arc),
        };
    }

    let left_arc =
        left_convex.then(|| make_arc(Side::Left, node, in_seg, delta_left, use_circle_space));
    let right_arc =
        right_convex.then(|| make_arc(Side::Right, node, in_seg, delta_right, use_circle_space));
    JointKind::CapJoin {
        left_arc,
        right_arc,
    }
}

fn prepare_segments(cleaned: &[CleanNode]) -> (Vec<SegmentFrame>, Vec<JointKind>) {
    let seg_count = cleaned.len().saturating_sub(1);
    let mut frames = Vec::with_capacity(seg_count);
    for i in 0..seg_count {
        if let Some(frame) = segment_frame(cleaned[i], cleaned[i + 1]) {
            frames.push(frame);
        }
    }
    let actual = frames.len();
    let mut joints = Vec::with_capacity(actual.saturating_sub(1));
    if actual >= 2 {
        for i in 0..(actual - 1) {
            let (left, right) = frames.split_at_mut(i + 1);
            let node = cleaned[(i + 1).min(cleaned.len() - 1)];
            let joint = evaluate_joint(node, &mut left[i], &mut right[0]);
            joints.push(joint);
        }
    }
    (frames, joints)
}

#[inline]
fn arc_step_count(radius: f64, sweep_rad: f64, for_contour: bool) -> usize {
    let sweep = sweep_rad.abs();
    if sweep < 1e-3 {
        return 0;
    }
    if for_contour {
        ((sweep / (PI / 8.0)).ceil() as usize).clamp(2, 12)
    } else {
        let by_angle = (sweep / (PI / 10.0)).ceil() as usize;
        let by_arc = (sweep * radius.max(1.0) / 1.5).ceil() as usize;
        by_angle.max(by_arc).clamp(2, 20)
    }
}

fn sample_arc_points(
    node: CleanNode,
    start_phi: f64,
    delta: f64,
    steps: usize,
    use_circle_space: bool,
    include_first: bool,
    include_last: bool,
) -> Vec<(Point, Vec2)> {
    let mut pts = Vec::with_capacity(steps + 1);
    if steps == 0 {
        return pts;
    }
    let start_k = if include_first { 0 } else { 1 };
    let end_k = if include_last {
        steps
    } else {
        steps.saturating_sub(1)
    };
    if start_k > end_k {
        return pts;
    }
    for k in start_k..=end_k {
        let t = k as f64 / steps as f64;
        let phi = start_phi + delta * t;
        let sample = if use_circle_space {
            node.contact_and_normal_at_circle_angle(phi)
        } else {
            node.contact_and_normal_at_world_angle(phi)
        };
        pts.push(sample);
    }
    pts
}

fn push_contour_point(out: &mut Vec<Point>, p: Point) {
    if !p.is_finite() {
        return;
    }
    if let Some(&prev) = out.last()
        && prev.distance(p) < 1e-4
    {
        return;
    }
    out.push(p);
}

/// Extract a closed, self-consistent outer contour polygon (round joins, round caps,
/// common bitangents, and bounded inner creases) for vector SVG/PDF stroke export.
pub fn build_stroke_contour(nodes: &[SpineNode]) -> Vec<Point> {
    build_stroke_contour_with_flat_caps(nodes, false)
}

/// Extracts a stroke contour, optionally cutting the two endpoints flat like a marker nib.
pub fn build_stroke_contour_with_flat_caps(nodes: &[SpineNode], flat_caps: bool) -> Vec<Point> {
    let cleaned = clean_spine_nodes(nodes);
    if cleaned.is_empty() {
        return Vec::new();
    }
    if cleaned.len() == 1 {
        let c = cleaned[0];
        let steps = 16usize;
        let mut pts = Vec::with_capacity(steps);
        for i in 0..steps {
            let phi = (i as f64 / steps as f64) * (2.0 * PI);
            pts.push(c.contact_and_normal_at_circle_angle(phi).0);
        }
        return pts;
    }

    let (frames, joints) = prepare_segments(&cleaned);
    if frames.is_empty() {
        return Vec::new();
    }

    let m = frames.len();
    let mut contour = Vec::with_capacity(m * 4 + 24);

    // The cap runs from right to left; a flat marker cut closes directly across the spine.
    let first = &frames[0];
    let first_node = cleaned[0];
    if flat_caps {
        push_contour_point(&mut contour, first.r0);
        push_contour_point(&mut contour, first.l0);
    } else {
        let start_phi = if first.uniform_nib {
            first.u_right_c.angle_rad()
        } else {
            first.n_right.angle_rad()
        };
        let start_delta = -(PI + 2.0 * first.alpha);
        let start_steps = arc_step_count(first_node.radius, start_delta, true).max(4);
        for (p, _) in sample_arc_points(
            first_node,
            start_phi,
            start_delta,
            start_steps,
            first.uniform_nib,
            true,
            true,
        ) {
            push_contour_point(&mut contour, p);
        }
    }

    // Forward walk along the left boundary + joints.
    for i in 0..m {
        push_contour_point(&mut contour, frames[i].l0);
        push_contour_point(&mut contour, frames[i].l1);
        if i + 1 < m {
            match joints[i] {
                JointKind::Straight => {}
                JointKind::CleanMiter { outer_arc, .. } if outer_arc.side == Side::Left => {
                    let jnode = cleaned[i + 1];
                    let steps = arc_step_count(outer_arc.radius, outer_arc.delta, true);
                    for (p, _) in sample_arc_points(
                        jnode,
                        outer_arc.start_phi,
                        outer_arc.delta,
                        steps,
                        outer_arc.use_circle_space,
                        false,
                        false,
                    ) {
                        push_contour_point(&mut contour, p);
                    }
                }
                JointKind::CapJoin {
                    left_arc: Some(arc),
                    ..
                } => {
                    let jnode = cleaned[i + 1];
                    let steps = arc_step_count(arc.radius, arc.delta, true);
                    for (p, _) in sample_arc_points(
                        jnode,
                        arc.start_phi,
                        arc.delta,
                        steps,
                        arc.use_circle_space,
                        false,
                        false,
                    ) {
                        push_contour_point(&mut contour, p);
                    }
                }
                JointKind::CapJoin { left_arc: None, .. } => {
                    // Inner crease without a clean miter: fold strictly through the joint center.
                    push_contour_point(&mut contour, cleaned[i + 1].point);
                }
                _ => {}
            }
        }
    }

    // The end cap runs from left to right, matching the start cap style.
    let last = &frames[m - 1];
    let last_node = cleaned[cleaned.len() - 1];
    if flat_caps {
        push_contour_point(&mut contour, last.l1);
        push_contour_point(&mut contour, last.r1);
    } else {
        let end_phi = if last.uniform_nib {
            last.u_left_c.angle_rad()
        } else {
            last.n_left.angle_rad()
        };
        let end_delta = -(PI - 2.0 * last.alpha);
        let end_steps = arc_step_count(last_node.radius, end_delta, true).max(4);
        for (p, _) in sample_arc_points(
            last_node,
            end_phi,
            end_delta,
            end_steps,
            last.uniform_nib,
            false,
            true,
        ) {
            push_contour_point(&mut contour, p);
        }
    }

    // Backward walk along the right boundary + joints.
    for i in (0..m).rev() {
        push_contour_point(&mut contour, frames[i].r1);
        push_contour_point(&mut contour, frames[i].r0);
        if i > 0 {
            match joints[i - 1] {
                JointKind::Straight => {}
                JointKind::CleanMiter { outer_arc, .. } if outer_arc.side == Side::Right => {
                    let jnode = cleaned[i];
                    let steps = arc_step_count(outer_arc.radius, outer_arc.delta, true);
                    let rev_start = outer_arc.start_phi + outer_arc.delta;
                    let rev_delta = -outer_arc.delta;
                    for (p, _) in sample_arc_points(
                        jnode,
                        rev_start,
                        rev_delta,
                        steps,
                        outer_arc.use_circle_space,
                        false,
                        false,
                    ) {
                        push_contour_point(&mut contour, p);
                    }
                }
                JointKind::CapJoin {
                    right_arc: Some(arc),
                    ..
                } => {
                    let jnode = cleaned[i];
                    let steps = arc_step_count(arc.radius, arc.delta, true);
                    let rev_start = arc.start_phi + arc.delta;
                    let rev_delta = -arc.delta;
                    for (p, _) in sample_arc_points(
                        jnode,
                        rev_start,
                        rev_delta,
                        steps,
                        arc.use_circle_space,
                        false,
                        false,
                    ) {
                        push_contour_point(&mut contour, p);
                    }
                }
                JointKind::CapJoin {
                    right_arc: None, ..
                } => {
                    push_contour_point(&mut contour, cleaned[i].point);
                }
                _ => {}
            }
        }
    }

    if contour.len() >= 2
        && let (Some(&first_pt), Some(&last_pt)) = (contour.first(), contour.last())
        && first_pt.distance(last_pt) < 1e-4
    {
        contour.pop();
    }
    contour
}

trait RibbonSink {
    fn has_feather(&self) -> bool;
    fn push_solid(&mut self, p: Point, rgb: [f32; 3], alpha: f32) -> u32;
    fn push_aa(&mut self, base: Point, normal: Vec2, rgb: [f32; 3]) -> u32;
    fn push_tri(&mut self, a: u32, b: u32, c: u32);
    #[inline]
    fn push_quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.push_tri(a, b, c);
        self.push_tri(a, c, d);
    }
}

struct BakedRibbonSink {
    mesh: RibbonMesh,
    feather: f64,
}

impl RibbonSink for BakedRibbonSink {
    #[inline]
    fn has_feather(&self) -> bool {
        self.feather > 0.0
    }

    #[inline]
    fn push_solid(&mut self, p: Point, rgb: [f32; 3], alpha: f32) -> u32 {
        let idx = self.mesh.vertices.len() as u32;
        self.mesh.vertices.push(RibbonVertex {
            pos: [p.x as f32, p.y as f32],
            rgba: [rgb[0], rgb[1], rgb[2], alpha],
        });
        idx
    }

    #[inline]
    fn push_aa(&mut self, base: Point, normal: Vec2, rgb: [f32; 3]) -> u32 {
        let p = offset_point(base, normal.scale(self.feather));
        let idx = self.mesh.vertices.len() as u32;
        self.mesh.vertices.push(RibbonVertex {
            pos: [p.x as f32, p.y as f32],
            rgba: [rgb[0], rgb[1], rgb[2], 0.0],
        });
        idx
    }

    #[inline]
    fn push_tri(&mut self, a: u32, b: u32, c: u32) {
        self.mesh.indices.push(a);
        self.mesh.indices.push(b);
        self.mesh.indices.push(c);
    }
}

struct ScalableRibbonSink {
    mesh: ScalableRibbonMesh,
}

impl RibbonSink for ScalableRibbonSink {
    #[inline]
    fn has_feather(&self) -> bool {
        true
    }

    #[inline]
    fn push_solid(&mut self, p: Point, rgb: [f32; 3], alpha: f32) -> u32 {
        let idx = self.mesh.vertices.len() as u32;
        self.mesh.vertices.push(ScalableRibbonVertex {
            pos: [p.x as f32, p.y as f32],
            aa_offset: [0.0, 0.0],
            rgba: [rgb[0], rgb[1], rgb[2], alpha],
        });
        idx
    }

    #[inline]
    fn push_aa(&mut self, base: Point, normal: Vec2, rgb: [f32; 3]) -> u32 {
        let idx = self.mesh.vertices.len() as u32;
        self.mesh.vertices.push(ScalableRibbonVertex {
            pos: [base.x as f32, base.y as f32],
            aa_offset: [normal.x as f32, normal.y as f32],
            rgba: [rgb[0], rgb[1], rgb[2], 0.0],
        });
        idx
    }

    #[inline]
    fn push_tri(&mut self, a: u32, b: u32, c: u32) {
        self.mesh.indices.push(a);
        self.mesh.indices.push(b);
        self.mesh.indices.push(c);
    }
}

fn append_full_dot<S: RibbonSink>(sink: &mut S, node: CleanNode) {
    let steps = (node.radius * 2.0).clamp(12.0, 28.0) as usize;
    let rgb = node.rgb;
    let has_feather = sink.has_feather();
    let center = sink.push_solid(node.point, rgb, node.alpha);
    let mut ring = Vec::with_capacity(steps);
    let mut outer = Vec::with_capacity(steps);
    for i in 0..steps {
        let phi = (i as f64 / steps as f64) * (2.0 * PI);
        let (pt, n) = node.contact_and_normal_at_circle_angle(phi);
        ring.push(sink.push_solid(pt, rgb, node.alpha));
        if has_feather {
            outer.push(sink.push_aa(pt, n, rgb));
        }
    }
    for i in 0..steps {
        let next = (i + 1) % steps;
        sink.push_tri(center, ring[i], ring[next]);
        if has_feather {
            sink.push_quad(ring[i], ring[next], outer[next], outer[i]);
        }
    }
}

#[derive(Clone, Copy)]
struct SectorFanEndpoint {
    pt: Point,
    normal: Vec2,
    existing: Option<(u32, Option<u32>)>,
}

struct SectorFanSpec {
    pivot: Point,
    existing_pivot: Option<u32>,
    node: CleanNode,
    start_phi: f64,
    delta: f64,
    use_circle_space: bool,
    first: Option<SectorFanEndpoint>,
    last: Option<SectorFanEndpoint>,
}

#[inline]
fn is_small_bend_chord(radius: f64, delta: f64) -> bool {
    let sweep = delta.abs();
    if sweep >= 0.30 {
        return false;
    }
    let sagitta = radius * (1.0 - (sweep * 0.5).cos());
    sagitta < 0.12
}

fn append_sector_fan<S: RibbonSink>(sink: &mut S, spec: SectorFanSpec) {
    let steps = if spec.first.is_some()
        && spec.last.is_some()
        && is_small_bend_chord(spec.node.radius, spec.delta)
    {
        if spec.delta.abs() < 1e-3 { 0 } else { 1 }
    } else {
        arc_step_count(spec.node.radius, spec.delta, false)
    };
    if steps == 0 {
        return;
    }

    let rgb = spec.node.rgb;
    let has_feather = sink.has_feather();
    let pivot_v = spec
        .existing_pivot
        .unwrap_or_else(|| sink.push_solid(spec.pivot, rgb, spec.node.alpha));

    if steps == 1
        && let (Some(first), Some(last)) = (spec.first, spec.last)
    {
        let (v0, aa0) = match first.existing {
            Some(pair) => pair,
            None => (
                sink.push_solid(first.pt, rgb, spec.node.alpha),
                has_feather.then(|| sink.push_aa(first.pt, first.normal, rgb)),
            ),
        };
        let (v1, aa1) = match last.existing {
            Some(pair) => pair,
            None => (
                sink.push_solid(last.pt, rgb, spec.node.alpha),
                has_feather.then(|| sink.push_aa(last.pt, last.normal, rgb)),
            ),
        };
        sink.push_tri(pivot_v, v0, v1);
        if let (Some(a0), Some(a1)) = (aa0, aa1) {
            sink.push_quad(v0, v1, a1, a0);
        }
        return;
    }

    let mut arc = sample_arc_points(
        spec.node,
        spec.start_phi,
        spec.delta,
        steps,
        spec.use_circle_space,
        true,
        true,
    );
    if arc.len() < 2 {
        return;
    }
    if let Some(first) = spec.first {
        arc[0] = (first.pt, first.normal);
    }
    if let Some(last) = spec.last
        && let Some(slot) = arc.last_mut()
    {
        *slot = (last.pt, last.normal);
    }

    let (mut prev_solid, mut prev_aa) = match spec.first.and_then(|f| f.existing) {
        Some(pair) => pair,
        None => (
            sink.push_solid(arc[0].0, rgb, spec.node.alpha),
            has_feather.then(|| sink.push_aa(arc[0].0, arc[0].1, rgb)),
        ),
    };

    let last_idx = arc.len() - 1;
    for (idx, &(pt, n)) in arc.iter().enumerate().skip(1) {
        let is_last = idx == last_idx;
        let (cur_solid, cur_aa) = if is_last && let Some(pair) = spec.last.and_then(|l| l.existing)
        {
            pair
        } else {
            (
                sink.push_solid(pt, rgb, spec.node.alpha),
                has_feather.then(|| sink.push_aa(pt, n, rgb)),
            )
        };
        sink.push_tri(pivot_v, prev_solid, cur_solid);
        if let (Some(p_aa), Some(c_aa)) = (prev_aa, cur_aa) {
            sink.push_quad(prev_solid, cur_solid, c_aa, p_aa);
        }
        prev_solid = cur_solid;
        prev_aa = cur_aa;
    }
}

fn append_flat_cap_feather<S: RibbonSink>(
    sink: &mut S,
    node: CleanNode,
    edge_a: (Point, u32),
    edge_b: (Point, u32),
    outward: Vec2,
) {
    if !sink.has_feather() {
        return;
    }
    let rgb = node.rgb;
    let outer_b = sink.push_aa(edge_b.0, outward, rgb);
    let outer_a = sink.push_aa(edge_a.0, outward, rgb);
    sink.push_quad(edge_a.1, edge_b.1, outer_b, outer_a);
}

#[inline]
fn joint_edge_normal(joint: Option<&JointKind>, side: Side, fallback: Vec2) -> Vec2 {
    if let Some(JointKind::CleanMiter {
        inner_side,
        miter_normal,
        ..
    }) = joint
        && *inner_side == side
    {
        *miter_normal
    } else {
        fallback
    }
}

#[derive(Clone, Copy)]
struct SegEdgeVerts {
    vl: u32,
    vr: u32,
    fl: Option<u32>,
    fr: Option<u32>,
}

fn push_seg_edge<S: RibbonSink>(
    sink: &mut S,
    left_pt: Point,
    right_pt: Point,
    left_n: Vec2,
    right_n: Vec2,
    rgb: [f32; 3],
    alpha: f32,
) -> SegEdgeVerts {
    let has_feather = sink.has_feather();
    let vl = sink.push_solid(left_pt, rgb, alpha);
    let vr = sink.push_solid(right_pt, rgb, alpha);
    let fl = has_feather.then(|| sink.push_aa(left_pt, left_n, rgb));
    let fr = has_feather.then(|| sink.push_aa(right_pt, right_n, rgb));
    SegEdgeVerts { vl, vr, fl, fr }
}

fn append_joint_bridging<S: RibbonSink>(
    sink: &mut S,
    jnode: CleanNode,
    prev_seg: &SegmentFrame,
    next_seg: &SegmentFrame,
    prev_end: SegEdgeVerts,
    next_start: SegEdgeVerts,
    joint: JointKind,
) {
    let rgb = jnode.rgb;
    match joint {
        JointKind::Straight => {}
        JointKind::CleanMiter {
            outer_arc,
            inner_side,
            miter_point,
            ..
        } => {
            let (inner_v, first, last) = match inner_side {
                Side::Left => (
                    prev_end.vl,
                    SectorFanEndpoint {
                        pt: prev_seg.r1,
                        normal: prev_seg.n_right,
                        existing: Some((prev_end.vr, prev_end.fr)),
                    },
                    SectorFanEndpoint {
                        pt: next_seg.r0,
                        normal: next_seg.n_right,
                        existing: Some((next_start.vr, next_start.fr)),
                    },
                ),
                Side::Right => (
                    prev_end.vr,
                    SectorFanEndpoint {
                        pt: prev_seg.l1,
                        normal: prev_seg.n_left,
                        existing: Some((prev_end.vl, prev_end.fl)),
                    },
                    SectorFanEndpoint {
                        pt: next_seg.l0,
                        normal: next_seg.n_left,
                        existing: Some((next_start.vl, next_start.fl)),
                    },
                ),
            };
            append_sector_fan(
                sink,
                SectorFanSpec {
                    pivot: miter_point,
                    existing_pivot: Some(inner_v),
                    node: jnode,
                    start_phi: outer_arc.start_phi,
                    delta: outer_arc.delta,
                    use_circle_space: outer_arc.use_circle_space,
                    first: Some(first),
                    last: Some(last),
                },
            );
        }
        JointKind::CapJoin {
            left_arc,
            right_arc,
        } => {
            let mut center_v: Option<u32> = None;
            if left_arc.is_some() || right_arc.is_some() {
                if prev_seg.alpha < -1e-4 {
                    let c = *center_v
                        .get_or_insert_with(|| sink.push_solid(jnode.point, rgb, jnode.alpha));
                    sink.push_tri(c, prev_end.vr, prev_end.vl);
                }
                if next_seg.alpha > 1e-4 {
                    let c = *center_v
                        .get_or_insert_with(|| sink.push_solid(jnode.point, rgb, jnode.alpha));
                    sink.push_tri(c, next_start.vl, next_start.vr);
                }
            }
            if let Some(arc) = left_arc {
                let c =
                    *center_v.get_or_insert_with(|| sink.push_solid(jnode.point, rgb, jnode.alpha));
                append_sector_fan(
                    sink,
                    SectorFanSpec {
                        pivot: jnode.point,
                        existing_pivot: Some(c),
                        node: jnode,
                        start_phi: arc.start_phi,
                        delta: arc.delta,
                        use_circle_space: arc.use_circle_space,
                        first: Some(SectorFanEndpoint {
                            pt: prev_seg.l1,
                            normal: prev_seg.n_left,
                            existing: Some((prev_end.vl, prev_end.fl)),
                        }),
                        last: Some(SectorFanEndpoint {
                            pt: next_seg.l0,
                            normal: next_seg.n_left,
                            existing: Some((next_start.vl, next_start.fl)),
                        }),
                    },
                );
            }
            if let Some(arc) = right_arc {
                let c =
                    *center_v.get_or_insert_with(|| sink.push_solid(jnode.point, rgb, jnode.alpha));
                append_sector_fan(
                    sink,
                    SectorFanSpec {
                        pivot: jnode.point,
                        existing_pivot: Some(c),
                        node: jnode,
                        start_phi: arc.start_phi,
                        delta: arc.delta,
                        use_circle_space: arc.use_circle_space,
                        first: Some(SectorFanEndpoint {
                            pt: prev_seg.r1,
                            normal: prev_seg.n_right,
                            existing: Some((prev_end.vr, prev_end.fr)),
                        }),
                        last: Some(SectorFanEndpoint {
                            pt: next_seg.r0,
                            normal: next_seg.n_right,
                            existing: Some((next_start.vr, next_start.fr)),
                        }),
                    },
                );
            }
        }
    }
}

fn build_ribbon_with_sink<S: RibbonSink>(pen: &PenSettings, nodes: &[SpineNode], sink: &mut S) {
    let cleaned = clean_spine_nodes(nodes);
    if cleaned.is_empty() {
        return;
    }
    if cleaned.len() == 1 {
        append_full_dot(sink, cleaned[0]);
        return;
    }

    let (frames, joints) = prepare_segments(&cleaned);
    if frames.is_empty() {
        append_full_dot(sink, cleaned[0]);
        return;
    }
    let m = frames.len();
    let flat_caps = pen.kind == PenKind::Highlighter;
    let has_feather = sink.has_feather();

    let first = &frames[0];
    let first_start = push_seg_edge(
        sink,
        first.l0,
        first.r0,
        first.n_left,
        first.n_right,
        cleaned[0].rgb,
        cleaned[0].alpha,
    );

    // Highlighter strokes use flat marker cuts; other pens keep rounded nib caps.
    if flat_caps {
        let start_tangent = Vec2::from_points(cleaned[0].point, cleaned[1].point)
            .normalized()
            .unwrap_or(Vec2::new(1.0, 0.0));
        append_flat_cap_feather(
            sink,
            cleaned[0],
            (first.r0, first_start.vr),
            (first.l0, first_start.vl),
            start_tangent.scale(-1.0),
        );
    } else {
        let start_pivot = first.r0.lerp(first.l0, 0.5);
        let start_phi = if first.uniform_nib {
            first.u_right_c.angle_rad()
        } else {
            first.n_right.angle_rad()
        };
        append_sector_fan(
            sink,
            SectorFanSpec {
                pivot: start_pivot,
                existing_pivot: None,
                node: cleaned[0],
                start_phi,
                delta: -(PI + 2.0 * first.alpha),
                use_circle_space: first.uniform_nib,
                first: Some(SectorFanEndpoint {
                    pt: first.r0,
                    normal: first.n_right,
                    existing: Some((first_start.vr, first_start.fr)),
                }),
                last: Some(SectorFanEndpoint {
                    pt: first.l0,
                    normal: first.n_left,
                    existing: Some((first_start.vl, first_start.fl)),
                }),
            },
        );
    }

    let mut prev_end = first_start;
    // Emit each segment quad and its outer AA skirts, reusing shared vertices across joints.
    for i in 0..m {
        let seg = &frames[i];
        let a0 = cleaned[i].alpha;
        let a1 = cleaned[i + 1].alpha;
        let prev_joint = i.checked_sub(1).and_then(|idx| joints.get(idx));
        let next_joint = joints.get(i);

        let l0_n = joint_edge_normal(prev_joint, Side::Left, seg.n_left);
        let r0_n = joint_edge_normal(prev_joint, Side::Right, seg.n_right);
        let l1_n = joint_edge_normal(next_joint, Side::Left, seg.n_left);
        let r1_n = joint_edge_normal(next_joint, Side::Right, seg.n_right);

        let rgb0 = cleaned[i].rgb;
        let rgb1 = cleaned[i + 1].rgb;

        let start = if i == 0 {
            first_start
        } else {
            match joints[i - 1] {
                JointKind::Straight => prev_end,
                JointKind::CleanMiter { inner_side, .. } => match inner_side {
                    Side::Left => {
                        let vr = sink.push_solid(seg.r0, rgb0, a0);
                        let fr = has_feather.then(|| sink.push_aa(seg.r0, r0_n, rgb0));
                        SegEdgeVerts {
                            vl: prev_end.vl,
                            vr,
                            fl: prev_end.fl,
                            fr,
                        }
                    }
                    Side::Right => {
                        let vl = sink.push_solid(seg.l0, rgb0, a0);
                        let fl = has_feather.then(|| sink.push_aa(seg.l0, l0_n, rgb0));
                        SegEdgeVerts {
                            vl,
                            vr: prev_end.vr,
                            fl,
                            fr: prev_end.fr,
                        }
                    }
                },
                JointKind::CapJoin { .. } => {
                    push_seg_edge(sink, seg.l0, seg.r0, l0_n, r0_n, rgb0, a0)
                }
            }
        };

        if i > 0 {
            append_joint_bridging(
                sink,
                cleaned[i],
                &frames[i - 1],
                seg,
                prev_end,
                start,
                joints[i - 1],
            );
        }

        let end = push_seg_edge(sink, seg.l1, seg.r1, l1_n, r1_n, rgb1, a1);
        sink.push_quad(start.vl, end.vl, end.vr, start.vr);
        if let (Some(fl0), Some(fl1)) = (start.fl, end.fl) {
            sink.push_quad(fl0, fl1, end.vl, start.vl);
        }
        if let (Some(fr0), Some(fr1)) = (start.fr, end.fr) {
            sink.push_quad(start.vr, end.vr, fr1, fr0);
        }
        prev_end = end;
    }

    // Close the final edge with the matching flat or rounded cap.
    let last = &frames[m - 1];
    let last_node = cleaned[cleaned.len() - 1];
    if flat_caps {
        let end_tangent = Vec2::from_points(cleaned[cleaned.len() - 2].point, last_node.point)
            .normalized()
            .unwrap_or(Vec2::new(1.0, 0.0));
        append_flat_cap_feather(
            sink,
            last_node,
            (last.l1, prev_end.vl),
            (last.r1, prev_end.vr),
            end_tangent,
        );
    } else {
        let end_pivot = last.l1.lerp(last.r1, 0.5);
        let end_phi = if last.uniform_nib {
            last.u_left_c.angle_rad()
        } else {
            last.n_left.angle_rad()
        };
        append_sector_fan(
            sink,
            SectorFanSpec {
                pivot: end_pivot,
                existing_pivot: None,
                node: last_node,
                start_phi: end_phi,
                delta: -(PI - 2.0 * last.alpha),
                use_circle_space: last.uniform_nib,
                first: Some(SectorFanEndpoint {
                    pt: last.l1,
                    normal: last.n_left,
                    existing: Some((prev_end.vl, prev_end.fl)),
                }),
                last: Some(SectorFanEndpoint {
                    pt: last.r1,
                    normal: last.n_right,
                    existing: Some((prev_end.vr, prev_end.fr)),
                }),
            },
        );
    }
}

/// Build an anti-aliased variable-width triangle ribbon mesh with round joins,
/// elliptical/round caps, common outer bitangents, and bounded inner corners.
pub fn build_stroke_ribbon(pen: &PenSettings, nodes: &[SpineNode], feather_px: f32) -> RibbonMesh {
    let feather = if feather_px.is_finite() {
        f64::from(feather_px).max(0.0)
    } else {
        0.0
    };
    let mut sink = BakedRibbonSink {
        mesh: RibbonMesh::default(),
        feather,
    };
    build_ribbon_with_sink(pen, nodes, &mut sink);
    sink.mesh
}

/// Build a zoom-invariant ribbon mesh storing unit AA feather vectors so any zoom
/// level can evaluate `pos + aa_offset * feather` directly in a vertex shader.
pub fn build_stroke_ribbon_scalable(pen: &PenSettings, nodes: &[SpineNode]) -> ScalableRibbonMesh {
    let mut sink = ScalableRibbonSink {
        mesh: ScalableRibbonMesh::default(),
    };
    build_ribbon_with_sink(pen, nodes, &mut sink);
    sink.mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use drawcraft_color::Color;
    use std::f64::consts::FRAC_PI_2;

    fn node(x: f64, y: f64, radius: f32) -> SpineNode {
        SpineNode {
            point: Point::new(x, y),
            radius,
            roundness: 1.0,
            angle_rad: 0.0,
            alpha: 1.0,
            color: Color::BLACK,
            arc_length: 0.0,
            direction_rad: 0.0,
        }
    }

    fn max_solid_distance_from_nodes(mesh: &RibbonMesh, nodes: &[SpineNode]) -> f64 {
        mesh.vertices
            .iter()
            .filter(|v| v.rgba[3] > 0.5)
            .map(|v| {
                let p = Point::new(f64::from(v.pos[0]), f64::from(v.pos[1]));
                nodes
                    .iter()
                    .map(|n| p.distance(n.point) - f64::from(n.radius))
                    .fold(f64::INFINITY, f64::min)
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn ribbon_mesh_generates_valid_indices_for_dot_and_stroke() {
        let pen = PenSettings::default();
        let dot = build_stroke_ribbon(&pen, &[node(50.0, 60.0, 4.0)], 0.75);
        assert!(!dot.vertices.is_empty());
        assert_eq!(dot.indices.len() % 3, 0);
        for &idx in &dot.indices {
            assert!((idx as usize) < dot.vertices.len());
        }

        let stroke = build_stroke_ribbon(
            &pen,
            &[
                node(10.0, 20.0, 3.0),
                node(50.0, 20.0, 4.0),
                node(90.0, 35.0, 2.5),
            ],
            0.75,
        );
        assert!(!stroke.vertices.is_empty());
        assert_eq!(stroke.indices.len() % 3, 0);
        for &idx in &stroke.indices {
            assert!((idx as usize) < stroke.vertices.len());
        }
    }

    #[test]
    fn highlighter_ribbon_caps_stay_flush_without_circular_bulges() {
        let pen = PenSettings {
            color: Color::new(1.0, 0.9, 0.1, 0.35),
            size: 18.0,
            ..Default::default()
        };
        let nodes = vec![
            SpineNode {
                point: Point::new(80.0, 200.0),
                radius: pen.size * 0.5,
                roundness: 0.35,
                angle_rad: FRAC_PI_2 as f32,
                alpha: 0.35,
                color: pen.color,
                arc_length: 0.0,
                direction_rad: 0.0,
            },
            SpineNode {
                point: Point::new(260.0, 200.0),
                radius: pen.size * 0.5,
                roundness: 0.35,
                angle_rad: FRAC_PI_2 as f32,
                alpha: 0.35,
                color: pen.color,
                arc_length: 180.0,
                direction_rad: 0.0,
            },
        ];
        let half_w = f64::from(pen.size) * 0.5;
        assert!((elliptical_half_width(&nodes[0], Vec2::new(0.0, 1.0)) - half_w).abs() < 1e-4);

        let feather = 0.75_f32;
        let mesh = build_stroke_ribbon(&pen, &nodes, feather);
        for v in &mesh.vertices {
            assert!(
                (f64::from(v.pos[1]) - 200.0).abs() <= half_w + f64::from(feather) + 0.05,
                "vertex y={} exceeded flush chisel half-width",
                v.pos[1]
            );
        }
    }

    #[test]
    fn sharp_turns_and_hairpins_never_emit_pointy_corner_spikes() {
        let pen = PenSettings::default();
        let feather = 0.75_f32;

        // 90 deg corner, 150 deg acute V-bend, and 180 deg exact hairpin.
        let corners = [
            vec![
                node(0.0, 0.0, 6.0),
                node(40.0, 0.0, 6.0),
                node(40.0, 40.0, 6.0),
            ],
            vec![
                node(0.0, 0.0, 6.0),
                node(100.0, 0.0, 6.0),
                node(10.0, -25.0, 6.0),
            ],
            vec![
                node(0.0, 0.0, 6.0),
                node(50.0, 0.0, 6.0),
                node(5.0, 0.0, 6.0),
            ],
        ];

        for nodes in &corners {
            let mesh = build_stroke_ribbon(&pen, nodes, feather);
            assert!(!mesh.vertices.is_empty());
            // Every solid vertex must stay on or inside a segment/disk hull (bounded cleanly
            // near node disks; miter points stay <= 1.85*r on inside, corner fan at exact r).
            for v in &mesh.vertices {
                let p = Point::new(f64::from(v.pos[0]), f64::from(v.pos[1]));
                assert!(p.is_finite());
                if v.rgba[3] > 0.5 {
                    let excess = nodes
                        .iter()
                        .map(|n| {
                            p.distance(n.point) - f64::from(n.radius) * CLEAN_MITER_MAX_RADIUS_MULT
                        })
                        .fold(f64::INFINITY, f64::min);
                    assert!(excess <= 0.05, "unexpected solid spike at {p:?}");
                }
            }
            // Specifically verify acute V-bend tip (x=100) has zero solid vertices past 100 + 6.
            let corner_x = nodes[1].point.x;
            let corner_r = f64::from(nodes[1].radius);
            let max_x = mesh
                .vertices
                .iter()
                .filter(|v| v.rgba[3] > 0.5)
                .map(|v| f64::from(v.pos[0]))
                .fold(f64::NEG_INFINITY, f64::max);
            assert!(
                max_x <= corner_x + corner_r + 0.05,
                "tip protruded past round join: max_x={max_x}, corner_x={corner_x}"
            );

            // Closed contour must also stay bounded past the round join apex.
            let contour = build_stroke_contour(nodes);
            assert!(contour.len() >= 8);
            let contour_max_x = contour
                .iter()
                .map(|p| p.x)
                .fold(f64::NEG_INFINITY, f64::max);
            assert!(
                contour_max_x <= corner_x + corner_r + 0.05,
                "contour tip protruded past round join: {contour_max_x}"
            );
        }

        // Verify pure collinear bend & steep taper remain tight.
        let taper = vec![
            node(0.0, 0.0, 8.0),
            node(30.0, 0.0, 3.0),
            node(60.0, 0.0, 7.0),
        ];
        let taper_mesh = build_stroke_ribbon(&pen, &taper, feather);
        assert!(max_solid_distance_from_nodes(&taper_mesh, &taper) <= 0.1);
    }

    fn point_in_tri(p: Point, a: Point, b: Point, c: Point) -> bool {
        let eps = 1e-4;
        let s1 = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        let s2 = (c.x - b.x) * (p.y - b.y) - (c.y - b.y) * (p.x - b.x);
        let s3 = (a.x - c.x) * (p.y - c.y) - (a.y - c.y) * (p.x - c.x);
        let pos = s1 >= -eps && s2 >= -eps && s3 >= -eps;
        let neg = s1 <= eps && s2 <= eps && s3 <= eps;
        pos || neg
    }

    fn solid_tri_hits(mesh: &RibbonMesh, p: Point) -> usize {
        let mut hits = 0_usize;
        for &tri in mesh.indices.as_chunks::<3>().0 {
            let va = mesh.vertices[tri[0] as usize];
            let vb = mesh.vertices[tri[1] as usize];
            let vc = mesh.vertices[tri[2] as usize];
            if va.rgba[3] <= 0.05 && vb.rgba[3] <= 0.05 && vc.rgba[3] <= 0.05 {
                continue;
            }
            let a = Point::new(f64::from(va.pos[0]), f64::from(va.pos[1]));
            let b = Point::new(f64::from(vb.pos[0]), f64::from(vb.pos[1]));
            let c = Point::new(f64::from(vc.pos[0]), f64::from(vc.pos[1]));
            if point_in_tri(p, a, b, c) {
                hits += 1;
            }
        }
        hits
    }

    #[test]
    fn tapered_tips_and_swallowed_ramps_have_zero_holes_or_protruding_warts() {
        let pen = PenSettings::default();
        let feather = 0.75_f32;

        // Dense vertical lift-off taper ("1" / "+" tail) + dense growing touchdown ramp + "V" bend.
        let strokes = [
            vec![
                node(100.0, 100.0, 7.0),
                node(100.0, 140.0, 7.0),
                node(100.0, 142.0, 5.5),
                node(100.0, 144.0, 3.8),
                node(100.0, 146.0, 2.0),
                node(100.0, 148.0, 1.0),
            ],
            vec![
                node(100.0, 200.0, 1.2),
                node(101.2, 199.6, 3.5),
                node(102.5, 200.3, 6.8),
                node(104.5, 201.5, 8.5),
                node(140.0, 220.0, 8.5),
            ],
            vec![
                node(50.0, 50.0, 6.0),
                node(95.0, 50.0, 6.0),
                node(72.0, 88.0, 6.0),
            ],
        ];

        for nodes in &strokes {
            let mesh = build_stroke_ribbon(&pen, nodes, feather);
            // Every sample inside every disk interior (out to 0.85 * r) and along segments must be covered.
            for win in nodes.windows(2) {
                let (a, b) = (win[0], win[1]);
                for step in 0..=8 {
                    let t = f64::from(step) / 8.0;
                    let c = a.point.lerp(b.point, t);
                    assert!(solid_tri_hits(&mesh, c) >= 1, "hole on centerline at {c:?}");
                }
            }
            for n in nodes {
                let r = f64::from(n.radius) * 0.85;
                for k in 0..8 {
                    let ang = (f64::from(k) / 8.0) * std::f64::consts::TAU;
                    let probe = Point::new(n.point.x + r * ang.cos(), n.point.y + r * ang.sin());
                    assert!(
                        solid_tri_hits(&mesh, probe) >= 1,
                        "hole inside node disk {n:?} at {probe:?}"
                    );
                }
            }
        }

        // Partially overlapping ramp keeps non-swallowed leading disks cleanly without shooting past min-x.
        let ramp = &strokes[1];
        let ramp_mesh = build_stroke_ribbon(&pen, ramp, feather);
        let true_left = ramp
            .iter()
            .skip(2)
            .map(|n| n.point.x - f64::from(n.radius))
            .fold(f64::INFINITY, f64::min);
        for v in &ramp_mesh.vertices {
            if v.rgba[3] > 0.5 {
                assert!(
                    f64::from(v.pos[0]) >= true_left - 0.1,
                    "touchdown node leaked backward: x={} vs {}",
                    v.pos[0],
                    true_left
                );
            }
        }

        // Completely swallowed touchdown cluster collapses cleanly to the enveloping disk.
        let swallowed_touchdown = vec![
            node(100.0, 200.0, 1.2),
            node(100.8, 199.3, 3.0),
            node(101.6, 200.2, 8.5),
            node(140.0, 220.0, 8.5),
        ];
        let swallowed_mesh = build_stroke_ribbon(&pen, &swallowed_touchdown, feather);
        let min_x = swallowed_mesh
            .vertices
            .iter()
            .filter(|v| v.rgba[3] > 0.5)
            .map(|v| f64::from(v.pos[0]))
            .fold(f64::INFINITY, f64::min);
        assert!(
            min_x >= (101.6 - 8.5) - 0.1,
            "swallowed touchdown wart protruded past envelope: min_x={min_x}"
        );

        // Clean 125 deg corner must have zero overlapping solid interior triangles (safe for translucent ink).
        let acute_translucent = &strokes[2];
        let corner_mesh = build_stroke_ribbon(&pen, acute_translucent, feather);
        assert_eq!(solid_tri_hits(&corner_mesh, Point::new(97.5, 50.5)), 1);
    }
}
