//! `cargo xtask branding` - regenerate the original DrawCraft app icon set (pure Rust, no
//! external tools). Flat design from `DESIGN.md`: accent-blue tile, white page, ruled lines
//! and one handwriting stroke. No gradients.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::{Compression, Crc, write::ZlibEncoder};

const ACCENT: [f32; 3] = [0x0B as f32 / 255.0, 0x5F as f32 / 255.0, 1.0];
const PAGE: [f32; 3] = [1.0, 1.0, 1.0];
const RULE: [f32; 3] = [0xC7 as f32 / 255.0, 0xD6 as f32 / 255.0, 1.0];

/// One output icon.
struct Spec {
    path: &'static str,
    size: usize,
    /// Full-bleed square tile with content inside the safe zone (PWA maskable icons).
    maskable: bool,
    /// Write raw RGBA bytes instead of a PNG (window icon for the desktop shell).
    raw_rgba: bool,
}

const fn png(path: &'static str, size: usize) -> Spec {
    Spec {
        path,
        size,
        maskable: false,
        raw_rgba: false,
    }
}

const SPECS: &[Spec] = &[
    png("crates/ui/assets/branding/drawcraft-128.png", 128),
    png("apps/drawcraft/assets/branding/drawcraft-48.png", 48),
    Spec {
        path: "apps/drawcraft/assets/branding/drawcraft-256.rgba",
        size: 256,
        maskable: false,
        raw_rgba: true,
    },
    png("apps/drawcraft-web/assets/icons/favicon-32.png", 32),
    png(
        "apps/drawcraft-web/assets/icons/apple-touch-icon-180.png",
        180,
    ),
    png("apps/drawcraft-web/assets/icons/icon-192.png", 192),
    png("apps/drawcraft-web/assets/icons/icon-512.png", 512),
    Spec {
        path: "apps/drawcraft-web/assets/icons/icon-maskable-512.png",
        size: 512,
        maskable: true,
        raw_rgba: false,
    },
];

/// Signed distance (in unit space) to a rounded rectangle.
fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - hw + r;
    let qy = (py - cy).abs() - hh + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Distance to the segment `a-b`.
fn sd_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let (apx, apy) = (px - a.0, py - a.1);
    let len2 = abx * abx + aby * aby;
    let t = if len2 > 0.0 {
        ((apx * abx + apy * aby) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (px - (a.0 + abx * t)).hypot(py - (a.1 + aby * t))
}

/// Straight RGBA8 icon.
pub fn render(size: usize, maskable: bool) -> Vec<u8> {
    let n = size as f32;
    let content_scale = if maskable { 0.74 } else { 1.0 };
    // Distances are in unit space; one pixel is 1/n units, so coverage = 0.5 - d * n.
    let coverage = |d: f32| (0.5 - d * n).clamp(0.0, 1.0);
    let wave: Vec<(f32, f32)> = (0..=48)
        .map(|i| {
            let x = 0.30 + 0.40 * i as f32 / 48.0;
            let y = 0.685 + 0.035 * ((x - 0.30) / 0.40 * std::f32::consts::TAU * 1.5).sin();
            (x, y)
        })
        .collect();
    let mut out = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let (ux, uy) = ((x as f32 + 0.5) / n, (y as f32 + 0.5) / n);
            // Content coordinates (shrunk around the center for maskable icons).
            let (cx, cy) = (
                (ux - 0.5) / content_scale + 0.5,
                (uy - 0.5) / content_scale + 0.5,
            );
            let mut rgb = [0.0f32; 3];
            let mut alpha = 0.0f32;
            let mut paint = |color: [f32; 3], cov: f32| {
                if cov <= 0.0 {
                    return;
                }
                let out_a = cov + alpha * (1.0 - cov);
                for (acc, c) in rgb.iter_mut().zip(color) {
                    *acc = (c * cov + *acc * alpha * (1.0 - cov)) / out_a;
                }
                alpha = out_a;
            };
            let radius = if maskable { 0.0 } else { 0.22 };
            paint(
                ACCENT,
                coverage(sd_round_rect(ux, uy, 0.5, 0.5, 0.5, 0.5, radius)),
            );
            // Distances measured in content space must be scaled back to unit space.
            let cov_c = |d: f32| coverage(d * content_scale);
            paint(
                PAGE,
                cov_c(sd_round_rect(cx, cy, 0.5, 0.5, 0.26, 0.32, 0.04)),
            );
            for line_y in [0.33, 0.45, 0.57] {
                paint(
                    RULE,
                    cov_c(sd_segment(cx, cy, (0.33, line_y), (0.67, line_y)) - 0.011),
                );
            }
            let stroke = wave
                .windows(2)
                .map(|w| sd_segment(cx, cy, w[0], w[1]))
                .fold(f32::MAX, f32::min);
            paint(ACCENT, cov_c(stroke - 0.017));

            let i = (y * size + x) * 4;
            for (c, v) in rgb.iter().enumerate() {
                out[i + c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            out[i + 3] = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
}

/// Encodes straight RGBA8 pixels as a PNG.
pub fn encode_png(size: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    if rgba.len() != size * size * 4 {
        return Err("pixel buffer does not match the icon size".to_string());
    }
    let mut raw = Vec::with_capacity(size * (size * 4 + 1));
    for row in rgba.chunks_exact(size * 4) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
    enc.write_all(&raw).map_err(|e| e.to_string())?;
    let idat = enc.finish().map_err(|e| e.to_string())?;

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

/// Regenerates every icon under `root`.
pub fn run(root: &Path) -> Result<(), String> {
    for spec in SPECS {
        let rgba = render(spec.size, spec.maskable);
        let bytes = if spec.raw_rgba {
            rgba
        } else {
            encode_png(spec.size, &rgba)?
        };
        let path: PathBuf = root.join(spec.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("branding: {} ({} bytes)", spec.path, bytes.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(buf: &[u8], size: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * size + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn tile_is_accent_blue_with_a_white_page_and_transparent_corners() {
        let size = 128;
        let img = render(size, false);
        assert_eq!(pixel(&img, size, 64, 12)[3], 255);
        assert_eq!(&pixel(&img, size, 64, 12)[..3], &[0x0B, 0x5F, 0xFF]);
        assert_eq!(
            pixel(&img, size, 0, 0)[3],
            0,
            "rounded corner is transparent"
        );
        assert_eq!(
            &pixel(&img, size, 64, 30)[..3],
            &[255, 255, 255],
            "page is white"
        );
    }

    #[test]
    fn maskable_icon_is_full_bleed() {
        let size = 64;
        let img = render(size, true);
        assert_eq!(pixel(&img, size, 0, 0), [0x0B, 0x5F, 0xFF, 255]);
        assert_eq!(pixel(&img, size, 63, 63)[3], 255);
    }

    #[test]
    fn png_has_valid_signature_header_and_ending() -> Result<(), String> {
        let png = encode_png(16, &render(16, false))?;
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes([png[16], png[17], png[18], png[19]]), 16);
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        assert!(encode_png(16, &[0; 3]).is_err());
        Ok(())
    }
}
