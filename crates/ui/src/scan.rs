//! Document-scan cleanup: turns a camera photo of a sheet of paper into a flat, high-contrast
//! page (grayscale, paper mapped to white, ink pushed toward black).
//!
//! Pure and panic-free: every input is treated as hostile (empty, truncated, enormous).

use image::imageops::FilterType;
use std::io::Cursor;

/// Longest side kept for a scan; larger photos are downscaled to bound memory on iPadOS Safari.
const MAX_SCAN_SIDE: u32 = 2400;
/// Share of the darkest pixels treated as pure ink.
const BLACK_PERCENTILE: f32 = 0.01;
/// Pixels brighter than this percentile of the image are treated as paper.
const WHITE_PERCENTILE: f32 = 0.80;

/// Cleans up a photographed document and returns it as PNG bytes, or `None` when `bytes` is not a
/// decodable image.
#[must_use]
pub(crate) fn enhance_document_scan(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.is_empty() {
        return None;
    }
    let decoded = image::load_from_memory(bytes).ok()?;
    if decoded.width() == 0 || decoded.height() == 0 {
        return None;
    }
    let decoded = if decoded.width().max(decoded.height()) > MAX_SCAN_SIDE {
        decoded.resize(MAX_SCAN_SIDE, MAX_SCAN_SIDE, FilterType::Triangle)
    } else {
        decoded
    };
    let mut gray = decoded.to_luma8();

    let mut histogram = [0u64; 256];
    for px in gray.pixels() {
        if let Some(slot) = histogram.get_mut(usize::from(px.0[0])) {
            *slot += 1;
        }
    }
    let total: u64 = histogram.iter().sum();
    if total == 0 {
        return None;
    }
    let black = percentile_level(&histogram, total, BLACK_PERCENTILE);
    let white = percentile_level(&histogram, total, WHITE_PERCENTILE).max(black.saturating_add(8));

    let span = f32::from(white.saturating_sub(black)).max(1.0);
    for px in gray.pixels_mut() {
        let v = (f32::from(px.0[0]) - f32::from(black)) / span;
        // A mild curve keeps pencil and highlighter strokes from clipping to solid black.
        let mapped = v.clamp(0.0, 1.0).powf(1.15);
        px.0[0] = (mapped * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    let mut out = Vec::new();
    gray.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// Smallest gray level whose cumulative share of pixels reaches `fraction`.
fn percentile_level(histogram: &[u64; 256], total: u64, fraction: f32) -> u8 {
    let target = (total as f64 * f64::from(fraction.clamp(0.0, 1.0))).ceil() as u64;
    let mut seen = 0u64;
    for (level, count) in histogram.iter().enumerate() {
        seen += count;
        if seen >= target {
            return u8::try_from(level).unwrap_or(u8::MAX);
        }
    }
    u8::MAX
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    fn encode(img: &GrayImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn dingy_paper_becomes_white_and_ink_stays_dark() {
        // Gray-ish paper (190) with a dark text block (40).
        let mut img = GrayImage::from_pixel(80, 80, Luma([190]));
        for y in 30..40 {
            for x in 10..70 {
                img.put_pixel(x, y, Luma([40]));
            }
        }
        let out = enhance_document_scan(&encode(&img)).unwrap();
        let cleaned = image::load_from_memory(&out).unwrap().to_luma8();
        assert_eq!(cleaned.get_pixel(2, 2).0[0], 255, "paper maps to white");
        assert!(cleaned.get_pixel(40, 35).0[0] < 60, "ink stays dark");
    }

    #[test]
    fn hostile_input_is_rejected_without_panicking() {
        assert!(enhance_document_scan(&[]).is_none());
        assert!(enhance_document_scan(b"not an image").is_none());
        let png = encode(&GrayImage::from_pixel(8, 8, Luma([128])));
        assert!(enhance_document_scan(&png[..png.len() / 2]).is_none());
    }

    #[test]
    fn flat_images_survive_and_large_photos_are_bounded() {
        let flat = encode(&GrayImage::from_pixel(16, 16, Luma([200])));
        assert!(enhance_document_scan(&flat).is_some());
        let big = encode(&GrayImage::from_pixel(MAX_SCAN_SIDE + 600, 40, Luma([220])));
        let out = enhance_document_scan(&big).unwrap();
        let decoded = image::load_from_memory(&out).unwrap();
        assert!(decoded.width() <= MAX_SCAN_SIDE);
    }
}
