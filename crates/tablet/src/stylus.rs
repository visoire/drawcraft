//! Platform-neutral stylus sample modelled on the W3C Pointer Events fields that iPadOS Safari
//! exposes for Apple Pencil (`pressure`, `tiltX/Y`, `azimuthAngle`, `altitudeAngle`, `twist`,
//! `buttons`). Pure data and math, so it compiles unchanged for native and `wasm32`.

use crate::Sample;
use std::f32::consts::{FRAC_PI_2, TAU};

/// Which physical input produced a pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputKind {
    /// Pen / Apple Pencil (`pointerType == "pen"`).
    Stylus,
    /// Finger (`pointerType == "touch"`).
    Touch,
    /// Mouse or trackpad cursor (`pointerType == "mouse"`).
    Mouse,
}

impl InputKind {
    /// Maps a W3C `pointerType` string; unknown values are treated as a mouse.
    #[must_use]
    pub fn from_pointer_type(pointer_type: &str) -> Self {
        match pointer_type {
            "pen" => Self::Stylus,
            "touch" => Self::Touch,
            _ => Self::Mouse,
        }
    }
}

/// `PointerEvent.buttons` bit for the pen barrel button (same bit as a mouse secondary button).
pub const BUTTON_BARREL: u16 = 1 << 1;
/// `PointerEvent.buttons` bit reported while the inverted pen end (eraser) is in use.
pub const BUTTON_ERASER: u16 = 1 << 5;

/// Highest tilt magnitude used for trigonometry (avoids `tan(90deg)` blowing up).
const TILT_TRIG_LIMIT: f32 = 89.999;
/// Page-space coordinates beyond this magnitude are treated as hostile.
const COORD_LIMIT: f32 = 1.0e7;

/// Raw values of one DOM `PointerEvent`, as handed over by the browser host glue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawPointer<'a> {
    pub pointer_type: &'a str,
    pub pointer_id: i32,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    /// `azimuthAngle` in radians, when the browser provides it.
    pub azimuth_angle: Option<f32>,
    /// `altitudeAngle` in radians, when the browser provides it.
    pub altitude_angle: Option<f32>,
    /// Barrel rotation in degrees (`twist`).
    pub twist: f32,
    pub buttons: u16,
    /// `PointerEvent.timeStamp` in milliseconds.
    pub time_ms: f64,
}

/// One normalized stylus / touch / mouse reading with every Apple Pencil field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StylusSample {
    pub kind: InputKind,
    pub pointer_id: i32,
    pub x: f32,
    pub y: f32,
    /// `0.0..=1.0`.
    pub pressure: f32,
    /// Degrees, `-90.0..=90.0`.
    pub tilt_x: f32,
    /// Degrees, `-90.0..=90.0`.
    pub tilt_y: f32,
    /// Radians in `0.0..TAU`; `0` points along `+x`, increasing clockwise on screen.
    pub azimuth: f32,
    /// Radians in `0.0..=PI/2`; `PI/2` means the pen is perpendicular to the surface.
    pub altitude: f32,
    /// Barrel rotation in degrees, `0.0..360.0`.
    pub twist: f32,
    pub barrel_button: bool,
    pub eraser_end: bool,
    pub time_ms: f64,
}

/// Converts W3C `tiltX/tiltY` (degrees) into `(azimuth, altitude)` radians.
#[must_use]
pub fn tilt_to_spherical(tilt_x: f32, tilt_y: f32) -> (f32, f32) {
    let tx = if tilt_x.is_finite() { tilt_x } else { 0.0 }.clamp(-TILT_TRIG_LIMIT, TILT_TRIG_LIMIT);
    let ty = if tilt_y.is_finite() { tilt_y } else { 0.0 }.clamp(-TILT_TRIG_LIMIT, TILT_TRIG_LIMIT);
    if tx == 0.0 && ty == 0.0 {
        return (0.0, FRAC_PI_2);
    }
    let tan_x = tx.to_radians().tan();
    let tan_y = ty.to_radians().tan();
    let mut azimuth = tan_y.atan2(tan_x);
    if azimuth < 0.0 {
        azimuth += TAU;
    }
    let altitude = (1.0 / tan_x.hypot(tan_y)).atan();
    (azimuth.rem_euclid(TAU), altitude.clamp(0.0, FRAC_PI_2))
}

fn finite_or(v: f32, default: f32) -> f32 {
    if v.is_finite() { v } else { default }
}

impl StylusSample {
    /// Builds a sanitized sample from a raw DOM pointer event.
    ///
    /// Missing or non-finite `azimuth`/`altitude` are derived from `tiltX/tiltY`. Touch and mouse
    /// inputs report no pressure while idle, so they fall back to `0.5` like the W3C spec.
    #[must_use]
    pub fn from_raw(raw: &RawPointer<'_>) -> Self {
        let kind = InputKind::from_pointer_type(raw.pointer_type);
        let (derived_az, derived_alt) = tilt_to_spherical(raw.tilt_x, raw.tilt_y);
        let azimuth = raw
            .azimuth_angle
            .filter(|a| a.is_finite())
            .unwrap_or(derived_az);
        let altitude = raw
            .altitude_angle
            .filter(|a| a.is_finite())
            .unwrap_or(derived_alt);
        let default_pressure = if kind == InputKind::Stylus { 0.0 } else { 0.5 };
        Self {
            kind,
            pointer_id: raw.pointer_id,
            x: raw.x,
            y: raw.y,
            pressure: if raw.pressure.is_finite() {
                raw.pressure
            } else {
                default_pressure
            },
            tilt_x: raw.tilt_x,
            tilt_y: raw.tilt_y,
            azimuth,
            altitude,
            twist: raw.twist,
            barrel_button: kind == InputKind::Stylus && raw.buttons & BUTTON_BARREL != 0,
            eraser_end: kind == InputKind::Stylus && raw.buttons & BUTTON_ERASER != 0,
            time_ms: raw.time_ms,
        }
        .sanitized()
    }

    /// Clamps every field to its documented range and replaces non-finite values.
    #[must_use]
    pub fn sanitized(self) -> Self {
        let time_ms = if self.time_ms.is_finite() && self.time_ms >= 0.0 {
            self.time_ms
        } else {
            0.0
        };
        Self {
            x: finite_or(self.x, 0.0).clamp(-COORD_LIMIT, COORD_LIMIT),
            y: finite_or(self.y, 0.0).clamp(-COORD_LIMIT, COORD_LIMIT),
            pressure: finite_or(self.pressure, 0.5).clamp(0.0, 1.0),
            tilt_x: finite_or(self.tilt_x, 0.0).clamp(-90.0, 90.0),
            tilt_y: finite_or(self.tilt_y, 0.0).clamp(-90.0, 90.0),
            azimuth: finite_or(self.azimuth, 0.0).rem_euclid(TAU),
            altitude: finite_or(self.altitude, FRAC_PI_2).clamp(0.0, FRAC_PI_2),
            twist: finite_or(self.twist, 0.0).rem_euclid(360.0),
            time_ms,
            ..self
        }
    }

    /// Converts into the OS-oriented [`Sample`] used by the ink engine (integer pixel position is
    /// left unset because browser coordinates are fractional).
    #[must_use]
    pub fn to_sample(&self) -> Sample {
        Sample {
            x_px: None,
            y_px: None,
            pressure: self.pressure,
            tilt_x: self.tilt_x,
            tilt_y: self.tilt_y,
            rotation: self.twist,
            eraser: self.eraser_end,
            barrel: self.barrel_button,
            barrel_secondary: false,
            time_ms: Some(self.time_ms),
        }
        .sanitized()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn raw(pointer_type: &str) -> RawPointer<'_> {
        RawPointer {
            pointer_type,
            pointer_id: 1,
            x: 10.0,
            y: 20.0,
            pressure: 0.6,
            tilt_x: 0.0,
            tilt_y: 0.0,
            azimuth_angle: None,
            altitude_angle: None,
            twist: 0.0,
            buttons: 0,
            time_ms: 5.0,
        }
    }

    #[test]
    fn pointer_types_map_to_kinds() {
        assert_eq!(InputKind::from_pointer_type("pen"), InputKind::Stylus);
        assert_eq!(InputKind::from_pointer_type("touch"), InputKind::Touch);
        assert_eq!(InputKind::from_pointer_type("mouse"), InputKind::Mouse);
        assert_eq!(InputKind::from_pointer_type("???"), InputKind::Mouse);
    }

    #[test]
    fn upright_pencil_has_vertical_altitude() {
        let s = StylusSample::from_raw(&raw("pen"));
        assert!((s.altitude - FRAC_PI_2).abs() < 1e-6);
        assert_eq!(s.azimuth, 0.0);
    }

    #[test]
    fn tilt_derives_azimuth_and_altitude_on_each_axis() {
        let (az, alt) = tilt_to_spherical(45.0, 0.0);
        assert!(az.abs() < 1e-6 && (alt - PI / 4.0).abs() < 1e-5);
        let (az, _) = tilt_to_spherical(0.0, 45.0);
        assert!((az - FRAC_PI_2).abs() < 1e-5);
        let (az, _) = tilt_to_spherical(-45.0, 0.0);
        assert!((az - PI).abs() < 1e-5);
        let (az, _) = tilt_to_spherical(0.0, -45.0);
        assert!((az - 3.0 * FRAC_PI_2).abs() < 1e-5);
        // Lying almost flat gives a near-zero altitude, never NaN.
        let (_, alt) = tilt_to_spherical(90.0, 90.0);
        assert!(alt.is_finite() && alt < 0.05);
    }

    #[test]
    fn browser_provided_angles_win_over_derived_ones() {
        let mut r = raw("pen");
        r.tilt_x = 30.0;
        r.azimuth_angle = Some(1.0);
        r.altitude_angle = Some(0.5);
        let s = StylusSample::from_raw(&r);
        assert_eq!((s.azimuth, s.altitude), (1.0, 0.5));
    }

    #[test]
    fn barrel_and_eraser_bits_only_apply_to_the_pencil() {
        let mut r = raw("pen");
        r.buttons = BUTTON_BARREL | BUTTON_ERASER;
        let s = StylusSample::from_raw(&r);
        assert!(s.barrel_button && s.eraser_end);

        let mut m = raw("mouse");
        m.buttons = BUTTON_BARREL | BUTTON_ERASER;
        let s = StylusSample::from_raw(&m);
        assert!(!s.barrel_button && !s.eraser_end);
    }

    #[test]
    fn hostile_values_are_sanitized() {
        let mut r = raw("pen");
        r.x = f32::NAN;
        r.y = 1e30;
        r.pressure = f32::INFINITY;
        r.tilt_x = f32::NAN;
        r.tilt_y = 1e9;
        r.twist = -90.0;
        r.azimuth_angle = Some(f32::NAN);
        r.altitude_angle = Some(f32::INFINITY);
        r.time_ms = f64::NAN;
        let s = StylusSample::from_raw(&r);
        assert_eq!(s.x, 0.0);
        assert_eq!(s.y, COORD_LIMIT);
        assert_eq!(s.pressure, 0.0);
        assert!(s.tilt_x == 0.0 && s.tilt_y == 90.0);
        assert_eq!(s.twist, 270.0);
        assert!(s.azimuth.is_finite() && s.altitude.is_finite());
        assert_eq!(s.time_ms, 0.0);
    }

    #[test]
    fn idle_touch_defaults_to_half_pressure() {
        let mut r = raw("touch");
        r.pressure = f32::NAN;
        assert_eq!(StylusSample::from_raw(&r).pressure, 0.5);
    }

    #[test]
    fn converts_to_the_ink_engine_sample() {
        let mut r = raw("pen");
        r.twist = 45.0;
        r.buttons = BUTTON_ERASER;
        let sample = StylusSample::from_raw(&r).to_sample();
        assert!(sample.eraser && !sample.barrel);
        assert_eq!(sample.rotation, 45.0);
        assert_eq!(sample.time_ms, Some(5.0));
    }
}
