//! L0 color representation, blend math, and curated palettes.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use serde::{Deserialize, Serialize};

pub mod theme;
pub use theme::{
    PDF_DARK_CEIL, PDF_DARK_FLOOR, ThemeMode, Tokens, invert_color_for_dark,
    invert_rgba8_buffer_for_dark, invert_rgba8_for_dark,
};

/// Straight RGBA color with components in `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Default for Color {
    fn default() -> Self {
        Self::BLACK
    }
}

impl Color {
    pub const TRANSPARENT: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
    pub const BLACK: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const WHITE: Self = Self {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };

    #[inline]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    #[inline]
    pub fn sanitized(self) -> Self {
        let f = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        Self {
            r: f(self.r),
            g: f(self.g),
            b: f(self.b),
            a: f(self.a),
        }
    }

    #[inline]
    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
            a: f32::from(a) / 255.0,
        }
    }

    #[inline]
    pub fn to_rgba8(self) -> [u8; 4] {
        let s = self.sanitized();
        [
            (s.r * 255.0).round() as u8,
            (s.g * 255.0).round() as u8,
            (s.b * 255.0).round() as u8,
            (s.a * 255.0).round() as u8,
        ]
    }

    #[inline]
    pub const fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    #[inline]
    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
                let g = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
                let b = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
                Some(Self::from_rgba8(r, g, b, 255))
            }
            8 => {
                let r = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
                let g = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
                let b = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
                let a = u8::from_str_radix(hex.get(6..8)?, 16).ok()?;
                Some(Self::from_rgba8(r, g, b, a))
            }
            _ => None,
        }
    }

    /// Formats the RGB channels as `#RRGGBB`.
    pub fn to_hex(self) -> String {
        let [r, g, b, _] = self.to_rgba8();
        format!("#{r:02X}{g:02X}{b:02X}")
    }

    /// Converts RGB channels to `[hue, saturation, value]` in `0.0..=1.0`.
    pub fn to_hsv(self) -> [f32; 3] {
        let s = self.sanitized();
        let max = s.r.max(s.g).max(s.b);
        let min = s.r.min(s.g).min(s.b);
        let delta = max - min;
        let v = max;
        let sat = if max > 1e-6 {
            (delta / max).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let hue = if delta <= 1e-6 {
            0.0
        } else if (max - s.r).abs() <= 1e-6 {
            ((s.g - s.b) / delta).rem_euclid(6.0) / 6.0
        } else if (max - s.g).abs() <= 1e-6 {
            ((s.b - s.r) / delta + 2.0) / 6.0
        } else {
            ((s.r - s.g) / delta + 4.0) / 6.0
        };
        [hue.clamp(0.0, 1.0), sat, v.clamp(0.0, 1.0)]
    }

    /// Builds a color from HSV (`0.0..=1.0` each) and alpha (`0.0..=1.0`).
    pub fn from_hsv(hue: f32, sat: f32, val: f32, alpha: f32) -> Self {
        let h = if hue.is_finite() {
            hue.rem_euclid(1.0)
        } else {
            0.0
        };
        let s = if sat.is_finite() {
            sat.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let v = if val.is_finite() {
            val.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let a = if alpha.is_finite() {
            alpha.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let sector = (h * 6.0).floor();
        let f = h * 6.0 - sector;
        let p = v * (1.0 - s);
        let q = v * (1.0 - f * s);
        let t = v * (1.0 - (1.0 - f) * s);
        let (r, g, b) = match (sector as i32).rem_euclid(6) {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        Self::new(r, g, b, a).sanitized()
    }
}

/// Compositing blend mode for ink and highlighters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    /// Multiplies underlying dark ink cleanly without washing it out (used by Highlighter).
    Multiply,
}

#[inline]
fn clean01(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Blends straight RGBA `src` over `dst` using `mode`.
#[inline]
pub fn blend_pixel(dst: [f32; 4], src: [f32; 4], mode: BlendMode) -> [f32; 4] {
    let sa = clean01(src[3]);
    if sa <= 0.0 {
        return [
            clean01(dst[0]),
            clean01(dst[1]),
            clean01(dst[2]),
            clean01(dst[3]),
        ];
    }
    let da = clean01(dst[3]);
    let (sr, sg, sb) = (clean01(src[0]), clean01(src[1]), clean01(src[2]));
    let (dr, dg, db) = (clean01(dst[0]), clean01(dst[1]), clean01(dst[2]));

    let (br, bg, bb) = match mode {
        BlendMode::Normal => (sr, sg, sb),
        BlendMode::Multiply => (sr * dr, sg * dg, sb * db),
    };

    // ISO 32000 / Porter-Duff separable blend compositing in straight alpha:
    // out_a = sa + da * (1 - sa)
    // out_c * out_a = sa * (1 - da) * Cs + da * (1 - sa) * Cd + sa * da * B(Cd, Cs)
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 1e-6 {
        return [0.0, 0.0, 0.0, 0.0];
    }
    let inv_a = 1.0 / out_a;
    let chan = |c_s: f32, c_d: f32, c_b: f32| -> f32 {
        let pm = sa * (1.0 - da) * c_s + da * (1.0 - sa) * c_d + sa * da * c_b;
        (pm * inv_a).clamp(0.0, 1.0)
    };
    [
        chan(sr, dr, br),
        chan(sg, dg, bg),
        chan(sb, db, bb),
        out_a.clamp(0.0, 1.0),
    ]
}

/// Curated ink swatch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swatch {
    pub name: &'static str,
    pub color: Color,
}

const fn rgba_const(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    }
}

/// Curated ink colors for Fountain, Ball, and Brush pens.
pub const INK_PALETTE: &[Swatch] = &[
    Swatch {
        name: "Ink Black",
        color: rgba_const(0x11, 0x18, 0x27, 0xFF),
    },
    Swatch {
        name: "Cobalt Blue",
        color: rgba_const(0x00, 0x52, 0xFF, 0xFF),
    },
    Swatch {
        name: "Azure Blue",
        color: rgba_const(0x00, 0x96, 0xFF, 0xFF),
    },
    Swatch {
        name: "Crimson Red",
        color: rgba_const(0xF5, 0x17, 0x20, 0xFF),
    },
    Swatch {
        name: "Emerald Green",
        color: rgba_const(0x00, 0xB8, 0x4C, 0xFF),
    },
    Swatch {
        name: "Electric Purple",
        color: rgba_const(0x7C, 0x1E, 0xF5, 0xFF),
    },
    Swatch {
        name: "Tangerine Orange",
        color: rgba_const(0xFF, 0x6D, 0x00, 0xFF),
    },
    Swatch {
        name: "Vivid Teal",
        color: rgba_const(0x00, 0xB4, 0xB8, 0xFF),
    },
];

/// Curated highlighter colors (Classic preset).
pub const HIGHLIGHTER_PALETTE: &[Swatch] = &[
    Swatch {
        name: "Canary Yellow",
        color: rgba_const(0xFF, 0xEB, 0x3B, 0xFF),
    },
    Swatch {
        name: "Pastel Pink",
        color: rgba_const(0xFF, 0x80, 0xAB, 0xFF),
    },
    Swatch {
        name: "Mint Green",
        color: rgba_const(0x69, 0xF0, 0xAE, 0xFF),
    },
    Swatch {
        name: "Sky Blue",
        color: rgba_const(0x40, 0xC4, 0xFF, 0xFF),
    },
    Swatch {
        name: "Soft Lavender",
        color: rgba_const(0xB3, 0x88, 0xFF, 0xFF),
    },
    Swatch {
        name: "Peach",
        color: rgba_const(0xFF, 0xAB, 0x40, 0xFF),
    },
];

/// Soft pastel ink colors balanced for readable notes on paper (Pastel preset).
pub const PASTEL_INK_PALETTE: &[Swatch] = &[
    Swatch {
        name: "Lavender Mist",
        color: rgba_const(0x9B, 0x72, 0xF2, 0xFF),
    },
    Swatch {
        name: "Periwinkle",
        color: rgba_const(0x5B, 0x7C, 0xFA, 0xFF),
    },
    Swatch {
        name: "Sky Mist",
        color: rgba_const(0x38, 0xB6, 0xFF, 0xFF),
    },
    Swatch {
        name: "Coral Blush",
        color: rgba_const(0xFF, 0x6B, 0x8B, 0xFF),
    },
    Swatch {
        name: "Sage Mint",
        color: rgba_const(0x2E, 0xD1, 0x99, 0xFF),
    },
    Swatch {
        name: "Orchid Lilac",
        color: rgba_const(0xC7, 0x6B, 0xFF, 0xFF),
    },
    Swatch {
        name: "Apricot Glow",
        color: rgba_const(0xFF, 0x9E, 0x44, 0xFF),
    },
    Swatch {
        name: "Seafoam",
        color: rgba_const(0x22, 0xC7, 0xD6, 0xFF),
    },
];

/// Milky pastel highlighter colors (Pastel preset).
pub const PASTEL_HIGHLIGHTER_PALETTE: &[Swatch] = &[
    Swatch {
        name: "Cream Lemon",
        color: rgba_const(0xFF, 0xF3, 0xA8, 0xFF),
    },
    Swatch {
        name: "Petal Pink",
        color: rgba_const(0xFF, 0xD1, 0xE3, 0xFF),
    },
    Swatch {
        name: "Mint Sorbet",
        color: rgba_const(0xC8, 0xF7, 0xDC, 0xFF),
    },
    Swatch {
        name: "Baby Blue",
        color: rgba_const(0xC7, 0xE8, 0xFF, 0xFF),
    },
    Swatch {
        name: "Lilac Whisper",
        color: rgba_const(0xE3, 0xD4, 0xFF, 0xFF),
    },
    Swatch {
        name: "Apricot Milk",
        color: rgba_const(0xFF, 0xE0, 0xC2, 0xFF),
    },
];

/// Warm organic botanical and earth ink colors (Earth Tones preset).
pub const EARTH_INK_PALETTE: &[Swatch] = &[
    Swatch {
        name: "Espresso",
        color: rgba_const(0x23, 0x1E, 0x1A, 0xFF),
    },
    Swatch {
        name: "Slate Blue",
        color: rgba_const(0x2B, 0x65, 0x9E, 0xFF),
    },
    Swatch {
        name: "Terracotta",
        color: rgba_const(0xE0, 0x53, 0x28, 0xFF),
    },
    Swatch {
        name: "Moss Green",
        color: rgba_const(0x2D, 0x8C, 0x4E, 0xFF),
    },
    Swatch {
        name: "Ochre",
        color: rgba_const(0xD9, 0x86, 0x16, 0xFF),
    },
    Swatch {
        name: "Sienna",
        color: rgba_const(0xA6, 0x3D, 0x33, 0xFF),
    },
    Swatch {
        name: "Deep Pine",
        color: rgba_const(0x16, 0x82, 0x72, 0xFF),
    },
    Swatch {
        name: "Warm Umber",
        color: rgba_const(0x7D, 0x4A, 0x8C, 0xFF),
    },
];

/// Warm muted earth highlighter colors (Earth Tones preset).
pub const EARTH_HIGHLIGHTER_PALETTE: &[Swatch] = &[
    Swatch {
        name: "Sand Gold",
        color: rgba_const(0xF2, 0xD6, 0x94, 0xFF),
    },
    Swatch {
        name: "Clay Blush",
        color: rgba_const(0xED, 0xB8, 0xAA, 0xFF),
    },
    Swatch {
        name: "Eucalyptus",
        color: rgba_const(0xC2, 0xD6, 0xBA, 0xFF),
    },
    Swatch {
        name: "Mist Slate",
        color: rgba_const(0xB8, 0xCE, 0xD8, 0xFF),
    },
    Swatch {
        name: "Warm Mauve",
        color: rgba_const(0xD9, 0xBE, 0xD0, 0xFF),
    },
    Swatch {
        name: "Biscuit",
        color: rgba_const(0xE8, 0xC9, 0xA8, 0xFF),
    },
];

fn srgb_to_linear(c: f32) -> f32 {
    let c = if c.is_finite() {
        c.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = if c.is_finite() {
        c.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if c <= 0.003_130_8 {
        (12.92 * c).clamp(0.0, 1.0)
    } else {
        (1.055 * c.powf(1.0 / 2.4) - 0.055).clamp(0.0, 1.0)
    }
}

/// Computes the WCAG 2.1 relative luminance in `[0.0, 1.0]`.
#[must_use]
pub fn relative_luminance(color: Color) -> f32 {
    let c = color.sanitized();
    (0.2126 * srgb_to_linear(c.r) + 0.7152 * srgb_to_linear(c.g) + 0.0722 * srgb_to_linear(c.b))
        .clamp(0.0, 1.0)
}

/// Computes the WCAG 2.1 contrast ratio in `[1.0, 21.0]`.
#[must_use]
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    let lighter = la.max(lb);
    let darker = la.min(lb);
    (lighter + 0.05) / (darker + 0.05)
}

fn rgb_to_oklab(color: Color) -> (f32, f32, f32) {
    let c = color.sanitized();
    let lr = srgb_to_linear(c.r);
    let lg = srgb_to_linear(c.g);
    let lb = srgb_to_linear(c.b);

    let l = 0.412_221_47 * lr + 0.536_332_54 * lg + 0.051_445_993 * lb;
    let m = 0.211_903_5 * lr + 0.680_699_5 * lg + 0.107_396_96 * lb;
    let s = 0.088_302_46 * lr + 0.281_718_84 * lg + 0.629_978_7 * lb;

    let l_ = l.max(0.0).cbrt();
    let m_ = m.max(0.0).cbrt();
    let s_ = s.max(0.0).cbrt();

    let ok_l = 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_;
    let ok_a = 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_;
    let ok_b = 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_;
    (ok_l, ok_a, ok_b)
}

fn oklab_to_linear_rgb(ok_l: f32, ok_a: f32, ok_b: f32) -> (f32, f32, f32) {
    let l_ = ok_l + 0.396_337_78 * ok_a + 0.215_803_76 * ok_b;
    let m_ = ok_l - 0.105_561_346 * ok_a - 0.063_854_17 * ok_b;
    let s_ = ok_l - 0.089_484_18 * ok_a - 1.291_485_5 * ok_b;

    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;

    let lr = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_93 * s;
    let lg = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let lb = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
    (lr, lg, lb)
}

fn in_linear_srgb_gamut(lr: f32, lg: f32, lb: f32) -> bool {
    const EPS: f32 = 1e-5;
    (-EPS..=1.0 + EPS).contains(&lr)
        && (-EPS..=1.0 + EPS).contains(&lg)
        && (-EPS..=1.0 + EPS).contains(&lb)
}

fn max_gamut_chroma(ok_l: f32, cos_h: f32, sin_h: f32) -> f32 {
    if !ok_l.is_finite() || ok_l <= 0.0 || ok_l >= 1.0 {
        return 0.0;
    }
    let mut lo = 0.0f32;
    let mut hi = 0.5f32;
    for _ in 0..18 {
        let mid = 0.5 * (lo + hi);
        let (lr, lg, lb) = oklab_to_linear_rgb(ok_l, mid * cos_h, mid * sin_h);
        if in_linear_srgb_gamut(lr, lg, lb) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

fn oklch_to_color(ok_l: f32, c_desired: f32, cos_h: f32, sin_h: f32, alpha: f32) -> Color {
    let l_clamped = ok_l.clamp(0.0, 1.0);
    let c_max = max_gamut_chroma(l_clamped, cos_h, sin_h);
    let c = c_desired.max(0.0).min(c_max);
    let (lr, lg, lb) = oklab_to_linear_rgb(l_clamped, c * cos_h, c * sin_h);
    Color::new(
        linear_to_srgb(lr),
        linear_to_srgb(lg),
        linear_to_srgb(lb),
        alpha,
    )
    .sanitized()
}

/// Adapts an ink or highlighter color from `source_bg` to `target_bg` while preserving its
/// perceptual OKLCH hue and enforcing strong WCAG readability contrast against `target_bg`.
#[must_use]
pub fn adapt_color_for_background(
    color: Color,
    source_bg: Color,
    target_bg: Color,
    is_highlighter: bool,
) -> Color {
    let c_in_san = color.sanitized();
    let src_bg = source_bg.sanitized();
    let tgt_bg = target_bg.sanitized();

    let src_is_dark = relative_luminance(src_bg) < 0.25;
    let tgt_is_dark = relative_luminance(tgt_bg) < 0.25;
    let polarity_flipped = src_is_dark != tgt_is_dark;

    let (l_in, a_in, b_in) = rgb_to_oklab(c_in_san);
    let c_in = (a_in * a_in + b_in * b_in).sqrt();
    let (cos_h, sin_h) = if c_in > 1e-5 {
        (a_in / c_in, b_in / c_in)
    } else {
        (1.0, 0.0)
    };

    let c_max_in = max_gamut_chroma(l_in, cos_h, sin_h);
    let rel_sat = if c_max_in > 1e-4 {
        (c_in / c_max_in).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let chroma_at_l = |ok_l: f32| -> f32 {
        if c_in < 0.008 {
            return c_in;
        }
        let c_max_out = max_gamut_chroma(ok_l, cos_h, sin_h);
        let chroma_conf = ((c_in - 0.012) / 0.03).clamp(0.0, 1.0);
        let c_pres = c_in.min(c_max_out);
        let c_scaled = (rel_sat * c_max_out).min(c_max_out);
        let c_vivid = 0.35 * c_pres + 0.65 * c_scaled;
        c_pres + chroma_conf * (c_vivid - c_pres)
    };

    let eval_at_l =
        |ok_l: f32| -> Color { oklch_to_color(ok_l, chroma_at_l(ok_l), cos_h, sin_h, c_in_san.a) };

    let (l_src_bg, _, _) = rgb_to_oklab(src_bg);
    let (l_tgt_bg, _, _) = rgb_to_oklab(tgt_bg);
    let lum_tgt_bg = relative_luminance(tgt_bg);
    let cr_target = contrast_ratio(c_in_san, tgt_bg);
    let cr_source = contrast_ratio(c_in_san, src_bg);

    if is_highlighter {
        if tgt_is_dark {
            if cr_target >= 5.5 && l_in >= 0.70 {
                return c_in_san;
            }
            return search_contrasting_l(l_tgt_bg.max(0.18), 0.96, 5.5, tgt_bg, &eval_at_l);
        }
        if l_in >= 0.62 {
            return c_in_san;
        }
        let l_bright = (1.35 - l_in).clamp(0.68, 0.88);
        return eval_at_l(l_bright);
    }

    if polarity_flipped && cr_source < 1.12 {
        return tgt_bg.with_alpha(c_in_san.a);
    }

    let is_neutral = c_in < 0.025;
    let min_cr = if is_neutral {
        if tgt_is_dark { 9.5 } else { 8.0 }
    } else if tgt_is_dark {
        6.5
    } else {
        4.5
    };

    if !polarity_flipped {
        if !tgt_is_dark {
            if cr_target >= 3.0 || cr_source < 1.08 {
                return c_in_san;
            }
            return search_contrasting_l(l_tgt_bg.min(0.98), 0.0, min_cr, tgt_bg, &eval_at_l);
        }
        if cr_target >= min_cr || cr_source < 1.05 {
            return c_in_san;
        }
    }

    let l_mirror = if polarity_flipped {
        (l_src_bg + l_tgt_bg - l_in).clamp(0.12, 0.96)
    } else {
        (1.16 - l_in).clamp(0.22, 0.95)
    };

    let cand = eval_at_l(l_mirror);
    let lum_cand = relative_luminance(cand);
    let right_side = if tgt_is_dark {
        lum_cand > lum_tgt_bg
    } else {
        lum_cand < lum_tgt_bg
    };
    if right_side && contrast_ratio(cand, tgt_bg) >= min_cr {
        return cand;
    }

    let l_end = if tgt_is_dark { 1.0 } else { 0.0 };
    search_contrasting_l(l_tgt_bg, l_end, min_cr, tgt_bg, &eval_at_l)
}

fn search_contrasting_l(
    l_near_bg: f32,
    l_far_end: f32,
    min_cr: f32,
    target_bg: Color,
    eval_at_l: &dyn Fn(f32) -> Color,
) -> Color {
    const STEPS: usize = 32;
    let lum_bg = relative_luminance(target_bg);
    let want_lighter = l_far_end > l_near_bg;

    let passes = |cand: Color| -> bool {
        let lum = relative_luminance(cand);
        let right_side = if want_lighter {
            lum > lum_bg
        } else {
            lum < lum_bg
        };
        right_side && contrast_ratio(cand, target_bg) >= min_cr
    };

    let mut prev_l = l_near_bg;
    for i in 1..=STEPS {
        let t = i as f32 / STEPS as f32;
        let l = l_near_bg + (l_far_end - l_near_bg) * t;
        let cand = eval_at_l(l);
        if passes(cand) {
            let mut lo = prev_l;
            let mut hi = l;
            let mut best = cand;
            for _ in 0..10 {
                let mid = 0.5 * (lo + hi);
                let mid_col = eval_at_l(mid);
                if passes(mid_col) {
                    hi = mid;
                    best = mid_col;
                } else {
                    lo = mid;
                }
            }
            return best;
        }
        prev_l = l;
    }

    // Fallback: pick whichever endpoint yields the highest contrast ratio.
    let mut best = eval_at_l(l_far_end);
    let mut best_cr = contrast_ratio(best, target_bg);
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let l = l_near_bg + (l_far_end - l_near_bg) * t;
        let cand = eval_at_l(l);
        let cr = contrast_ratio(cand, target_bg);
        if cr > best_cr {
            best = cand;
            best_cr = cr;
        }
    }
    best
}

#[cfg(test)]
mod contrast_tests {
    use super::*;

    fn hue_deg(c: Color) -> f32 {
        let (_, a, b) = rgb_to_oklab(c);
        b.atan2(a).to_degrees().rem_euclid(360.0)
    }

    fn angle_diff_deg(a: f32, b: f32) -> f32 {
        let d = (a - b).abs() % 360.0;
        if d > 180.0 { 360.0 - d } else { d }
    }

    #[test]
    fn dark_and_medium_greens_preserve_hue_and_contrast_on_dark_bg() {
        let light_paper = Color::from_rgba8(252, 249, 240, 255); // WarmCream
        let dark_paper = Color::from_rgba8(30, 29, 27, 255); // Warm dark charcoal

        let greens = [
            Color::from_rgba8(0x1D, 0x7A, 0x46, 255), // Forest green
            Color::from_rgba8(0x22, 0xC5, 0x5E, 255), // Mid green
            Color::from_rgba8(0x0B, 0x4F, 0x28, 255), // Deep pine green
            Color::from_rgba8(0x4D, 0x7C, 0x0F, 255), // Olive green
        ];

        for orig in greens {
            let adapted = adapt_color_for_background(orig, light_paper, dark_paper, false);
            let cr = contrast_ratio(adapted, dark_paper);
            assert!(
                cr >= 6.49,
                "expected high contrast on dark paper for {:?} -> {:?}, got {:.2}",
                orig.to_hex(),
                adapted.to_hex(),
                cr
            );
            let dh = angle_diff_deg(hue_deg(orig), hue_deg(adapted));
            assert!(
                dh < 6.0,
                "expected green hue preserved for {:?} -> {:?}, diff {:.2} deg",
                orig.to_hex(),
                adapted.to_hex(),
                dh
            );
            assert!(
                adapted.g > adapted.r && adapted.g > adapted.b,
                "adapted color {:?} must stay dominantly green",
                adapted.to_hex()
            );
        }
    }

    #[test]
    fn dark_ink_and_palette_adapt_cleanly_light_to_dark() {
        let light_paper = Color::from_rgba8(255, 255, 255, 255);
        let dark_paper = Color::from_rgba8(23, 25, 32, 255);

        let black_ink = Color::from_rgba8(0x1A, 0x1A, 0x1E, 255);
        let adapted_black = adapt_color_for_background(black_ink, light_paper, dark_paper, false);
        assert!(contrast_ratio(adapted_black, dark_paper) >= 9.5);

        for sw in INK_PALETTE {
            let adapted = adapt_color_for_background(sw.color, light_paper, dark_paper, false);
            assert!(
                contrast_ratio(adapted, dark_paper) >= 6.49,
                "palette swatch {} ({}) failed dark contrast -> {} ({:.2})",
                sw.name,
                sw.color.to_hex(),
                adapted.to_hex(),
                contrast_ratio(adapted, dark_paper)
            );
        }
    }

    #[test]
    fn builtin_pastel_and_earth_palettes_are_valid_and_distinct() {
        for pal in [INK_PALETTE, PASTEL_INK_PALETTE, EARTH_INK_PALETTE] {
            assert_eq!(pal.len(), 8);
            for sw in pal {
                assert_eq!(sw.color.sanitized(), sw.color);
                assert_eq!(sw.color.a, 1.0);
            }
        }
        for pal in [
            HIGHLIGHTER_PALETTE,
            PASTEL_HIGHLIGHTER_PALETTE,
            EARTH_HIGHLIGHTER_PALETTE,
        ] {
            assert_eq!(pal.len(), 6);
            for sw in pal {
                assert_eq!(sw.color.sanitized(), sw.color);
                assert_eq!(sw.color.a, 1.0);
            }
        }
        assert_ne!(INK_PALETTE[0].color, PASTEL_INK_PALETTE[0].color);
        assert_ne!(INK_PALETTE[0].color, EARTH_INK_PALETTE[0].color);
        assert_ne!(PASTEL_INK_PALETTE[0].color, EARTH_INK_PALETTE[0].color);
    }

    #[test]
    fn bright_green_and_white_adapt_dark_to_light() {
        let dark_paper = Color::from_rgba8(0x22, 0x25, 0x2A, 255);
        let light_paper = Color::from_rgba8(0xF4, 0xF6, 0xFA, 255);

        let neon_green = Color::from_rgba8(0x4A, 0xDE, 0x80, 255);
        let adapted_green = adapt_color_for_background(neon_green, dark_paper, light_paper, false);
        assert!(contrast_ratio(adapted_green, light_paper) >= 4.49);
        assert!(angle_diff_deg(hue_deg(neon_green), hue_deg(adapted_green)) < 6.0);

        let white_ink = Color::from_rgba8(0xFF, 0xFF, 0xFF, 255);
        let adapted_white = adapt_color_for_background(white_ink, dark_paper, light_paper, false);
        assert!(contrast_ratio(adapted_white, light_paper) >= 8.0);
    }

    #[test]
    fn paper_matching_whiteout_and_highlighters_and_nan_are_safe() {
        let light_paper = Color::from_rgba8(252, 249, 240, 255);
        let dark_paper = Color::from_rgba8(30, 29, 27, 255);

        // White-out painted over cream paper turns into dark-out over dark charcoal.
        let whiteout = Color::from_rgba8(255, 255, 255, 255);
        let darkout = adapt_color_for_background(whiteout, light_paper, dark_paper, false);
        assert!(contrast_ratio(darkout, dark_paper) < 1.1);

        // Highlighters preserve translucency and legibility on dark bg.
        for sw in HIGHLIGHTER_PALETTE {
            let adapted = adapt_color_for_background(sw.color, light_paper, dark_paper, true);
            assert!(contrast_ratio(adapted, dark_paper) >= 5.4);
            assert!((adapted.a - sw.color.a).abs() < 1e-5);
        }

        // Non-finite inputs never produce NaN/Inf.
        let hostile = Color::new(f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::NAN);
        let out = adapt_color_for_background(hostile, light_paper, dark_paper, false);
        assert!(out.r.is_finite() && out.g.is_finite() && out.b.is_finite() && out.a.is_finite());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing_handles_valid_and_invalid_inputs() {
        assert_eq!(
            Color::from_hex("#1E63E9").unwrap().to_rgba8(),
            [0x1E, 0x63, 0xE9, 0xFF]
        );
        assert_eq!(
            Color::from_hex("FFEB3B80").unwrap().to_rgba8(),
            [0xFF, 0xEB, 0x3B, 0x80]
        );
        assert!(Color::from_hex("zzz").is_none());
        assert!(Color::from_hex("#12345").is_none());
    }

    #[test]
    fn multiply_highlighter_over_black_ink_stays_black() {
        let black_ink = [0.0, 0.0, 0.0, 1.0];
        let yellow_highlighter = [1.0, 0.92, 0.23, 0.45];
        let out = blend_pixel(black_ink, yellow_highlighter, BlendMode::Multiply);
        assert!(out[0] < 1e-5 && out[1] < 1e-5 && out[2] < 1e-5);
        assert!((out[3] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn normal_blend_interpolates_alpha_cleanly() {
        let white = [1.0, 1.0, 1.0, 1.0];
        let half_blue = [0.0, 0.0, 1.0, 0.5];
        let out = blend_pixel(white, half_blue, BlendMode::Normal);
        assert!((out[0] - 0.5).abs() < 1e-5);
        assert!((out[1] - 0.5).abs() < 1e-5);
        assert!((out[2] - 1.0).abs() < 1e-5);
        assert!((out[3] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn hsv_and_hex_roundtrip_and_resist_hostile_floats() {
        let royal = Color::from_hex("#1E63E9").unwrap();
        assert_eq!(royal.to_hex(), "#1E63E9");
        let [h, s, v] = royal.to_hsv();
        let rebuilt = Color::from_hsv(h, s, v, 1.0);
        assert_eq!(rebuilt.to_rgba8(), royal.to_rgba8());

        let hostile = Color::from_hsv(f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::NAN);
        assert!(hostile.r.is_finite() && hostile.g.is_finite() && hostile.b.is_finite());
    }
}
