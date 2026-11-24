//! Pure Windows Ink (`WM_POINTER*` / `POINTER_PEN_INFO`) packet decoder and state tracker.

use crate::Sample;

pub const WM_POINTERUPDATE: u32 = 0x0245;
pub const WM_POINTERDOWN: u32 = 0x0246;
pub const WM_POINTERUP: u32 = 0x0247;
pub const WM_POINTERENTER: u32 = 0x0249;
pub const WM_POINTERLEAVE: u32 = 0x024A;
pub const WM_POINTERCAPTURECHANGED: u32 = 0x024C;

pub const PEN_FLAG_BARREL: u32 = 0x0000_0001;
pub const PEN_FLAG_INVERTED: u32 = 0x0000_0002;
pub const PEN_FLAG_ERASER: u32 = 0x0000_0004;

pub const PEN_MASK_PRESSURE: u32 = 0x0000_0001;
pub const PEN_MASK_ROTATION: u32 = 0x0000_0002;
pub const PEN_MASK_TILT_X: u32 = 0x0000_0004;
pub const PEN_MASK_TILT_Y: u32 = 0x0000_0008;

pub const POINTER_FLAG_INCONTACT: u32 = 0x0000_0004;
pub const POINTER_FLAG_FIRSTBUTTON: u32 = 0x0000_0010;
pub const POINTER_FLAG_SECONDBUTTON: u32 = 0x0000_0020;
pub const POINTER_FLAG_THIRDBUTTON: u32 = 0x0000_0040;
pub const POINTER_FLAG_FOURTHBUTTON: u32 = 0x0000_0080;
pub const POINTER_FLAG_CANCELED: u32 = 0x0004_0000;

pub const MI_WP_SIGNATURE: u32 = 0xFF51_5700;
pub const MI_WP_SIGNATURE_MASK: u32 = 0xFFFF_FF00;
pub const STALE_PEN_MOUSE_FALLBACK_MS: u32 = 90;

const MIN_CONTACT_PRESSURE: f32 = 0.02;

/// True when `GetMessageExtraInfo()` carries Win32's pointer-to-mouse promotion signature.
#[inline]
pub fn is_pointer_promoted_mouse_extra_info(extra_info: isize) -> bool {
    let low = extra_info as u32;
    let high = (extra_info as i64) >> 32;
    (low & MI_WP_SIGNATURE_MASK) == MI_WP_SIGNATURE && (high == 0 || high == -1)
}

/// True when a genuine non-promoted mouse message should clear a stale `pen_id`
/// (e.g., if `WM_POINTERLEAVE` was lost or the stylus remained idle while the user grabbed a mouse).
#[inline]
pub fn should_clear_stale_pen_on_mouse(
    has_active_pen: bool,
    is_promoted_from_pointer: bool,
    os_pointer_still_alive: bool,
    elapsed_since_pen_ms: Option<u32>,
) -> bool {
    if !has_active_pen || is_promoted_from_pointer {
        return false;
    }
    !os_pointer_still_alive
        || elapsed_since_pen_ms.is_some_and(|ms| ms >= STALE_PEN_MOUSE_FALLBACK_MS)
}

const MAX_CLIPBOARD_DIB_BYTES: usize = 64 * 1024 * 1024;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
const BI_JPEG: u32 = 4;
const BI_PNG: u32 = 5;
const BI_ALPHABITFIELDS: u32 = 6;

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_i32_le(bytes: &[u8], offset: usize) -> Option<i32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(i32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Trims trailing Win32 `GlobalAlloc` heap slack past the PNG `IEND` chunk, if present.
pub fn trim_png_trailing_bytes(bytes: Vec<u8>) -> Vec<u8> {
    const PNG_SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if !bytes.starts_with(PNG_SIG) {
        return bytes;
    }
    let mut pos = 8_usize;
    while pos + 12 <= bytes.len() {
        let Some(len) = read_u32_le(&bytes, pos).map(|v| v as usize) else {
            break;
        };
        let Some(tag) = bytes.get(pos + 4..pos + 8) else {
            break;
        };
        let Some(next) = pos.checked_add(12).and_then(|v| v.checked_add(len)) else {
            break;
        };
        if next > bytes.len() {
            break;
        }
        if tag == b"IEND" {
            let mut out = bytes;
            out.truncate(next);
            return out;
        }
        pos = next;
    }
    bytes
}

fn wrap_bmp_header_and_dib(pixel_offset: u32, header: &[u8], body: &[u8]) -> Option<Vec<u8>> {
    let total_dib_len = header.len().checked_add(body.len())?;
    let file_size = 14_u32.checked_add(u32::try_from(total_dib_len).ok()?)?;
    if pixel_offset > file_size {
        return None;
    }
    let mut bmp = Vec::with_capacity(14 + total_dib_len);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&file_size.to_le_bytes());
    bmp.extend_from_slice(&0_u32.to_le_bytes());
    bmp.extend_from_slice(&pixel_offset.to_le_bytes());
    bmp.extend_from_slice(header);
    bmp.extend_from_slice(body);
    Some(bmp)
}

fn dib_core_to_bmp_bytes(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 12 {
        return None;
    }
    let bit_count = read_u16_le(dib, 10)?;
    let palette_bytes = if bit_count > 0 && bit_count <= 8 {
        (1_u32 << bit_count).checked_mul(3)?
    } else {
        0_u32
    };
    let pixel_offset = 14_u32.checked_add(12)?.checked_add(palette_bytes)?;
    wrap_bmp_header_and_dib(pixel_offset, dib, &[])
}

fn expected_dib_pixel_bytes(width: i32, height: i32, bit_count: u16) -> u64 {
    if width <= 0 || height == 0 || height == i32::MIN || bit_count == 0 {
        return 0;
    }
    let row_bits = (width as u64).saturating_mul(u64::from(bit_count));
    let row_stride = row_bits.div_ceil(32).saturating_mul(4);
    row_stride.saturating_mul(u64::from(height.unsigned_abs()))
}

fn normalize_v3_v5_bitfields(bmp_header: &mut [u8], bit_count: u16, compression: u32) {
    if bmp_header.len() < 56 || bit_count != 32 {
        return;
    }
    let r_mask = read_u32_le(bmp_header, 40).unwrap_or(0);
    let g_mask = read_u32_le(bmp_header, 44).unwrap_or(0);
    let b_mask = read_u32_le(bmp_header, 48).unwrap_or(0);
    let alpha_mask = read_u32_le(bmp_header, 52).unwrap_or(0);
    let promote_rgb = compression == BI_RGB && alpha_mask == 0xFF00_0000;
    if promote_rgb {
        bmp_header[16..20].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
    }
    if (promote_rgb || compression == BI_BITFIELDS) && r_mask == 0 && g_mask == 0 && b_mask == 0 {
        bmp_header[40..44].copy_from_slice(&0x00FF_0000_u32.to_le_bytes());
        bmp_header[44..48].copy_from_slice(&0x0000_FF00_u32.to_le_bytes());
        bmp_header[48..52].copy_from_slice(&0x0000_00FF_u32.to_le_bytes());
    }
}

fn upgrade_alpha_bitfields_dib(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 56 {
        return None;
    }
    let mut header = [0_u8; 56];
    header[0..40].copy_from_slice(&dib[0..40]);
    header[0..4].copy_from_slice(&56_u32.to_le_bytes());
    header[16..20].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
    header[40..56].copy_from_slice(&dib[40..56]);
    wrap_bmp_header_and_dib(14 + 56, &header, &dib[56..])
}

fn compute_dib_mask_bytes(
    dib: &[u8],
    header_size: u32,
    compression: u32,
    expected_pixels: u64,
) -> (u32, Option<[u8; 12]>) {
    if header_size == 40 {
        let mask_bytes = match compression {
            BI_BITFIELDS => 12_u32,
            BI_ALPHABITFIELDS => 16_u32,
            _ => 0_u32,
        };
        return (mask_bytes, None);
    }
    if header_size < 52 || compression != BI_BITFIELDS || expected_pixels == 0 {
        return (0, None);
    }
    let header_usize = header_size as usize;
    if (dib.len() as u64) < (header_usize as u64) + 12 + expected_pixels {
        return (0, None);
    }
    let Some(tail) = dib.get(header_usize..header_usize + 12) else {
        return (0, None);
    };
    let r_mask = read_u32_le(dib, 40).unwrap_or(0);
    let g_mask = read_u32_le(dib, 44).unwrap_or(0);
    let b_mask = read_u32_le(dib, 48).unwrap_or(0);
    if r_mask == 0 && g_mask == 0 && b_mask == 0 {
        let tail_nonzero = read_u32_le(tail, 0).unwrap_or(0) != 0
            && read_u32_le(tail, 4).unwrap_or(0) != 0
            && read_u32_le(tail, 8).unwrap_or(0) != 0;
        if tail_nonzero {
            let masks = [
                tail[0], tail[1], tail[2], tail[3], tail[4], tail[5], tail[6], tail[7], tail[8],
                tail[9], tail[10], tail[11],
            ];
            return (12, Some(masks));
        }
        return (0, None);
    }
    if dib.get(40..52) == Some(tail) {
        (12, None)
    } else {
        (0, None)
    }
}

fn compute_dib_palette_bytes(bit_count: u16, compression: u32, clr_used: u32) -> Option<u32> {
    if bit_count <= 8 {
        return if clr_used > 0 {
            clr_used.checked_mul(4)
        } else if bit_count > 0 {
            (1_u32 << bit_count).checked_mul(4)
        } else {
            Some(0)
        };
    }
    if clr_used == 0 || (compression == BI_BITFIELDS && clr_used <= 4) {
        return Some(0);
    }
    clr_used.checked_mul(4)
}

fn compute_dib_profile_skip(
    dib: &[u8],
    header_size: u32,
    mask_bytes: u32,
    palette_bytes: u32,
    expected_pixels: u64,
) -> Option<u32> {
    if header_size < 124 {
        return Some(0);
    }
    let profile_data = read_u32_le(dib, 112).unwrap_or(0);
    let profile_size = read_u32_le(dib, 116).unwrap_or(0);
    let pre_profile = header_size
        .checked_add(mask_bytes)?
        .checked_add(palette_bytes)?;
    let min_len = u64::from(pre_profile)
        .saturating_add(u64::from(profile_size))
        .saturating_add(expected_pixels);
    if profile_size > 0 && profile_data == pre_profile && (dib.len() as u64) >= min_len {
        Some(profile_size)
    } else {
        Some(0)
    }
}

/// Wraps a raw Win32 `CF_DIB` or `CF_DIBV5` buffer in a 14-byte `BITMAPFILEHEADER`
/// (or extracts embedded JPEG/PNG / upgrades `BI_ALPHABITFIELDS`) so standard decoders
/// can read it directly.
pub fn dib_to_bmp_bytes(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 12 || dib.len() > MAX_CLIPBOARD_DIB_BYTES {
        return None;
    }
    let header_size = read_u32_le(dib, 0)?;
    if header_size == 12 {
        return dib_core_to_bmp_bytes(dib);
    }
    if header_size < 40 || usize::try_from(header_size).ok()? > dib.len() {
        return None;
    }
    let header_usize = header_size as usize;
    let width = read_i32_le(dib, 4)?;
    let height = read_i32_le(dib, 8)?;
    let bit_count = read_u16_le(dib, 14)?;
    let compression = read_u32_le(dib, 16)?;
    let clr_used = read_u32_le(dib, 32)?;

    if matches!(compression, BI_JPEG | BI_PNG) {
        return (header_usize < dib.len()).then(|| dib[header_usize..].to_vec());
    }
    if header_size == 40 && compression == BI_ALPHABITFIELDS {
        return upgrade_alpha_bitfields_dib(dib);
    }

    let expected_pixels = expected_dib_pixel_bytes(width, height, bit_count);
    let (mask_bytes, inline_masks) =
        compute_dib_mask_bytes(dib, header_size, compression, expected_pixels);
    let palette_bytes = compute_dib_palette_bytes(bit_count, compression, clr_used)?;
    let profile_skip =
        compute_dib_profile_skip(dib, header_size, mask_bytes, palette_bytes, expected_pixels)?;

    let normalized_header_size = match header_size {
        40 | 52 | 56 | 108 | 124 => header_size,
        s if s > 124 => 124,
        s if s >= 56 => 56,
        s if s >= 52 => 52,
        _ => 40,
    };
    let mut header = dib[0..normalized_header_size as usize].to_vec();
    header[0..4].copy_from_slice(&normalized_header_size.to_le_bytes());
    if let Some(masks) = inline_masks
        && header.len() >= 52
    {
        header[40..52].copy_from_slice(&masks);
    }
    normalize_v3_v5_bitfields(&mut header, bit_count, compression);

    let pixel_offset = 14_u32
        .checked_add(normalized_header_size)?
        .checked_add(mask_bytes)?
        .checked_add(palette_bytes)?
        .checked_add(profile_skip)?;
    wrap_bmp_header_and_dib(pixel_offset, &header, &dib[header_usize..])
}

/// Returns `true` when `path` has a supported raster image file extension.
pub fn is_supported_image_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png"
                    | "jpg"
                    | "jpeg"
                    | "jfif"
                    | "webp"
                    | "gif"
                    | "bmp"
                    | "dib"
                    | "ico"
                    | "tif"
                    | "tiff"
                    | "tga"
                    | "pnm"
                    | "ppm"
                    | "pgm"
                    | "pbm"
                    | "qoi"
            )
        })
}

/// Pure mirror of Win32 `POINTER_PEN_INFO` fields needed for decoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawPenPacket {
    pub pointer_id: u32,
    pub pointer_flags: u32,
    pub pen_flags: u32,
    pub pen_mask: u32,
    pub pressure: u32,
    pub rotation: u32,
    pub tilt_x: i32,
    pub tilt_y: i32,
    pub pixel_x: i32,
    pub pixel_y: i32,
    pub dw_time: u32,
}

#[inline]
pub fn is_eraser_end(pen_flags: u32) -> bool {
    (pen_flags & (PEN_FLAG_INVERTED | PEN_FLAG_ERASER)) != 0
}

#[inline]
pub fn is_barrel_pressed(pen_flags: u32, pointer_flags: u32) -> bool {
    (pen_flags & PEN_FLAG_BARREL) != 0 || (pointer_flags & POINTER_FLAG_SECONDBUTTON) != 0
}

#[inline]
pub fn is_secondary_barrel_pressed(pointer_flags: u32) -> bool {
    (pointer_flags & (POINTER_FLAG_THIRDBUTTON | POINTER_FLAG_FOURTHBUTTON)) != 0
}

/// Sanitizes a raw OS UTF-16 digitizer device name into a clean, bounded display label.
pub fn sanitize_pen_device_name(raw_utf16: &[u16], is_external: bool) -> String {
    let end = raw_utf16
        .iter()
        .position(|&u| u == 0)
        .unwrap_or(raw_utf16.len());
    let decoded = String::from_utf16_lossy(&raw_utf16[..end]);
    let mut cleaned = String::with_capacity(decoded.len().min(64));
    let mut prev_space = false;
    let mut count = 0usize;
    for ch in decoded.chars().filter(|c| !c.is_control()) {
        if ch.is_whitespace() {
            if !cleaned.is_empty() {
                prev_space = true;
            }
            continue;
        }
        if prev_space {
            if count >= 64 {
                break;
            }
            cleaned.push(' ');
            count += 1;
            prev_space = false;
        }
        if count >= 64 {
            break;
        }
        cleaned.push(ch);
        count += 1;
    }
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        if is_external {
            "External Pen Tablet".to_string()
        } else {
            "Integrated Stylus".to_string()
        }
    } else {
        trimmed.to_string()
    }
}

/// Decodes a `RawPenPacket` into a normalized [`Sample`].
pub fn decode_pen_packet(pkt: &RawPenPacket) -> Sample {
    let in_contact = (pkt.pointer_flags & POINTER_FLAG_INCONTACT) != 0;
    let pressure = if (pkt.pen_mask & PEN_MASK_PRESSURE) != 0 {
        (pkt.pressure as f32 / 1024.0).clamp(0.0, 1.0)
    } else if in_contact {
        1.0
    } else {
        0.0
    };
    let tilt_x = if (pkt.pen_mask & PEN_MASK_TILT_X) != 0 {
        (pkt.tilt_x as f32).clamp(-90.0, 90.0)
    } else {
        0.0
    };
    let tilt_y = if (pkt.pen_mask & PEN_MASK_TILT_Y) != 0 {
        (pkt.tilt_y as f32).clamp(-90.0, 90.0)
    } else {
        0.0
    };
    let rotation = if (pkt.pen_mask & PEN_MASK_ROTATION) != 0 {
        (pkt.rotation as f32).rem_euclid(360.0)
    } else {
        0.0
    };
    Sample {
        x_px: Some(pkt.pixel_x),
        y_px: Some(pkt.pixel_y),
        pressure,
        tilt_x,
        tilt_y,
        rotation,
        eraser: is_eraser_end(pkt.pen_flags),
        barrel: is_barrel_pressed(pkt.pen_flags, pkt.pointer_flags),
        barrel_secondary: is_secondary_barrel_pressed(pkt.pointer_flags),
        time_ms: Some(f64::from(pkt.dw_time)),
    }
    .sanitized()
}

fn extract_contact_samples(
    message: u32,
    packets: &[RawPenPacket],
    prev_latest: Option<Sample>,
) -> Vec<Sample> {
    // Exclude hovering (`!INCONTACT`) and canceled packets from pressure samples so
    // hover/lift zero readings never corrupt active stroke pressure or mouse fallback.
    let mut prev_p = prev_latest.map_or(0.0, |s| s.pressure);
    let mut out = Vec::with_capacity(packets.len());
    for pkt in packets {
        let in_contact =
            (pkt.pointer_flags & POINTER_FLAG_INCONTACT) != 0 || message == WM_POINTERDOWN;
        let canceled = (pkt.pointer_flags & POINTER_FLAG_CANCELED) != 0;
        if !in_contact || canceled {
            continue;
        }
        let mut s = decode_pen_packet(pkt);
        if s.pressure <= 0.0 {
            s.pressure = if message == WM_POINTERDOWN && prev_p <= 0.0 {
                MIN_CONTACT_PRESSURE
            } else {
                prev_p.max(MIN_CONTACT_PRESSURE)
            };
        }
        prev_p = s.pressure;
        out.push(s);
    }
    out
}

fn resolve_barrel_flags(packets: &[RawPenPacket], msg_flags: u32) -> (bool, bool) {
    if let Some(last_pkt) = packets.last() {
        let combined = last_pkt.pointer_flags | msg_flags;
        (
            is_barrel_pressed(last_pkt.pen_flags, combined),
            is_secondary_barrel_pressed(combined),
        )
    } else {
        (
            (msg_flags & POINTER_FLAG_SECONDBUTTON) != 0,
            is_secondary_barrel_pressed(msg_flags),
        )
    }
}

/// Pure state machine tracking pen proximity, barrel buttons, and eraser end across `WM_POINTER*` messages.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PenState {
    pub barrel: bool,
    pub barrel_secondary: bool,
    pub end: Option<bool>,
    pub pen_id: Option<u32>,
    pub contact_end: Option<bool>,
    pub latest: Option<Sample>,
}

impl PenState {
    pub fn step(
        self,
        message: u32,
        w_param: usize,
        read_packets: impl FnOnce(u32) -> Vec<RawPenPacket>,
    ) -> (Self, Vec<Sample>) {
        let pointer_id = (w_param & 0xFFFF) as u32;
        match message {
            WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERENTER => {
                let msg_flags = ((w_param >> 16) & 0xFFFF) as u32;
                let packets = read_packets(pointer_id);
                if packets.is_empty() {
                    if self.pen_id == Some(pointer_id) {
                        let (barrel, barrel_secondary) = resolve_barrel_flags(&packets, msg_flags);
                        return (
                            Self {
                                barrel,
                                barrel_secondary,
                                ..self
                            },
                            Vec::new(),
                        );
                    }
                    return (self, Vec::new());
                }
                let eraser = packets.last().map(|p| is_eraser_end(p.pen_flags));
                let (barrel, barrel_secondary) = resolve_barrel_flags(&packets, msg_flags);
                let contact_samples = extract_contact_samples(message, &packets, self.latest);
                let latest = contact_samples.last().copied();
                let next = Self {
                    barrel,
                    barrel_secondary,
                    end: eraser.or(self.end),
                    pen_id: Some(pointer_id),
                    contact_end: if message == WM_POINTERDOWN {
                        eraser.or(self.contact_end)
                    } else {
                        self.contact_end
                    },
                    latest,
                };
                (next, contact_samples)
            }
            WM_POINTERUP | WM_POINTERCAPTURECHANGED
                if self.pen_id.is_some_and(|id| id != pointer_id) =>
            {
                (self, Vec::new())
            }
            WM_POINTERUP => {
                let msg_flags = ((w_param >> 16) & 0xFFFF) as u32;
                let packets = read_packets(pointer_id);
                let (barrel, barrel_secondary) = resolve_barrel_flags(&packets, msg_flags);
                (
                    Self {
                        barrel,
                        barrel_secondary,
                        latest: None,
                        ..self
                    },
                    Vec::new(),
                )
            }
            WM_POINTERCAPTURECHANGED => (
                Self {
                    barrel: false,
                    barrel_secondary: false,
                    latest: None,
                    ..self
                },
                Vec::new(),
            ),
            WM_POINTERLEAVE if self.pen_id.is_some_and(|id| id != pointer_id) => (self, Vec::new()),
            WM_POINTERLEAVE => (
                Self {
                    contact_end: self.contact_end,
                    ..Self::default()
                },
                Vec::new(),
            ),
            _ => (self, Vec::new()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn decode_pen_packet_reads_pressure_tilt_rotation_eraser_and_barrel() {
        let pkt = RawPenPacket {
            pointer_id: 7,
            pointer_flags: POINTER_FLAG_INCONTACT,
            pen_flags: PEN_FLAG_BARREL | PEN_FLAG_INVERTED,
            pen_mask: PEN_MASK_PRESSURE | PEN_MASK_TILT_X | PEN_MASK_TILT_Y | PEN_MASK_ROTATION,
            pressure: 512,
            rotation: 90,
            tilt_x: -25,
            tilt_y: 40,
            pixel_x: 320,
            pixel_y: 240,
            dw_time: 1234,
        };
        let s = decode_pen_packet(&pkt);
        assert!((s.pressure - 0.5).abs() < 1e-6);
        assert_eq!(s.tilt_x, -25.0);
        assert_eq!(s.tilt_y, 40.0);
        assert_eq!(s.rotation, 90.0);
        assert!(s.eraser);
        assert!(s.barrel);
        assert_eq!(s.x_px, Some(320));
        assert_eq!(s.y_px, Some(240));
        assert_eq!(s.time_ms, Some(1234.0));
    }

    #[test]
    fn plain_contact_is_not_barrel_and_hover_pressure_is_zero() {
        let pkt = RawPenPacket {
            pointer_flags: POINTER_FLAG_FIRSTBUTTON,
            pen_flags: 0,
            pen_mask: 0,
            pressure: 0,
            ..Default::default()
        };
        let s = decode_pen_packet(&pkt);
        assert!(!s.barrel);
        assert!(!s.eraser);
        assert_eq!(s.pressure, 0.0);
    }

    #[test]
    fn pen_state_tracks_leave_only_for_active_pen() {
        let st = PenState::default();
        let (st, samples) = st.step(WM_POINTERDOWN, 5, |id| {
            vec![RawPenPacket {
                pointer_id: id,
                pointer_flags: POINTER_FLAG_INCONTACT,
                pen_flags: PEN_FLAG_INVERTED,
                pen_mask: PEN_MASK_PRESSURE,
                pressure: 768,
                ..Default::default()
            }]
        });
        assert_eq!(samples.len(), 1);
        assert_eq!(st.end, Some(true));
        assert_eq!(st.pen_id, Some(5));

        // Unrelated pointer leaving does not clear pen state.
        let (st2, _) = st.step(WM_POINTERLEAVE, 99, |_| Vec::new());
        assert_eq!(st2.pen_id, Some(5));

        // Active pen leaving clears proximity while keeping contact_end.
        let (st3, _) = st2.step(WM_POINTERLEAVE, 5, |_| Vec::new());
        assert_eq!(st3.pen_id, None);
        assert_eq!(st3.end, None);
        assert_eq!(st3.contact_end, Some(true));
    }

    #[test]
    fn hover_and_zero_pressure_contact_do_not_corrupt_pressure_stream() {
        let st = PenState::default();
        // Hovering update preserves proximity without queueing 0.0 pressure samples.
        let (st, samples) = st.step(WM_POINTERUPDATE, 9, |id| {
            vec![RawPenPacket {
                pointer_id: id,
                pointer_flags: 0,
                pen_flags: PEN_FLAG_BARREL,
                pen_mask: PEN_MASK_PRESSURE,
                pressure: 0,
                ..Default::default()
            }]
        });
        assert!(samples.is_empty());
        assert!(st.barrel);
        assert_eq!(st.latest, None);
        assert_eq!(st.pen_id, Some(9));

        // Active contact with zero sensor reading preserves minimum contact pressure floor.
        let (st, samples) = st.step(WM_POINTERDOWN, 9, |id| {
            vec![RawPenPacket {
                pointer_id: id,
                pointer_flags: POINTER_FLAG_INCONTACT,
                pen_mask: PEN_MASK_PRESSURE,
                pressure: 0,
                ..Default::default()
            }]
        });
        assert_eq!(samples.len(), 1);
        assert!(samples[0].pressure >= MIN_CONTACT_PRESSURE);

        // Unrelated touch pointer up or empty-packet update does not alter active stylus state or barrel.
        let (st2, _) = st.step(WM_POINTERUPDATE, 42 | (0xFFFF_usize << 16), |_| Vec::new());
        assert!(st2.latest.is_some());
        assert!(!st2.barrel);
        assert_eq!(st2.pen_id, Some(9));
        let (st2, _) = st2.step(WM_POINTERUP, 42, |_| Vec::new());
        assert!(st2.latest.is_some());

        // Stylus lift ends contact while keeping pen_id active until pointer leave.
        let (st_hover, _) = st2.step(WM_POINTERUP, 9, |_| Vec::new());
        assert_eq!(st_hover.latest, None);
        assert_eq!(st_hover.pen_id, Some(9));
        let (st_left, _) = st_hover.step(WM_POINTERLEAVE, 9, |_| Vec::new());
        assert_eq!(st_left.pen_id, None);

        // Capture change for active pen clears latest and barrel cleanly.
        let (st3, _) = st2.step(WM_POINTERCAPTURECHANGED, 9, |_| Vec::new());
        assert_eq!(st3.latest, None);
        assert!(!st3.barrel);
    }

    #[test]
    fn promoted_mouse_signature_and_stale_pen_recovery_work_cleanly() {
        assert!(is_pointer_promoted_mouse_extra_info(
            0xFF51_5700_u32 as isize
        ));
        assert!(is_pointer_promoted_mouse_extra_info(
            0xFF51_5780_u32 as isize
        ));
        assert!(is_pointer_promoted_mouse_extra_info(
            (0xFF51_5702_u32 as i32) as isize
        ));
        assert!(!is_pointer_promoted_mouse_extra_info(0));
        assert!(!is_pointer_promoted_mouse_extra_info(0x1234_5600));

        assert!(!should_clear_stale_pen_on_mouse(false, false, false, None));
        assert!(!should_clear_stale_pen_on_mouse(
            true,
            true,
            false,
            Some(500)
        ));
        assert!(should_clear_stale_pen_on_mouse(true, false, false, Some(5)));
        assert!(!should_clear_stale_pen_on_mouse(
            true,
            false,
            true,
            Some(20)
        ));
        assert!(should_clear_stale_pen_on_mouse(
            true,
            false,
            true,
            Some(STALE_PEN_MOUSE_FALLBACK_MS),
        ));
        assert!(!should_clear_stale_pen_on_mouse(true, false, true, None));
    }

    #[test]
    fn dib_to_bmp_bytes_wraps_standard_and_bitfield_dibs_and_rejects_hostile() {
        assert_eq!(dib_to_bmp_bytes(&[]), None);
        assert_eq!(dib_to_bmp_bytes(&[0; 39]), None);

        // Header size larger than buffer is rejected.
        let mut bad_hdr = vec![0_u8; 44];
        bad_hdr[0..4].copy_from_slice(&100_u32.to_le_bytes());
        assert_eq!(dib_to_bmp_bytes(&bad_hdr), None);

        // Hostile clr_used overflowing pixel offset past file_size is rejected.
        let mut bad_clr = vec![0_u8; 44];
        bad_clr[0..4].copy_from_slice(&40_u32.to_le_bytes());
        bad_clr[32..36].copy_from_slice(&500_u32.to_le_bytes());
        assert_eq!(dib_to_bmp_bytes(&bad_clr), None);

        // Standard 32bpp BI_RGB 1x1 DIB (40 byte header + 4 byte pixel).
        let mut dib32 = vec![0_u8; 44];
        dib32[0..4].copy_from_slice(&40_u32.to_le_bytes());
        dib32[4..8].copy_from_slice(&1_i32.to_le_bytes());
        dib32[8..12].copy_from_slice(&1_i32.to_le_bytes());
        dib32[12..14].copy_from_slice(&1_u16.to_le_bytes());
        dib32[14..16].copy_from_slice(&32_u16.to_le_bytes());
        let bmp32 = dib_to_bmp_bytes(&dib32).unwrap();
        assert_eq!(&bmp32[0..2], b"BM");
        assert_eq!(u32::from_le_bytes(bmp32[2..6].try_into().unwrap()), 58);
        assert_eq!(u32::from_le_bytes(bmp32[10..14].try_into().unwrap()), 54);

        // 32bpp BI_BITFIELDS DIB (40 byte header + 12 byte masks + 4 byte pixel).
        let mut dib_bf = vec![0_u8; 56];
        dib_bf[0..4].copy_from_slice(&40_u32.to_le_bytes());
        dib_bf[14..16].copy_from_slice(&32_u16.to_le_bytes());
        dib_bf[16..20].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
        let bmp_bf = dib_to_bmp_bytes(&dib_bf).unwrap();
        assert_eq!(u32::from_le_bytes(bmp_bf[10..14].try_into().unwrap()), 66);

        // 8bpp paletted DIB (40 byte header + 256*4 palette + 4 byte pixel).
        let mut dib8 = vec![0_u8; 40 + 1024 + 4];
        dib8[0..4].copy_from_slice(&40_u32.to_le_bytes());
        dib8[14..16].copy_from_slice(&8_u16.to_le_bytes());
        let bmp8 = dib_to_bmp_bytes(&dib8).unwrap();
        assert_eq!(
            u32::from_le_bytes(bmp8[10..14].try_into().unwrap()),
            14 + 40 + 1024
        );

        // BITMAPV5HEADER with BI_RGB and bV5AlphaMask = 0xFF00_0000 is promoted to BI_BITFIELDS.
        let mut dib_v5 = vec![0_u8; 124 + 4];
        dib_v5[0..4].copy_from_slice(&124_u32.to_le_bytes());
        dib_v5[14..16].copy_from_slice(&32_u16.to_le_bytes());
        dib_v5[52..56].copy_from_slice(&0xFF00_0000_u32.to_le_bytes());
        let bmp_v5 = dib_to_bmp_bytes(&dib_v5).unwrap();
        assert_eq!(u32::from_le_bytes(bmp_v5[10..14].try_into().unwrap()), 138);
        assert_eq!(
            u32::from_le_bytes(bmp_v5[14 + 16..14 + 20].try_into().unwrap()),
            BI_BITFIELDS
        );

        // BITMAPINFOHEADER with BI_ALPHABITFIELDS (6) upgrades to 56-byte V3 BITMAPV3INFOHEADER.
        let mut dib_ab = vec![0_u8; 40 + 16 + 4];
        dib_ab[0..4].copy_from_slice(&40_u32.to_le_bytes());
        dib_ab[4..8].copy_from_slice(&1_i32.to_le_bytes());
        dib_ab[8..12].copy_from_slice(&1_i32.to_le_bytes());
        dib_ab[14..16].copy_from_slice(&32_u16.to_le_bytes());
        dib_ab[16..20].copy_from_slice(&BI_ALPHABITFIELDS.to_le_bytes());
        dib_ab[40..44].copy_from_slice(&0x00FF_0000_u32.to_le_bytes());
        dib_ab[44..48].copy_from_slice(&0x0000_FF00_u32.to_le_bytes());
        dib_ab[48..52].copy_from_slice(&0x0000_00FF_u32.to_le_bytes());
        dib_ab[52..56].copy_from_slice(&0xFF00_0000_u32.to_le_bytes());
        let bmp_ab = dib_to_bmp_bytes(&dib_ab).unwrap();
        assert_eq!(u32::from_le_bytes(bmp_ab[10..14].try_into().unwrap()), 70);
        assert_eq!(u32::from_le_bytes(bmp_ab[14..18].try_into().unwrap()), 56);
        assert_eq!(
            u32::from_le_bytes(bmp_ab[14 + 16..14 + 20].try_into().unwrap()),
            BI_BITFIELDS
        );

        // BITMAPV5HEADER with BI_BITFIELDS and duplicate trailing 12 mask bytes skips them cleanly.
        let mut dib_v5_dup = vec![0_u8; 124 + 12 + 4];
        dib_v5_dup[0..4].copy_from_slice(&124_u32.to_le_bytes());
        dib_v5_dup[4..8].copy_from_slice(&1_i32.to_le_bytes());
        dib_v5_dup[8..12].copy_from_slice(&1_i32.to_le_bytes());
        dib_v5_dup[14..16].copy_from_slice(&32_u16.to_le_bytes());
        dib_v5_dup[16..20].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
        let masks = [
            0x00FF_0000_u32.to_le_bytes(),
            0x0000_FF00_u32.to_le_bytes(),
            0x0000_00FF_u32.to_le_bytes(),
        ]
        .concat();
        dib_v5_dup[40..52].copy_from_slice(&masks);
        dib_v5_dup[124..136].copy_from_slice(&masks);
        let bmp_v5_dup = dib_to_bmp_bytes(&dib_v5_dup).unwrap();
        assert_eq!(
            u32::from_le_bytes(bmp_v5_dup[10..14].try_into().unwrap()),
            14 + 124 + 12
        );

        // BI_PNG embedded inside DIB returns raw PNG payload directly.
        let mut dib_png = vec![0_u8; 40];
        dib_png[0..4].copy_from_slice(&40_u32.to_le_bytes());
        dib_png[16..20].copy_from_slice(&BI_PNG.to_le_bytes());
        dib_png.extend_from_slice(b"\x89PNG\r\n\x1a\nABCD");
        assert_eq!(
            dib_to_bmp_bytes(&dib_png).unwrap(),
            b"\x89PNG\r\n\x1a\nABCD"
        );

        // PNG trimming stops cleanly at IEND and drops trailing heap padding.
        let png_with_pad = [
            &b"\x89PNG\r\n\x1a\n"[..],
            &0_u32.to_be_bytes(),
            b"IEND",
            &[0xAE, 0x42, 0x60, 0x82],
            &[0, 0, 0, 0],
        ]
        .concat();
        let trimmed = trim_png_trailing_bytes(png_with_pad.clone());
        assert_eq!(trimmed.len(), png_with_pad.len() - 4);
    }

    #[test]
    fn supported_image_extension_checks_case_insensitively() {
        use std::path::Path;
        assert!(is_supported_image_extension(Path::new("photo.png")));
        assert!(is_supported_image_extension(Path::new("SCAN.JPG")));
        assert!(is_supported_image_extension(Path::new("pic.Jpeg")));
        assert!(is_supported_image_extension(Path::new("browser.JFIF")));
        assert!(is_supported_image_extension(Path::new("web.WEBP")));
        assert!(is_supported_image_extension(Path::new("anim.gif")));
        assert!(is_supported_image_extension(Path::new("scan.TIFF")));
        assert!(is_supported_image_extension(Path::new("raw.bmp")));
        assert!(is_supported_image_extension(Path::new("icon.ICO")));
        assert!(!is_supported_image_extension(Path::new("notes.ncraft")));
        assert!(!is_supported_image_extension(Path::new("noext")));
    }

    #[test]
    fn secondary_barrel_decoding_and_pointer_up_preservation() {
        let pkt = RawPenPacket {
            pointer_id: 12,
            pointer_flags: POINTER_FLAG_INCONTACT | POINTER_FLAG_THIRDBUTTON,
            pen_flags: 0,
            pen_mask: PEN_MASK_PRESSURE,
            pressure: 512,
            ..Default::default()
        };
        let s = decode_pen_packet(&pkt);
        assert!(s.barrel_secondary);
        assert!(!s.barrel);

        let pkt_fourth = RawPenPacket {
            pointer_flags: POINTER_FLAG_FOURTHBUTTON,
            ..pkt
        };
        assert!(decode_pen_packet(&pkt_fourth).barrel_secondary);

        // Lifting pen tip while holding primary or secondary barrel button preserves held state.
        let st = PenState::default();
        let (st, _) = st.step(WM_POINTERDOWN, 12, |id| {
            vec![RawPenPacket {
                pointer_id: id,
                pointer_flags: POINTER_FLAG_INCONTACT | POINTER_FLAG_THIRDBUTTON,
                pen_flags: PEN_FLAG_BARREL,
                pen_mask: PEN_MASK_PRESSURE,
                pressure: 400,
                ..Default::default()
            }]
        });
        assert!(st.barrel && st.barrel_secondary);

        let (st_up, _) = st.step(WM_POINTERUP, 12, |id| {
            vec![RawPenPacket {
                pointer_id: id,
                pointer_flags: POINTER_FLAG_THIRDBUTTON,
                pen_flags: PEN_FLAG_BARREL,
                ..Default::default()
            }]
        });
        assert_eq!(st_up.latest, None);
        assert!(st_up.barrel);
        assert!(st_up.barrel_secondary);
    }

    #[test]
    fn sanitize_pen_device_name_handles_normal_nul_control_empty_and_long_names() {
        let normal: Vec<u16> = "Active Pen   Digitizer\0IGNORED".encode_utf16().collect();
        assert_eq!(
            sanitize_pen_device_name(&normal, false),
            "Active Pen Digitizer"
        );

        let with_ctrl: Vec<u16> = "  USB\x01\x07  Pen \t\nTablet  \0".encode_utf16().collect();
        assert_eq!(sanitize_pen_device_name(&with_ctrl, true), "USB Pen Tablet");

        assert_eq!(
            sanitize_pen_device_name(&[0, 0], false),
            "Integrated Stylus"
        );
        assert_eq!(
            sanitize_pen_device_name(&[b' ' as u16, 0], true),
            "External Pen Tablet"
        );

        let long: Vec<u16> = "A".repeat(100).encode_utf16().collect();
        let sanitized_long = sanitize_pen_device_name(&long, true);
        assert_eq!(sanitized_long.chars().count(), 64);
    }
}
