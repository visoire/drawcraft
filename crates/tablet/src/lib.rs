//! Cross-platform pen tablet input: pressure, tilt, barrel rotation, barrel button,
//! and inverted eraser-end tracking.
//!
//! Platform-specific message extraction is kept thin and isolated; pure normalization and state
//! machines live in [`win_ink`], [`appkit`], and [`xi`] and are unit-tested on all platforms.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod appkit;
pub mod linux;
pub mod palm;
pub mod stylus;
pub mod win_ink;
pub mod xi;

#[cfg(target_os = "windows")]
pub mod windows;

/// Image payload extracted from the platform clipboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardImage {
    /// Encoded image file bytes (PNG, BMP, JPEG, etc.).
    EncodedBytes(Vec<u8>),
    /// Path to an image file copied in the OS file manager (`CF_HDROP`).
    FilePath(std::path::PathBuf),
}

/// Physical attachment type reported by the OS pen digitizer driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PenDeviceKind {
    Integrated,
    External,
}

/// Connected hardware pen digitizer detected by the platform backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectedPenDevice {
    pub name: String,
    pub kind: PenDeviceKind,
}

/// One normalized pen reading.
///
/// Tilt is in degrees (`-90.0..=90.0`, W3C Pointer Events convention),
/// barrel rotation in degrees (`0.0..360.0`), and pressure in `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub x_px: Option<i32>,
    pub y_px: Option<i32>,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub rotation: f32,
    pub eraser: bool,
    pub barrel: bool,
    pub barrel_secondary: bool,
    pub time_ms: Option<f64>,
}

impl Default for Sample {
    fn default() -> Self {
        Self {
            x_px: None,
            y_px: None,
            pressure: 1.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            rotation: 0.0,
            eraser: false,
            barrel: false,
            barrel_secondary: false,
            time_ms: None,
        }
    }
}

impl Sample {
    /// Clamps all fields to documented ranges and replaces non-finite values.
    ///
    /// A `NaN` pressure falls back to `1.0` (mouse-like); `NaN` tilt or rotation falls back to `0.0`.
    pub fn sanitized(self) -> Self {
        let clamp_finite = |v: f32, lo: f32, hi: f32, default: f32| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                default
            }
        };
        let rotation = if self.rotation.is_finite() {
            let r = self.rotation.rem_euclid(360.0);
            if r >= 360.0 { 0.0 } else { r }
        } else {
            0.0
        };
        let time_ms = self.time_ms.filter(|t| t.is_finite() && *t >= 0.0);
        Self {
            x_px: self.x_px,
            y_px: self.y_px,
            pressure: clamp_finite(self.pressure, 0.0, 1.0, 1.0),
            tilt_x: clamp_finite(self.tilt_x, -90.0, 90.0, 0.0),
            tilt_y: clamp_finite(self.tilt_y, -90.0, 90.0, 0.0),
            rotation,
            eraser: self.eraser,
            barrel: self.barrel,
            barrel_secondary: self.barrel_secondary,
            time_ms,
        }
    }
}

/// What a raw platform event means for the active stylus stream.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Update {
    Keep,
    Set(Option<Sample>),
}

/// Tablet initialization failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    NotMainThread,
    Unsupported(String),
    Platform(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotMainThread => write!(f, "tablet input must be started on the main thread"),
            Self::Unsupported(why) => write!(f, "tablet input unsupported: {why}"),
            Self::Platform(why) => write!(f, "tablet input failed: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// Sanitizes a sample and invokes `callback`, catching any panic so it never unwinds across an OS callback.
pub fn deliver(callback: &dyn Fn(Option<Sample>), sample: Option<Sample>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        callback(sample.map(Sample::sanitized));
    }));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn sanitized_clamps_and_replaces_non_finite() {
        let s = Sample {
            pressure: 1.7,
            tilt_x: -120.0,
            tilt_y: f32::NAN,
            rotation: -90.0,
            eraser: true,
            barrel: true,
            time_ms: Some(-5.0),
            ..Default::default()
        }
        .sanitized();
        assert_eq!(s.pressure, 1.0);
        assert_eq!(s.tilt_x, -90.0);
        assert_eq!(s.tilt_y, 0.0);
        assert_eq!(s.rotation, 270.0);
        assert!(s.eraser && s.barrel);
        assert_eq!(s.time_ms, None);

        let s2 = Sample {
            pressure: f32::NAN,
            tilt_x: f32::INFINITY,
            tilt_y: f32::NEG_INFINITY,
            rotation: f32::INFINITY,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(s2, Sample::default());

        let s3 = Sample {
            rotation: -1e-9,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(s3.rotation, 0.0);
    }

    #[test]
    fn deliver_swallows_callback_panic() {
        let seen = std::cell::Cell::new(None);
        deliver(
            &|s| seen.set(s),
            Some(Sample {
                pressure: 2.5,
                ..Default::default()
            }),
        );
        assert_eq!(seen.get().map(|s| s.pressure), Some(1.0));
        deliver(&|_| panic!("boom"), None);
    }
}
