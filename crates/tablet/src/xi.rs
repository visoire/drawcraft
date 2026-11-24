//! Pure Linux X11 XInput2 tablet valuator normalizer (cross-platform tested).

use std::collections::HashMap;

use crate::{Sample, Update};

pub const MAX_DEVICES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisKind {
    Pressure,
    TiltX,
    TiltY,
    Rotation,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisSpec {
    pub kind: AxisKind,
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Debug)]
pub struct Device {
    pub id: u16,
    pub eraser: bool,
    pub axes: HashMap<u16, AxisSpec>,
    last: [f64; 4],
}

impl Device {
    pub fn new(
        id: u16,
        name: &str,
        valuators: impl IntoIterator<Item = (u16, String, f64, f64)>,
    ) -> Self {
        let lower = name.to_ascii_lowercase();
        let is_pad = lower.contains("pad");
        let eraser = lower.contains("eraser");
        let mut axes = HashMap::new();
        if !is_pad {
            for (num, label, min, max) in valuators {
                if num > 256 || !min.is_finite() || !max.is_finite() || (max - min).abs() <= 1e-9 {
                    continue;
                }
                let kind = match label.to_ascii_lowercase().as_str() {
                    s if s.contains("pressure") => Some(AxisKind::Pressure),
                    s if s.contains("tilt x") => Some(AxisKind::TiltX),
                    s if s.contains("tilt y") => Some(AxisKind::TiltY),
                    s if s.contains("rotary z") || s.contains("rotation") => {
                        Some(AxisKind::Rotation)
                    }
                    _ => None,
                };
                if let Some(kind) = kind {
                    axes.insert(num, AxisSpec { kind, min, max });
                }
            }
        }
        Self {
            id,
            eraser,
            axes,
            last: [0.0; 4],
        }
    }

    pub fn is_tablet(&self) -> bool {
        self.axes.contains_key(&2) || self.axes.values().any(|a| a.kind == AxisKind::Pressure)
    }
}

#[derive(Clone, Debug, Default)]
pub struct State {
    devices: HashMap<u16, Device>,
}

impl State {
    pub fn set_devices(&mut self, list: impl IntoIterator<Item = Device>) {
        self.devices.clear();
        for d in list.into_iter().take(MAX_DEVICES) {
            self.devices.insert(d.id, d);
        }
    }

    pub fn handle(
        &mut self,
        device_id: u16,
        valuators: impl IntoIterator<Item = (u16, f64)>,
    ) -> Update {
        let Some(dev) = self.devices.get_mut(&device_id) else {
            return Update::Keep;
        };
        if !dev.is_tablet() {
            return Update::Set(None);
        }
        for (axis_idx, val) in valuators {
            if !val.is_finite() {
                continue;
            }
            if let Some(spec) = dev.axes.get(&axis_idx) {
                let norm = ((val - spec.min) / (spec.max - spec.min)).clamp(0.0, 1.0);
                let slot = match spec.kind {
                    AxisKind::Pressure => 0,
                    AxisKind::TiltX => 1,
                    AxisKind::TiltY => 2,
                    AxisKind::Rotation => 3,
                };
                if let Some(target) = dev.last.get_mut(slot) {
                    *target = match spec.kind {
                        AxisKind::Pressure => norm,
                        AxisKind::TiltX | AxisKind::TiltY => (norm * 2.0 - 1.0) * 60.0,
                        AxisKind::Rotation => norm * 360.0,
                    };
                }
            }
        }
        Update::Set(Some(
            Sample {
                pressure: dev.last[0] as f32,
                tilt_x: dev.last[1] as f32,
                tilt_y: dev.last[2] as f32,
                rotation: dev.last[3] as f32,
                eraser: dev.eraser,
                ..Default::default()
            }
            .sanitized(),
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn xinput2_valuators_normalize_and_resist_hostile_ranges() {
        let mut st = State::default();
        let pen = Device::new(
            9,
            "Wacom Pen stylus",
            [
                (2, "Abs Pressure".into(), 0.0, 65535.0),
                (3, "Abs Tilt X".into(), -60.0, 60.0),
                (4, "Abs Tilt Y".into(), -60.0, 60.0),
                (5, "Abs Rotary Z".into(), 0.0, 360.0),
            ],
        );
        let eraser = Device::new(
            10,
            "Wacom Pen eraser",
            [(2, "Abs Pressure".into(), 0.0, 65535.0)],
        );
        let mouse = Device::new(12, "USB Mouse", [(0, "Rel X".into(), -1.0, 1.0)]);
        st.set_devices([pen, eraser, mouse]);

        let u = st.handle(9, [(2, 32767.5), (3, -30.0), (4, 60.0), (5, 180.0)]);
        assert!(
            matches!(u, Update::Set(Some(s)) if (s.pressure - 0.5).abs() < 1e-4 && !s.eraser && (s.tilt_x - -30.0).abs() < 1e-3)
        );

        let u_eraser = st.handle(10, [(2, 65535.0)]);
        assert!(matches!(u_eraser, Update::Set(Some(s)) if s.eraser && s.pressure == 1.0));

        assert_eq!(st.handle(12, [(0, 5.0)]), Update::Set(None));
        assert_eq!(st.handle(999, [(2, 1.0)]), Update::Keep);
    }
}
