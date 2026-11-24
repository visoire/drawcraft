//! Which physical input produced a DOM touch, and which touches are a resting palm.
//!
//! On iPadOS Safari one Apple Pencil stroke fires *both* a `PointerEvent` with
//! `pointerType === "pen"` *and* a `TouchEvent` whose `Touch.touchType` is `"stylus"`; a finger
//! reports `"direct"`. The palm guard must therefore never drop a stylus touch, only direct
//! (finger/palm) touches that start while the Pencil is down or hovering. This module is pure
//! logic (no browser types) so the Safari behaviour is unit-tested natively.

use drawcraft_ui::{TouchSource, TouchSourceEvent};

/// Upper bound for tracked touch identifiers (hostile or buggy event streams).
const MAX_TRACKED: usize = 64;

/// Phase of a DOM touch event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Start,
    Move,
    End,
    Cancel,
}

impl TouchPhase {
    /// Maps a DOM event name (`touchstart`, ...) to a phase.
    #[must_use]
    pub fn from_event_name(name: &str) -> Option<Self> {
        match name {
            "touchstart" => Some(Self::Start),
            "touchmove" => Some(Self::Move),
            "touchend" => Some(Self::End),
            "touchcancel" => Some(Self::Cancel),
            _ => None,
        }
    }
}

/// Classifies a touch from Safari's non-standard `touchType` (`"stylus"` | `"direct"`).
///
/// Browsers without `touchType` fall back to `force`: a Pencil reports pressure, a finger on an
/// iPad reports `0`.
#[must_use]
pub fn classify_touch(touch_type: Option<&str>, force: f32) -> TouchSource {
    match touch_type {
        Some("stylus") => TouchSource::Stylus,
        Some("direct") => TouchSource::Direct,
        _ if force.is_finite() && force > 0.0 => TouchSource::Stylus,
        _ => TouchSource::Direct,
    }
}

/// Tracks live touches: their source (reported to the UI) and which were swallowed as palms.
#[derive(Debug, Default)]
pub struct TouchTracker {
    sources: Vec<(i32, TouchSource)>,
    rejected: Vec<i32>,
    reports: Vec<TouchSourceEvent>,
}

impl TouchTracker {
    /// Handles one changed touch and returns `true` when the browser event for it must be
    /// swallowed (a resting palm).
    ///
    /// `palm_guard` is `PalmRejection::touch_rejected`: the Pencil is down, hovering or lifted a
    /// moment ago.
    pub fn handle(
        &mut self,
        phase: TouchPhase,
        id: i32,
        source: TouchSource,
        palm_guard: bool,
    ) -> bool {
        match phase {
            TouchPhase::Start => {
                // A Pencil touch is never a palm, whatever the guard says.
                let reject = source == TouchSource::Direct && palm_guard;
                if self.sources.len() >= MAX_TRACKED {
                    self.sources.remove(0);
                }
                self.sources.retain(|(i, _)| *i != id);
                self.sources.push((id, source));
                if reject && self.rejected.len() < MAX_TRACKED {
                    self.rejected.push(id);
                }
                self.reports.push(TouchSourceEvent {
                    id,
                    source,
                    active: true,
                });
                reject
            }
            TouchPhase::Move => self.rejected.contains(&id),
            TouchPhase::End | TouchPhase::Cancel => {
                let was_rejected = self.rejected.contains(&id);
                self.rejected.retain(|i| *i != id);
                if let Some(pos) = self.sources.iter().position(|(i, _)| *i == id) {
                    let (_, source) = self.sources.remove(pos);
                    self.reports.push(TouchSourceEvent {
                        id,
                        source,
                        active: false,
                    });
                }
                was_rejected
            }
        }
    }

    /// Source reports produced since the last call (consumed by the UI each frame).
    pub fn drain_reports(&mut self) -> Vec<TouchSourceEvent> {
        std::mem::take(&mut self.reports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safari_touch_type_decides_and_force_is_the_fallback() {
        assert_eq!(classify_touch(Some("stylus"), 0.0), TouchSource::Stylus);
        assert_eq!(classify_touch(Some("direct"), 0.9), TouchSource::Direct);
        assert_eq!(classify_touch(None, 0.4), TouchSource::Stylus);
        assert_eq!(classify_touch(None, 0.0), TouchSource::Direct);
        assert_eq!(classify_touch(Some("???"), f32::NAN), TouchSource::Direct);
    }

    #[test]
    fn the_apple_pencils_own_touch_events_are_never_swallowed() {
        // pointerdown("pen") has armed the palm guard; the paired stylus touch must still pass.
        let mut t = TouchTracker::default();
        assert!(!t.handle(TouchPhase::Start, 1, TouchSource::Stylus, true));
        assert!(!t.handle(TouchPhase::Move, 1, TouchSource::Stylus, true));
        assert!(!t.handle(TouchPhase::End, 1, TouchSource::Stylus, true));
    }

    #[test]
    fn a_finger_landing_while_the_pencil_is_active_is_a_palm_for_its_whole_life() {
        let mut t = TouchTracker::default();
        assert!(t.handle(TouchPhase::Start, 2, TouchSource::Direct, true));
        // Even after the guard window expired the same finger stays rejected.
        assert!(t.handle(TouchPhase::Move, 2, TouchSource::Direct, false));
        assert!(t.handle(TouchPhase::End, 2, TouchSource::Direct, false));
        // The identifier is free again afterwards.
        assert!(!t.handle(TouchPhase::Start, 2, TouchSource::Direct, false));
    }

    #[test]
    fn a_finger_that_started_before_the_pencil_keeps_navigating() {
        let mut t = TouchTracker::default();
        assert!(!t.handle(TouchPhase::Start, 3, TouchSource::Direct, false));
        assert!(!t.handle(TouchPhase::Move, 3, TouchSource::Direct, true));
        assert!(!t.handle(TouchPhase::End, 3, TouchSource::Direct, true));
    }

    #[test]
    fn pencil_and_finger_at_once_each_keep_their_own_fate() {
        let mut t = TouchTracker::default();
        assert!(!t.handle(TouchPhase::Start, 10, TouchSource::Stylus, true));
        assert!(t.handle(TouchPhase::Start, 11, TouchSource::Direct, true));
        assert!(!t.handle(TouchPhase::Move, 10, TouchSource::Stylus, true));
        assert!(t.handle(TouchPhase::Move, 11, TouchSource::Direct, true));
    }

    #[test]
    fn sources_are_reported_to_the_ui_on_start_and_end() {
        let mut t = TouchTracker::default();
        t.handle(TouchPhase::Start, 5, TouchSource::Direct, false);
        t.handle(TouchPhase::Start, 6, TouchSource::Stylus, false);
        t.handle(TouchPhase::End, 5, TouchSource::Direct, false);
        let reports = t.drain_reports();
        assert_eq!(reports.len(), 3);
        assert!(
            reports[0].active && reports[0].source == TouchSource::Direct && reports[0].id == 5
        );
        assert!(reports[1].active && reports[1].source == TouchSource::Stylus);
        assert!(!reports[2].active && reports[2].id == 5);
        assert!(t.drain_reports().is_empty());
    }

    #[test]
    fn hostile_streams_stay_bounded() {
        let mut t = TouchTracker::default();
        for id in 0..1_000 {
            t.handle(TouchPhase::Start, id, TouchSource::Direct, true);
        }
        assert!(t.sources.len() <= MAX_TRACKED && t.rejected.len() <= MAX_TRACKED);
        assert!(!t.handle(TouchPhase::Move, -1, TouchSource::Direct, true));
        assert_eq!(
            TouchPhase::from_event_name("touchmove"),
            Some(TouchPhase::Move)
        );
        assert_eq!(TouchPhase::from_event_name("click"), None);
    }
}
