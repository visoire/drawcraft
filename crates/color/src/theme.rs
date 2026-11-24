//! Light/Dark semantic design tokens (mirroring `DESIGN.md`) and PDF dark-mode inversion.

use crate::{Color, contrast_ratio};

/// UI chrome appearance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    Light,
    Dark,
}

/// Semantic color roles. Widgets read these instead of hardcoding hex values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    /// Backdrop behind pages and PDFs.
    pub canvas_bg: Color,
    /// Top toolbar and side panels.
    pub toolbar_bg: Color,
    /// Popovers, modals, notebook cards.
    pub surface_bg: Color,
    /// Search inputs and numeric fields.
    pub field_bg: Color,
    /// 1px hairlines and dividers (translucent).
    pub border: Color,
    /// Primary labels and headings.
    pub text: Color,
    /// Subtitles, metadata, hints.
    pub text_muted: Color,
    /// Active tool, focus ring, primary call to action.
    pub accent: Color,
    /// Destructive actions and errors.
    pub danger: Color,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
}

impl Tokens {
    /// Warm daylight studio tokens.
    #[must_use]
    pub const fn light() -> Self {
        Self {
            canvas_bg: rgb(0xEB, 0xEC, 0xEF),
            toolbar_bg: rgb(0xFF, 0xFF, 0xFF),
            surface_bg: rgb(0xFF, 0xFF, 0xFF),
            field_bg: rgb(0xF5, 0xF5, 0xF7),
            border: rgb(0xE5, 0xE5, 0xEA),
            text: rgb(0x1C, 0x1C, 0x1E),
            text_muted: rgb(0x66, 0x66, 0x6B),
            accent: rgb(0x00, 0x68, 0xF5),
            danger: rgb(0xD9, 0x2D, 0x20),
        }
    }

    /// Neutral anthracite studio tokens (no blue cast).
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            canvas_bg: rgb(0x0E, 0x0E, 0x10),
            toolbar_bg: rgb(0x1C, 0x1C, 0x1E),
            surface_bg: rgb(0x1C, 0x1C, 0x1E),
            field_bg: rgb(0x26, 0x26, 0x29),
            border: rgb(0x2A, 0x2A, 0x2E),
            text: rgb(0xF2, 0xF2, 0xF5),
            text_muted: rgb(0x8E, 0x8E, 0x93),
            accent: rgb(0x4D, 0x8B, 0xFF),
            danger: rgb(0xF9, 0x70, 0x66),
        }
    }

    #[must_use]
    pub const fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Light => Self::light(),
            ThemeMode::Dark => Self::dark(),
        }
    }

    /// The surfaces text may be drawn on.
    #[must_use]
    pub fn surfaces(&self) -> [Color; 4] {
        [
            self.canvas_bg,
            self.toolbar_bg,
            self.surface_bg,
            self.field_bg,
        ]
    }

    /// Best label color (white, or the dark ink used for light-theme text) for text placed on
    /// the accent color.
    #[must_use]
    pub fn on_accent(&self) -> Color {
        let ink = Self::light().text;
        let white_ratio = contrast_ratio(Color::WHITE, self.accent);
        let ink_ratio = contrast_ratio(ink, self.accent);
        if white_ratio >= ink_ratio {
            Color::WHITE
        } else {
            ink
        }
    }
}

/// Darkest lightness (0-255) a page's white maps to when inverting for dark reading.
pub const PDF_DARK_FLOOR: f32 = 26.0;
/// Lightest lightness (0-255) a page's black maps to; avoids harsh pure-white text.
pub const PDF_DARK_CEIL: f32 = 235.0;

/// Inverts the lightness of one straight RGBA8 pixel while keeping hue and chroma, so black
/// text becomes soft light text and white paper becomes a dark slate, but red stays red.
/// Alpha is preserved.
#[must_use]
pub fn invert_rgba8_for_dark(px: [u8; 4]) -> [u8; 4] {
    let (r, g, b) = (f32::from(px[0]), f32::from(px[1]), f32::from(px[2]));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) * 0.5;
    let target = PDF_DARK_FLOOR + (255.0 - lightness) * (PDF_DARK_CEIL - PDF_DARK_FLOOR) / 255.0;
    let delta = target - lightness;
    let shift = |c: f32| (c + delta).clamp(0.0, 255.0).round() as u8;
    [shift(r), shift(g), shift(b), px[3]]
}

/// Inverts a whole straight-RGBA8 buffer in place. Trailing bytes that do not form a full pixel
/// are left untouched.
pub fn invert_rgba8_buffer_for_dark(rgba: &mut [u8]) {
    let (pixels, _trailing) = rgba.as_chunks_mut::<4>();
    for px in pixels {
        *px = invert_rgba8_for_dark(*px);
    }
}

/// Inverts a [`Color`] with the same rule as [`invert_rgba8_for_dark`].
#[must_use]
pub fn invert_color_for_dark(color: Color) -> Color {
    let c = color.sanitized();
    let q = |v: f32| (v * 255.0).round() as u8;
    let out = invert_rgba8_for_dark([q(c.r), q(c.g), q(c.b), q(c.a)]);
    Color::new(
        f32::from(out[0]) / 255.0,
        f32::from(out[1]) / 255.0,
        f32::from(out[2]) / 255.0,
        c.a,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn muted_text_meets_wcag_aa_on_every_surface_in_both_themes() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let t = Tokens::for_mode(mode);
            for surface in t.surfaces() {
                let ratio = contrast_ratio(t.text_muted, surface);
                assert!(
                    ratio >= 4.5,
                    "{mode:?} muted text {ratio:.2}:1 on {surface:?}"
                );
            }
        }
    }

    #[test]
    fn primary_text_exceeds_12_to_1_on_the_surface_color() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let t = Tokens::for_mode(mode);
            let ratio = contrast_ratio(t.text, t.surface_bg);
            assert!(ratio >= 12.0, "{mode:?} text {ratio:.2}:1");
        }
    }

    #[test]
    fn accent_and_danger_remain_legible() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let t = Tokens::for_mode(mode);
            assert!(
                contrast_ratio(t.on_accent(), t.accent) >= 4.5,
                "{mode:?} accent label"
            );
            assert!(
                contrast_ratio(t.accent, t.surface_bg) >= 3.0,
                "{mode:?} accent control"
            );
            assert!(
                contrast_ratio(t.danger, t.surface_bg) >= 3.0,
                "{mode:?} danger"
            );
        }
    }

    #[test]
    fn dark_chrome_is_darker_than_light_chrome() {
        let l = crate::relative_luminance(Tokens::light().canvas_bg);
        let d = crate::relative_luminance(Tokens::dark().canvas_bg);
        assert!(d < 0.05 && l > 0.8);
    }

    #[test]
    fn inversion_turns_white_paper_dark_and_black_ink_light_without_pure_extremes() {
        let paper = invert_rgba8_for_dark([255, 255, 255, 255]);
        let ink = invert_rgba8_for_dark([0, 0, 0, 255]);
        assert_eq!(paper, [26, 26, 26, 255]);
        assert_eq!(ink, [235, 235, 235, 255]);
        let ratio = contrast_ratio(
            Color::from_rgba8(ink[0], ink[1], ink[2], 255),
            Color::from_rgba8(paper[0], paper[1], paper[2], 255),
        );
        assert!(ratio >= 12.0, "inverted text/paper contrast {ratio:.2}");
    }

    #[test]
    fn inversion_preserves_saturated_hues_and_alpha() {
        let red = invert_rgba8_for_dark([255, 0, 0, 128]);
        assert_eq!(red[3], 128);
        assert!(red[0] > red[1] && red[0] > red[2], "red stays red: {red:?}");
        let blue = invert_rgba8_for_dark([0, 0, 255, 255]);
        assert!(
            blue[2] > blue[0] && blue[2] > blue[1],
            "blue stays blue: {blue:?}"
        );
    }

    #[test]
    fn inversion_is_order_reversing_on_grays() {
        let mut prev = u8::MAX;
        for v in (0u8..=255).step_by(5) {
            let out = invert_rgba8_for_dark([v, v, v, 255])[0];
            assert!(
                out <= prev,
                "darker input must give lighter output ({v} -> {out})"
            );
            prev = out;
        }
    }

    #[test]
    fn buffer_inversion_handles_partial_and_hostile_buffers() {
        let mut buf = vec![255, 255, 255, 255, 0, 0, 0, 255, 9, 9];
        invert_rgba8_buffer_for_dark(&mut buf);
        assert_eq!(&buf[..4], &[26, 26, 26, 255]);
        assert_eq!(&buf[4..8], &[235, 235, 235, 255]);
        assert_eq!(&buf[8..], &[9, 9], "trailing partial pixel untouched");
        let mut empty: Vec<u8> = Vec::new();
        invert_rgba8_buffer_for_dark(&mut empty);

        let nan = invert_color_for_dark(Color::new(f32::NAN, f32::INFINITY, -4.0, f32::NAN));
        assert!(nan.r.is_finite() && nan.g.is_finite() && nan.b.is_finite());
    }
}
