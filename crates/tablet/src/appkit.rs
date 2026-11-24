//! Pure macOS AppKit `NSEvent` tablet payload normalizer (cross-platform tested).

use crate::{Sample, Update};

/// Subset of macOS `NSEvent` tablet fields mapped safely without Objective-C runtime calls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppKitEvent {
    pub is_tablet_point: bool,
    pub is_tablet_proximity: bool,
    pub entering_proximity: bool,
    pub is_eraser_device: bool,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub rotation: f32,
    pub is_mouse_event: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    eraser: bool,
    in_proximity: bool,
}

impl State {
    pub fn handle(&mut self, ev: AppKitEvent) -> Update {
        if ev.is_tablet_proximity {
            self.in_proximity = ev.entering_proximity;
            self.eraser = ev.entering_proximity && ev.is_eraser_device;
            if !ev.entering_proximity {
                return Update::Set(None);
            }
            return Update::Keep;
        }
        if ev.is_tablet_point || (self.in_proximity && ev.is_mouse_event) {
            return Update::Set(Some(
                Sample {
                    pressure: ev.pressure,
                    tilt_x: ev.tilt_x * 60.0,
                    tilt_y: -ev.tilt_y * 60.0,
                    rotation: ev.rotation,
                    eraser: self.eraser,
                    ..Default::default()
                }
                .sanitized(),
            ));
        }
        if ev.is_mouse_event {
            self.eraser = false;
            return Update::Set(None);
        }
        Update::Keep
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn appkit_proximity_and_point_normalize() {
        let mut st = State::default();
        assert_eq!(
            st.handle(AppKitEvent {
                is_tablet_point: false,
                is_tablet_proximity: true,
                entering_proximity: true,
                is_eraser_device: true,
                pressure: 0.0,
                tilt_x: 0.0,
                tilt_y: 0.0,
                rotation: 0.0,
                is_mouse_event: false,
            }),
            Update::Keep
        );
        let u = st.handle(AppKitEvent {
            is_tablet_point: true,
            is_tablet_proximity: false,
            entering_proximity: false,
            is_eraser_device: false,
            pressure: 0.75,
            tilt_x: 0.5,
            tilt_y: -0.5,
            rotation: 45.0,
            is_mouse_event: false,
        });
        assert!(
            matches!(u, Update::Set(Some(s)) if (s.pressure - 0.75).abs() < 1e-6 && s.eraser && (s.tilt_x - 30.0).abs() < 1e-6)
        );
    }
}
