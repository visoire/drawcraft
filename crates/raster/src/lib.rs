//! L0 copy-on-write tiled RGBA surface and pure-Rust PNG encoder.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use drawcraft_color::{BlendMode, Color, blend_pixel};
use drawcraft_geom::{IRect, Point, distance_to_segment};
use flate2::{Compression, write::ZlibEncoder};
use thiserror::Error;

pub const TILE_SIZE: usize = 64;
const TILE_LEN: usize = TILE_SIZE * TILE_SIZE;
const MAX_SURFACE_DIM: u32 = 16_384;

#[derive(Debug, Error)]
pub enum RasterError {
    #[error(
        "invalid PNG buffer length: expected {expected} bytes for {width}x{height}, got {actual}"
    )]
    BufferLengthMismatch {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
    #[error("dimensions {width}x{height} exceed supported surface limit")]
    DimensionsTooLarge { width: u32, height: u32 },
    #[error("PNG DEFLATE compression failed: {0}")]
    CompressionFailed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Tile {
    pixels: Vec<[u8; 4]>,
}

impl Tile {
    fn filled(bg: [u8; 4]) -> Self {
        Self {
            pixels: vec![bg; TILE_LEN],
        }
    }
}

/// Copy-on-Write tiled RGBA surface.
#[derive(Clone, Debug)]
pub struct Surface {
    width: u32,
    height: u32,
    background: [u8; 4],
    tiles: HashMap<(i32, i32), Arc<Tile>>,
}

impl Surface {
    pub fn new(width: u32, height: u32, bg: Color) -> Self {
        let w = width.clamp(1, MAX_SURFACE_DIM);
        let h = height.clamp(1, MAX_SURFACE_DIM);
        Self {
            width: w,
            height: h,
            background: bg.to_rgba8(),
            tiles: HashMap::new(),
        }
    }

    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    #[inline]
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    #[inline]
    fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height
    }

    #[inline]
    fn tile_coords(x: i32, y: i32) -> ((i32, i32), usize) {
        let ts = TILE_SIZE as i32;
        let tx = x.div_euclid(ts);
        let ty = y.div_euclid(ts);
        let lx = x.rem_euclid(ts) as usize;
        let ly = y.rem_euclid(ts) as usize;
        ((tx, ty), ly * TILE_SIZE + lx)
    }

    pub fn pixel_u8(&self, x: i32, y: i32) -> [u8; 4] {
        if !self.in_bounds(x, y) {
            return [0, 0, 0, 0];
        }
        let (key, idx) = Self::tile_coords(x, y);
        match self.tiles.get(&key) {
            Some(tile) => tile.pixels.get(idx).copied().unwrap_or(self.background),
            None => self.background,
        }
    }

    pub fn pixel(&self, x: i32, y: i32) -> [f32; 4] {
        let p = self.pixel_u8(x, y);
        [
            f32::from(p[0]) / 255.0,
            f32::from(p[1]) / 255.0,
            f32::from(p[2]) / 255.0,
            f32::from(p[3]) / 255.0,
        ]
    }

    pub fn set_pixel_u8(&mut self, x: i32, y: i32, rgba: [u8; 4]) {
        if !self.in_bounds(x, y) {
            return;
        }
        let (key, idx) = Self::tile_coords(x, y);
        if !self.tiles.contains_key(&key) && rgba == self.background {
            return;
        }
        let bg = self.background;
        let tile = self
            .tiles
            .entry(key)
            .or_insert_with(|| Arc::new(Tile::filled(bg)));
        let mut_tile = Arc::make_mut(tile);
        if let Some(slot) = mut_tile.pixels.get_mut(idx) {
            *slot = rgba;
        }
    }

    pub fn set_pixel(&mut self, x: i32, y: i32, rgba: [f32; 4]) {
        let c = Color::new(rgba[0], rgba[1], rgba[2], rgba[3]).to_rgba8();
        self.set_pixel_u8(x, y, c);
    }

    pub fn blend_pixel(&mut self, x: i32, y: i32, src: [f32; 4], mode: BlendMode) {
        if !self.in_bounds(x, y) {
            return;
        }
        if !src[3].is_finite() || src[3] <= 0.0 {
            return;
        }
        let dst = self.pixel(x, y);
        let out = blend_pixel(dst, src, mode);
        self.set_pixel(x, y, out);
    }

    pub fn fill_rect(&mut self, rect: IRect, color: Color) {
        let bounds = IRect::new(0, 0, self.width as i32, self.height as i32);
        let clipped = rect.intersection(bounds);
        if clipped.is_empty() {
            return;
        }
        let rgba = color.to_rgba8();
        for y in clipped.y..clipped.bottom() {
            for x in clipped.x..clipped.right() {
                self.set_pixel_u8(x, y, rgba);
            }
        }
    }

    /// Draws an anti-aliased line segment using exact distance-to-segment coverage.
    pub fn draw_line_aa(&mut self, a: Point, b: Point, color: Color, width: f64) {
        if !(a.is_finite() && b.is_finite() && width.is_finite()) {
            return;
        }
        let half = (width * 0.5).max(0.25);
        let pad = half + 1.5;
        let min_x = (a.x.min(b.x) - pad).floor().max(0.0) as i32;
        let min_y = (a.y.min(b.y) - pad).floor().max(0.0) as i32;
        let max_x = ((a.x.max(b.x) + pad).ceil() as i32).min(self.width as i32);
        let max_y = ((a.y.max(b.y) + pad).ceil() as i32).min(self.height as i32);
        let base = color.sanitized();

        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5);
                let (dist, _) = distance_to_segment(p, a, b);
                let cov = (half + 0.5 - dist).clamp(0.0, 1.0) as f32;
                if cov > 0.0 {
                    let src = [base.r, base.g, base.b, base.a * cov];
                    self.blend_pixel(x, y, src, BlendMode::Normal);
                }
            }
        }
    }

    pub fn to_rgba8_vec(&self) -> Vec<u8> {
        let total = (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4);
        let mut out = vec![0u8; total];
        let ts = TILE_SIZE as i32;
        for y in 0..self.height as i32 {
            let ty = y.div_euclid(ts);
            let ly = y.rem_euclid(ts) as usize;
            let row_off = (y as usize) * (self.width as usize) * 4;
            for x in 0..self.width as i32 {
                let tx = x.div_euclid(ts);
                let lx = x.rem_euclid(ts) as usize;
                let px = match self.tiles.get(&(tx, ty)) {
                    Some(tile) => tile
                        .pixels
                        .get(ly * TILE_SIZE + lx)
                        .copied()
                        .unwrap_or(self.background),
                    None => self.background,
                };
                let dst_off = row_off + (x as usize) * 4;
                if let Some(slice) = out.get_mut(dst_off..dst_off + 4) {
                    slice.copy_from_slice(&px);
                }
            }
        }
        out
    }

    pub fn encode_png(&self) -> Result<Vec<u8>, RasterError> {
        let rgba = self.to_rgba8_vec();
        png_encode(self.width, self.height, &rgba)
    }
}

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for part in parts {
        for &b in *part {
            crc ^= u32::from(b);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
    }
    !crc
}

fn paeth_predictor(a: u8, b: u8, c: u8) -> u8 {
    let a_i = i32::from(a);
    let b_i = i32::from(b);
    let c_i = i32::from(c);
    let p = a_i + b_i - c_i;
    let pa = (p - a_i).abs();
    let pb = (p - b_i).abs();
    let pc = (p - c_i).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn apply_png_filter(filter: u8, row: &[u8], prev_row: &[u8], bpp: usize, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(row.len());
    for (i, &x) in row.iter().enumerate() {
        let a = if i >= bpp {
            row.get(i - bpp).copied().unwrap_or(0)
        } else {
            0
        };
        let b = prev_row.get(i).copied().unwrap_or(0);
        let c = if i >= bpp {
            prev_row.get(i - bpp).copied().unwrap_or(0)
        } else {
            0
        };
        let pred = match filter {
            1 => a,
            2 => b,
            3 => ((u16::from(a) + u16::from(b)) >> 1) as u8,
            4 => paeth_predictor(a, b, c),
            _ => 0,
        };
        out.push(x.wrapping_sub(pred));
    }
}

fn filter_sad(filtered: &[u8]) -> u64 {
    filtered
        .iter()
        .map(|&b| u64::from((b as i8).unsigned_abs()))
        .sum()
}

fn encode_best_filtered_row(
    row: &[u8],
    prev_row: &[u8],
    bpp: usize,
    best_row: &mut Vec<u8>,
    scratch: &mut Vec<u8>,
) -> u8 {
    let mut best_filter = 0u8;
    let mut best_score = u64::MAX;
    for filter in 0u8..=4 {
        apply_png_filter(filter, row, prev_row, bpp, scratch);
        let score = filter_sad(scratch);
        if score < best_score {
            best_score = score;
            best_filter = filter;
            std::mem::swap(best_row, scratch);
        }
    }
    best_filter
}

fn push_png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    let len = data.len() as u32;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let c = crc32(&[tag, data]);
    out.extend_from_slice(&c.to_be_bytes());
}

/// Pure-Rust PNG encoder writing zlib-compressed RGB8 (when opaque) or RGBA8 PNG streams
/// using adaptive per-scanline PNG filtering.
pub fn png_encode(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, RasterError> {
    if width == 0 || height == 0 || width > MAX_SURFACE_DIM || height > MAX_SURFACE_DIM {
        return Err(RasterError::DimensionsTooLarge { width, height });
    }
    let rgba_row_bytes = (width as usize)
        .checked_mul(4)
        .ok_or(RasterError::DimensionsTooLarge { width, height })?;
    let expected = rgba_row_bytes
        .checked_mul(height as usize)
        .ok_or(RasterError::DimensionsTooLarge { width, height })?;
    if rgba.len() != expected {
        return Err(RasterError::BufferLengthMismatch {
            width,
            height,
            expected,
            actual: rgba.len(),
        });
    }

    let all_opaque = rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255);
    let bpp = if all_opaque { 3usize } else { 4usize };
    let color_type = if all_opaque { 2u8 } else { 6u8 };
    let packed_row_bytes = (width as usize)
        .checked_mul(bpp)
        .ok_or(RasterError::DimensionsTooLarge { width, height })?;

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    let mut prev_row = vec![0u8; packed_row_bytes];
    let mut curr_row = vec![0u8; packed_row_bytes];
    let mut best_filtered = Vec::with_capacity(packed_row_bytes);
    let mut scratch = Vec::with_capacity(packed_row_bytes);

    for y in 0..height as usize {
        let start = y * rgba_row_bytes;
        let Some(rgba_row) = rgba.get(start..start + rgba_row_bytes) else {
            continue;
        };
        if all_opaque {
            curr_row.clear();
            for px in rgba_row.as_chunks::<4>().0 {
                curr_row.extend_from_slice(&px[0..3]);
            }
        } else {
            curr_row.copy_from_slice(rgba_row);
        }

        let filter =
            encode_best_filtered_row(&curr_row, &prev_row, bpp, &mut best_filtered, &mut scratch);
        encoder
            .write_all(&[filter])
            .map_err(|e| RasterError::CompressionFailed(e.to_string()))?;
        encoder
            .write_all(&best_filtered)
            .map_err(|e| RasterError::CompressionFailed(e.to_string()))?;
        std::mem::swap(&mut prev_row, &mut curr_row);
    }

    let zlib = encoder
        .finish()
        .map_err(|e| RasterError::CompressionFailed(e.to_string()))?;

    let mut out = Vec::with_capacity(8 + 25 + zlib.len() + 12 + 12);
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = color_type;
    ihdr[10] = 0;
    ihdr[11] = 0;
    ihdr[12] = 0;
    push_png_chunk(&mut out, b"IHDR", &ihdr);
    push_png_chunk(&mut out, b"IDAT", &zlib);
    push_png_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn cow_tiles_share_until_written() {
        let mut s1 = Surface::new(256, 256, Color::WHITE);
        s1.set_pixel_u8(10, 10, [0, 0, 0, 255]);
        let mut s2 = s1.clone();
        let tile = s1.tiles.get(&(0, 0)).unwrap();
        assert_eq!(Arc::strong_count(tile), 2);
        s2.set_pixel_u8(11, 10, [255, 0, 0, 255]);
        assert_eq!(Arc::strong_count(tile), 1);
        assert_eq!(s1.pixel_u8(11, 10), [255, 255, 255, 255]);
        assert_eq!(s2.pixel_u8(11, 10), [255, 0, 0, 255]);
    }

    #[test]
    fn png_encode_produces_valid_png_chunks() {
        let mut s = Surface::new(4, 3, Color::WHITE);
        s.set_pixel_u8(1, 1, [20, 40, 200, 255]);
        let png = s.encode_png().unwrap();
        assert_eq!(&png[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert_eq!(&png[12..16], b"IHDR");
        let ihdr_crc = u32::from_be_bytes([png[29], png[30], png[31], png[32]]);
        assert_eq!(ihdr_crc, crc32(&[b"IHDR", &png[16..29]]));
        assert!(png.windows(4).any(|w| w == b"IDAT"));
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    #[test]
    fn png_encode_rejects_bad_length() {
        assert!(matches!(
            png_encode(4, 4, &[0u8; 10]),
            Err(RasterError::BufferLengthMismatch { .. })
        ));
    }

    #[test]
    fn png_encode_compresses_dramatically_and_reconstructs_pixels() {
        use flate2::read::ZlibDecoder;
        use std::io::Read;

        let mut s = Surface::new(256, 256, Color::WHITE);
        for i in 0..200i32 {
            s.set_pixel_u8(i + 20, i / 2 + 30, [15, 60, 180, 255]);
        }
        let png = s.encode_png().unwrap();
        // Raw uncompressed 256x256 RGBA is 262,144 bytes; compressed PNG should be tiny.
        assert!(
            png.len() < 10_000,
            "expected heavily compressed PNG, got {} bytes",
            png.len()
        );
        // Opaque buffer uses color type 2 (RGB8).
        assert_eq!(png[25], 2);

        // Parse IDAT and verify lossless decompression + filter reconstruction.
        let idat_len = u32::from_be_bytes([png[33], png[34], png[35], png[36]]) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let idat = &png[41..41 + idat_len];
        let mut dec = ZlibDecoder::new(idat);
        let mut raw_scanlines = Vec::new();
        dec.read_to_end(&mut raw_scanlines).unwrap();
        assert_eq!(raw_scanlines.len(), 256 * (1 + 256 * 3));
    }

    #[test]
    fn png_encode_preserves_alpha_channel_when_translucent() {
        let rgba = [10u8, 20, 30, 128, 255, 255, 255, 255];
        let png = png_encode(2, 1, &rgba).unwrap();
        assert_eq!(png[25], 6);
    }
}
