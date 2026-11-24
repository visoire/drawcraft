//! Palm rejection and multi-finger tap gestures for tablets.
//!
//! The state machine is fed pointer down / move / up / cancel notifications and answers what each
//! pointer is *for*: drawing, panning/zooming, or nothing (a resting palm). It also recognises
//! a quick two-finger tap (undo) and three-finger tap (redo), matching common tablet conventions.

use crate::stylus::InputKind;

/// How fingers are treated while a stylus is available.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PalmMode {
    /// Only the pencil (or a mouse) draws; one finger pans, two fingers pinch.
    #[default]
    PencilOnly,
    /// A single finger may draw as well; more fingers or a pencil take over.
    PencilAndFinger,
}

/// What the host should do with a pointer's movement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerRole {
    Draw,
    Pan,
    /// Palm or stray contact: drop every event of this pointer.
    Ignore,
}

/// Multi-finger tap gesture recognised when the last finger lifts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// Quick two-finger tap.
    Undo,
    /// Quick three-finger tap.
    Redo,
}

/// Result of a pointer-down notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DownOutcome {
    pub role: PointerRole,
    /// An already-drawing finger that must abandon its stroke because a second finger arrived.
    pub cancel_stroke_for: Option<i32>,
}

/// Longest contact that still counts as a tap.
pub const TAP_MAX_MS: f64 = 300.0;
/// Largest finger travel (in CSS px) that still counts as a tap.
pub const TAP_MAX_TRAVEL_PX: f32 = 12.0;
/// Touches are ignored this long after the pencil lifts or hovers (resting-palm guard).
pub const PALM_GUARD_MS: f64 = 350.0;
const MAX_TRACKED_POINTERS: usize = 16;

#[derive(Clone, Copy, Debug)]
struct Tracked {
    id: i32,
    kind: InputKind,
    role: PointerRole,
    start_x: f32,
    start_y: f32,
}

/// Palm-rejection and tap-gesture state machine.
#[derive(Clone, Debug, Default)]
pub struct PalmRejection {
    mode: PalmMode,
    pointers: Vec<Tracked>,
    pencil_seen: bool,
    /// Time of the latest pencil contact or hover; `None` before the pencil is used.
    last_pencil_ms: Option<f64>,
    touch_session_start_ms: Option<f64>,
    touch_session_max_count: usize,
    touch_session_max_travel: f32,
    touch_session_tainted: bool,
}

fn clean_time(t: f64) -> f64 {
    if t.is_finite() && t >= 0.0 { t } else { 0.0 }
}

fn clean_coord(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

impl PalmRejection {
    #[must_use]
    pub fn new(mode: PalmMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn mode(&self) -> PalmMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: PalmMode) {
        self.mode = mode;
    }

    /// Whether a pencil has been observed (hosts use this to default to [`PalmMode::PencilOnly`]).
    #[must_use]
    pub fn pencil_seen(&self) -> bool {
        self.pencil_seen
    }

    /// `true` while a resting palm should be ignored: the pencil is down, hovering, or lifted less
    /// than [`PALM_GUARD_MS`] ago. Hosts use this to drop touch events before the page sees them.
    #[must_use]
    pub fn touch_rejected(&self, time_ms: f64) -> bool {
        self.pencil_guard_active(clean_time(time_ms))
    }

    /// Records pencil hover so a palm landing next to it is rejected before the tip touches down.
    pub fn pencil_hover(&mut self, time_ms: f64) {
        self.pencil_seen = true;
        self.last_pencil_ms = Some(clean_time(time_ms));
    }

    fn pencil_guard_active(&self, now_ms: f64) -> bool {
        let pencil_down = self.pointers.iter().any(|p| p.kind == InputKind::Stylus);
        pencil_down
            || self
                .last_pencil_ms
                .is_some_and(|t| now_ms >= t && now_ms - t <= PALM_GUARD_MS)
    }

    /// Active (non-rejected) fingers; rejected palms never count towards pan/draw/tap logic.
    fn touch_count(&self) -> usize {
        self.pointers
            .iter()
            .filter(|p| p.kind == InputKind::Touch && p.role != PointerRole::Ignore)
            .count()
    }

    /// Notifies a new contact and returns its role.
    pub fn pointer_down(
        &mut self,
        kind: InputKind,
        id: i32,
        x: f32,
        y: f32,
        time_ms: f64,
    ) -> DownOutcome {
        let now = clean_time(time_ms);
        self.pointers.retain(|p| p.id != id);
        if self.pointers.len() >= MAX_TRACKED_POINTERS {
            return DownOutcome {
                role: PointerRole::Ignore,
                cancel_stroke_for: None,
            };
        }
        let mut cancel = None;
        let role = match kind {
            InputKind::Stylus => {
                self.pencil_seen = true;
                self.last_pencil_ms = Some(now);
                // Anything already touching is now a palm: abandon draws and taint gestures.
                self.touch_session_tainted = true;
                for p in self
                    .pointers
                    .iter_mut()
                    .filter(|p| p.kind == InputKind::Touch)
                {
                    if p.role == PointerRole::Draw {
                        cancel = Some(p.id);
                    }
                    p.role = PointerRole::Ignore;
                }
                PointerRole::Draw
            }
            InputKind::Mouse => PointerRole::Draw,
            InputKind::Touch => {
                let guarded = self.pencil_guard_active(now);
                if guarded {
                    self.touch_session_tainted = true;
                    PointerRole::Ignore
                } else {
                    let existing = self.touch_count();
                    if existing == 0 {
                        self.touch_session_start_ms = Some(now);
                        self.touch_session_max_count = 0;
                        self.touch_session_max_travel = 0.0;
                        self.touch_session_tainted = false;
                    } else if let Some(first) = self
                        .pointers
                        .iter_mut()
                        .find(|p| p.kind == InputKind::Touch && p.role == PointerRole::Draw)
                    {
                        // A second finger turns the first finger's stroke into a pan/pinch.
                        cancel = Some(first.id);
                        first.role = PointerRole::Pan;
                    }
                    if self.mode == PalmMode::PencilAndFinger && existing == 0 {
                        PointerRole::Draw
                    } else {
                        PointerRole::Pan
                    }
                }
            }
        };
        self.pointers.push(Tracked {
            id,
            kind,
            role,
            start_x: clean_coord(x),
            start_y: clean_coord(y),
        });
        if kind == InputKind::Touch && role != PointerRole::Ignore {
            self.touch_session_max_count = self.touch_session_max_count.max(self.touch_count());
        }
        DownOutcome {
            role,
            cancel_stroke_for: cancel,
        }
    }

    /// Notifies movement and returns the pointer's current role (`Ignore` if unknown).
    pub fn pointer_move(&mut self, id: i32, x: f32, y: f32, time_ms: f64) -> PointerRole {
        let now = clean_time(time_ms);
        let (kind, role, sx, sy) = match self.pointers.iter().find(|p| p.id == id) {
            Some(p) => (p.kind, p.role, p.start_x, p.start_y),
            None => return PointerRole::Ignore,
        };
        match kind {
            InputKind::Stylus => self.last_pencil_ms = Some(now),
            InputKind::Touch => {
                let travel = (clean_coord(x) - sx).hypot(clean_coord(y) - sy);
                self.touch_session_max_travel = self.touch_session_max_travel.max(travel);
            }
            InputKind::Mouse => {}
        }
        role
    }

    /// Notifies a lift; returns a gesture when the last finger of a quick multi-finger tap rises.
    pub fn pointer_up(&mut self, id: i32, time_ms: f64) -> Option<Gesture> {
        let now = clean_time(time_ms);
        let removed = self.pointers.iter().position(|p| p.id == id)?;
        let tracked = self.pointers.remove(removed);
        match tracked.kind {
            InputKind::Stylus => {
                self.last_pencil_ms = Some(now);
                None
            }
            InputKind::Mouse => None,
            InputKind::Touch => {
                if self.touch_count() > 0 {
                    return None;
                }
                self.finish_touch_session(now)
            }
        }
    }

    /// Notifies a cancelled pointer; never produces a gesture.
    pub fn pointer_cancel(&mut self, id: i32) {
        if let Some(pos) = self.pointers.iter().position(|p| p.id == id) {
            let tracked = self.pointers.remove(pos);
            if tracked.kind == InputKind::Touch {
                self.touch_session_tainted = true;
                if self.touch_count() == 0 {
                    self.reset_touch_session();
                }
            }
        }
    }

    fn reset_touch_session(&mut self) {
        self.touch_session_start_ms = None;
        self.touch_session_max_count = 0;
        self.touch_session_max_travel = 0.0;
        self.touch_session_tainted = false;
    }

    fn finish_touch_session(&mut self, now: f64) -> Option<Gesture> {
        let start = self.touch_session_start_ms;
        let count = self.touch_session_max_count;
        let travel = self.touch_session_max_travel;
        let tainted = self.touch_session_tainted;
        self.reset_touch_session();
        let elapsed = now - start?;
        if tainted || !(0.0..=TAP_MAX_MS).contains(&elapsed) || travel > TAP_MAX_TRAVEL_PX {
            return None;
        }
        match count {
            2 => Some(Gesture::Undo),
            3 => Some(Gesture::Redo),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(p: &mut PalmRejection, kind: InputKind, id: i32, t: f64) -> DownOutcome {
        p.pointer_down(kind, id, 100.0, 100.0, t)
    }

    #[test]
    fn pencil_only_lets_the_pencil_draw_and_fingers_pan() {
        let mut p = PalmRejection::new(PalmMode::PencilOnly);
        assert_eq!(
            down(&mut p, InputKind::Stylus, 1, 0.0).role,
            PointerRole::Draw
        );
        p.pointer_up(1, 10.0);
        // Long after the pencil lifted a finger pans instead of drawing.
        assert_eq!(
            down(&mut p, InputKind::Touch, 2, 2_000.0).role,
            PointerRole::Pan
        );
        assert!(p.pencil_seen());
    }

    #[test]
    fn touches_are_ignored_while_the_pencil_is_down_or_hovering() {
        let mut p = PalmRejection::new(PalmMode::PencilAndFinger);
        down(&mut p, InputKind::Stylus, 1, 0.0);
        assert_eq!(
            down(&mut p, InputKind::Touch, 2, 5.0).role,
            PointerRole::Ignore
        );
        p.pointer_up(1, 100.0);
        // Resting palm just after lift is still rejected, but not once the guard expires.
        assert_eq!(
            down(&mut p, InputKind::Touch, 3, 200.0).role,
            PointerRole::Ignore
        );
        assert_eq!(
            down(&mut p, InputKind::Touch, 4, 100.0 + PALM_GUARD_MS + 50.0).role,
            PointerRole::Draw
        );
        p.pointer_cancel(3);
        p.pointer_cancel(4);

        let mut hover = PalmRejection::new(PalmMode::PencilOnly);
        hover.pencil_hover(1_000.0);
        assert_eq!(
            down(&mut hover, InputKind::Touch, 9, 1_100.0).role,
            PointerRole::Ignore
        );
    }

    #[test]
    fn palm_already_on_screen_is_dropped_when_the_pencil_lands() {
        let mut p = PalmRejection::new(PalmMode::PencilAndFinger);
        assert_eq!(
            down(&mut p, InputKind::Touch, 1, 0.0).role,
            PointerRole::Draw
        );
        let out = down(&mut p, InputKind::Stylus, 2, 20.0);
        assert_eq!(out.role, PointerRole::Draw);
        assert_eq!(out.cancel_stroke_for, Some(1));
        assert_eq!(p.pointer_move(1, 5.0, 5.0, 30.0), PointerRole::Ignore);
    }

    #[test]
    fn finger_mode_draws_with_one_finger_and_pans_with_two() {
        let mut p = PalmRejection::new(PalmMode::PencilAndFinger);
        assert_eq!(
            down(&mut p, InputKind::Touch, 1, 0.0).role,
            PointerRole::Draw
        );
        let second = down(&mut p, InputKind::Touch, 2, 40.0);
        assert_eq!(second.role, PointerRole::Pan);
        assert_eq!(second.cancel_stroke_for, Some(1));
        assert_eq!(p.pointer_move(1, 100.0, 100.0, 50.0), PointerRole::Pan);
    }

    #[test]
    fn mouse_always_draws() {
        let mut p = PalmRejection::default();
        assert_eq!(
            down(&mut p, InputKind::Mouse, 1, 0.0).role,
            PointerRole::Draw
        );
        assert_eq!(p.pointer_up(1, 5.0), None);
    }

    #[test]
    fn quick_two_finger_tap_is_undo_and_three_finger_tap_is_redo() {
        let mut p = PalmRejection::default();
        down(&mut p, InputKind::Touch, 1, 0.0);
        down(&mut p, InputKind::Touch, 2, 10.0);
        assert_eq!(p.pointer_up(1, 90.0), None);
        assert_eq!(p.pointer_up(2, 100.0), Some(Gesture::Undo));

        down(&mut p, InputKind::Touch, 3, 1_000.0);
        down(&mut p, InputKind::Touch, 4, 1_005.0);
        down(&mut p, InputKind::Touch, 5, 1_010.0);
        p.pointer_up(3, 1_080.0);
        p.pointer_up(4, 1_090.0);
        assert_eq!(p.pointer_up(5, 1_100.0), Some(Gesture::Redo));
    }

    #[test]
    fn slow_moved_or_single_finger_contacts_are_not_gestures() {
        let mut p = PalmRejection::default();
        down(&mut p, InputKind::Touch, 1, 0.0);
        down(&mut p, InputKind::Touch, 2, 5.0);
        p.pointer_up(1, 50.0);
        assert_eq!(p.pointer_up(2, TAP_MAX_MS + 100.0), None, "too slow");

        down(&mut p, InputKind::Touch, 3, 1_000.0);
        down(&mut p, InputKind::Touch, 4, 1_005.0);
        p.pointer_move(3, 100.0 + TAP_MAX_TRAVEL_PX + 30.0, 100.0, 1_020.0);
        p.pointer_up(3, 1_050.0);
        assert_eq!(p.pointer_up(4, 1_060.0), None, "moved too far");

        down(&mut p, InputKind::Touch, 5, 2_000.0);
        assert_eq!(p.pointer_up(5, 2_050.0), None, "single finger");
    }

    #[test]
    fn a_pencil_during_the_contact_cancels_the_gesture() {
        let mut p = PalmRejection::default();
        down(&mut p, InputKind::Touch, 1, 0.0);
        down(&mut p, InputKind::Touch, 2, 5.0);
        down(&mut p, InputKind::Stylus, 3, 10.0);
        p.pointer_up(3, 20.0);
        p.pointer_up(1, 40.0);
        assert_eq!(p.pointer_up(2, 50.0), None);
    }

    #[test]
    fn touch_rejection_follows_the_pencil_guard_window() {
        let mut p = PalmRejection::default();
        assert!(!p.touch_rejected(0.0), "no pencil seen yet");
        down(&mut p, InputKind::Stylus, 1, 100.0);
        assert!(p.touch_rejected(150.0), "pencil is down");
        p.pointer_up(1, 200.0);
        assert!(p.touch_rejected(200.0 + PALM_GUARD_MS - 1.0));
        assert!(!p.touch_rejected(200.0 + PALM_GUARD_MS + 1.0));
        p.pencil_hover(1_000.0);
        assert!(p.touch_rejected(1_100.0));
        // A hostile timestamp is treated as time zero, never as a panic.
        let _ = p.touch_rejected(f64::NAN);
    }

    #[test]
    fn hostile_input_never_panics_and_bounds_tracking() {
        let mut p = PalmRejection::default();
        for id in 0..100 {
            let out = p.pointer_down(InputKind::Touch, id, f32::NAN, f32::INFINITY, f64::NAN);
            if id >= 16 {
                assert_eq!(out.role, PointerRole::Ignore);
            }
        }
        assert_eq!(
            p.pointer_move(9_999, f32::NAN, 0.0, f64::NAN),
            PointerRole::Ignore
        );
        assert_eq!(p.pointer_up(9_999, 0.0), None);
        p.pointer_cancel(9_999);
        for id in 0..100 {
            p.pointer_cancel(id);
        }
        // Reusing an id replaces the stale entry instead of duplicating it.
        down(&mut p, InputKind::Mouse, 1, 0.0);
        down(&mut p, InputKind::Mouse, 1, 1.0);
        assert_eq!(p.pointer_up(1, 2.0), None);
        assert_eq!(p.pointer_up(1, 3.0), None);
    }
}
