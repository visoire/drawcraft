//! Pure Linux sysfs input-device pen digitizer discovery (cross-platform unit tested).
//!
//! Reads `/sys/class/input/input*/{name,capabilities/key,id/bustype}` using safe `std::fs`
//! calls (zero `unsafe`) to detect connected hardware pens and drawing tablets.

use crate::{ConnectedPenDevice, PenDeviceKind};

/// Linux `BTN_TOOL_PEN` key code (`0x140` = 320).
pub const BTN_TOOL_PEN: usize = 0x140;

#[cfg(target_os = "linux")]
const MAX_SYSFS_DEVICES: usize = 64;
const BUS_USB: u16 = 0x0003;
const BUS_BLUETOOTH: u16 = 0x0005;

/// Returns `true` if a Linux sysfs `capabilities/key` bitmap line advertises `BTN_TOOL_PEN`.
///
/// Linux prints key bitmaps as space-separated hexadecimal words from most-significant word
/// to least-significant word, where each word has `word_bits` bits (`usize::BITS` on the host).
#[must_use]
pub fn key_bitmap_has_pen(hex_line: &str, word_bits: usize) -> bool {
    if word_bits == 0 {
        return false;
    }
    let words: Vec<&str> = hex_line.split_whitespace().collect();
    let target_word_from_lsb = BTN_TOOL_PEN / word_bits;
    let target_bit = BTN_TOOL_PEN % word_bits;
    if target_word_from_lsb >= words.len() {
        return false;
    }
    let idx_from_msb = words.len() - 1 - target_word_from_lsb;
    let Some(word_str) = words.get(idx_from_msb) else {
        return false;
    };
    let Ok(bits) = u128::from_str_radix(word_str, 16) else {
        return false;
    };
    if target_bit >= 128 {
        return false;
    }
    (bits & (1u128 << target_bit)) != 0
}

/// Classifies a Linux `id/bustype` hex string as external (USB / Bluetooth) or integrated.
#[must_use]
pub fn classify_bus_type(hex_bus: &str) -> PenDeviceKind {
    match u16::from_str_radix(hex_bus.trim(), 16) {
        Ok(BUS_USB | BUS_BLUETOOTH) => PenDeviceKind::External,
        _ => PenDeviceKind::Integrated,
    }
}

/// Lists connected hardware pen digitizers on Linux via `/sys/class/input`.
#[cfg(target_os = "linux")]
#[must_use]
pub fn connected_pen_devices() -> Vec<ConnectedPenDevice> {
    let Ok(entries) = std::fs::read_dir("/sys/class/input") else {
        return Vec::new();
    };
    let word_bits = usize::BITS as usize;
    let mut devices = Vec::new();
    for entry in entries.flatten().take(512) {
        let fname = entry.file_name();
        let Some(fname_str) = fname.to_str() else {
            continue;
        };
        if !fname_str.starts_with("input") {
            continue;
        }
        let path = entry.path();
        let Ok(key_caps) = std::fs::read_to_string(path.join("capabilities/key")) else {
            continue;
        };
        if !key_bitmap_has_pen(&key_caps, word_bits) {
            continue;
        }
        let name = std::fs::read_to_string(path.join("name"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Linux Hardware Stylus".to_string());
        if devices.iter().any(|d: &ConnectedPenDevice| d.name == name) {
            continue;
        }
        let kind = std::fs::read_to_string(path.join("id/bustype"))
            .ok()
            .map_or(PenDeviceKind::Integrated, |s| classify_bus_type(&s));
        devices.push(ConnectedPenDevice { name, kind });
        if devices.len() >= MAX_SYSFS_DEVICES {
            break;
        }
    }
    devices
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn connected_pen_devices() -> Vec<ConnectedPenDevice> {
    Vec::new()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn detects_btn_tool_pen_on_64bit_and_32bit_bitmaps() {
        // 64-bit host: bit 320 is bit 0 of word index 5 from LSB (6 words total).
        assert!(key_bitmap_has_pen("1 0 0 0 0 0", 64));
        // Typical Wacom stylus bitmap with BTN_TOOL_PEN + BTN_TOUCH + BTN_STYLUS in word 5.
        assert!(key_bitmap_has_pen("1c01 0 0 0 0 0", 64));
        // Mouse-only bitmap (word 4 = BTN_LEFT/RIGHT/MIDDLE, word 5 absent or zero).
        assert!(!key_bitmap_has_pen("0 70000 0 0 0 0", 64));
        assert!(!key_bitmap_has_pen("70000 0 0 0 0", 64));

        // 32-bit host: bit 320 is bit 0 of word index 10 from LSB (11 words total).
        assert!(key_bitmap_has_pen("1 0 0 0 0 0 0 0 0 0 0", 32));
        assert!(!key_bitmap_has_pen("0 1 0 0 0 0 0 0 0 0 0", 32));

        // Hostile / malformed inputs never panic.
        assert!(!key_bitmap_has_pen("", 64));
        assert!(!key_bitmap_has_pen("zzzz 0 0 0 0 0", 64));
        assert!(!key_bitmap_has_pen("1 0 0 0 0 0", 0));
    }

    #[test]
    fn classifies_linux_bus_types_cleanly() {
        assert_eq!(classify_bus_type("0003"), PenDeviceKind::External);
        assert_eq!(classify_bus_type("0005\n"), PenDeviceKind::External);
        assert_eq!(classify_bus_type("0018"), PenDeviceKind::Integrated);
        assert_eq!(classify_bus_type("invalid"), PenDeviceKind::Integrated);
    }
}
