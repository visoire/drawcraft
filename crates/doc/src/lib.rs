//! L1 pure-data document model for DrawCraft.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use drawcraft_color::{
    Color, EARTH_HIGHLIGHTER_PALETTE, EARTH_INK_PALETTE, HIGHLIGHTER_PALETTE, INK_PALETTE,
    PASTEL_HIGHLIGHTER_PALETTE, PASTEL_INK_PALETTE,
};
pub use drawcraft_geom::Point;
use drawcraft_geom::Rect;
use serde::{Deserialize, Serialize};

pub mod study;
pub use study::{
    Flashcard, MAX_FLASHCARDS, MAX_STUDY_TAPES_PER_PAGE, ReviewGrade, ReviewSchedule,
    StudyTapeItem, sanitize_flashcards, sanitize_study_tapes,
};

/// Unique identifier for a committed ink stroke within a document.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct StrokeId(pub u64);

/// Default multiplier applied to text size for line height.
pub const DEFAULT_TEXT_LINE_SPACING: f32 = 1.2;
/// Default extra spacing in page points between characters.
pub const DEFAULT_TEXT_LETTER_SPACING: f32 = 0.0;
/// Right margin kept clear when wrapping text inside a page.
pub const TEXT_PAGE_RIGHT_MARGIN_PT: f64 = 8.0;
/// Minimum user-defined text wrap width in page points.
pub const MIN_TEXT_BOX_WIDTH: f32 = 16.0;
/// Maximum user-defined text wrap width in page points.
pub const MAX_TEXT_BOX_WIDTH: f32 = 8192.0;
/// Maximum length in characters accepted for a text hyperlink URL.
pub const MAX_LINK_URL_CHARS: usize = 2048;

/// A positioned block of page text entered from the keyboard.
pub const MAX_TEXT_SPANS: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextAlignment {
    #[default]
    Left,
    Center,
    Right,
}

impl TextAlignment {
    pub const ALL: [Self; 3] = [Self::Left, Self::Center, Self::Right];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Center => "Center",
            Self::Right => "Right",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextStyleAttr {
    Bold,
    Italic,
    Underline,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextSpan {
    /// UTF-8 byte start offset in `TextItem::text`.
    pub start: usize,
    /// UTF-8 byte end offset (exclusive) in `TextItem::text`.
    pub end: usize,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<TextFont>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
}

impl TextSpan {
    #[inline]
    pub fn has_style(&self) -> bool {
        self.bold
            || self.italic
            || self.underline
            || self.highlight.is_some()
            || self.font.is_some()
            || self.size.is_some()
            || self.color.is_some()
    }

    #[inline]
    pub fn same_style(&self, other: &Self) -> bool {
        let same_size = match (self.size, other.size) {
            (None, None) => true,
            (Some(a), Some(b)) => (a - b).abs() <= 1e-4,
            _ => false,
        };
        self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
            && self.highlight == other.highlight
            && self.font == other.font
            && same_size
            && self.color == other.color
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListItemKind {
    Bullet,
    Checkbox { checked: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct StyledTextRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub highlight: Option<Color>,
    pub font: Option<TextFont>,
    pub size: Option<f32>,
    pub color: Option<Color>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StyledWrappedLine {
    pub paragraph_index: usize,
    pub list_kind: Option<ListItemKind>,
    pub is_continuation: bool,
    pub runs: Vec<StyledTextRun>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextItem {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub size: f32,
    /// Rotation in degrees around the text's top-left anchor.
    pub angle_degrees: f64,
    pub color: Color,
    pub font: TextFont,
    /// Multiplier applied to font size to compute line height.
    pub line_spacing: f32,
    /// Extra spacing in page points added after each character.
    pub letter_spacing: f32,
    /// Optional user-defined wrap width in page points (`None` wraps at the page right margin).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<f32>,
    /// Optional target URL attached to this text item (`https://...`, `http://...`, or `mailto:...`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_url: Option<String>,
    /// Paragraph horizontal alignment (`Left`, `Center`, or `Right`).
    #[serde(default)]
    pub alignment: TextAlignment,
    /// Span-level rich formatting ranges over `text` UTF-8 byte offsets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<TextSpan>,
}

/// Font families available for page text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextFont {
    #[default]
    Sans,
    Monospace,
}

impl TextFont {
    pub const ALL: &[Self] = &[Self::Sans, Self::Monospace];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Sans => "Sans",
            Self::Monospace => "Monospace",
        }
    }
}

impl Default for TextItem {
    fn default() -> Self {
        Self {
            text: String::new(),
            x: 0.0,
            y: 0.0,
            size: 24.0,
            angle_degrees: 0.0,
            color: Color::BLACK,
            font: TextFont::Sans,
            line_spacing: DEFAULT_TEXT_LINE_SPACING,
            letter_spacing: DEFAULT_TEXT_LETTER_SPACING,
            max_width: None,
            link_url: None,
            alignment: TextAlignment::Left,
            spans: Vec::new(),
        }
    }
}

/// Byte range of a recognized hyperlink inside a [`TextItem::text`] string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextUrlSpan {
    pub byte_start: usize,
    pub byte_end: usize,
    pub url: String,
}

fn is_valid_host_and_tld(candidate: &str) -> bool {
    let authority = candidate.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let host = if let Some((h, port)) = authority.rsplit_once(':') {
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        h
    } else {
        authority
    };
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let valid_labels = labels.iter().all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    });
    let tld_ok = labels.last().is_some_and(|tld| {
        (2..=24).contains(&tld.len()) && tld.bytes().all(|b| b.is_ascii_alphabetic())
    });
    valid_labels && tld_ok
}

/// Validates and normalizes an external link URL (`http://`, `https://`, `mailto:`, `www.`, or bare domain).
pub fn sanitize_link_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_LINK_URL_CHARS
        || trimmed
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '"' | '<' | '>'))
    {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    for blocked in ["javascript:", "data:", "vbscript:", "file:"] {
        if lower.starts_with(blocked) {
            return None;
        }
    }
    for scheme in ["https://", "http://"] {
        if lower.starts_with(scheme) {
            let rest = &trimmed[scheme.len()..];
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            if authority.is_empty() {
                return None;
            }
            return Some(format!("{scheme}{rest}"));
        }
    }
    if lower.starts_with("mailto:") {
        let rest = &trimmed["mailto:".len()..];
        let (local, domain) = rest.split_once('@')?;
        if local.is_empty() || !is_valid_host_and_tld(domain) {
            return None;
        }
        return Some(format!("mailto:{rest}"));
    }
    if lower.starts_with("www.") && is_valid_host_and_tld(trimmed) {
        return Some(format!("https://{trimmed}"));
    }
    if (!trimmed.contains(':') || is_valid_host_and_tld(trimmed)) && is_valid_host_and_tld(trimmed)
    {
        return Some(format!("https://{trimmed}"));
    }
    None
}

fn trim_url_token(token: &str) -> (usize, usize) {
    let mut start = 0usize;
    let mut end = token.len();
    while let Some(ch) = token[start..end].chars().next() {
        if matches!(ch, '(' | '[' | '<' | '"' | '\'' | '‘' | '“') {
            start += ch.len_utf8();
        } else {
            break;
        }
    }
    while let Some(ch) = token[start..end].chars().next_back() {
        if matches!(
            ch,
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '>' | ']' | '’' | '”'
        ) {
            end -= ch.len_utf8();
        } else if ch == ')' {
            let slice = &token[start..end];
            let opens = slice.bytes().filter(|&b| b == b'(').count();
            let closes = slice.bytes().filter(|&b| b == b')').count();
            if closes > opens {
                end -= 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    (start, end)
}

/// Scans `text` for inline URLs and domains, returning byte spans and normalized target URLs.
pub fn find_text_url_spans(text: &str) -> Vec<TextUrlSpan> {
    let mut spans = Vec::new();
    let mut pos = 0usize;
    let bytes = text.as_bytes();
    while pos < bytes.len() {
        while pos < text.len() {
            let Some(ch) = text[pos..].chars().next() else {
                break;
            };
            if ch.is_whitespace() {
                pos += ch.len_utf8();
            } else {
                break;
            }
        }
        if pos >= text.len() {
            break;
        }
        let tok_start = pos;
        while pos < text.len() {
            let Some(ch) = text[pos..].chars().next() else {
                break;
            };
            if ch.is_whitespace() {
                break;
            }
            pos += ch.len_utf8();
        }
        let token = &text[tok_start..pos];
        let (rel_start, rel_end) = trim_url_token(token);
        if rel_start >= rel_end {
            continue;
        }
        let candidate = &token[rel_start..rel_end];
        let lower = candidate.to_ascii_lowercase();
        let looks_like_link = lower.starts_with("https://")
            || lower.starts_with("http://")
            || lower.starts_with("mailto:")
            || lower.starts_with("www.")
            || is_valid_host_and_tld(candidate);
        if looks_like_link && let Some(url) = sanitize_link_url(candidate) {
            spans.push(TextUrlSpan {
                byte_start: tok_start + rel_start,
                byte_end: tok_start + rel_end,
                url,
            });
        }
    }
    spans
}

fn snap_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn merge_span_fields(dst: &mut TextSpan, src: &TextSpan) {
    dst.bold |= src.bold;
    dst.italic |= src.italic;
    dst.underline |= src.underline;
    if let Some(hl) = src.highlight {
        dst.highlight = Some(hl);
    }
    if let Some(font) = src.font {
        dst.font = Some(font);
    }
    if let Some(size) = src.size {
        dst.size = Some(size);
    }
    if let Some(color) = src.color {
        dst.color = Some(color);
    }
}

/// Normalizes rich text spans against `text` so they are sorted, disjoint, char-boundary aligned,
/// non-empty, and capped to [`MAX_TEXT_SPANS`].
pub fn normalize_text_spans(text: &str, raw: &[TextSpan]) -> Vec<TextSpan> {
    if text.is_empty() || raw.is_empty() {
        return Vec::new();
    }
    let clean_raw: Vec<TextSpan> = raw
        .iter()
        .take(MAX_TEXT_SPANS.saturating_mul(2))
        .filter_map(|span| {
            let start = snap_char_boundary(text, span.start);
            let end = snap_char_boundary(text, span.end);
            let highlight = span.highlight.map(|c| c.sanitized()).filter(|c| c.a > 0.0);
            let size = span
                .size
                .filter(|s| s.is_finite())
                .map(|s| s.clamp(6.0, 120.0));
            let color = span.color.map(|c| c.sanitized());
            let clean = TextSpan {
                start,
                end,
                bold: span.bold,
                italic: span.italic,
                underline: span.underline,
                highlight,
                font: span.font,
                size,
                color,
            };
            (start < end && clean.has_style()).then_some(clean)
        })
        .collect();
    if clean_raw.is_empty() {
        return Vec::new();
    }

    let mut cuts: Vec<usize> = Vec::with_capacity(clean_raw.len() * 2);
    for span in &clean_raw {
        cuts.push(span.start);
        cuts.push(span.end);
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut out: Vec<TextSpan> = Vec::new();
    for pair in cuts.windows(2) {
        let a = pair[0];
        let b = pair[1];
        if a >= b {
            continue;
        }
        let mut merged = TextSpan {
            start: a,
            end: b,
            ..TextSpan::default()
        };
        for span in &clean_raw {
            if span.start <= a && span.end >= b {
                merge_span_fields(&mut merged, span);
            }
        }
        if !merged.has_style() {
            continue;
        }
        if let Some(prev) = out.last_mut()
            && prev.end == merged.start
            && prev.same_style(&merged)
        {
            prev.end = merged.end;
        } else {
            out.push(merged);
            if out.len() >= MAX_TEXT_SPANS {
                break;
            }
        }
    }
    out
}

/// Parses an optional bullet (`- `, `* `, `• `) or checkbox (`[ ] `, `[x] `) prefix on a line.
pub fn parse_list_line_prefix(line: &str) -> (Option<ListItemKind>, usize) {
    if let Some(_rest) = line.strip_prefix("[ ] ") {
        return (Some(ListItemKind::Checkbox { checked: false }), 4);
    }
    if line.starts_with("[x] ") || line.starts_with("[X] ") {
        return (Some(ListItemKind::Checkbox { checked: true }), 4);
    }
    if line.starts_with("- ") || line.starts_with("* ") {
        return (Some(ListItemKind::Bullet), 2);
    }
    if line.starts_with("• ") {
        return (Some(ListItemKind::Bullet), "• ".len());
    }
    (None, 0)
}

impl TextItem {
    /// Removes hostile control characters and bounds the stored text and geometry.
    pub fn sanitized(&self) -> Self {
        let text: String = self
            .text
            .chars()
            .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
            .take(MAX_TEXT_CHARS)
            .collect();
        let spans = normalize_text_spans(&text, &self.spans);
        Self {
            text,
            x: if self.x.is_finite() {
                self.x.clamp(-1e6, 1e6)
            } else {
                0.0
            },
            y: if self.y.is_finite() {
                self.y.clamp(-1e6, 1e6)
            } else {
                0.0
            },
            size: if self.size.is_finite() {
                self.size.clamp(6.0, 120.0)
            } else {
                24.0
            },
            angle_degrees: if self.angle_degrees.is_finite() {
                (self.angle_degrees + 180.0).rem_euclid(360.0) - 180.0
            } else {
                0.0
            },
            color: self.color.sanitized(),
            font: self.font,
            line_spacing: if self.line_spacing.is_finite() {
                self.line_spacing.clamp(0.8, 3.0)
            } else {
                DEFAULT_TEXT_LINE_SPACING
            },
            letter_spacing: if self.letter_spacing.is_finite() {
                self.letter_spacing.clamp(-4.0, 32.0)
            } else {
                DEFAULT_TEXT_LETTER_SPACING
            },
            max_width: self
                .max_width
                .filter(|w| w.is_finite())
                .map(|w| w.clamp(MIN_TEXT_BOX_WIDTH, MAX_TEXT_BOX_WIDTH)),
            link_url: self.link_url.as_deref().and_then(sanitize_link_url),
            alignment: self.alignment,
            spans,
        }
    }

    /// Queries the active style attributes across `range_start..range_end` (all bytes covered).
    pub fn style_active_across(
        &self,
        range_start: usize,
        range_end: usize,
    ) -> (bool, bool, bool, Option<Color>) {
        let s = snap_char_boundary(&self.text, range_start);
        let e = snap_char_boundary(&self.text, range_end);
        if s >= e {
            return (false, false, false, None);
        }
        let spans = normalize_text_spans(&self.text, &self.spans);
        let mut bold_cov = 0usize;
        let mut italic_cov = 0usize;
        let mut underline_cov = 0usize;
        let mut hl: Option<Color> = None;
        for span in &spans {
            let ov_a = span.start.max(s);
            let ov_b = span.end.min(e);
            if ov_a >= ov_b {
                continue;
            }
            let len = ov_b - ov_a;
            if span.bold {
                bold_cov += len;
            }
            if span.italic {
                italic_cov += len;
            }
            if span.underline {
                underline_cov += len;
            }
            if hl.is_none() {
                hl = span.highlight;
            }
        }
        let target_len = e - s;
        (
            bold_cov == target_len,
            italic_cov == target_len,
            underline_cov == target_len,
            hl,
        )
    }

    fn mutate_spans_in_range(
        &mut self,
        range_start: usize,
        range_end: usize,
        mut apply: impl FnMut(&mut TextSpan),
    ) -> bool {
        let s = snap_char_boundary(&self.text, range_start);
        let e = snap_char_boundary(&self.text, range_end);
        if s >= e {
            return false;
        }
        let base = normalize_text_spans(&self.text, &self.spans);
        let mut updated: Vec<TextSpan> = Vec::new();
        let mut cuts = vec![s, e];
        for sp in &base {
            cuts.push(sp.start);
            cuts.push(sp.end);
        }
        cuts.sort_unstable();
        cuts.dedup();
        for win in cuts.windows(2) {
            let (a, b) = (win[0], win[1]);
            if a >= b {
                continue;
            }
            let mut seg = TextSpan {
                start: a,
                end: b,
                ..TextSpan::default()
            };
            for sp in &base {
                if sp.start <= a && sp.end >= b {
                    merge_span_fields(&mut seg, sp);
                }
            }
            if a >= s && b <= e {
                apply(&mut seg);
            }
            if seg.has_style() {
                updated.push(seg);
            }
        }
        self.spans = normalize_text_spans(&self.text, &updated);
        true
    }

    fn uniform_attr_across<T: Copy>(
        &self,
        range_start: usize,
        range_end: usize,
        default_val: T,
        extract: impl Fn(&TextSpan) -> Option<T>,
        same: impl Fn(T, T) -> bool,
    ) -> Option<T> {
        let s = snap_char_boundary(&self.text, range_start);
        let e = snap_char_boundary(&self.text, range_end);
        if s >= e {
            return Some(default_val);
        }
        let spans = normalize_text_spans(&self.text, &self.spans);
        let mut cursor = s;
        let mut resolved: Option<T> = None;
        for sp in &spans {
            if sp.end <= s || sp.start >= e {
                continue;
            }
            let ov_a = sp.start.max(s);
            let ov_b = sp.end.min(e);
            if ov_a > cursor {
                match resolved {
                    None => resolved = Some(default_val),
                    Some(prev) if !same(prev, default_val) => return None,
                    _ => {}
                }
            }
            let val = extract(sp).unwrap_or(default_val);
            match resolved {
                None => resolved = Some(val),
                Some(prev) if !same(prev, val) => return None,
                _ => {}
            }
            cursor = ov_b;
        }
        if cursor < e {
            match resolved {
                None => resolved = Some(default_val),
                Some(prev) if !same(prev, default_val) => return None,
                _ => {}
            }
        }
        resolved
    }

    /// Queries the active font across `range_start..range_end`, or `None` if mixed.
    pub fn effective_font_across(&self, range_start: usize, range_end: usize) -> Option<TextFont> {
        self.uniform_attr_across(
            range_start,
            range_end,
            self.font,
            |sp| sp.font,
            |a, b| a == b,
        )
    }

    /// Queries the active size across `range_start..range_end`, or `None` if mixed.
    pub fn effective_size_across(&self, range_start: usize, range_end: usize) -> Option<f32> {
        let base_size = if self.size.is_finite() {
            self.size.clamp(6.0, 120.0)
        } else {
            24.0
        };
        self.uniform_attr_across(
            range_start,
            range_end,
            base_size,
            |sp| sp.size,
            |a, b| (a - b).abs() <= 0.05,
        )
    }

    /// Queries the active color across `range_start..range_end`, or `None` if mixed.
    pub fn effective_color_across(&self, range_start: usize, range_end: usize) -> Option<Color> {
        self.uniform_attr_across(
            range_start,
            range_end,
            self.color.sanitized(),
            |sp| sp.color,
            |a, b| a == b,
        )
    }

    /// Toggles `attr` on `range_start..range_end` (`0..self.text.len()` if inverted/empty range given by caller).
    pub fn toggle_span_style(
        &mut self,
        range_start: usize,
        range_end: usize,
        attr: TextStyleAttr,
    ) -> bool {
        let s = snap_char_boundary(&self.text, range_start);
        let e = snap_char_boundary(&self.text, range_end);
        if s >= e {
            return false;
        }
        let (all_b, all_i, all_u, _) = self.style_active_across(s, e);
        let currently_on = match attr {
            TextStyleAttr::Bold => all_b,
            TextStyleAttr::Italic => all_i,
            TextStyleAttr::Underline => all_u,
        };
        let target_on = !currently_on;
        self.mutate_spans_in_range(s, e, |seg| match attr {
            TextStyleAttr::Bold => seg.bold = target_on,
            TextStyleAttr::Italic => seg.italic = target_on,
            TextStyleAttr::Underline => seg.underline = target_on,
        })
    }

    /// Sets or clears a highlight background color on `range_start..range_end`.
    pub fn set_span_highlight(
        &mut self,
        range_start: usize,
        range_end: usize,
        highlight: Option<Color>,
    ) -> bool {
        let clean_hl = highlight.map(|c| c.sanitized()).filter(|c| c.a > 0.0);
        self.mutate_spans_in_range(range_start, range_end, |seg| {
            seg.highlight = clean_hl;
        })
    }

    /// Sets or clears a font family override on `range_start..range_end`.
    pub fn set_span_font(
        &mut self,
        range_start: usize,
        range_end: usize,
        font: Option<TextFont>,
    ) -> bool {
        self.mutate_spans_in_range(range_start, range_end, |seg| {
            seg.font = font;
        })
    }

    /// Sets or clears a font size override on `range_start..range_end`.
    pub fn set_span_size(
        &mut self,
        range_start: usize,
        range_end: usize,
        size: Option<f32>,
    ) -> bool {
        let clean_size = size.filter(|s| s.is_finite()).map(|s| s.clamp(6.0, 120.0));
        self.mutate_spans_in_range(range_start, range_end, |seg| {
            seg.size = clean_size;
        })
    }

    /// Sets or clears a foreground color override on `range_start..range_end`.
    pub fn set_span_color(
        &mut self,
        range_start: usize,
        range_end: usize,
        color: Option<Color>,
    ) -> bool {
        let clean_color = color.map(|c| c.sanitized());
        self.mutate_spans_in_range(range_start, range_end, |seg| {
            seg.color = clean_color;
        })
    }

    /// Adjusts [`Self::spans`] byte offsets after editing `old_text` into `new_text`.
    pub fn adjust_spans_for_edit(&mut self, old_text: &str, new_text: &str) {
        if self.spans.is_empty() || old_text == new_text {
            self.spans = normalize_text_spans(new_text, &self.spans);
            return;
        }
        let mut prefix = 0usize;
        for (a, b) in old_text.chars().zip(new_text.chars()) {
            if a == b {
                prefix += a.len_utf8();
            } else {
                break;
            }
        }
        let old_rem = &old_text[prefix..];
        let new_rem = &new_text[prefix..];
        let mut suffix = 0usize;
        for (a, b) in old_rem.chars().rev().zip(new_rem.chars().rev()) {
            if a == b {
                suffix += a.len_utf8();
            } else {
                break;
            }
        }
        let old_end = old_text.len().saturating_sub(suffix);
        let new_end = new_text.len().saturating_sub(suffix);
        let delta = new_end as isize - old_end as isize;

        let mut adjusted = Vec::with_capacity(self.spans.len());
        for mut span in self.spans.drain(..) {
            if span.end <= prefix {
                adjusted.push(span);
            } else if span.start >= old_end {
                span.start = (span.start as isize + delta).max(0) as usize;
                span.end = (span.end as isize + delta).max(0) as usize;
                if span.start < span.end {
                    adjusted.push(span);
                }
            } else {
                let is_pure_insert_inside = prefix == old_end;
                if is_pure_insert_inside && span.start <= prefix && prefix <= span.end {
                    span.end = (span.end as isize + delta).max(0) as usize;
                } else {
                    if span.start > prefix {
                        span.start = new_end;
                    }
                    if span.end >= old_end {
                        span.end = (span.end as isize + delta).max(0) as usize;
                    } else {
                        span.end = span.end.min(prefix);
                    }
                }
                if span.start < span.end {
                    adjusted.push(span);
                }
            }
        }
        self.spans = normalize_text_spans(new_text, &adjusted);
    }

    /// Toggles `[ ] ` <-> `[x] ` on `paragraph_index`. Returns `true` if toggled.
    pub fn toggle_checkbox_at_paragraph(&mut self, paragraph_index: usize) -> bool {
        let mut line_start = 0usize;
        for (idx, part) in self.text.split('\n').enumerate() {
            if idx == paragraph_index {
                let replacement = if part.starts_with("[ ] ") {
                    Some("[x] ")
                } else if part.starts_with("[x] ") || part.starts_with("[X] ") {
                    Some("[ ] ")
                } else {
                    None
                };
                if let Some(new_prefix) = replacement {
                    let mut next = String::with_capacity(self.text.len());
                    next.push_str(&self.text[..line_start]);
                    next.push_str(new_prefix);
                    next.push_str(&self.text[line_start + 4..]);
                    self.text = next;
                    return true;
                }
                return false;
            }
            line_start += part.len() + 1;
        }
        false
    }

    /// Toggles bullet or checkbox list formatting across paragraphs intersecting `range_start..range_end`.
    pub fn toggle_list_in_range(
        &mut self,
        range_start: usize,
        range_end: usize,
        want_checkbox: bool,
    ) -> bool {
        let old_text = self.text.clone();
        let target_prefix = if want_checkbox { "[ ] " } else { "- " };
        let s = range_start.min(old_text.len());
        let e = range_end.min(old_text.len()).max(s);

        let lines: Vec<&str> = if old_text.is_empty() {
            vec![""]
        } else {
            old_text.split('\n').collect()
        };
        let mut cursor = 0usize;
        let mut selected_indices = Vec::new();
        for (idx, line) in lines.iter().enumerate() {
            let line_end = cursor + line.len();
            if e >= cursor && s <= line_end {
                selected_indices.push(idx);
            }
            cursor = line_end + 1;
        }
        if selected_indices.is_empty() {
            selected_indices.push(0);
        }

        let all_already_target = selected_indices.iter().all(|&idx| {
            let (kind, _) = parse_list_line_prefix(lines[idx]);
            matches!(
                (want_checkbox, kind),
                (true, Some(ListItemKind::Checkbox { .. })) | (false, Some(ListItemKind::Bullet))
            )
        });

        let mut rebuilt_lines = Vec::with_capacity(lines.len());
        for (idx, line) in lines.iter().enumerate() {
            if selected_indices.contains(&idx) {
                let (_, prefix_bytes) = parse_list_line_prefix(line);
                let body = &line[prefix_bytes..];
                if all_already_target {
                    rebuilt_lines.push(body.to_string());
                } else {
                    rebuilt_lines.push(format!("{target_prefix}{body}"));
                }
            } else {
                rebuilt_lines.push((*line).to_string());
            }
        }
        let next_text = rebuilt_lines.join("\n");
        if next_text == old_text {
            return false;
        }
        self.text = next_text.clone();
        self.adjust_spans_for_edit(&old_text, &next_text);
        *self = self.sanitized();
        true
    }

    /// Returns recognized inline URL spans in [`Self::text`], falling back to the whole text span
    /// when an explicit [`Self::link_url`] is attached without inline URLs.
    pub fn resolved_links(&self) -> Vec<TextUrlSpan> {
        let spans = find_text_url_spans(&self.text);
        if !spans.is_empty() {
            return spans;
        }
        if !self.text.is_empty()
            && let Some(url) = self.link_url.as_deref().and_then(sanitize_link_url)
        {
            return vec![TextUrlSpan {
                byte_start: 0,
                byte_end: self.text.len(),
                url,
            }];
        }
        Vec::new()
    }

    /// Returns the explicit [`Self::link_url`] if valid, or the first inline URL found in [`Self::text`].
    pub fn primary_link_url(&self) -> Option<String> {
        self.link_url
            .as_deref()
            .and_then(sanitize_link_url)
            .or_else(|| {
                find_text_url_spans(&self.text)
                    .into_iter()
                    .next()
                    .map(|span| span.url)
            })
    }

    /// Returns the horizontal wrap width in page points for a page of `page_width`.
    pub fn effective_max_width(&self, page_width: u32) -> f32 {
        match self.max_width {
            Some(width) if width.is_finite() => width.clamp(MIN_TEXT_BOX_WIDTH, MAX_TEXT_BOX_WIDTH),
            _ => text_max_page_width(page_width, self.x),
        }
    }
}

/// Computes the maximum horizontal text width from `x` to the page's right margin.
pub fn text_max_page_width(page_width: u32, x: f64) -> f32 {
    let width = f64::from(page_width);
    let safe_x = if x.is_finite() {
        x.clamp(0.0, width)
    } else {
        0.0
    };
    (width - safe_x - TEXT_PAGE_RIGHT_MARGIN_PT).max(16.0) as f32
}

struct WrapLineState {
    lines: Vec<String>,
    current: String,
    current_w: f32,
    limit: f32,
}

impl WrapLineState {
    fn push_word(&mut self, word: &str, word_w: f32, measure_char: &mut impl FnMut(char) -> f32) {
        if word.is_empty() {
            return;
        }
        if !self.current.is_empty() && self.current_w + word_w > self.limit {
            self.lines.push(std::mem::take(&mut self.current));
            self.current_w = 0.0;
        }
        for ch in word.chars() {
            let cw = measure_char(ch).max(0.0);
            if !self.current.is_empty() && self.current_w + cw > self.limit {
                self.lines.push(std::mem::take(&mut self.current));
                self.current_w = 0.0;
            }
            self.current.push(ch);
            self.current_w += cw;
        }
    }
}

/// Splits `text` into wrapped lines bounded by `max_width` using `measure_char` widths.
pub fn wrap_text_lines(
    text: &str,
    max_width: f32,
    mut measure_char: impl FnMut(char) -> f32,
) -> Vec<String> {
    let limit = if max_width.is_finite() {
        max_width.max(1.0)
    } else {
        f32::MAX
    };
    let mut state = WrapLineState {
        lines: Vec::new(),
        current: String::new(),
        current_w: 0.0,
        limit,
    };
    for raw_line in text.split('\n') {
        if raw_line.is_empty() {
            state.lines.push(String::new());
            continue;
        }
        let mut word = String::new();
        let mut word_w = 0.0_f32;
        for ch in raw_line.chars() {
            if ch.is_whitespace() {
                state.push_word(&word, word_w, &mut measure_char);
                word.clear();
                word_w = 0.0;
                let cw = measure_char(ch).max(0.0);
                if !state.current.is_empty() && state.current_w + cw > state.limit {
                    state.lines.push(std::mem::take(&mut state.current));
                    state.current_w = 0.0;
                } else {
                    state.current.push(ch);
                    state.current_w += cw;
                }
            } else {
                word.push(ch);
                word_w += measure_char(ch).max(0.0);
            }
        }
        state.push_word(&word, word_w, &mut measure_char);
        state.lines.push(std::mem::take(&mut state.current));
        state.current_w = 0.0;
    }
    if state.lines.is_empty() {
        state.lines.push(String::new());
    }
    state.lines
}

fn font_size_at_byte(
    spans: &[TextSpan],
    byte_idx: usize,
    base_font: TextFont,
    base_size: f32,
) -> (TextFont, f32) {
    let mut font = base_font;
    let mut size = base_size;
    for sp in spans {
        if sp.start <= byte_idx && byte_idx < sp.end {
            if let Some(f) = sp.font {
                font = f;
            }
            if let Some(s) = sp.size {
                size = s;
            }
        }
    }
    (font, size)
}

struct WrapLineCursor<'a> {
    body_start_global: usize,
    body: &'a str,
    effective_w: f32,
    spans: &'a [TextSpan],
    base_font: TextFont,
    base_size: f32,
    lines: Vec<(usize, usize)>,
    c_start: usize,
    c_end: usize,
    c_w: f32,
}

fn flush_wrap_token(
    tok_s: usize,
    tok_e: usize,
    tok_w: f32,
    is_ws: bool,
    cur: &mut WrapLineCursor<'_>,
    measure: &mut dyn FnMut(char, TextFont, f32) -> f32,
) {
    if tok_s >= tok_e {
        return;
    }
    if is_ws {
        if cur.c_start == cur.c_end || cur.c_w + tok_w <= cur.effective_w {
            if cur.c_start == cur.c_end {
                cur.c_start = tok_s;
            }
            cur.c_end = tok_e;
            cur.c_w += tok_w;
        } else {
            cur.lines.push((cur.c_start, cur.c_end));
            cur.c_start = tok_s;
            cur.c_end = tok_e;
            cur.c_w = tok_w;
        }
        return;
    }
    if cur.c_w > 0.0 && cur.c_w + tok_w <= cur.effective_w {
        cur.c_end = tok_e;
        cur.c_w += tok_w;
        return;
    }
    if cur.c_w > 0.0 && tok_w <= cur.effective_w {
        cur.lines.push((cur.c_start, cur.c_end));
        cur.c_start = tok_s;
        cur.c_end = tok_e;
        cur.c_w = tok_w;
        return;
    }
    if cur.c_w > 0.0 {
        cur.lines.push((cur.c_start, cur.c_end));
        cur.c_start = tok_s;
        cur.c_end = tok_s;
        cur.c_w = 0.0;
    }
    let local_s = tok_s - cur.body_start_global;
    let local_e = tok_e - cur.body_start_global;
    for (rel_idx, ch) in cur.body[local_s..local_e].char_indices() {
        let g_idx = tok_s + rel_idx;
        let (ch_font, ch_size) = font_size_at_byte(cur.spans, g_idx, cur.base_font, cur.base_size);
        let cw = measure(ch, ch_font, ch_size).max(0.0);
        if cur.c_w > 0.0 && cur.c_w + cw > cur.effective_w {
            cur.lines.push((cur.c_start, cur.c_end));
            cur.c_start = g_idx;
            cur.c_w = 0.0;
        }
        if cur.c_start == cur.c_end && cur.c_w == 0.0 {
            cur.c_start = g_idx;
        }
        cur.c_end = g_idx + ch.len_utf8();
        cur.c_w += cw;
    }
}

/// Breaks `item.text` into wrapped lines while preserving list markers and slicing [`TextItem::spans`]
/// into styled runs, passing each character's effective [`TextFont`] and size to `measure_char`.
pub fn wrap_styled_text_lines_with(
    item: &TextItem,
    max_width: f32,
    mut measure_char: impl FnMut(char, TextFont, f32) -> f32,
) -> Vec<StyledWrappedLine> {
    let clean = item.sanitized();
    let max_w = if max_width.is_finite() {
        max_width.max(16.0)
    } else {
        400.0
    };
    let spans = normalize_text_spans(&clean.text, &clean.spans);
    let gutter_w = (clean.size * 1.15).max(14.0);
    let mut out = Vec::new();
    let mut raw_offset = 0usize;

    for (paragraph_index, raw_line) in clean.text.split('\n').enumerate() {
        let (list_kind, prefix_bytes) = parse_list_line_prefix(raw_line);
        let body_start_global = raw_offset + prefix_bytes;
        let body = &raw_line[prefix_bytes..];
        let effective_w = if list_kind.is_some() {
            (max_w - gutter_w).max(24.0)
        } else {
            max_w
        };

        let mut cur = WrapLineCursor {
            body_start_global,
            body,
            effective_w,
            spans: &spans,
            base_font: clean.font,
            base_size: clean.size,
            lines: Vec::new(),
            c_start: body_start_global,
            c_end: body_start_global,
            c_w: 0.0,
        };
        let mut token_start: Option<usize> = None;
        let mut token_is_ws = false;
        let mut token_w = 0.0_f32;

        for (rel_idx, ch) in body.char_indices() {
            let g_idx = body_start_global + rel_idx;
            let (ch_font, ch_size) = font_size_at_byte(&spans, g_idx, clean.font, clean.size);
            let cw = measure_char(ch, ch_font, ch_size).max(0.0);
            let is_ws = ch.is_whitespace();
            match token_start {
                None => {
                    token_start = Some(g_idx);
                    token_is_ws = is_ws;
                    token_w = cw;
                }
                Some(_ts) if token_is_ws == is_ws => {
                    token_w += cw;
                }
                Some(ts) => {
                    flush_wrap_token(ts, g_idx, token_w, token_is_ws, &mut cur, &mut measure_char);
                    token_start = Some(g_idx);
                    token_is_ws = is_ws;
                    token_w = cw;
                }
            }
        }
        if let Some(ts) = token_start {
            flush_wrap_token(
                ts,
                body_start_global + body.len(),
                token_w,
                token_is_ws,
                &mut cur,
                &mut measure_char,
            );
        }
        cur.lines.push((cur.c_start, cur.c_end));

        for (wrap_idx, (seg_start, seg_end)) in cur.lines.into_iter().enumerate() {
            let runs = slice_styled_runs(&clean.text, &spans, seg_start, seg_end);
            out.push(StyledWrappedLine {
                paragraph_index,
                list_kind: if wrap_idx == 0 { list_kind } else { None },
                is_continuation: wrap_idx > 0 && list_kind.is_some(),
                runs,
            });
        }
        raw_offset += raw_line.len() + 1;
    }

    if out.is_empty() {
        out.push(StyledWrappedLine {
            paragraph_index: 0,
            list_kind: None,
            is_continuation: false,
            runs: Vec::new(),
        });
    }
    out
}

/// Breaks `item.text` into wrapped lines while preserving list markers and slicing [`TextItem::spans`]
/// into styled runs for consistent rendering across Canvas, PNG, SVG, and PDF.
pub fn wrap_styled_text_lines(
    item: &TextItem,
    max_width: f32,
    mut measure_char: impl FnMut(char) -> f32,
) -> Vec<StyledWrappedLine> {
    wrap_styled_text_lines_with(item, max_width, |ch, _, _| measure_char(ch))
}

fn slice_styled_runs(
    full_text: &str,
    spans: &[TextSpan],
    seg_start: usize,
    seg_end: usize,
) -> Vec<StyledTextRun> {
    if seg_start >= seg_end {
        return Vec::new();
    }
    let mut cuts = vec![seg_start, seg_end];
    for sp in spans {
        if sp.end <= seg_start || sp.start >= seg_end {
            continue;
        }
        cuts.push(sp.start.clamp(seg_start, seg_end));
        cuts.push(sp.end.clamp(seg_start, seg_end));
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut runs = Vec::new();
    for win in cuts.windows(2) {
        let (a, b) = (win[0], win[1]);
        if a >= b {
            continue;
        }
        let mut bold = false;
        let mut italic = false;
        let mut underline = false;
        let mut highlight = None;
        let mut font = None;
        let mut size = None;
        let mut color = None;
        for sp in spans {
            if sp.start <= a && sp.end >= b {
                bold |= sp.bold;
                italic |= sp.italic;
                underline |= sp.underline;
                if let Some(hl) = sp.highlight {
                    highlight = Some(hl);
                }
                if let Some(f) = sp.font {
                    font = Some(f);
                }
                if let Some(sz) = sp.size {
                    size = Some(sz);
                }
                if let Some(c) = sp.color {
                    color = Some(c);
                }
            }
        }
        runs.push(StyledTextRun {
            text: full_text[a..b].to_string(),
            bold,
            italic,
            underline,
            highlight,
            font,
            size,
            color,
        });
    }
    runs
}

/// Maximum number of Unicode scalar values stored in one text item.
pub const MAX_TEXT_CHARS: usize = 4096;
/// Maximum number of text items accepted on one page.
pub const MAX_PAGE_TEXT_ITEMS: usize = 512;
/// Maximum combined number of Unicode scalar values accepted on one page.
pub const MAX_PAGE_TEXT_CHARS: usize = 16_384;

/// Outline shape tools available on the page canvas.
pub const MAX_POLYGON_VERTICES: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ShapeKind {
    Line,
    #[default]
    Rectangle,
    Ellipse,
    Triangle,
    Arrow,
    Polygon,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ShapeLineStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

impl ShapeLineStyle {
    pub const ALL: [Self; 3] = [Self::Solid, Self::Dashed, Self::Dotted];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Solid => "Solid",
            Self::Dashed => "Dashed",
            Self::Dotted => "Dotted",
        }
    }

    /// Returns `(dash_len, gap_len)` in page points for stroked outlines, or `None` for solid.
    pub fn dash_pattern(self, width: f32) -> Option<(f32, f32)> {
        let w = if width.is_finite() {
            width.clamp(0.5, 40.0)
        } else {
            2.0
        };
        match self {
            Self::Solid => None,
            Self::Dashed => Some(((w * 4.0).max(8.0), (w * 2.8).max(6.0))),
            Self::Dotted => Some(((w * 1.1).max(2.0), (w * 2.2).max(4.5))),
        }
    }
}

impl ShapeKind {
    pub const ALL: &[Self] = &[
        Self::Line,
        Self::Rectangle,
        Self::Ellipse,
        Self::Triangle,
        Self::Arrow,
        Self::Polygon,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Line => "Line",
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Triangle => "Triangle",
            Self::Arrow => "Arrow",
            Self::Polygon => "Polygon",
        }
    }
}

/// A positioned geometric outline stored on a notebook page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeometricShape {
    pub kind: ShapeKind,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    /// Rotation in degrees around the center of the shape's bounding box.
    pub angle_degrees: f64,
    /// Corner rounding radius in page points for polygons that support curves.
    pub curve: f64,
    pub color: Color,
    pub width: f32,
    /// Optional fill tint (`Some(color)` fills closed shapes before stroking the border).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_color: Option<Color>,
    /// Outline stroke pattern (`Solid`, `Dashed`, or `Dotted`).
    #[serde(default)]
    pub line_style: ShapeLineStyle,
    /// Vertices in page coordinates for `ShapeKind::Polygon` prior to `angle_degrees` rotation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<Point>,
}

impl Default for GeometricShape {
    fn default() -> Self {
        Self {
            kind: ShapeKind::default(),
            x1: 0.0,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
            angle_degrees: 0.0,
            curve: 0.0,
            color: Color::BLACK,
            width: 2.0,
            fill_color: None,
            line_style: ShapeLineStyle::Solid,
            points: Vec::new(),
        }
    }
}

impl GeometricShape {
    pub fn sanitized(&self) -> Self {
        let clean_coord = |value: f64| {
            if value.is_finite() {
                value.clamp(-1e6, 1e6)
            } else {
                0.0
            }
        };
        let points: Vec<Point> = if self.kind == ShapeKind::Polygon {
            self.points
                .iter()
                .take(MAX_POLYGON_VERTICES)
                .map(|p| Point::new(clean_coord(p.x), clean_coord(p.y)))
                .collect()
        } else {
            Vec::new()
        };
        let (x1, y1, x2, y2) = if self.kind == ShapeKind::Polygon && points.len() >= 3 {
            let mut min_x = f64::INFINITY;
            let mut min_y = f64::INFINITY;
            let mut max_x = f64::NEG_INFINITY;
            let mut max_y = f64::NEG_INFINITY;
            for p in &points {
                min_x = min_x.min(p.x);
                min_y = min_y.min(p.y);
                max_x = max_x.max(p.x);
                max_y = max_y.max(p.y);
            }
            (min_x, min_y, max_x, max_y)
        } else {
            (
                clean_coord(self.x1),
                clean_coord(self.y1),
                clean_coord(self.x2),
                clean_coord(self.y2),
            )
        };
        Self {
            kind: self.kind,
            x1,
            y1,
            x2,
            y2,
            angle_degrees: if self.angle_degrees.is_finite() {
                (self.angle_degrees + 180.0).rem_euclid(360.0) - 180.0
            } else {
                0.0
            },
            curve: if self.curve.is_finite() {
                self.curve.clamp(0.0, 256.0)
            } else {
                0.0
            },
            color: self.color.sanitized(),
            width: if self.width.is_finite() {
                self.width.clamp(0.5, 64.0)
            } else {
                2.0
            },
            fill_color: self.fill_color.map(|c| c.sanitized()).filter(|c| c.a > 0.0),
            line_style: self.line_style,
            points,
        }
    }

    /// Returns a flattened outline and whether it should be closed.
    /// Use `path_segments` when drawing shapes with separate subpaths, such as arrows.
    pub fn path_points(&self) -> (Vec<Point>, bool) {
        let shape = self.sanitized();
        let segments = shape.path_segments();
        if shape.kind == ShapeKind::Arrow {
            if let (Some((shaft, _)), Some((head, _))) = (segments.first(), segments.get(1))
                && let (Some(start), Some(tip), Some(left), Some(right)) =
                    (shaft.first(), head.get(1), head.first(), head.get(2))
            {
                return (vec![*start, *tip, *left, *tip, *right], false);
            }
            return (Vec::new(), false);
        }
        segments.into_iter().next().unwrap_or_default()
    }

    /// Returns separate outline paths, keeping arrow shafts and heads from sharing a sharp join.
    pub fn path_segments(&self) -> Vec<(Vec<Point>, bool)> {
        let shape = self.sanitized();
        let start = Point::new(shape.x1, shape.y1);
        let end = Point::new(shape.x2, shape.y2);
        let left = shape.x1.min(shape.x2);
        let right = shape.x1.max(shape.x2);
        let top = shape.y1.min(shape.y2);
        let bottom = shape.y1.max(shape.y2);

        let mut segments = match shape.kind {
            ShapeKind::Line => vec![(vec![start, end], false)],
            ShapeKind::Rectangle => vec![(
                round_polygon(
                    vec![
                        Point::new(left, top),
                        Point::new(right, top),
                        Point::new(right, bottom),
                        Point::new(left, bottom),
                    ],
                    shape.curve,
                ),
                true,
            )],
            ShapeKind::Ellipse => {
                let center_x = (left + right) * 0.5;
                let center_y = (top + bottom) * 0.5;
                let radius_x = (right - left) * 0.5;
                let radius_y = (bottom - top) * 0.5;
                let radius = radius_x.max(radius_y).max(1.0);
                let count = ((std::f64::consts::PI * (radius / 0.35).sqrt()).ceil() as usize)
                    .clamp(32, 360);
                let points = (0..count)
                    .map(|idx| {
                        let angle = std::f64::consts::TAU * idx as f64 / count as f64;
                        Point::new(
                            center_x + radius_x * angle.cos(),
                            center_y + radius_y * angle.sin(),
                        )
                    })
                    .collect();
                vec![(points, true)]
            }
            ShapeKind::Triangle => vec![(
                round_polygon(
                    vec![
                        Point::new((left + right) * 0.5, top),
                        Point::new(right, bottom),
                        Point::new(left, bottom),
                    ],
                    shape.curve,
                ),
                true,
            )],
            ShapeKind::Arrow => {
                let dx = end.x - start.x;
                let dy = end.y - start.y;
                let length = dx.hypot(dy);
                if length <= 1e-6 {
                    return vec![(vec![start, end], false)];
                }
                let unit_x = dx / length;
                let unit_y = dy / length;
                let head = (length * 0.22)
                    .max(f64::from(shape.width) * 2.5)
                    .min(length * 0.6);
                let half_width = head * 0.48;
                let base_x = end.x - unit_x * head;
                let base_y = end.y - unit_y * head;
                let left_head =
                    Point::new(base_x - unit_y * half_width, base_y + unit_x * half_width);
                let right_head =
                    Point::new(base_x + unit_y * half_width, base_y - unit_x * half_width);
                vec![
                    (vec![start, end], false),
                    (vec![left_head, end, right_head], false),
                ]
            }
            ShapeKind::Polygon => {
                let raw_pts = if shape.points.len() >= 3 {
                    shape.points.clone()
                } else {
                    let cx = (left + right) * 0.5;
                    let cy = (top + bottom) * 0.5;
                    let rx = ((right - left) * 0.5).max(1.0);
                    let ry = ((bottom - top) * 0.5).max(1.0);
                    (0..5)
                        .map(|i| {
                            let angle = -std::f64::consts::FRAC_PI_2
                                + std::f64::consts::TAU * (i as f64 / 5.0);
                            Point::new(cx + rx * angle.cos(), cy + ry * angle.sin())
                        })
                        .collect()
                };
                vec![(round_polygon(raw_pts, shape.curve), true)]
            }
        };

        if shape.angle_degrees.abs() > f64::EPSILON {
            let center = Point::new((left + right) * 0.5, (top + bottom) * 0.5);
            let angle = shape.angle_degrees.to_radians();
            let (sin, cos) = angle.sin_cos();
            for (points, _) in &mut segments {
                for point in points {
                    let dx = point.x - center.x;
                    let dy = point.y - center.y;
                    *point = Point::new(
                        center.x + dx * cos - dy * sin,
                        center.y + dx * sin + dy * cos,
                    );
                }
            }
        }
        segments
    }

    pub fn is_closed(&self) -> bool {
        matches!(
            self.kind,
            ShapeKind::Rectangle | ShapeKind::Ellipse | ShapeKind::Triangle | ShapeKind::Polygon
        )
    }
}

fn round_polygon(points: Vec<Point>, radius: f64) -> Vec<Point> {
    if points.len() < 3 || radius <= 0.0 {
        return points;
    }
    let samples = 8;
    let mut rounded = Vec::with_capacity(points.len() * (samples + 1));
    for idx in 0..points.len() {
        let previous = points[(idx + points.len() - 1) % points.len()];
        let corner = points[idx];
        let next = points[(idx + 1) % points.len()];
        let to_previous = Point::new(previous.x - corner.x, previous.y - corner.y);
        let to_next = Point::new(next.x - corner.x, next.y - corner.y);
        let previous_length = to_previous.x.hypot(to_previous.y);
        let next_length = to_next.x.hypot(to_next.y);
        if previous_length <= f64::EPSILON || next_length <= f64::EPSILON {
            continue;
        }
        let corner_radius = radius.min(previous_length * 0.5).min(next_length * 0.5);
        let entry = Point::new(
            corner.x + to_previous.x / previous_length * corner_radius,
            corner.y + to_previous.y / previous_length * corner_radius,
        );
        let exit = Point::new(
            corner.x + to_next.x / next_length * corner_radius,
            corner.y + to_next.y / next_length * corner_radius,
        );
        for step in 0..=samples {
            let t = step as f64 / samples as f64;
            let inverse = 1.0 - t;
            let pt = Point::new(
                inverse * inverse * entry.x + 2.0 * inverse * t * corner.x + t * t * exit.x,
                inverse * inverse * entry.y + 2.0 * inverse * t * corner.y + t * t * exit.y,
            );
            if rounded
                .last()
                .is_none_or(|prev: &Point| (prev.x - pt.x).hypot(prev.y - pt.y) > 1e-4)
            {
                rounded.push(pt);
            }
        }
    }
    if rounded.len() >= 3
        && rounded
            .first()
            .zip(rounded.last())
            .is_some_and(|(a, b)| (a.x - b.x).hypot(a.y - b.y) <= 1e-4)
    {
        rounded.pop();
    }
    rounded
}

/// Maximum number of geometric shapes accepted on one page.
pub const MAX_PAGE_SHAPES: usize = 512;

/// Maximum number of raster images accepted on one page.
pub const MAX_PAGE_IMAGES: usize = 128;
/// Maximum PNG payload size for a single placed page image (16 MiB).
pub const MAX_IMAGE_PNG_BYTES: usize = 16 * 1024 * 1024;
/// Maximum combined PNG payload size for all images on a single page (64 MiB).
pub const MAX_PAGE_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// Maximum stored pixel dimension (width or height) for a placed page image.
pub const MAX_IMAGE_PIXEL_DIM: u32 = 4096;

/// Encodes arbitrary bytes into standard Base64 without line breaks.
pub fn base64_encode_bytes(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for chunk in bytes.chunks(3) {
        let a = chunk.first().copied().unwrap_or(0);
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(a) << 16) | (u32::from(b) << 8) | u32::from(c);
        out.push(char::from(TABLE[((n >> 18) & 0x3f) as usize]));
        out.push(char::from(TABLE[((n >> 12) & 0x3f) as usize]));
        if chunk.len() > 1 {
            out.push(char::from(TABLE[((n >> 6) & 0x3f) as usize]));
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(TABLE[(n & 0x3f) as usize]));
        } else {
            out.push('=');
        }
    }
    out
}

fn base64_val(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decodes a Base64 string into raw bytes, rejecting malformed encodings and payloads
/// exceeding [`MAX_IMAGE_PNG_BYTES`].
pub fn base64_decode_bytes(raw: &str) -> Result<Vec<u8>, String> {
    base64_decode_bytes_inner(raw, Some(MAX_IMAGE_PNG_BYTES))
}

/// Decodes a Base64 string into raw bytes without an arbitrary payload size cap.
pub fn base64_decode_bytes_unbounded(raw: &str) -> Result<Vec<u8>, String> {
    base64_decode_bytes_inner(raw, None)
}

/// Decodes a Base64 string into raw bytes, rejecting malformed encodings and payloads
/// exceeding `max_decoded_bytes`.
pub fn base64_decode_bytes_with_limit(
    raw: &str,
    max_decoded_bytes: usize,
) -> Result<Vec<u8>, String> {
    base64_decode_bytes_inner(raw, Some(max_decoded_bytes))
}

fn base64_decode_bytes_inner(
    raw: &str,
    max_decoded_bytes: Option<usize>,
) -> Result<Vec<u8>, String> {
    let max_b64_chars = max_decoded_bytes.map(|limit| limit.div_ceil(3).saturating_mul(4));
    let mut clean = Vec::with_capacity(raw.len().min(65_536));
    for byte in raw.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        if max_b64_chars.is_some_and(|limit| clean.len() >= limit) {
            return Err("base64 payload exceeds maximum size limit".to_string());
        }
        clean.push(byte);
    }
    if clean.is_empty() {
        return Ok(Vec::new());
    }
    if !clean.len().is_multiple_of(4) {
        return Err("invalid base64 length".to_string());
    }
    let mut out = Vec::with_capacity((clean.len() / 4).saturating_mul(3));
    let (chunks, _) = clean.as_chunks::<4>();
    let total_chunks = chunks.len();
    for (idx, chunk) in chunks.iter().enumerate() {
        let a = base64_val(chunk[0]).ok_or_else(|| "invalid base64 character".to_string())?;
        let b = base64_val(chunk[1]).ok_or_else(|| "invalid base64 character".to_string())?;
        let is_last = idx + 1 == total_chunks;
        let pad_c = chunk[2] == b'=';
        let pad_d = chunk[3] == b'=';
        if (pad_c || pad_d) && !is_last {
            return Err("unexpected base64 padding before end of input".to_string());
        }
        if pad_c && !pad_d {
            return Err("malformed base64 padding".to_string());
        }
        out.push((a << 2) | (b >> 4));
        if !pad_c {
            let c = base64_val(chunk[2]).ok_or_else(|| "invalid base64 character".to_string())?;
            out.push((b << 4) | (c >> 2));
            if !pad_d {
                let d =
                    base64_val(chunk[3]).ok_or_else(|| "invalid base64 character".to_string())?;
                out.push((c << 6) | d);
            }
        }
        if max_decoded_bytes.is_some_and(|limit| out.len() > limit) {
            return Err("decoded base64 payload exceeds maximum size limit".to_string());
        }
    }
    Ok(out)
}

fn serialize_arc_base64<S>(bytes: &Arc<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let encoded = base64_encode_bytes(bytes.as_slice());
    serializer.serialize_str(&encoded)
}

fn deserialize_arc_base64<'de, D>(
    deserializer: D,
    max_bytes: usize,
    limit_err: &'static str,
) -> Result<Arc<Vec<u8>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let encoded = String::deserialize(deserializer)?;
    let decoded =
        base64_decode_bytes_with_limit(&encoded, max_bytes).map_err(serde::de::Error::custom)?;
    if decoded.len() > max_bytes {
        return Err(serde::de::Error::custom(limit_err));
    }
    Ok(Arc::new(decoded))
}

mod arc_base64_bytes {
    use super::{MAX_IMAGE_PNG_BYTES, deserialize_arc_base64, serialize_arc_base64};
    use serde::{Deserializer, Serializer};
    use std::sync::Arc;

    pub fn serialize<S>(bytes: &Arc<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_arc_base64(bytes, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Arc<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_arc_base64(
            deserializer,
            MAX_IMAGE_PNG_BYTES,
            "image PNG payload exceeds maximum limit",
        )
    }
}

/// Maximum display name length for an embedded PDF attachment.
pub const MAX_PDF_NAME_CHARS: usize = 128;

mod arc_pdf_base64_bytes {
    use super::{base64_decode_bytes_unbounded, serialize_arc_base64};
    use serde::{Deserialize, Deserializer, Serializer};
    use std::sync::Arc;

    pub fn serialize<S>(bytes: &Arc<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_arc_base64(bytes, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Arc<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let decoded = base64_decode_bytes_unbounded(&encoded).map_err(serde::de::Error::custom)?;
        Ok(Arc::new(decoded))
    }
}

/// Returns `true` when `bytes` begins with a PDF `%PDF-` header within the standard 1 KiB preamble.
#[must_use]
pub fn looks_like_pdf_bytes(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    head.windows(5).any(|w| w == b"%PDF-")
}

/// Sanitizes an embedded PDF filename.
#[must_use]
pub fn sanitize_pdf_name(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_PDF_NAME_CHARS)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Imported.pdf".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Source PDF payload stored once on a [`Document`] and shared across annotated pages.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddedPdf {
    pub name: String,
    #[serde(with = "arc_pdf_base64_bytes")]
    pub bytes: Arc<Vec<u8>>,
}

impl EmbeddedPdf {
    /// Clamps display name and drops empty or non-PDF payloads.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        let valid = !self.bytes.is_empty() && looks_like_pdf_bytes(&self.bytes);
        Self {
            name: sanitize_pdf_name(&self.name),
            bytes: if valid {
                Arc::clone(&self.bytes)
            } else {
                Arc::new(Vec::new())
            },
        }
    }
}

/// Links a [`NotebookPage`] to a specific 0-based page of an [`EmbeddedPdf`] on the parent [`Document`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PdfPageBackdrop {
    /// Index into [`Document::pdf_documents`].
    pub pdf_index: usize,
    /// 0-based page index inside that PDF.
    pub page_index: usize,
}

/// A positioned, resizable, rotatable raster image stored on a notebook page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PageImage {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Rotation in degrees around the center of the image's bounding box.
    pub angle_degrees: f64,
    pub pixel_width: u32,
    pub pixel_height: u32,
    #[serde(with = "arc_base64_bytes")]
    pub png_bytes: Arc<Vec<u8>>,
}

impl Default for PageImage {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 120.0,
            height: 120.0,
            angle_degrees: 0.0,
            pixel_width: 1,
            pixel_height: 1,
            png_bytes: Arc::new(Vec::new()),
        }
    }
}

impl PageImage {
    /// Clamps coordinates, dimensions, rotation, and PNG payload bounds.
    pub fn sanitized(&self) -> Self {
        let clean_pos = |value: f64| {
            if value.is_finite() {
                value.clamp(-1e6, 1e6)
            } else {
                0.0
            }
        };
        let clean_dim = |value: f64| {
            if value.is_finite() {
                value.clamp(8.0, 8192.0)
            } else {
                120.0
            }
        };
        Self {
            x: clean_pos(self.x),
            y: clean_pos(self.y),
            width: clean_dim(self.width),
            height: clean_dim(self.height),
            angle_degrees: if self.angle_degrees.is_finite() {
                (self.angle_degrees + 180.0).rem_euclid(360.0) - 180.0
            } else {
                0.0
            },
            pixel_width: self.pixel_width.clamp(1, MAX_IMAGE_PIXEL_DIM),
            pixel_height: self.pixel_height.clamp(1, MAX_IMAGE_PIXEL_DIM),
            png_bytes: if self.png_bytes.len() > MAX_IMAGE_PNG_BYTES {
                Arc::new(Vec::new())
            } else {
                Arc::clone(&self.png_bytes)
            },
        }
    }

    /// Returns the rotation pivot center of this image in page coordinates.
    pub fn center(&self) -> Point {
        let clean = self.sanitized();
        Point::new(clean.x + clean.width * 0.5, clean.y + clean.height * 0.5)
    }

    /// Returns the four corners `[top_left, top_right, bottom_right, bottom_left]`
    /// in page coordinates after applying `angle_degrees` around [`Self::center`].
    pub fn corners(&self) -> [Point; 4] {
        let clean = self.sanitized();
        let cx = clean.x + clean.width * 0.5;
        let cy = clean.y + clean.height * 0.5;
        let hw = clean.width * 0.5;
        let hh = clean.height * 0.5;
        let (sin, cos) = clean.angle_degrees.to_radians().sin_cos();
        let rotate =
            |lx: f64, ly: f64| Point::new(cx + lx * cos - ly * sin, cy + lx * sin + ly * cos);
        [
            rotate(-hw, -hh),
            rotate(hw, -hh),
            rotate(hw, hh),
            rotate(-hw, hh),
        ]
    }

    /// Returns the axis-aligned bounding box enclosing the rotated image corners.
    pub fn bounds(&self) -> Rect {
        Rect::from_point_iter(self.corners())
    }
}

/// Page ruling templates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaperTemplate {
    Blank,
    #[default]
    RuledNarrow,
    RuledWide,
    Grid,
    Dotted,
    Cornell,
    Isometric,
    MusicStaff,
    WeeklyPlanner,
    MonthlyPlanner,
    Checklist,
}

impl PaperTemplate {
    pub const ALL: &[Self] = &[
        Self::RuledNarrow,
        Self::RuledWide,
        Self::Grid,
        Self::Dotted,
        Self::Cornell,
        Self::Isometric,
        Self::MusicStaff,
        Self::WeeklyPlanner,
        Self::MonthlyPlanner,
        Self::Checklist,
        Self::Blank,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Blank => "Blank",
            Self::RuledNarrow => "Ruled (Narrow)",
            Self::RuledWide => "Ruled (Wide)",
            Self::Grid => "Grid",
            Self::Dotted => "Dotted",
            Self::Cornell => "Cornell",
            Self::Isometric => "Isometric Grid",
            Self::MusicStaff => "Music Staff",
            Self::WeeklyPlanner => "Weekly Planner",
            Self::MonthlyPlanner => "Monthly Planner",
            Self::Checklist => "Checklist",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Blank => "Unruled open canvas for freeform sketching and mind maps.",
            Self::RuledNarrow => {
                "Compact horizontal ruling with left margin line for everyday notes."
            }
            Self::RuledWide => "Generous line spacing for larger handwriting and lecture notes.",
            Self::Grid => "Square engineering grid for math, tables, and technical diagrams.",
            Self::Dotted => "Subtle dot matrix that guides alignment without visual clutter.",
            Self::Cornell => "Structured cue column, main notes area, and bottom summary section.",
            Self::Isometric => {
                "Equilateral 60-degree isometric grid for 3D sketching and technical layouts."
            }
            Self::MusicStaff => {
                "Five-line musical staves with crisp barlines for composition and notation."
            }
            Self::WeeklyPlanner => {
                "Structured seven-day schedule grid with dedicated weekly notes space."
            }
            Self::MonthlyPlanner => {
                "Full monthly calendar grid with weekday headers and bottom notes section."
            }
            Self::Checklist => {
                "Dedicated checkbox margin paired with clean ruled lines for tasks and lists."
            }
        }
    }
}

/// Vector ruling segment clipped strictly within `[0, width] x [0, height]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaperLine {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub accent: bool,
    pub dashed: bool,
    pub stroke_width: f32,
}

/// Square checkbox primitive emitted by structured checklist templates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaperBox {
    pub x: f32,
    pub y: f32,
    pub size: f32,
}

/// Optional crisp printed section heading or weekday header inside structured templates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaperLabel {
    pub x: f32,
    pub y: f32,
    pub font_size: f32,
    pub text: &'static str,
    pub accent: bool,
}

/// Shared paper template geometry consumed uniformly by canvas, raster compositor, SVG, and PDF.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaperGeometry {
    pub lines: Vec<PaperLine>,
    pub dots: Vec<(f32, f32)>,
    pub checkboxes: Vec<PaperBox>,
    pub labels: Vec<PaperLabel>,
}

fn clip_segment_to_bounds(
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    width: f32,
    height: f32,
) -> Option<(f32, f32, f32, f32)> {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let mut t0 = 0.0_f32;
    let mut t1 = 1.0_f32;
    for (p, q) in [(-dx, x1), (dx, width - x1), (-dy, y1), (dy, height - y1)] {
        if p.abs() <= 1e-6 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                if t > t1 {
                    return None;
                }
                t0 = t0.max(t);
            } else {
                if t < t0 {
                    return None;
                }
                t1 = t1.min(t);
            }
        }
    }
    let cx1 = (x1 + t0 * dx).clamp(0.0, width);
    let cy1 = (y1 + t0 * dy).clamp(0.0, height);
    let cx2 = (x1 + t1 * dx).clamp(0.0, width);
    let cy2 = (y1 + t1 * dy).clamp(0.0, height);
    if (cx2 - cx1).hypot(cy2 - cy1) < 0.5 {
        return None;
    }
    Some((cx1, cy1, cx2, cy2))
}

fn push_clipped_line(
    geom: &mut PaperGeometry,
    start: [f32; 2],
    end: [f32; 2],
    bounds: (f32, f32),
    accent: bool,
    stroke_width: f32,
) {
    if let Some((cx1, cy1, cx2, cy2)) =
        clip_segment_to_bounds(start[0], start[1], end[0], end[1], bounds.0, bounds.1)
    {
        geom.lines.push(PaperLine {
            x1: cx1,
            y1: cy1,
            x2: cx2,
            y2: cy2,
            accent,
            dashed: false,
            stroke_width,
        });
    }
}

/// Computes deterministic vector paper geometry for `template` sized to `(width, height)`.
#[must_use]
pub fn paper_template_geometry(width: u32, height: u32, template: PaperTemplate) -> PaperGeometry {
    let mut geom = PaperGeometry::default();
    if width < 16 || height < 16 {
        return geom;
    }
    let w = width as f32;
    let h = height as f32;
    let b = (w, h);

    match template {
        PaperTemplate::Blank => {}
        PaperTemplate::RuledNarrow | PaperTemplate::RuledWide => {
            let spacing = if template == PaperTemplate::RuledNarrow {
                36.0_f32
            } else {
                52.0_f32
            };
            let margin_x = (w * 0.1).clamp(64.0, 140.0).min(w - 16.0);
            let top_y = (h * 0.08).clamp(48.0, 132.0);
            push_clipped_line(&mut geom, [margin_x, 0.0], [margin_x, h], b, true, 1.4);
            let mut y = top_y;
            while y < h - 16.0 {
                push_clipped_line(&mut geom, [0.0, y], [w, y], b, false, 1.0);
                y += spacing;
            }
        }
        PaperTemplate::Grid => {
            let step = 40.0_f32;
            let mut x = step;
            while x < w {
                push_clipped_line(&mut geom, [x, 0.0], [x, h], b, false, 1.0);
                x += step;
            }
            let mut y = step;
            while y < h {
                push_clipped_line(&mut geom, [0.0, y], [w, y], b, false, 1.0);
                y += step;
            }
        }
        PaperTemplate::Dotted => {
            let step = 40.0_f32;
            let mut y = step;
            while y < h {
                let mut x = step;
                while x < w {
                    geom.dots.push((x, y));
                    x += step;
                }
                y += step;
            }
        }
        PaperTemplate::Cornell => {
            let top_y = (h * 0.09).clamp(64.0, 148.0);
            let summary_y = (h * 0.80).clamp(top_y + 40.0, h - 48.0);
            let cue_x = (w * 0.27).clamp(120.0, w - 120.0);
            push_clipped_line(&mut geom, [0.0, top_y], [w, top_y], b, true, 1.5);
            push_clipped_line(&mut geom, [0.0, summary_y], [w, summary_y], b, true, 1.5);
            push_clipped_line(&mut geom, [cue_x, top_y], [cue_x, summary_y], b, true, 1.5);
            let mut y = top_y + 40.0;
            while y < summary_y - 12.0 {
                push_clipped_line(&mut geom, [cue_x, y], [w, y], b, false, 1.0);
                y += 40.0;
            }
            let mut sy = summary_y + 40.0;
            while sy < h - 16.0 {
                push_clipped_line(&mut geom, [0.0, sy], [w, sy], b, false, 1.0);
                sy += 40.0;
            }
        }
        PaperTemplate::Isometric => {
            let side = 36.0_f32;
            let half_sqrt3 = 3.0_f32.sqrt() * 0.5;
            let dx = side * half_sqrt3;
            let slope = 1.0_f32 / 3.0_f32.sqrt();
            let mut x = dx;
            while x < w {
                push_clipped_line(&mut geom, [x, 0.0], [x, h], b, false, 0.9);
                x += dx;
            }
            let max_y_span = w * slope;
            let mut y0 = -max_y_span;
            while y0 <= h + max_y_span {
                // Ascending +30 deg from left to right.
                push_clipped_line(&mut geom, [0.0, y0], [w, y0 + w * slope], b, false, 0.9);
                // Descending -30 deg from left to right.
                push_clipped_line(&mut geom, [0.0, y0], [w, y0 - w * slope], b, false, 0.9);
                y0 += side;
            }
        }
        PaperTemplate::MusicStaff => {
            let margin_x = (w * 0.065).clamp(24.0, 76.0).min(w * 0.2);
            let top_y = (h * 0.06).clamp(32.0, 96.0);
            let bottom_limit = h - (h * 0.045).clamp(24.0, 72.0);
            let line_gap = 8.5_f32;
            let staff_height = line_gap * 4.0;
            let staff_pitch = staff_height + 46.0;
            let right_x = w - margin_x;
            let mut staff_top = top_y;
            while staff_top + staff_height <= bottom_limit {
                for i in 0..5 {
                    let ly = staff_top + i as f32 * line_gap;
                    push_clipped_line(&mut geom, [margin_x, ly], [right_x, ly], b, false, 1.05);
                }
                push_clipped_line(
                    &mut geom,
                    [margin_x, staff_top],
                    [margin_x, staff_top + staff_height],
                    b,
                    true,
                    1.4,
                );
                push_clipped_line(
                    &mut geom,
                    [right_x, staff_top],
                    [right_x, staff_top + staff_height],
                    b,
                    true,
                    1.4,
                );
                staff_top += staff_pitch;
            }
        }
        PaperTemplate::WeeklyPlanner => {
            let margin_x = (w * 0.05).clamp(24.0, 64.0);
            let top_y = (h * 0.045).clamp(28.0, 68.0);
            let bottom_y = h - (h * 0.04).clamp(20.0, 56.0);
            let right_x = w - margin_x;
            let grid_top = top_y + 42.0;
            if right_x - margin_x < 64.0 || bottom_y - grid_top < 80.0 {
                return geom;
            }
            geom.labels.push(PaperLabel {
                x: margin_x,
                y: top_y,
                font_size: 20.0,
                text: "WEEKLY PLANNER",
                accent: true,
            });
            push_clipped_line(
                &mut geom,
                [margin_x, top_y + 28.0],
                [right_x, top_y + 28.0],
                b,
                true,
                1.6,
            );
            let col_w = (right_x - margin_x) * 0.5;
            let row_h = (bottom_y - grid_top) / 4.0;
            let mid_x = margin_x + col_w;
            push_clipped_line(
                &mut geom,
                [margin_x, grid_top],
                [right_x, grid_top],
                b,
                true,
                1.4,
            );
            push_clipped_line(
                &mut geom,
                [margin_x, bottom_y],
                [right_x, bottom_y],
                b,
                true,
                1.4,
            );
            push_clipped_line(
                &mut geom,
                [margin_x, grid_top],
                [margin_x, bottom_y],
                b,
                true,
                1.4,
            );
            push_clipped_line(
                &mut geom,
                [right_x, grid_top],
                [right_x, bottom_y],
                b,
                true,
                1.4,
            );
            push_clipped_line(
                &mut geom,
                [mid_x, grid_top],
                [mid_x, bottom_y],
                b,
                true,
                1.3,
            );
            for r in 1..4 {
                let y = grid_top + r as f32 * row_h;
                push_clipped_line(&mut geom, [margin_x, y], [right_x, y], b, true, 1.3);
            }
            let day_names = [["MON", "TUE", "WED", "THU"], ["FRI", "SAT", "SUN", "NOTES"]];
            for (c, col_labels) in day_names.iter().enumerate() {
                let cell_x = margin_x + c as f32 * col_w;
                for (r, &label) in col_labels.iter().enumerate() {
                    let cell_y = grid_top + r as f32 * row_h;
                    geom.labels.push(PaperLabel {
                        x: cell_x + 12.0,
                        y: cell_y + 8.0,
                        font_size: 13.0,
                        text: label,
                        accent: true,
                    });
                    let header_line_y = (cell_y + 28.0).min(cell_y + row_h - 4.0);
                    push_clipped_line(
                        &mut geom,
                        [cell_x + 8.0, header_line_y],
                        [cell_x + col_w - 8.0, header_line_y],
                        b,
                        true,
                        1.0,
                    );
                    let mut ly = header_line_y + 28.0;
                    while ly < cell_y + row_h - 10.0 {
                        push_clipped_line(
                            &mut geom,
                            [cell_x + 10.0, ly],
                            [cell_x + col_w - 10.0, ly],
                            b,
                            false,
                            0.85,
                        );
                        ly += 28.0;
                    }
                }
            }
        }
        PaperTemplate::MonthlyPlanner => {
            let margin_x = (w * 0.05).clamp(24.0, 64.0);
            let top_y = (h * 0.045).clamp(28.0, 68.0);
            let bottom_y = h - (h * 0.04).clamp(20.0, 56.0);
            let right_x = w - margin_x;
            let weekday_y = top_y + 42.0;
            let cal_top = weekday_y + 26.0;
            let notes_top = (cal_top + (bottom_y - cal_top) * 0.78).min(bottom_y - 40.0);
            if right_x - margin_x < 140.0 || notes_top - cal_top < 80.0 {
                return geom;
            }
            geom.labels.push(PaperLabel {
                x: margin_x,
                y: top_y,
                font_size: 20.0,
                text: "MONTHLY PLANNER",
                accent: true,
            });
            push_clipped_line(
                &mut geom,
                [margin_x, top_y + 28.0],
                [right_x, top_y + 28.0],
                b,
                true,
                1.6,
            );
            let col_w = (right_x - margin_x) / 7.0;
            let row_h = (notes_top - cal_top) / 5.0;
            let weekdays = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];
            for (c, &day) in weekdays.iter().enumerate() {
                let cx = margin_x + c as f32 * col_w;
                geom.labels.push(PaperLabel {
                    x: cx + 8.0,
                    y: weekday_y + 4.0,
                    font_size: 12.0,
                    text: day,
                    accent: true,
                });
            }
            for c in 0..=7 {
                let x = margin_x + c as f32 * col_w;
                push_clipped_line(&mut geom, [x, cal_top], [x, notes_top], b, true, 1.15);
            }
            for r in 0..=5 {
                let y = cal_top + r as f32 * row_h;
                push_clipped_line(&mut geom, [margin_x, y], [right_x, y], b, true, 1.15);
            }
            let notes_label_y = notes_top + 14.0;
            geom.labels.push(PaperLabel {
                x: margin_x,
                y: notes_label_y,
                font_size: 14.0,
                text: "NOTES",
                accent: true,
            });
            let mut ly = notes_label_y + 26.0;
            while ly < bottom_y - 8.0 {
                push_clipped_line(&mut geom, [margin_x, ly], [right_x, ly], b, false, 0.95);
                ly += 30.0;
            }
        }
        PaperTemplate::Checklist => {
            let margin_x = (w * 0.055).clamp(24.0, 68.0);
            let top_y = (h * 0.05).clamp(28.0, 72.0);
            let right_x = w - margin_x;
            let divider_x = (margin_x + 44.0).min(right_x - 32.0);
            let header_line_y = top_y + 30.0;
            geom.labels.push(PaperLabel {
                x: margin_x,
                y: top_y,
                font_size: 20.0,
                text: "CHECKLIST",
                accent: true,
            });
            push_clipped_line(
                &mut geom,
                [margin_x, header_line_y],
                [right_x, header_line_y],
                b,
                true,
                1.6,
            );
            push_clipped_line(
                &mut geom,
                [divider_x, header_line_y],
                [divider_x, h - 24.0],
                b,
                true,
                1.3,
            );
            let box_size = 16.0_f32;
            let row_spacing = 36.0_f32;
            let mut y = header_line_y + row_spacing;
            while y < h - 24.0 {
                push_clipped_line(&mut geom, [margin_x, y], [right_x, y], b, false, 1.0);
                geom.checkboxes.push(PaperBox {
                    x: margin_x + (divider_x - margin_x - box_size) * 0.5,
                    y: y - box_size - 6.0,
                    size: box_size,
                });
                y += row_spacing;
            }
        }
    }
    geom
}

/// Paper surface tint presets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaperColor {
    WarmCream,
    #[default]
    PureWhite,
    DarkSlate,
}

impl PaperColor {
    pub const ALL: &[Self] = &[Self::WarmCream, Self::PureWhite, Self::DarkSlate];

    pub fn label(self) -> &'static str {
        match self {
            Self::WarmCream => "Warm Cream",
            Self::PureWhite => "Pure White",
            Self::DarkSlate => "Dark Slate",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::WarmCream => {
                "Low-glare ivory paper tint designed for comfortable long reading and writing sessions."
            }
            Self::PureWhite => {
                "High-contrast bright white sheet for crisp ink and vibrant highlighter colors."
            }
            Self::DarkSlate => "Dark mode charcoal sheet engineered for low-light environments.",
        }
    }

    pub fn background_color(self) -> Color {
        match self {
            Self::WarmCream => Color::from_rgba8(0xFC, 0xF9, 0xF0, 0xFF),
            Self::PureWhite => Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xFF),
            Self::DarkSlate => Color::from_rgba8(0x22, 0x25, 0x2A, 0xFF),
        }
    }

    pub fn line_color(self) -> Color {
        match self {
            Self::WarmCream => Color::from_rgba8(0xDD, 0xD6, 0xC7, 0xFF),
            Self::PureWhite => Color::from_rgba8(0xDC, 0xE1, 0xE8, 0xFF),
            Self::DarkSlate => Color::from_rgba8(0x36, 0x3B, 0x43, 0xFF),
        }
    }

    pub fn margin_color(self) -> Color {
        match self {
            Self::WarmCream | Self::PureWhite => Color::from_rgba8(0xE0, 0x8B, 0x8B, 0xD8),
            Self::DarkSlate => Color::from_rgba8(0x9E, 0x52, 0x52, 0xD8),
        }
    }
}

/// Pen engine styles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PenKind {
    #[default]
    FountainPen,
    BallPen,
    BrushPen,
    Highlighter,
    Pencil,
}

impl PenKind {
    pub const ALL: &[Self] = &[
        Self::FountainPen,
        Self::BallPen,
        Self::BrushPen,
        Self::Highlighter,
        Self::Pencil,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::FountainPen => "Fountain Pen",
            Self::BallPen => "Ball Pen",
            Self::BrushPen => "Brush Pen",
            Self::Highlighter => "Highlighter",
            Self::Pencil => "Pencil",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::FountainPen => "Fountain",
            Self::BallPen => "Ball",
            Self::BrushPen => "Brush",
            Self::Highlighter => "Highlight",
            Self::Pencil => "Pencil",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::FountainPen => {
                "Direction- and pressure-sensitive calligraphic nib that creates expressive thick-and-thin strokes. Draw and hold at the end to snap shapes or straighten lines."
            }
            Self::BallPen => {
                "Consistent rollerball tip for uniform everyday handwriting, formulas, and technical notes. Draw and hold at the end to snap shapes or straighten lines."
            }
            Self::BrushPen => {
                "Flexible art brush tip with wide pressure and stylus-tilt modulation. Draw and hold at the end to snap shapes or straighten lines."
            }
            Self::Highlighter => {
                "Flat chisel marker with squared ends aligned to each stroke; dark ink underneath stays sharp and legible. Draw a line, then hold for 1 second to straighten it."
            }
            Self::Pencil => {
                "Textured graphite lead with natural pressure shading and wide stylus tilt angle response. Draw and hold at the end to snap shapes or straighten lines."
            }
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::FountainPen => 0,
            Self::BallPen => 1,
            Self::BrushPen => 2,
            Self::Highlighter => 3,
            Self::Pencil => 4,
        }
    }
}

/// Eraser behavior modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EraserMode {
    #[default]
    StrokeEraser,
    PrecisionEraser,
}

impl EraserMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::StrokeEraser => "Stroke Eraser",
            Self::PrecisionEraser => "Precision Eraser",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::StrokeEraser => {
                "Removes an entire ink stroke the moment the eraser touches any part of it."
            }
            Self::PrecisionEraser => {
                "Erases only the exact section touched, cleanly splitting strokes and rebuilding tapered tips."
            }
        }
    }
}

/// Highlighter drawing modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HighlighterMode {
    #[default]
    Freehand,
    Straight,
}

impl HighlighterMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Freehand => "Freehand Highlighter",
            Self::Straight => "Straight Highlighter",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Freehand => {
                "Follows your hand freely for curved highlights, circles, and organic annotations."
            }
            Self::Straight => {
                "Locks each highlight into a crisp straight line from start to finish—ideal for underlining or highlighting text lines."
            }
        }
    }
}

/// Live stroke stabilization algorithms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StabilizerMode {
    None,
    #[default]
    OneEuro,
    PulledString,
    ExponentialMovingAverage,
    GaussianWindow,
    CircularBuffer,
    ModeledSpring,
    CurvedBezier,
    SimpleDirect,
}

impl StabilizerMode {
    pub const ALL: &[Self] = &[
        Self::CircularBuffer,
        Self::ModeledSpring,
        Self::CurvedBezier,
        Self::SimpleDirect,
        Self::OneEuro,
        Self::PulledString,
        Self::ExponentialMovingAverage,
        Self::GaussianWindow,
        Self::None,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Off",
            Self::OneEuro => "1€ Adaptive",
            Self::PulledString => "Pulled String",
            Self::ExponentialMovingAverage => "EMA Catch-Up",
            Self::GaussianWindow => "Gaussian Window",
            Self::CircularBuffer => "Circular Buffer",
            Self::ModeledSpring => "Modeled Spring-Mass",
            Self::CurvedBezier => "Curved Cubic Spline",
            Self::SimpleDirect => "Simple Direct",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::OneEuro => {
                "Speed-adaptive low-pass filter: removes hand tremor on slow strokes while staying instant on fast strokes."
            }
            Self::PulledString => {
                "Tethers the ink tip behind your cursor on a virtual string for deliberate curves and line art."
            }
            Self::ExponentialMovingAverage => {
                "Smooths input with exponential momentum and glides to the pen tip at stroke end."
            }
            Self::GaussianWindow => {
                "Applies a symmetric bell-curve moving window for uniform, silk-like curve polishing."
            }
            Self::CircularBuffer => {
                "Circular event buffer averaging (Arithmetic Mean or Velocity Gaussian) with optional Deadzone/Inertia preprocessing and C1 quadratic spline finalization."
            }
            Self::ModeledSpring => {
                "Three-stage physical stroke modeler combining velocity wobble filtering, 180 Hz spring-mass tip dynamics, and arc-projected stylus state."
            }
            Self::CurvedBezier => {
                "Sliding 4-point Catmull-Rom to C1 cubic Bézier curve builder with adaptive flatness subdivision."
            }
            Self::SimpleDirect => {
                "Low-latency direct segment builder with sub-pixel noise filtering and linear pressure interpolation."
            }
            Self::None => {
                "Bypasses live stabilization filters for raw 1:1 hardware cursor tracking."
            }
        }
    }
}

/// Curated 1-click stroke stabilization presets exposed in the Pen settings drawer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StabilizationPreset {
    #[default]
    #[serde(alias = "ArithmeticBuffer20")]
    ArithmeticBuffer10,
    #[serde(alias = "QuickNotes")]
    ModeledSpring,
    #[serde(alias = "PrecisionPulledString")]
    CurvedBezier,
    #[serde(alias = "RawUnfiltered")]
    SimpleDirect,
}

impl StabilizationPreset {
    pub const ALL: &[Self] = &[
        Self::ArithmeticBuffer10,
        Self::ModeledSpring,
        Self::CurvedBezier,
        Self::SimpleDirect,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::ArithmeticBuffer10 => "Arithmetic Mean (10)",
            Self::ModeledSpring => "Modeled Spring-Mass",
            Self::CurvedBezier => "Curved Cubic Spline",
            Self::SimpleDirect => "Simple Direct",
        }
    }

    pub const fn badge(self) -> &'static str {
        match self {
            Self::ArithmeticBuffer10 => "Buffer 10 · C1 Finalize",
            Self::ModeledSpring => "180 Hz Spring · Wobble Filter",
            Self::CurvedBezier => "4-Knot Catmull · Cubic Bézier",
            Self::SimpleDirect => "1:1 Direct · Zero Lag",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::ArithmeticBuffer10 => {
                "Averages the last 10 samples in a pre-filled circular buffer and completes the tail with a C1 spline for calm, everyday handwriting."
            }
            Self::ModeledSpring => {
                "Dampens slow-speed hand tremor with a time-windowed wobble filter while pulling a 180 Hz spring-mass virtual nib with natural momentum."
            }
            Self::CurvedBezier => {
                "Fits continuous C1 cubic Bézier curves through a sliding 4-point Catmull-Rom window for flowing cursive writing and calligraphy."
            }
            Self::SimpleDirect => {
                "Connects pointer samples directly with sub-pixel noise filtering and pressure interpolation for immediate 1:1 hardware tracking."
            }
        }
    }

    pub fn to_smoothing_config(self) -> SmoothingConfig {
        match self {
            Self::ArithmeticBuffer10 => SmoothingConfig::input_stabilization_default(),
            Self::ModeledSpring => SmoothingConfig {
                stabilizer_mode: StabilizerMode::ModeledSpring,
                stabilizer_strength: 0.55,
                catmull_rom_subdivision: false,
                spline_tension: 0.5,
                adjust_for_zoom: true,
                catch_up: true,
                catch_up_on_end: true,
                streamline_amount: 0.0,
                prediction_ms: 6.0,
                ..SmoothingConfig::input_stabilization_default()
            },
            Self::CurvedBezier => SmoothingConfig {
                stabilizer_mode: StabilizerMode::CurvedBezier,
                stabilizer_strength: 0.65,
                catmull_rom_subdivision: false,
                spline_tension: 0.5,
                adjust_for_zoom: true,
                catch_up: false,
                catch_up_on_end: true,
                streamline_amount: 0.15,
                prediction_ms: 4.0,
                ..SmoothingConfig::input_stabilization_default()
            },
            Self::SimpleDirect => SmoothingConfig {
                stabilizer_mode: StabilizerMode::SimpleDirect,
                stabilizer_strength: 0.0,
                catmull_rom_subdivision: false,
                spline_tension: 0.0,
                adjust_for_zoom: true,
                catch_up: false,
                catch_up_on_end: false,
                streamline_amount: 0.0,
                prediction_ms: 0.0,
                buffer_averaging: BufferAveraging::None,
                buffer_preprocessor: BufferPreprocessor::None,
                buffer_finalize_stroke: false,
                ..SmoothingConfig::input_stabilization_default()
            },
        }
    }

    pub fn from_smoothing_config(cfg: &SmoothingConfig) -> Self {
        match cfg.stabilizer_mode {
            StabilizerMode::CircularBuffer => {
                if cfg.buffer_averaging == BufferAveraging::None
                    && cfg.buffer_preprocessor == BufferPreprocessor::None
                {
                    Self::SimpleDirect
                } else {
                    Self::ArithmeticBuffer10
                }
            }
            StabilizerMode::ModeledSpring
            | StabilizerMode::OneEuro
            | StabilizerMode::ExponentialMovingAverage => Self::ModeledSpring,
            StabilizerMode::CurvedBezier
            | StabilizerMode::GaussianWindow
            | StabilizerMode::PulledString => Self::CurvedBezier,
            StabilizerMode::SimpleDirect | StabilizerMode::None => Self::SimpleDirect,
        }
    }
}

/// Averaging backend used when [`StabilizerMode::CircularBuffer`] is active.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BufferAveraging {
    #[default]
    ArithmeticMean,
    VelocityGaussian,
    None,
}

impl BufferAveraging {
    pub const ALL: &[Self] = &[Self::None, Self::ArithmeticMean, Self::VelocityGaussian];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::ArithmeticMean => "Arithmetic mean",
            Self::VelocityGaussian => "Velocity based Gaussian weights",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::None => {
                "Disables coordinate averaging while still applying any active preprocessor and optional stroke finalization."
            }
            Self::ArithmeticMean => {
                "Uniformly averages the last N samples in a circular buffer pre-filled on pen down for calm, consistent lines."
            }
            Self::VelocityGaussian => {
                "Weights recent pointer samples with a Gaussian falloff scaled by pointer velocity and sigma."
            }
        }
    }
}

/// Optional spatial/physics preprocessor stage before [`BufferAveraging`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BufferPreprocessor {
    #[default]
    None,
    Deadzone,
    Inertia,
}

impl BufferPreprocessor {
    pub const ALL: &[Self] = &[Self::None, Self::Deadzone, Self::Inertia];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Deadzone => "Deadzone",
            Self::Inertia => "Inertia",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::None => {
                "Feeds raw pointer positions directly into the averaging buffer without pre-filtering."
            }
            Self::Deadzone => {
                "Ignores jitter inside a radial deadzone until movement exits the circle; supports sharp-corner cusp detection."
            }
            Self::Inertia => {
                "Models virtual stylus mass and surface drag as a damped spring pulled toward the cursor."
            }
        }
    }
}

/// Stabilization, spline subdivision, fairing, and tip prediction parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmoothingConfig {
    pub stabilizer_mode: StabilizerMode,
    pub stabilizer_strength: f32,
    pub catmull_rom_subdivision: bool,
    pub spline_tension: f32,
    pub adjust_for_zoom: bool,
    pub catch_up: bool,
    pub catch_up_on_end: bool,
    pub streamline_amount: f32,
    pub prediction_ms: f32,
    pub buffer_averaging: BufferAveraging,
    pub buffer_preprocessor: BufferPreprocessor,
    pub buffer_size: u16,
    pub buffer_sigma: f32,
    pub buffer_deadzone_radius: f32,
    pub buffer_cusp_detection: bool,
    pub buffer_drag: f32,
    pub buffer_mass: f32,
    pub buffer_finalize_stroke: bool,
}

impl Default for SmoothingConfig {
    fn default() -> Self {
        Self::input_stabilization_default()
    }
}

impl SmoothingConfig {
    /// Default input stabilization configuration (`Arithmetic mean`, buffer size `10`,
    /// preprocessor `None`, `Finalize the stroke` enabled).
    pub fn input_stabilization_default() -> Self {
        Self {
            stabilizer_mode: StabilizerMode::CircularBuffer,
            stabilizer_strength: 0.5,
            catmull_rom_subdivision: false,
            spline_tension: 0.0,
            adjust_for_zoom: false,
            catch_up: false,
            catch_up_on_end: true,
            streamline_amount: 0.0,
            prediction_ms: 0.0,
            buffer_averaging: BufferAveraging::ArithmeticMean,
            buffer_preprocessor: BufferPreprocessor::None,
            buffer_size: 10,
            buffer_sigma: 0.5,
            buffer_deadzone_radius: 1.3,
            buffer_cusp_detection: true,
            buffer_drag: 0.4,
            buffer_mass: 5.0,
            buffer_finalize_stroke: true,
        }
    }

    /// Maps this configuration onto the closest curated [`StabilizationPreset`].
    pub fn preset(&self) -> StabilizationPreset {
        StabilizationPreset::from_smoothing_config(self)
    }

    /// Returns `true` when either an averaging method or a preprocessor is enabled.
    pub fn is_buffer_active(&self) -> bool {
        self.stabilizer_mode == StabilizerMode::CircularBuffer
            && (self.buffer_averaging != BufferAveraging::None
                || self.buffer_preprocessor != BufferPreprocessor::None)
    }

    /// Concise summary label describing the active stabilization pipeline and key parameters.
    pub fn stabilization_summary(&self) -> String {
        let clean = self.sanitized();
        if clean.stabilizer_mode != StabilizerMode::CircularBuffer {
            return clean.stabilizer_mode.label().to_string();
        }
        let avg_part = match clean.buffer_averaging {
            BufferAveraging::None => None,
            BufferAveraging::ArithmeticMean => {
                Some(format!("Arithmetic mean ({})", clean.buffer_size))
            }
            BufferAveraging::VelocityGaussian => {
                Some(format!("Velocity Gaussian (σ {:.2})", clean.buffer_sigma))
            }
        };
        let pre_part = match clean.buffer_preprocessor {
            BufferPreprocessor::None => None,
            BufferPreprocessor::Deadzone => {
                Some(format!("Deadzone ({:.2})", clean.buffer_deadzone_radius))
            }
            BufferPreprocessor::Inertia => Some(format!(
                "Inertia ({:.2}, {:.1})",
                clean.buffer_drag, clean.buffer_mass
            )),
        };
        match (avg_part, pre_part) {
            (Some(a), Some(p)) => format!("{a} + {p}"),
            (Some(a), None) => a,
            (None, Some(p)) => p,
            (None, None) => "None".to_string(),
        }
    }

    pub fn sanitized(&self) -> Self {
        let clamp = |v: f32, lo: f32, hi: f32, d: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { d }
        };
        Self {
            stabilizer_mode: self.stabilizer_mode,
            stabilizer_strength: clamp(self.stabilizer_strength, 0.0, 1.0, 0.45),
            catmull_rom_subdivision: self.catmull_rom_subdivision,
            spline_tension: clamp(self.spline_tension, 0.0, 1.0, 0.5),
            adjust_for_zoom: self.adjust_for_zoom,
            catch_up: self.catch_up,
            catch_up_on_end: self.catch_up_on_end,
            streamline_amount: clamp(self.streamline_amount, 0.0, 1.0, 0.25),
            prediction_ms: clamp(self.prediction_ms, 0.0, 32.0, 8.0),
            buffer_averaging: self.buffer_averaging,
            buffer_preprocessor: self.buffer_preprocessor,
            buffer_size: self.buffer_size.clamp(2, 100),
            buffer_sigma: clamp(self.buffer_sigma, 0.05, 5.0, 0.5),
            buffer_deadzone_radius: clamp(self.buffer_deadzone_radius, 0.1, 50.0, 1.3),
            buffer_cusp_detection: self.buffer_cusp_detection,
            buffer_drag: clamp(self.buffer_drag, 0.0, 1.0, 0.4),
            buffer_mass: clamp(self.buffer_mass, 1.0, 30.0, 5.0),
            buffer_finalize_stroke: self.buffer_finalize_stroke,
        }
    }
}

/// Built-in pressure response curve presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PressureCurvePreset {
    Linear,
    Soft,
    Firm,
    SCurve,
    Constant,
}

impl PressureCurvePreset {
    pub const ALL: &[Self] = &[
        Self::Linear,
        Self::Soft,
        Self::Firm,
        Self::SCurve,
        Self::Constant,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::Soft => "Soft",
            Self::Firm => "Firm",
            Self::SCurve => "S-Curve",
            Self::Constant => "Constant",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Linear => {
                "Direct 1:1 response between physical stylus force and stroke thickness."
            }
            Self::Soft => "Reaches full stroke width with a lighter touch to reduce hand fatigue.",
            Self::Firm => {
                "Requires deliberate downward pressure before widening, ideal for heavy-handed writers."
            }
            Self::SCurve => {
                "Gentle transitions at both light and heavy extremes with responsive midrange control."
            }
            Self::Constant => "Locks output to a fixed full level regardless of stylus pressure.",
        }
    }
}

fn quick_notes_smoothing() -> SmoothingConfig {
    SmoothingConfig {
        stabilizer_mode: StabilizerMode::OneEuro,
        stabilizer_strength: 0.28,
        catmull_rom_subdivision: true,
        spline_tension: 0.5,
        adjust_for_zoom: true,
        catch_up: true,
        catch_up_on_end: true,
        streamline_amount: 0.12,
        prediction_ms: 8.0,
        ..SmoothingConfig::default()
    }
}

fn calligraphy_silky_smoothing() -> SmoothingConfig {
    SmoothingConfig {
        stabilizer_mode: StabilizerMode::GaussianWindow,
        stabilizer_strength: 0.70,
        catmull_rom_subdivision: true,
        spline_tension: 0.5,
        adjust_for_zoom: true,
        catch_up: true,
        catch_up_on_end: true,
        streamline_amount: 0.55,
        prediction_ms: 10.0,
        ..SmoothingConfig::default()
    }
}

fn velocity_gaussian_smoothing() -> SmoothingConfig {
    SmoothingConfig {
        buffer_averaging: BufferAveraging::VelocityGaussian,
        ..SmoothingConfig::default()
    }
}

fn pulled_string_smoothing() -> SmoothingConfig {
    SmoothingConfig {
        stabilizer_mode: StabilizerMode::PulledString,
        stabilizer_strength: 0.30,
        catmull_rom_subdivision: true,
        spline_tension: 0.45,
        adjust_for_zoom: true,
        catch_up: false,
        catch_up_on_end: true,
        streamline_amount: 0.20,
        prediction_ms: 4.0,
        ..SmoothingConfig::default()
    }
}

/// Curated ready-to-write pen presets pairing nib physics and stroke stabilization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PenPreset {
    FountainEveryday,
    FountainCalligraphy,
    FountainFineSketch,
    BallpointFine,
    BallpointGel,
    BallpointBold,
    BrushFude,
    BrushSoftWash,
    HighlighterNarrow,
    HighlighterBroad,
    PencilSketch,
    PencilShading,
}

impl PenPreset {
    pub const ALL: &[Self] = &[
        Self::FountainEveryday,
        Self::FountainCalligraphy,
        Self::FountainFineSketch,
        Self::BallpointFine,
        Self::BallpointGel,
        Self::BallpointBold,
        Self::BrushFude,
        Self::BrushSoftWash,
        Self::HighlighterNarrow,
        Self::HighlighterBroad,
        Self::PencilSketch,
        Self::PencilShading,
    ];

    const FOUNTAIN_LIST: &[Self] = &[
        Self::FountainEveryday,
        Self::FountainCalligraphy,
        Self::FountainFineSketch,
    ];
    const BALL_LIST: &[Self] = &[Self::BallpointFine, Self::BallpointGel, Self::BallpointBold];
    const BRUSH_LIST: &[Self] = &[Self::BrushFude, Self::BrushSoftWash];
    const HIGHLIGHTER_LIST: &[Self] = &[Self::HighlighterNarrow, Self::HighlighterBroad];
    const PENCIL_LIST: &[Self] = &[Self::PencilSketch, Self::PencilShading];

    pub fn for_kind(kind: PenKind) -> &'static [Self] {
        match kind {
            PenKind::FountainPen => Self::FOUNTAIN_LIST,
            PenKind::BallPen => Self::BALL_LIST,
            PenKind::BrushPen => Self::BRUSH_LIST,
            PenKind::Highlighter => Self::HIGHLIGHTER_LIST,
            PenKind::Pencil => Self::PENCIL_LIST,
        }
    }

    pub fn kind(self) -> PenKind {
        match self {
            Self::FountainEveryday | Self::FountainCalligraphy | Self::FountainFineSketch => {
                PenKind::FountainPen
            }
            Self::BallpointFine | Self::BallpointGel | Self::BallpointBold => PenKind::BallPen,
            Self::BrushFude | Self::BrushSoftWash => PenKind::BrushPen,
            Self::HighlighterNarrow | Self::HighlighterBroad => PenKind::Highlighter,
            Self::PencilSketch | Self::PencilShading => PenKind::Pencil,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::FountainEveryday => "Everyday Fountain",
            Self::FountainCalligraphy => "Broad Calligraphy",
            Self::FountainFineSketch => "Fine Technical Nib",
            Self::BallpointFine => "Fine 0.5mm Ball",
            Self::BallpointGel => "Smooth Gel Note",
            Self::BallpointBold => "Bold Roller Marker",
            Self::BrushFude => "Fude Brush Pen",
            Self::BrushSoftWash => "Broad Art Brush",
            Self::HighlighterNarrow => "Narrow Text Marker",
            Self::HighlighterBroad => "Wide Passage Highlighter",
            Self::PencilSketch => "HB Graphite Pencil",
            Self::PencilShading => "2B Tilted Shader",
        }
    }

    pub fn badge(self) -> &'static str {
        match self {
            Self::FountainEveryday => "4.5 pt · Balanced",
            Self::FountainCalligraphy => "8.0 pt · Silky Ribbon",
            Self::FountainFineSketch => "2.4 pt · Crisp",
            Self::BallpointFine => "2.2 pt · Fine",
            Self::BallpointGel => "3.2 pt · Everyday",
            Self::BallpointBold => "5.6 pt · Pulled String",
            Self::BrushFude => "12.0 pt · Expressive",
            Self::BrushSoftWash => "22.0 pt · Silky Tilt",
            Self::HighlighterNarrow => "14.0 pt · Precise",
            Self::HighlighterBroad => "24.0 pt · Standard",
            Self::PencilSketch => "2.8 pt · Graphite",
            Self::PencilShading => "8.0 pt · Wide Tilt",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::FountainEveryday => {
                "Balanced calligraphic nib with subtle directional flair and Arithmetic Mean (Buffer 10) smoothing for daily notes."
            }
            Self::FountainCalligraphy => {
                "Wide flat nib paired with silky Gaussian smoothing for ribbon-like headers and cursive flourishes."
            }
            Self::FountainFineSketch => {
                "Crisp fine fountain tip with firm pressure response for neat annotations and margins."
            }
            Self::BallpointFine => {
                "Ultra-consistent fine rollerball line ideal for formulas, small labels, and compact grids."
            }
            Self::BallpointGel => {
                "Classic smooth ballpoint with Arithmetic Mean (Buffer 10) smoothing for effortless everyday writing."
            }
            Self::BallpointBold => {
                "Thick steady marker line with Pulled-String stabilization for bold diagrams and underlines."
            }
            Self::BrushFude => {
                "Responsive tapered brush responding dynamically to pressure and stylus tilt with smooth circular buffering."
            }
            Self::BrushSoftWash => {
                "Wide, velvety art brush with heavy smoothing and expressive tilt-flattening."
            }
            Self::HighlighterNarrow => {
                "Compact flat chisel marker with Velocity Gaussian smoothing calibrated for highlighting single lines of text."
            }
            Self::HighlighterBroad => {
                "Standard translucent chisel highlighter with Multiply blend so dark ink underneath never washes out."
            }
            Self::PencilSketch => {
                "Textured graphite lead with natural light-touch transparency and crisp sketch response."
            }
            Self::PencilShading => {
                "Soft wide pencil tip that fans smoothly into grainy shading when the stylus is tilted."
            }
        }
    }

    pub fn settings(self) -> PenSettings {
        match self {
            Self::FountainEveryday => PenSettings::fountain_pen(),
            Self::FountainCalligraphy => PenSettings {
                size: 8.0,
                nib_flatness: 0.68,
                nib_angle_deg: -40.0,
                pressure_size_min: 0.24,
                smoothing: calligraphy_silky_smoothing(),
                pressure_curve: PressureCurve::from_preset(PressureCurvePreset::SCurve),
                ..PenSettings::fountain_pen()
            },
            Self::FountainFineSketch => PenSettings {
                size: 2.4,
                nib_flatness: 0.22,
                nib_angle_deg: -30.0,
                pressure_size_min: 0.42,
                smoothing: quick_notes_smoothing(),
                pressure_curve: PressureCurve::from_preset(PressureCurvePreset::Firm),
                ..PenSettings::fountain_pen()
            },
            Self::BallpointFine => PenSettings {
                size: 2.2,
                nib_flatness: 0.0,
                pressure_enabled: false,
                smoothing: quick_notes_smoothing(),
                ..PenSettings::ball_pen()
            },
            Self::BallpointGel => PenSettings::ball_pen(),
            Self::BallpointBold => PenSettings {
                size: 5.6,
                nib_flatness: 0.0,
                pressure_enabled: false,
                smoothing: pulled_string_smoothing(),
                ..PenSettings::ball_pen()
            },
            Self::BrushFude => PenSettings::brush_pen(),
            Self::BrushSoftWash => PenSettings {
                size: 22.0,
                opacity: 0.88,
                hardness: 0.72,
                nib_flatness: 0.28,
                tilt_sensitivity: 0.85,
                smoothing: calligraphy_silky_smoothing(),
                pressure_curve: PressureCurve::from_preset(PressureCurvePreset::Soft),
                ..PenSettings::brush_pen()
            },
            Self::HighlighterNarrow => PenSettings {
                size: 14.0,
                nib_flatness: 0.65,
                smoothing: velocity_gaussian_smoothing(),
                ..PenSettings::highlighter()
            },
            Self::HighlighterBroad => PenSettings::highlighter(),
            Self::PencilSketch => PenSettings::pencil(),
            Self::PencilShading => PenSettings {
                size: 8.0,
                opacity: 0.72,
                tilt_sensitivity: 1.0,
                ..PenSettings::pencil()
            },
        }
    }

    pub fn apply_preserving_color(self, current_color: Color) -> PenSettings {
        let mut s = self.settings();
        s.color = current_color.sanitized();
        s
    }

    pub fn matching(pen: &PenSettings) -> Option<Self> {
        let clean = pen.sanitized();
        Self::for_kind(clean.kind).iter().copied().find(|&preset| {
            let ref_pen = preset.settings();
            (clean.size - ref_pen.size).abs() <= 0.25
                && (clean.nib_flatness - ref_pen.nib_flatness).abs() <= 0.04
                && clean.pressure_enabled == ref_pen.pressure_enabled
                && clean.smoothing.stabilizer_mode == ref_pen.smoothing.stabilizer_mode
        })
    }
}

/// Monotone cubic Hermite (Fritsch-Carlson) response curve mapping raw stylus pressure `0..=1`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PressureCurve {
    pub points: Vec<[f32; 2]>,
}

impl Default for PressureCurve {
    fn default() -> Self {
        Self::from_preset(PressureCurvePreset::Linear)
    }
}

impl PressureCurve {
    pub const MAX_POINTS: usize = 16;

    pub fn from_preset(preset: PressureCurvePreset) -> Self {
        let pts = match preset {
            PressureCurvePreset::Linear => vec![[0.0, 0.0], [1.0, 1.0]],
            PressureCurvePreset::Soft => {
                vec![
                    [0.0, 0.0],
                    [0.06, 0.10],
                    [0.28, 0.62],
                    [0.62, 0.88],
                    [1.0, 1.0],
                ]
            }
            PressureCurvePreset::Firm => vec![[0.0, 0.0], [0.4, 0.16], [0.75, 0.54], [1.0, 1.0]],
            PressureCurvePreset::SCurve => vec![
                [0.0, 0.0],
                [0.25, 0.14],
                [0.5, 0.5],
                [0.75, 0.86],
                [1.0, 1.0],
            ],
            PressureCurvePreset::Constant => vec![[0.0, 0.75], [1.0, 0.75]],
        };
        Self::new(pts)
    }

    pub fn new(raw_points: Vec<[f32; 2]>) -> Self {
        let mut pts: Vec<[f32; 2]> = raw_points
            .into_iter()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .take(Self::MAX_POINTS)
            .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
            .collect();

        if pts.len() < 2 {
            return Self {
                points: vec![[0.0, 0.0], [1.0, 1.0]],
            };
        }

        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));

        // Deduplicate nearly-identical x positions while keeping endpoints bounded.
        let mut clean: Vec<[f32; 2]> = Vec::with_capacity(pts.len());
        for pt in pts {
            if let Some(last) = clean.last_mut()
                && (pt[0] - last[0]).abs() < 1e-4
            {
                last[1] = pt[1];
                continue;
            }
            clean.push(pt);
        }

        if clean.len() < 2 {
            let y = clean.first().map_or(0.5, |p| p[1]);
            clean = vec![[0.0, y], [1.0, y]];
        }
        if let Some(first) = clean.first_mut() {
            first[0] = 0.0;
        }
        if let Some(last) = clean.last_mut() {
            last[0] = 1.0;
        }
        Self { points: clean }
    }

    pub fn is_linear(&self) -> bool {
        self.points.len() == 2
            && (self.points[0][0] - 0.0).abs() < 1e-4
            && (self.points[0][1] - 0.0).abs() < 1e-4
            && (self.points[1][0] - 1.0).abs() < 1e-4
            && (self.points[1][1] - 1.0).abs() < 1e-4
    }

    /// Evaluates the pressure curve using Fritsch-Carlson monotone cubic Hermite interpolation.
    pub fn eval(&self, pressure: f32) -> f32 {
        let p = if pressure.is_finite() {
            pressure.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let pts = &self.points;
        let n = pts.len();
        if n == 0 {
            return p;
        }
        if n == 1 {
            return pts[0][1].clamp(0.0, 1.0);
        }
        if p <= pts[0][0] {
            return pts[0][1].clamp(0.0, 1.0);
        }
        if p >= pts[n - 1][0] {
            return pts[n - 1][1].clamp(0.0, 1.0);
        }

        let mut h = Vec::with_capacity(n - 1);
        let mut delta = Vec::with_capacity(n - 1);
        for i in 0..(n - 1) {
            let dx = (pts[i + 1][0] - pts[i][0]).max(1e-5);
            let dy = pts[i + 1][1] - pts[i][1];
            h.push(dx);
            delta.push(dy / dx);
        }

        let mut m = vec![0.0_f32; n];
        m[0] = delta[0];
        m[n - 1] = delta[n - 2];
        for i in 1..(n - 1) {
            if delta[i - 1] * delta[i] <= 0.0 {
                m[i] = 0.0;
            } else {
                m[i] = 0.5 * (delta[i - 1] + delta[i]);
            }
        }

        // Fritsch-Carlson monotonicity clamp.
        for i in 0..(n - 1) {
            if delta[i].abs() < 1e-6 {
                m[i] = 0.0;
                m[i + 1] = 0.0;
            } else {
                let alpha = m[i] / delta[i];
                let beta = m[i + 1] / delta[i];
                let norm_sq = alpha * alpha + beta * beta;
                if norm_sq > 9.0 {
                    let tau = 3.0 / norm_sq.sqrt();
                    m[i] = tau * alpha * delta[i];
                    m[i + 1] = tau * beta * delta[i];
                }
            }
        }

        let seg = (0..(n - 1))
            .find(|&i| p >= pts[i][0] && p <= pts[i + 1][0])
            .unwrap_or(n - 2);
        let dx = h[seg];
        let t = ((p - pts[seg][0]) / dx).clamp(0.0, 1.0);
        let t2 = t * t;
        let t3 = t2 * t;
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;

        let y =
            h00 * pts[seg][1] + h10 * dx * m[seg] + h01 * pts[seg + 1][1] + h11 * dx * m[seg + 1];
        if y.is_finite() { y.clamp(0.0, 1.0) } else { p }
    }
}

/// Complete configuration for an ink or highlighter pen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PenSettings {
    pub kind: PenKind,
    pub color: Color,
    pub size: f32,
    pub opacity: f32,
    pub hardness: f32,
    pub pressure_enabled: bool,
    /// Stroke width at zero pressure as a fraction of `size`; lower values make pressure changes more visible.
    pub pressure_size_min: f32,
    /// Minimum ink color strength at light stylus pressure, clamped to keep strokes readable.
    /// Retains its serialized legacy name for saved-document compatibility.
    pub pressure_opacity_min: f32,
    pub velocity_pressure_simulation: bool,
    pub tilt_sensitivity: f32,
    pub nib_flatness: f32,
    pub nib_angle_deg: f32,
    pub start_taper_px: f32,
    pub end_taper_px: f32,
    pub smoothing: SmoothingConfig,
    pub pressure_curve: PressureCurve,
}

impl Default for PenSettings {
    fn default() -> Self {
        Self::fountain_pen()
    }
}

impl PenSettings {
    pub fn for_kind(kind: PenKind) -> Self {
        match kind {
            PenKind::FountainPen => Self::fountain_pen(),
            PenKind::BallPen => Self::ball_pen(),
            PenKind::BrushPen => Self::brush_pen(),
            PenKind::Highlighter => Self::highlighter(),
            PenKind::Pencil => Self::pencil(),
        }
    }

    pub fn fountain_pen() -> Self {
        Self {
            kind: PenKind::FountainPen,
            color: INK_PALETTE[1].color,
            size: 4.5,
            opacity: 1.0,
            hardness: 0.95,
            pressure_enabled: true,
            pressure_size_min: 0.18,
            pressure_opacity_min: 0.68,
            velocity_pressure_simulation: true,
            tilt_sensitivity: 0.25,
            nib_flatness: 0.68,
            nib_angle_deg: -35.0,
            start_taper_px: 0.0,
            end_taper_px: 0.0,
            smoothing: SmoothingConfig::default(),
            pressure_curve: PressureCurve::from_preset(PressureCurvePreset::Linear),
        }
    }

    pub fn ball_pen() -> Self {
        Self {
            kind: PenKind::BallPen,
            color: INK_PALETTE[2].color,
            size: 3.2,
            opacity: 1.0,
            hardness: 0.98,
            pressure_enabled: true,
            pressure_size_min: 0.55,
            pressure_opacity_min: 0.68,
            velocity_pressure_simulation: false,
            tilt_sensitivity: 0.0,
            nib_flatness: 0.0,
            nib_angle_deg: 0.0,
            start_taper_px: 0.0,
            end_taper_px: 0.0,
            smoothing: SmoothingConfig::default(),
            pressure_curve: PressureCurve::from_preset(PressureCurvePreset::Linear),
        }
    }

    pub fn brush_pen() -> Self {
        Self {
            kind: PenKind::BrushPen,
            color: INK_PALETTE[1].color,
            size: 12.0,
            opacity: 1.0,
            hardness: 0.88,
            pressure_enabled: true,
            pressure_size_min: 0.10,
            pressure_opacity_min: 0.68,
            velocity_pressure_simulation: true,
            tilt_sensitivity: 0.85,
            nib_flatness: 0.15,
            nib_angle_deg: -30.0,
            start_taper_px: 0.0,
            end_taper_px: 0.0,
            smoothing: SmoothingConfig::default(),
            pressure_curve: PressureCurve::from_preset(PressureCurvePreset::Linear),
        }
    }

    pub fn highlighter() -> Self {
        Self {
            kind: PenKind::Highlighter,
            color: HIGHLIGHTER_PALETTE[0].color,
            size: 24.0,
            opacity: 0.42,
            hardness: 0.94,
            pressure_enabled: false,
            pressure_size_min: 0.92,
            pressure_opacity_min: 1.0,
            velocity_pressure_simulation: false,
            tilt_sensitivity: 0.0,
            nib_flatness: 0.68,
            nib_angle_deg: -25.0,
            start_taper_px: 0.0,
            end_taper_px: 0.0,
            smoothing: SmoothingConfig::default(),
            pressure_curve: PressureCurve::from_preset(PressureCurvePreset::Constant),
        }
    }

    pub fn pencil() -> Self {
        Self {
            kind: PenKind::Pencil,
            color: INK_PALETTE[0].color,
            size: 2.8,
            opacity: 0.78,
            hardness: 0.72,
            pressure_enabled: true,
            pressure_size_min: 0.42,
            pressure_opacity_min: 0.35,
            velocity_pressure_simulation: true,
            tilt_sensitivity: 0.90,
            nib_flatness: 0.25,
            nib_angle_deg: -35.0,
            start_taper_px: 0.0,
            end_taper_px: 0.0,
            smoothing: SmoothingConfig::default(),
            pressure_curve: PressureCurve::from_preset(PressureCurvePreset::Linear),
        }
    }

    pub fn sanitized(&self) -> Self {
        let clamp = |v: f32, lo: f32, hi: f32, d: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { d }
        };
        let (op_min_floor, op_min_fallback) = if matches!(self.kind, PenKind::Pencil) {
            (0.15, 0.35)
        } else {
            (0.68, 0.68)
        };
        Self {
            kind: self.kind,
            color: self.color.sanitized(),
            size: clamp(self.size, 0.5, 120.0, 4.0),
            opacity: clamp(self.opacity, 0.05, 1.0, 1.0),
            hardness: clamp(self.hardness, 0.0, 1.0, 0.9),
            pressure_enabled: self.pressure_enabled,
            pressure_size_min: clamp(self.pressure_size_min, 0.0, 1.0, 0.25),
            pressure_opacity_min: clamp(
                self.pressure_opacity_min,
                op_min_floor,
                1.0,
                op_min_fallback,
            ),
            velocity_pressure_simulation: self.velocity_pressure_simulation,
            tilt_sensitivity: clamp(self.tilt_sensitivity, 0.0, 1.0, 0.25),
            nib_flatness: clamp(self.nib_flatness, 0.0, 0.9, 0.0),
            nib_angle_deg: clamp(self.nib_angle_deg, -180.0, 180.0, 0.0),
            start_taper_px: clamp(self.start_taper_px, 0.0, 100.0, 0.0),
            end_taper_px: clamp(self.end_taper_px, 0.0, 100.0, 0.0),
            smoothing: self.smoothing.sanitized(),
            pressure_curve: PressureCurve::new(self.pressure_curve.points.clone()),
        }
    }
}

fn is_zero_f32(v: &f32) -> bool {
    *v == 0.0
}

fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}

/// One captured input sample along a stroke.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StrokeSample {
    pub x: f64,
    pub y: f64,
    pub pressure: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub tilt_x: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub tilt_y: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub rotation: f32,
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub time_ms: f64,
}

impl StrokeSample {
    #[inline]
    pub fn from_xy_pressure(x: f64, y: f64, pressure: f32) -> Self {
        Self {
            x,
            y,
            pressure,
            tilt_x: 0.0,
            tilt_y: 0.0,
            rotation: 0.0,
            time_ms: 0.0,
        }
        .sanitized()
    }

    #[inline]
    pub fn point(self) -> Point {
        Point::new(self.x, self.y)
    }

    #[inline]
    pub fn sanitized(self) -> Self {
        let f32_clean = |v: f32, lo: f32, hi: f32, d: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { d }
        };
        let rot = if self.rotation.is_finite() {
            let r = self.rotation.rem_euclid(360.0);
            if r >= 360.0 { 0.0 } else { r }
        } else {
            0.0
        };
        Self {
            x: if self.x.is_finite() {
                self.x.clamp(-1e6, 1e6)
            } else {
                0.0
            },
            y: if self.y.is_finite() {
                self.y.clamp(-1e6, 1e6)
            } else {
                0.0
            },
            pressure: f32_clean(self.pressure, 0.0, 1.0, 1.0),
            tilt_x: f32_clean(self.tilt_x, -90.0, 90.0, 0.0),
            tilt_y: f32_clean(self.tilt_y, -90.0, 90.0, 0.0),
            rotation: rot,
            time_ms: if self.time_ms.is_finite() && self.time_ms >= 0.0 {
                self.time_ms
            } else {
                0.0
            },
        }
    }

    /// Quantizes this sample onto a fixed lossless-visual grid for compact storage & canonical hashing.
    pub fn quantize(self) -> QuantizedSample {
        let s = self.sanitized();
        let rot_cdeg = ((f64::from(s.rotation) * 100.0).round() as i32).rem_euclid(36_000) as u16;
        QuantizedSample {
            x_centipx: (s.x * 100.0).round() as i64,
            y_centipx: (s.y * 100.0).round() as i64,
            pressure_q16: (f64::from(s.pressure) * 65535.0)
                .round()
                .clamp(0.0, 65535.0) as u16,
            tilt_x_cdeg: (f64::from(s.tilt_x) * 100.0).round().clamp(-9000.0, 9000.0) as i16,
            tilt_y_cdeg: (f64::from(s.tilt_y) * 100.0).round().clamp(-9000.0, 9000.0) as i16,
            rot_cdeg,
            time_ms: s.time_ms.clamp(0.0, 1e12).round() as u64,
        }
    }

    /// Reconstructs a `StrokeSample` from its quantized storage representation.
    pub fn from_quantized(q: QuantizedSample) -> Self {
        Self {
            x: (q.x_centipx as f64) / 100.0,
            y: (q.y_centipx as f64) / 100.0,
            pressure: f32::from(q.pressure_q16) / 65535.0,
            tilt_x: f32::from(q.tilt_x_cdeg) / 100.0,
            tilt_y: f32::from(q.tilt_y_cdeg) / 100.0,
            rotation: f32::from(q.rot_cdeg) / 100.0,
            time_ms: q.time_ms as f64,
        }
        .sanitized()
    }
}

/// Fixed-point sample representation shared by binary `.ncraft` v2 encoding and canonical sync hashing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuantizedSample {
    pub x_centipx: i64,
    pub y_centipx: i64,
    pub pressure_q16: u16,
    pub tilt_x_cdeg: i16,
    pub tilt_y_cdeg: i16,
    pub rot_cdeg: u16,
    pub time_ms: u64,
}

/// Computes an inflated bounding rectangle covering all samples and max pen radius.
pub fn compute_stroke_bounds(points: &[StrokeSample], size: f32) -> Rect {
    let base = Rect::from_point_iter(points.iter().map(|s| s.point()));
    let pad = f64::from(if size.is_finite() { size.max(1.0) } else { 4.0 }) * 0.75 + 2.0;
    base.inflated(pad)
}

#[derive(Deserialize)]
struct InkStrokeWire {
    id: StrokeId,
    pen: PenSettings,
    #[serde(default)]
    raw_points: Arc<Vec<StrokeSample>>,
    #[serde(default)]
    smoothed_points: Arc<Vec<StrokeSample>>,
    #[serde(default)]
    bounds: Option<Rect>,
}

/// One committed ink stroke in a DrawCraft page.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InkStroke {
    pub id: StrokeId,
    pub pen: PenSettings,
    #[serde(skip_serializing)]
    pub raw_points: Arc<Vec<StrokeSample>>,
    pub smoothed_points: Arc<Vec<StrokeSample>>,
    #[serde(skip_serializing)]
    pub bounds: Rect,
}

impl<'de> Deserialize<'de> for InkStroke {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = InkStrokeWire::deserialize(deserializer)?;
        let pen = wire.pen.sanitized();
        let bounds = wire
            .bounds
            .unwrap_or_else(|| compute_stroke_bounds(&wire.smoothed_points, pen.size));
        Ok(Self {
            id: wire.id,
            pen,
            raw_points: wire.raw_points,
            smoothed_points: wire.smoothed_points,
            bounds,
        })
    }
}

impl InkStroke {
    pub fn new(
        id: StrokeId,
        pen: PenSettings,
        raw_points: Vec<StrokeSample>,
        smoothed_points: Vec<StrokeSample>,
    ) -> Self {
        let pen = pen.sanitized();
        let bounds = compute_stroke_bounds(&smoothed_points, pen.size);
        Self {
            id,
            pen,
            raw_points: Arc::new(raw_points),
            smoothed_points: Arc::new(smoothed_points),
            bounds,
        }
    }

    /// Returns a copy with sanitized pen settings and finite point coordinates.
    pub fn sanitized(&self) -> Self {
        let clean_pts = |pts: &[StrokeSample]| -> Vec<StrokeSample> {
            pts.iter()
                .map(|s| StrokeSample {
                    x: if s.x.is_finite() {
                        s.x.clamp(-1e6, 1e6)
                    } else {
                        0.0
                    },
                    y: if s.y.is_finite() {
                        s.y.clamp(-1e6, 1e6)
                    } else {
                        0.0
                    },
                    pressure: if s.pressure.is_finite() {
                        s.pressure.clamp(0.0, 1.0)
                    } else {
                        0.5
                    },
                    tilt_x: if s.tilt_x.is_finite() {
                        s.tilt_x.clamp(-90.0, 90.0)
                    } else {
                        0.0
                    },
                    tilt_y: if s.tilt_y.is_finite() {
                        s.tilt_y.clamp(-90.0, 90.0)
                    } else {
                        0.0
                    },
                    rotation: if s.rotation.is_finite() {
                        s.rotation.rem_euclid(360.0)
                    } else {
                        0.0
                    },
                    time_ms: s.time_ms,
                })
                .collect()
        };
        Self::new(
            self.id,
            self.pen.sanitized(),
            clean_pts(self.raw_points.as_slice()),
            clean_pts(self.smoothed_points.as_slice()),
        )
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

/// Maximum sanitized title length for a notebook.
pub const MAX_NOTEBOOK_TITLE_LEN: usize = 120;

/// Maximum length for a hierarchical library folder path (e.g. `"School/Math"`).
pub const MAX_FOLDER_PATH_CHARS: usize = 160;

/// Maximum nested folder depth in library paths.
pub const MAX_FOLDER_DEPTH: usize = 8;

/// Maximum number of explicitly tracked library folders in studio configuration.
pub const MAX_LIBRARY_FOLDERS: usize = 128;

/// Maximum number of tags attached to a single notebook.
pub const MAX_TAGS_PER_NOTEBOOK: usize = 16;

/// Maximum characters allowed per notebook tag.
pub const MAX_TAG_CHARS: usize = 32;

/// Maximum number of custom tag color entries tracked in studio configuration.
pub const MAX_LIBRARY_TAG_COLORS: usize = 256;

/// Maximum saved user custom paper templates.
pub const MAX_CUSTOM_TEMPLATES: usize = 24;

/// Maximum characters for a custom paper template display name.
pub const MAX_TEMPLATE_NAME_CHARS: usize = 48;

/// Maximum characters for a custom paper template identifier.
pub const MAX_TEMPLATE_ID_LEN: usize = 64;

/// Maximum PNG payload size for a reusable custom image template (3 MiB).
pub const MAX_CUSTOM_TEMPLATE_IMAGE_BYTES: usize = 3 * 1024 * 1024;

/// Maximum PDF payload size for a reusable custom PDF template (4 MiB).
pub const MAX_CUSTOM_TEMPLATE_PDF_BYTES: usize = 4 * 1024 * 1024;

/// Maximum embedded image templates stored on a single notebook document.
pub const MAX_DOCUMENT_IMAGE_TEMPLATES: usize = 24;

/// Maximum combined bytes for embedded custom image templates on a notebook (24 MiB).
pub const MAX_DOCUMENT_IMAGE_TEMPLATE_BYTES: usize = 24 * 1024 * 1024;

mod arc_custom_template_bytes {
    use super::{MAX_CUSTOM_TEMPLATE_PDF_BYTES, deserialize_arc_base64, serialize_arc_base64};
    use serde::{Deserializer, Serializer};
    use std::sync::Arc;

    pub fn serialize<S>(bytes: &Arc<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_arc_base64(bytes, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Arc<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_arc_base64(
            deserializer,
            MAX_CUSTOM_TEMPLATE_PDF_BYTES,
            "custom template payload exceeds maximum limit",
        )
    }
}

fn default_notebook_title() -> String {
    "Quick Notes".to_string()
}

/// Sanitizes a notebook title by stripping control characters and bounding length.
pub fn sanitize_notebook_title(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_NOTEBOOK_TITLE_LEN)
        .collect();
    if cleaned.trim().is_empty() {
        default_notebook_title()
    } else {
        cleaned.trim().to_string()
    }
}

/// Normalizes a nested library folder path (forward-slash delimited, no traversal segments).
#[must_use]
pub fn sanitize_folder_path(raw: &str) -> String {
    let normalized = raw.replace('\\', "/");
    let mut segments = Vec::new();
    for part in normalized.split('/') {
        let cleaned: String = part
            .trim()
            .chars()
            .filter(|ch| !ch.is_control())
            .take(48)
            .collect();
        let trimmed = cleaned.trim();
        if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
            continue;
        }
        segments.push(trimmed.to_string());
        if segments.len() >= MAX_FOLDER_DEPTH {
            break;
        }
    }
    let joined = segments.join("/");
    if joined.chars().count() > MAX_FOLDER_PATH_CHARS {
        let mut out = String::new();
        for seg in segments {
            let needed = seg.chars().count() + usize::from(!out.is_empty());
            if out.chars().count() + needed > MAX_FOLDER_PATH_CHARS {
                break;
            }
            if !out.is_empty() {
                out.push('/');
            }
            out.push_str(&seg);
        }
        out
    } else {
        joined
    }
}

/// Normalizes and case-insensitively deduplicates a list of explicit folder paths,
/// automatically ensuring parent folders exist for nested subfolders.
#[must_use]
pub fn sanitize_library_folders(raw: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for entry in raw {
        let clean = sanitize_folder_path(entry);
        if clean.is_empty() {
            continue;
        }
        let mut prefix = String::new();
        for seg in clean.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(seg);
            if out.len() >= MAX_LIBRARY_FOLDERS {
                break;
            }
            let key = prefix.to_ascii_lowercase();
            if seen.insert(key) {
                out.push(prefix.clone());
            }
        }
        if out.len() >= MAX_LIBRARY_FOLDERS {
            break;
        }
    }
    out
}

/// Curated quick-pick colors for library folders (`(label, #RRGGBB)`).
pub const FOLDER_COLOR_PRESETS: &[(&str, &str)] = &[
    ("Blue", "#2563EB"),
    ("Teal", "#0D9488"),
    ("Green", "#16A34A"),
    ("Amber", "#D97706"),
    ("Orange", "#EA580C"),
    ("Red", "#DC2626"),
    ("Rose", "#DB2777"),
    ("Purple", "#7C3AED"),
];

/// Returns `Some("")` when `candidate` matches `prefix` case-insensitively,
/// `Some(relative_suffix)` when `candidate` is nested inside `prefix`, or `None` otherwise.
#[must_use]
pub fn folder_path_suffix(candidate: &str, prefix: &str) -> Option<String> {
    if candidate.is_empty() || prefix.is_empty() {
        return None;
    }
    if candidate.eq_ignore_ascii_case(prefix) {
        return Some(String::new());
    }
    let candidate_chars: Vec<char> = candidate.chars().collect();
    let prefix_chars: Vec<char> = prefix.chars().collect();
    if candidate_chars.len() <= prefix_chars.len() {
        return None;
    }
    let matches_head = candidate_chars[..prefix_chars.len()]
        .iter()
        .zip(&prefix_chars)
        .all(|(a, b)| a.eq_ignore_ascii_case(b));
    if matches_head && candidate_chars[prefix_chars.len()] == '/' {
        let rest: String = candidate_chars[prefix_chars.len() + 1..].iter().collect();
        return Some(rest);
    }
    None
}

fn sanitize_keyed_colors<F>(
    raw: &BTreeMap<String, Color>,
    max_entries: usize,
    mut normalize_key: F,
) -> BTreeMap<String, Color>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut out = BTreeMap::new();
    for (raw_key, raw_color) in raw {
        let Some(key) = normalize_key(raw_key) else {
            continue;
        };
        if !out.contains_key(&key) && out.len() >= max_entries {
            break;
        }
        out.insert(key, raw_color.sanitized().with_alpha(1.0));
    }
    out
}

/// Normalizes folder color mappings to lowercase sanitized folder paths and finite opaque colors.
#[must_use]
pub fn sanitize_library_folder_colors(raw: &BTreeMap<String, Color>) -> BTreeMap<String, Color> {
    sanitize_keyed_colors(raw, MAX_LIBRARY_FOLDERS, |raw_path| {
        let clean = sanitize_folder_path(raw_path).to_ascii_lowercase();
        (!clean.is_empty()).then_some(clean)
    })
}

/// Normalizes folder icon mappings to lowercase sanitized folder paths and clean ASCII icon keys.
#[must_use]
pub fn sanitize_library_folder_icons(raw: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (raw_path, raw_icon) in raw {
        let clean_path = sanitize_folder_path(raw_path).to_ascii_lowercase();
        let clean_icon: String = raw_icon
            .trim()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(32)
            .collect::<String>()
            .to_ascii_lowercase();
        if clean_path.is_empty() || clean_icon.is_empty() {
            continue;
        }
        if !out.contains_key(&clean_path) && out.len() >= MAX_LIBRARY_FOLDERS {
            break;
        }
        out.insert(clean_path, clean_icon);
    }
    out
}

/// Normalizes tag color mappings to lowercase sanitized tag names and finite opaque colors.
#[must_use]
pub fn sanitize_library_tag_colors(raw: &BTreeMap<String, Color>) -> BTreeMap<String, Color> {
    sanitize_keyed_colors(raw, MAX_LIBRARY_TAG_COLORS, |raw_tag| {
        sanitize_notebook_tag(raw_tag).map(|clean| clean.to_ascii_lowercase())
    })
}

/// Sanitizes a single notebook label tag. Returns `None` if empty after trimming.
#[must_use]
pub fn sanitize_notebook_tag(raw: &str) -> Option<String> {
    let stripped = raw.trim().trim_start_matches('#');
    let mut out = String::new();
    let mut prev_space = false;
    for ch in stripped.chars() {
        if ch.is_control() {
            continue;
        }
        if ch.is_whitespace() {
            if !out.is_empty() && !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
        if out.chars().count() >= MAX_TAG_CHARS {
            break;
        }
    }
    let trimmed = out.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Sanitizes and deduplicates a list of notebook tags.
#[must_use]
pub fn sanitize_notebook_tags(raw: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(raw.len().min(MAX_TAGS_PER_NOTEBOOK));
    for item in raw {
        if let Some(clean) = sanitize_notebook_tag(item)
            && seen.insert(clean.to_ascii_lowercase())
        {
            out.push(clean);
            if out.len() >= MAX_TAGS_PER_NOTEBOOK {
                break;
            }
        }
    }
    out
}

/// Library notebook presentation layout mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryViewMode {
    #[default]
    Grid,
    List,
}

impl LibraryViewMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Grid => "Grid",
            Self::List => "List",
        }
    }
}

/// Library sorting criterion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibrarySortBy {
    #[default]
    ModifiedDate,
    CreatedDate,
    Name,
}

impl LibrarySortBy {
    pub const ALL: &[Self] = &[Self::ModifiedDate, Self::CreatedDate, Self::Name];

    pub fn label(self) -> &'static str {
        match self {
            Self::ModifiedDate => "Modified Date",
            Self::CreatedDate => "Created Date",
            Self::Name => "Name",
        }
    }
}

/// Underlying asset format of a user-saved custom paper template.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomTemplateKind {
    #[default]
    Image,
    Pdf,
}

/// Deduplicated PNG custom template payload embedded inside a notebook document.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddedImageTemplate {
    pub name: String,
    #[serde(with = "arc_base64_bytes")]
    pub png_bytes: Arc<Vec<u8>>,
}

impl EmbeddedImageTemplate {
    /// Clamps display name and drops empty or oversized payloads.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        let valid =
            !self.png_bytes.is_empty() && self.png_bytes.len() <= MAX_CUSTOM_TEMPLATE_IMAGE_BYTES;
        Self {
            name: sanitize_trimmed_text(&self.name, MAX_TEMPLATE_NAME_CHARS, "Custom Template"),
            png_bytes: if valid {
                Arc::clone(&self.png_bytes)
            } else {
                Arc::new(Vec::new())
            },
        }
    }
}

/// User-saved reusable paper template backed by an image or a single PDF page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomPaperTemplate {
    pub id: String,
    pub name: String,
    pub kind: CustomTemplateKind,
    pub width: u32,
    pub height: u32,
    pub pdf_page_index: usize,
    #[serde(with = "arc_custom_template_bytes")]
    pub bytes: Arc<Vec<u8>>,
}

impl CustomPaperTemplate {
    /// Validates payload size, dimensions, and identifier bounds; returns `None` when empty/corrupt.
    #[must_use]
    pub fn sanitized(&self) -> Option<Self> {
        let valid_payload = match self.kind {
            CustomTemplateKind::Image => {
                !self.bytes.is_empty() && self.bytes.len() <= MAX_CUSTOM_TEMPLATE_IMAGE_BYTES
            }
            CustomTemplateKind::Pdf => {
                !self.bytes.is_empty()
                    && self.bytes.len() <= MAX_CUSTOM_TEMPLATE_PDF_BYTES
                    && looks_like_pdf_bytes(&self.bytes)
            }
        };
        if !valid_payload {
            return None;
        }
        Some(Self {
            id: sanitize_template_id(&self.id),
            name: sanitize_trimmed_text(&self.name, MAX_TEMPLATE_NAME_CHARS, "Custom Template"),
            kind: self.kind,
            width: self.width.clamp(64, 8192),
            height: self.height.clamp(64, 8192),
            pdf_page_index: if self.kind == CustomTemplateKind::Pdf {
                self.pdf_page_index
            } else {
                0
            },
            bytes: Arc::clone(&self.bytes),
        })
    }
}

pub const MAX_NOTEBOOK_ID_LEN: usize = 64;

/// Sanitizes a stable client-generated notebook identifier (`UUID` or ASCII slug).
#[must_use]
pub fn sanitize_notebook_id(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .take(MAX_NOTEBOOK_ID_LEN)
        .collect()
}

/// Sanitizes a custom paper template ID to safe identifier characters.
#[must_use]
pub fn sanitize_template_id(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .take(MAX_TEMPLATE_ID_LEN)
        .collect();
    cleaned
}

/// Notebook cover theme presets shown in the library and cover switcher, plus custom RGB covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NotebookCover {
    #[default]
    ClassicNavy,
    SageGreen,
    Terracotta,
    SlateGraphite,
    WarmSand,
    PlumStudio,
    Custom {
        r: u8,
        g: u8,
        b: u8,
    },
}

impl NotebookCover {
    pub const ALL: &[Self] = &[
        Self::ClassicNavy,
        Self::SageGreen,
        Self::Terracotta,
        Self::SlateGraphite,
        Self::WarmSand,
        Self::PlumStudio,
    ];

    /// Builds a custom cover from 8-bit sRGB channels.
    #[inline]
    pub const fn custom_rgb(r: u8, g: u8, b: u8) -> Self {
        Self::Custom { r, g, b }
    }

    /// Builds a custom cover from an opaque [`Color`].
    #[inline]
    pub fn from_color(color: Color) -> Self {
        let [r, g, b, _] = color.to_rgba8();
        Self::Custom { r, g, b }
    }

    /// Parses `#RRGGBB` or `RRGGBB` hex notation into a custom cover.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let trimmed = hex.trim();
        let raw = trimmed.strip_prefix('#').unwrap_or(trimmed);
        if raw.len() != 6 {
            return None;
        }
        Color::from_hex(raw).map(Self::from_color)
    }

    #[inline]
    pub const fn is_custom(self) -> bool {
        matches!(self, Self::Custom { .. })
    }

    #[inline]
    pub const fn custom_rgb_channels(self) -> Option<[u8; 3]> {
        match self {
            Self::Custom { r, g, b } => Some([r, g, b]),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ClassicNavy => "Classic Navy",
            Self::SageGreen => "Sage Green",
            Self::Terracotta => "Terracotta",
            Self::SlateGraphite => "Slate Graphite",
            Self::WarmSand => "Warm Sand",
            Self::PlumStudio => "Plum Studio",
            Self::Custom { .. } => "Custom",
        }
    }

    /// Human-readable label including the `#RRGGBB` hex code for custom covers.
    pub fn display_label(self) -> String {
        match self {
            Self::Custom { r, g, b } => format!("Custom (#{r:02X}{g:02X}{b:02X})"),
            _ => self.label().to_string(),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::ClassicNavy => {
                "Deep indigo cloth binding for everyday lecture and meeting notes."
            }
            Self::SageGreen => "Calm botanical sage cover for study journals and reading notes.",
            Self::Terracotta => "Warm clay cover designed for creative sketching and idea logs.",
            Self::SlateGraphite => {
                "Low-glare charcoal binding suited for technical diagrams and dark sheets."
            }
            Self::WarmSand => "Soft linen-sand cover for minimal planners and clean handouts.",
            Self::PlumStudio => {
                "Rich studio plum binding for reference binders and project notebooks."
            }
            Self::Custom { .. } => "Custom hardcover color chosen specifically for this notebook.",
        }
    }

    pub fn primary_color(self) -> Color {
        match self {
            Self::ClassicNavy => Color::from_rgba8(0x23, 0x3D, 0x63, 0xFF),
            Self::SageGreen => Color::from_rgba8(0x3D, 0x6B, 0x58, 0xFF),
            Self::Terracotta => Color::from_rgba8(0xB8, 0x56, 0x3D, 0xFF),
            Self::SlateGraphite => Color::from_rgba8(0x33, 0x38, 0x42, 0xFF),
            Self::WarmSand => Color::from_rgba8(0xC9, 0xB4, 0x91, 0xFF),
            Self::PlumStudio => Color::from_rgba8(0x63, 0x37, 0x5E, 0xFF),
            Self::Custom { r, g, b } => Color::from_rgba8(r, g, b, 0xFF),
        }
    }

    pub fn spine_color(self) -> Color {
        match self {
            Self::ClassicNavy => Color::from_rgba8(0x16, 0x28, 0x44, 0xFF),
            Self::SageGreen => Color::from_rgba8(0x29, 0x4B, 0x3C, 0xFF),
            Self::Terracotta => Color::from_rgba8(0x8E, 0x3D, 0x29, 0xFF),
            Self::SlateGraphite => Color::from_rgba8(0x20, 0x24, 0x2C, 0xFF),
            Self::WarmSand => Color::from_rgba8(0xA3, 0x8C, 0x67, 0xFF),
            Self::PlumStudio => Color::from_rgba8(0x45, 0x23, 0x41, 0xFF),
            Self::Custom { r, g, b } => derive_custom_spine_color(r, g, b),
        }
    }
}

fn derive_custom_spine_color(r: u8, g: u8, b: u8) -> Color {
    let [h, s, v] = Color::from_rgba8(r, g, b, 0xFF).to_hsv();
    if v < 0.16 {
        // Lift very dark covers slightly so the binding spine remains visually distinct.
        Color::from_hsv(
            h,
            (s * 0.85).clamp(0.0, 1.0),
            (v + 0.10).clamp(0.08, 0.28),
            1.0,
        )
    } else {
        Color::from_hsv(
            h,
            (s * 1.12).clamp(0.0, 1.0),
            (v * 0.68).clamp(0.04, 0.86),
            1.0,
        )
    }
}

/// Curated 1-click notebook presets pairing cover art, paper template, and sheet tint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NotebookPreset {
    #[default]
    EverydayLined,
    LectureCornell,
    EngineeringGrid,
    BulletDotted,
    SketchBlank,
    MidnightSlate,
}

impl NotebookPreset {
    pub const ALL: &[Self] = &[
        Self::EverydayLined,
        Self::LectureCornell,
        Self::EngineeringGrid,
        Self::BulletDotted,
        Self::SketchBlank,
        Self::MidnightSlate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::EverydayLined => "Everyday Lined",
            Self::LectureCornell => "Lecture Cornell",
            Self::EngineeringGrid => "Engineering Grid",
            Self::BulletDotted => "Bullet Dotted",
            Self::SketchBlank => "Sketchbook Blank",
            Self::MidnightSlate => "Midnight Dark Notes",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::EverydayLined => {
                "Narrow-ruled warm cream pages with a classic navy cover for daily writing."
            }
            Self::LectureCornell => {
                "Structured cue column and summary sheets ideal for classes and reading notes."
            }
            Self::EngineeringGrid => {
                "Crisp pure white engineering grid paired with a graphite binder."
            }
            Self::BulletDotted => {
                "Subtle dot-matrix cream sheets in a sage green cover for structured plans."
            }
            Self::SketchBlank => {
                "Unruled open canvas in a warm terracotta cover for diagrams and freehand work."
            }
            Self::MidnightSlate => {
                "Dark slate grid sheets designed for low-glare late-night problem solving."
            }
        }
    }

    pub fn default_title(self) -> &'static str {
        match self {
            Self::EverydayLined => "Daily Notebook",
            Self::LectureCornell => "Lecture Notes",
            Self::EngineeringGrid => "Engineering Pad",
            Self::BulletDotted => "Planner & Ideas",
            Self::SketchBlank => "Sketchbook",
            Self::MidnightSlate => "Night Study Pad",
        }
    }

    pub fn cover(self) -> NotebookCover {
        match self {
            Self::EverydayLined => NotebookCover::ClassicNavy,
            Self::LectureCornell => NotebookCover::PlumStudio,
            Self::EngineeringGrid => NotebookCover::SlateGraphite,
            Self::BulletDotted => NotebookCover::SageGreen,
            Self::SketchBlank => NotebookCover::Terracotta,
            Self::MidnightSlate => NotebookCover::SlateGraphite,
        }
    }

    pub fn template(self) -> PaperTemplate {
        match self {
            Self::EverydayLined => PaperTemplate::RuledNarrow,
            Self::LectureCornell => PaperTemplate::Cornell,
            Self::EngineeringGrid => PaperTemplate::Grid,
            Self::BulletDotted => PaperTemplate::Dotted,
            Self::SketchBlank => PaperTemplate::Blank,
            Self::MidnightSlate => PaperTemplate::Grid,
        }
    }

    pub fn paper_color(self) -> PaperColor {
        match self {
            Self::EverydayLined
            | Self::LectureCornell
            | Self::BulletDotted
            | Self::EngineeringGrid
            | Self::SketchBlank => PaperColor::PureWhite,
            Self::MidnightSlate => PaperColor::DarkSlate,
        }
    }

    /// Creates a new notebook configured from this preset.
    pub fn create_document(self) -> Document {
        let mut doc = Document::new(1200, 1600);
        doc.title = self.default_title().to_string();
        doc.cover = self.cover();
        doc.paper_template = self.template();
        doc.paper_color = self.paper_color();
        doc.sync_active_to_pages();
        doc
    }
}

/// Orientation of a page sheet (Portrait or Landscape).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PageOrientation {
    #[default]
    Portrait,
    Landscape,
}

impl PageOrientation {
    pub const ALL: &[Self] = &[Self::Portrait, Self::Landscape];

    /// Display label used in pickers and accessibility strings.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Portrait => "Portrait",
            Self::Landscape => "Landscape",
        }
    }

    /// Plain-English explanation of what this orientation sets.
    #[must_use]
    pub const fn short_hint(self) -> &'static str {
        match self {
            Self::Portrait => "Vertical sheet (taller than wide)",
            Self::Landscape => "Horizontal sheet (wider than tall)",
        }
    }

    /// Returns the opposite orientation.
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Portrait => Self::Landscape,
            Self::Landscape => Self::Portrait,
        }
    }

    /// Infers orientation from `(width, height)` (`Portrait` when `width <= height`).
    #[must_use]
    pub const fn from_dimensions(width: u32, height: u32) -> Self {
        if width > height {
            Self::Landscape
        } else {
            Self::Portrait
        }
    }

    /// Rearranges `(width, height)` into `(w, h)` matching this orientation.
    #[must_use]
    pub const fn apply(self, width: u32, height: u32) -> (u32, u32) {
        let short = if width <= height { width } else { height };
        let long = if width >= height { width } else { height };
        match self {
            Self::Portrait => (short, long),
            Self::Landscape => (long, short),
        }
    }
}

/// Standard sheet resolution presets paired with user-defined custom dimensions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PageResolutionPreset {
    #[default]
    Standard,
    A4,
    UsLetter,
    A5Compact,
    Widescreen,
    Square,
    Custom,
}

impl PageResolutionPreset {
    pub const ALL: &[Self] = &[
        Self::Standard,
        Self::A4,
        Self::UsLetter,
        Self::A5Compact,
        Self::Widescreen,
        Self::Square,
        Self::Custom,
    ];

    /// Short title for visual preset cards without embedded pixel numbers.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::A4 => "A4",
            Self::UsLetter => "US Letter",
            Self::A5Compact => "A5 Compact",
            Self::Widescreen => "16:9 Wide",
            Self::Square => "Square",
            Self::Custom => "Custom Size",
        }
    }

    /// Compact aspect/standard badge shown on visual format cards.
    #[must_use]
    pub const fn ratio_badge(self) -> &'static str {
        match self {
            Self::Standard => "3:4",
            Self::A4 => "ISO 216",
            Self::UsLetter => "ANSI",
            Self::A5Compact => "Pocket",
            Self::Widescreen => "16:9",
            Self::Square => "1:1",
            Self::Custom => "W × H",
        }
    }

    /// Short display name for dropdowns and summary pills.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Standard => "Standard (3:4)",
            Self::A4 => "A4",
            Self::UsLetter => "US Letter",
            Self::A5Compact => "A5 Compact",
            Self::Widescreen => "Widescreen (16:9)",
            Self::Square => "Square (1:1)",
            Self::Custom => "Custom",
        }
    }

    /// Full label including pixel/point dimensions for presets.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard (1200 × 1600)",
            Self::A4 => "A4 (1190 × 1684)",
            Self::UsLetter => "US Letter (1224 × 1584)",
            Self::A5Compact => "A5 Compact (840 × 1190)",
            Self::Widescreen => "Widescreen 16:9 (1920 × 1080)",
            Self::Square => "Square (1400 × 1400)",
            Self::Custom => "Custom Size",
        }
    }

    /// Plain-English explanation of this page resolution preset.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Standard => {
                "Default 3:4 studio sheet sized for everyday note-taking and crisp exports."
            }
            Self::A4 => "International ISO A4 proportions scaled for natural handwriting.",
            Self::UsLetter => {
                "North American US Letter proportions scaled for printing and handouts."
            }
            Self::A5Compact => "Compact pocket notebook dimensions for focused lists and memos.",
            Self::Widescreen => "Wide 16:9 HD canvas suited for slides, diagrams, and boards.",
            Self::Square => "Balanced 1:1 square sheet for sketches, charts, and mind maps.",
            Self::Custom => "Enter any custom page width and height between 64 and 8192 pt.",
        }
    }

    /// Default `(width, height)` dimensions in page points for this preset.
    #[must_use]
    pub const fn dimensions(self) -> (u32, u32) {
        match self {
            Self::Standard | Self::Custom => (1200, 1600),
            Self::A4 => (1190, 1684),
            Self::UsLetter => (1224, 1584),
            Self::A5Compact => (840, 1190),
            Self::Widescreen => (1920, 1080),
            Self::Square => (1400, 1400),
        }
    }

    /// Default orientation for this preset (`Landscape` for 16:9 Widescreen, `Portrait` otherwise).
    #[must_use]
    pub const fn default_orientation(self) -> PageOrientation {
        match self {
            Self::Widescreen => PageOrientation::Landscape,
            Self::Standard
            | Self::A4
            | Self::UsLetter
            | Self::A5Compact
            | Self::Square
            | Self::Custom => PageOrientation::Portrait,
        }
    }

    /// Whether switching between Portrait and Landscape changes this preset's dimensions.
    #[must_use]
    pub const fn supports_orientation(self) -> bool {
        !matches!(self, Self::Square)
    }

    /// Returns `(width, height)` for this preset in the requested `orientation`.
    #[must_use]
    pub const fn dimensions_with_orientation(self, orientation: PageOrientation) -> (u32, u32) {
        let (w, h) = self.dimensions();
        orientation.apply(w, h)
    }

    /// Detects the matching built-in preset (in either orientation) and active [`PageOrientation`].
    #[must_use]
    pub fn detect(width: u32, height: u32) -> (Self, PageOrientation) {
        let orientation = PageOrientation::from_dimensions(width, height);
        for &preset in Self::ALL {
            if preset == Self::Custom {
                continue;
            }
            let (pw, ph) = preset.dimensions();
            if (pw == width && ph == height) || (ph == width && pw == height) {
                return (preset, orientation);
            }
        }
        (Self::Custom, orientation)
    }

    /// Matches `(width, height)` dimensions (Portrait or Landscape) to a built-in preset, or [`Self::Custom`].
    #[must_use]
    pub fn from_dimensions(width: u32, height: u32) -> Self {
        Self::detect(width, height).0
    }
}

/// Individual page stored inside a multi-page notebook document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NotebookPage {
    pub width: u32,
    pub height: u32,
    pub paper_template: PaperTemplate,
    pub paper_color: PaperColor,
    pub strokes: Vec<Arc<InkStroke>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<Arc<TextItem>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapes: Vec<Arc<GeometricShape>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Arc<PageImage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pdf_backdrop: Option<PdfPageBackdrop>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bookmarked: bool,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub backdrop_rotation: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_image_backdrop: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_template_id: Option<String>,
    /// Reveal tapes covering parts of this page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub study_tapes: Vec<StudyTapeItem>,
}

impl NotebookPage {
    pub fn new(
        width: u32,
        height: u32,
        paper_template: PaperTemplate,
        paper_color: PaperColor,
    ) -> Self {
        Self {
            width: width.clamp(64, 8192),
            height: height.clamp(64, 8192),
            paper_template,
            paper_color,
            strokes: Vec::new(),
            texts: Vec::new(),
            shapes: Vec::new(),
            images: Vec::new(),
            pdf_backdrop: None,
            bookmarked: false,
            backdrop_rotation: 0,
            custom_image_backdrop: None,
            custom_template_id: None,
            study_tapes: Vec::new(),
        }
    }
}

/// Pure DrawCraft multi-page notebook document.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    /// Client-generated stable identifier (UUID) preserved across `.ncraft` saves and sync.
    pub id: String,
    pub title: String,
    pub cover: NotebookCover,
    pub folder_path: String,
    pub favorite: bool,
    pub tags: Vec<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// UTC timestamp of the latest metadata-only change (`title`, `cover`, `folder_path`, `favorite`, `tags`).
    pub meta_updated_at_ms: i64,
    pub width: u32,
    pub height: u32,
    pub paper_template: PaperTemplate,
    pub paper_color: PaperColor,
    pub strokes: Vec<Arc<InkStroke>>,
    pub texts: Vec<Arc<TextItem>>,
    pub shapes: Vec<Arc<GeometricShape>>,
    pub images: Vec<Arc<PageImage>>,
    pub pdf_backdrop: Option<PdfPageBackdrop>,
    pub pdf_documents: Vec<EmbeddedPdf>,
    pub bookmarked: bool,
    pub backdrop_rotation: u8,
    pub custom_image_backdrop: Option<usize>,
    pub custom_template_id: Option<String>,
    pub image_templates: Vec<EmbeddedImageTemplate>,
    /// Reveal tapes on the active page (mirrors `pages[current_page].study_tapes`).
    pub study_tapes: Vec<StudyTapeItem>,
    /// Notebook-wide spaced-repetition deck.
    pub flashcards: Vec<Flashcard>,
    pub next_stroke_id: u64,
    pub pages: Vec<NotebookPage>,
    pub current_page: usize,
}

#[derive(Serialize)]
struct DocumentWireSer<'a> {
    #[serde(skip_serializing_if = "str::is_empty")]
    id: &'a str,
    title: &'a str,
    cover: NotebookCover,
    #[serde(skip_serializing_if = "String::is_empty")]
    folder_path: String,
    #[serde(skip_serializing_if = "is_false")]
    favorite: bool,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    tags: &'a [String],
    #[serde(skip_serializing_if = "is_zero_i64")]
    created_at_ms: i64,
    #[serde(skip_serializing_if = "is_zero_i64")]
    updated_at_ms: i64,
    #[serde(skip_serializing_if = "is_zero_i64")]
    meta_updated_at_ms: i64,
    width: u32,
    height: u32,
    paper_template: PaperTemplate,
    paper_color: PaperColor,
    #[serde(skip_serializing_if = "Option::is_none")]
    strokes: Option<&'a [Arc<InkStroke>]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    texts: Option<&'a [Arc<TextItem>]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shapes: Option<&'a [Arc<GeometricShape>]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    images: Option<&'a [Arc<PageImage>]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pdf_backdrop: Option<PdfPageBackdrop>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bookmarked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backdrop_rotation: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    custom_image_backdrop: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    custom_template_id: Option<&'a str>,
    #[serde(skip_serializing_if = "<[EmbeddedPdf]>::is_empty")]
    pdf_documents: &'a [EmbeddedPdf],
    #[serde(skip_serializing_if = "<[EmbeddedImageTemplate]>::is_empty")]
    image_templates: &'a [EmbeddedImageTemplate],
    #[serde(skip_serializing_if = "Option::is_none")]
    study_tapes: Option<&'a [StudyTapeItem]>,
    #[serde(skip_serializing_if = "<[Flashcard]>::is_empty")]
    flashcards: &'a [Flashcard],
    next_stroke_id: u64,
    pages: &'a [NotebookPage],
    current_page: usize,
}

#[derive(Deserialize)]
struct DocumentWireDe {
    #[serde(default)]
    id: String,
    #[serde(default = "default_notebook_title")]
    title: String,
    #[serde(default)]
    cover: NotebookCover,
    #[serde(default)]
    folder_path: String,
    #[serde(default)]
    favorite: bool,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    created_at_ms: i64,
    #[serde(default)]
    updated_at_ms: i64,
    #[serde(default)]
    meta_updated_at_ms: i64,
    width: u32,
    height: u32,
    paper_template: PaperTemplate,
    paper_color: PaperColor,
    #[serde(default)]
    strokes: Option<Vec<Arc<InkStroke>>>,
    #[serde(default)]
    texts: Option<Vec<Arc<TextItem>>>,
    #[serde(default)]
    shapes: Option<Vec<Arc<GeometricShape>>>,
    #[serde(default)]
    images: Option<Vec<Arc<PageImage>>>,
    #[serde(default)]
    pdf_backdrop: Option<PdfPageBackdrop>,
    #[serde(default)]
    bookmarked: Option<bool>,
    #[serde(default)]
    backdrop_rotation: Option<u8>,
    #[serde(default)]
    custom_image_backdrop: Option<usize>,
    #[serde(default)]
    custom_template_id: Option<String>,
    #[serde(default)]
    pdf_documents: Vec<EmbeddedPdf>,
    #[serde(default)]
    image_templates: Vec<EmbeddedImageTemplate>,
    #[serde(default)]
    study_tapes: Option<Vec<StudyTapeItem>>,
    #[serde(default)]
    flashcards: Vec<Flashcard>,
    next_stroke_id: u64,
    #[serde(default)]
    pages: Vec<NotebookPage>,
    #[serde(default)]
    current_page: usize,
}

fn sanitize_text_items(items: &mut Vec<Arc<TextItem>>) {
    items.truncate(MAX_PAGE_TEXT_ITEMS);
    let mut remaining_chars = MAX_PAGE_TEXT_CHARS;
    for item in items.iter_mut() {
        let mut clean = item.as_ref().sanitized();
        if clean.text.chars().count() > remaining_chars {
            clean.text = clean.text.chars().take(remaining_chars).collect();
        }
        remaining_chars = remaining_chars.saturating_sub(clean.text.chars().count());
        *item = Arc::new(clean);
    }
}

fn sanitize_document_text(doc: &mut Document) {
    sanitize_text_items(&mut doc.texts);
    for page in &mut doc.pages {
        sanitize_text_items(&mut page.texts);
    }
}

fn sanitize_shape_items(items: &mut Vec<Arc<GeometricShape>>) {
    items.truncate(MAX_PAGE_SHAPES);
    for item in items.iter_mut() {
        *item = Arc::new(item.as_ref().sanitized());
    }
}

fn sanitize_document_study(doc: &mut Document) {
    sanitize_study_tapes(&mut doc.study_tapes);
    for page in &mut doc.pages {
        sanitize_study_tapes(&mut page.study_tapes);
    }
    sanitize_flashcards(&mut doc.flashcards);
}

fn sanitize_document_shapes(doc: &mut Document) {
    sanitize_shape_items(&mut doc.shapes);
    for page in &mut doc.pages {
        sanitize_shape_items(&mut page.shapes);
    }
}

fn sanitize_image_items(items: &mut Vec<Arc<PageImage>>) {
    let mut clean_list = Vec::with_capacity(items.len().min(MAX_PAGE_IMAGES));
    let mut total_bytes = 0_usize;
    for item in items.iter().take(MAX_PAGE_IMAGES) {
        let clean = item.as_ref().sanitized();
        if clean.png_bytes.is_empty() {
            continue;
        }
        let next_total = total_bytes.saturating_add(clean.png_bytes.len());
        if next_total > MAX_PAGE_IMAGE_BYTES {
            break;
        }
        total_bytes = next_total;
        clean_list.push(Arc::new(clean));
    }
    *items = clean_list;
}

fn sanitize_document_images(doc: &mut Document) {
    sanitize_image_items(&mut doc.images);
    for page in &mut doc.pages {
        sanitize_image_items(&mut page.images);
    }
}

fn remap_pdf_backdrop(
    backdrop: Option<PdfPageBackdrop>,
    remap: &[Option<usize>],
) -> Option<PdfPageBackdrop> {
    let b = backdrop?;
    let new_idx = remap.get(b.pdf_index).copied().flatten()?;
    Some(PdfPageBackdrop {
        pdf_index: new_idx,
        page_index: b.page_index,
    })
}

fn sanitize_template_id_opt(raw: Option<&str>) -> Option<String> {
    let clean = sanitize_template_id(raw?);
    (!clean.is_empty()).then_some(clean)
}

fn remap_image_backdrop(backdrop: Option<usize>, remap: &[Option<usize>]) -> Option<usize> {
    let idx = backdrop?;
    remap.get(idx).copied().flatten()
}

fn sanitize_document_image_templates(doc: &mut Document) {
    let mut clean_templates =
        Vec::with_capacity(doc.image_templates.len().min(MAX_DOCUMENT_IMAGE_TEMPLATES));
    let mut remap: Vec<Option<usize>> = Vec::with_capacity(doc.image_templates.len());
    let mut total_bytes = 0_usize;

    for tpl in doc
        .image_templates
        .iter()
        .take(MAX_DOCUMENT_IMAGE_TEMPLATES)
    {
        let clean = tpl.sanitized();
        if clean.png_bytes.is_empty() {
            remap.push(None);
            continue;
        }
        let next_total = total_bytes.saturating_add(clean.png_bytes.len());
        if next_total > MAX_DOCUMENT_IMAGE_TEMPLATE_BYTES {
            remap.push(None);
            continue;
        }
        total_bytes = next_total;
        let new_idx = clean_templates.len();
        clean_templates.push(clean);
        remap.push(Some(new_idx));
    }

    doc.image_templates = clean_templates;
    doc.custom_image_backdrop = remap_image_backdrop(doc.custom_image_backdrop, &remap);
    doc.custom_template_id = sanitize_template_id_opt(doc.custom_template_id.as_deref());
    doc.backdrop_rotation %= 4;
    for page in &mut doc.pages {
        page.custom_image_backdrop = remap_image_backdrop(page.custom_image_backdrop, &remap);
        page.custom_template_id = sanitize_template_id_opt(page.custom_template_id.as_deref());
        page.backdrop_rotation %= 4;
    }
}

fn sanitize_document_pdfs(doc: &mut Document) {
    let mut clean_pdfs = Vec::with_capacity(doc.pdf_documents.len());
    let mut remap: Vec<Option<usize>> = Vec::with_capacity(doc.pdf_documents.len());

    for pdf in &doc.pdf_documents {
        let clean = pdf.sanitized();
        if clean.bytes.is_empty() {
            remap.push(None);
            continue;
        }
        let new_idx = clean_pdfs.len();
        clean_pdfs.push(clean);
        remap.push(Some(new_idx));
    }

    doc.pdf_documents = clean_pdfs;
    doc.pdf_backdrop = remap_pdf_backdrop(doc.pdf_backdrop, &remap);
    for page in &mut doc.pages {
        page.pdf_backdrop = remap_pdf_backdrop(page.pdf_backdrop, &remap);
    }
}

impl Serialize for Document {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // When `pages` already holds the exact active page snapshot, omit redundant top-level
        // stroke and text copies to avoid duplicating active-page payloads.
        let active_synced = !self.pages.is_empty()
            && self.pages.get(self.active_page_index()) == Some(&self.active_page_snapshot());
        let strokes = (!active_synced).then_some(self.strokes.as_slice());
        let texts = (!active_synced).then_some(self.texts.as_slice());
        let shapes = (!active_synced).then_some(self.shapes.as_slice());
        let images = (!active_synced).then_some(self.images.as_slice());
        let pdf_backdrop = (!active_synced).then_some(self.pdf_backdrop).flatten();
        let bookmarked = (!active_synced && self.bookmarked).then_some(true);
        let backdrop_rotation = (!active_synced && !self.backdrop_rotation.is_multiple_of(4))
            .then_some(self.backdrop_rotation % 4);
        let custom_image_backdrop = (!active_synced)
            .then_some(self.custom_image_backdrop)
            .flatten();
        let custom_template_id = (!active_synced)
            .then_some(self.custom_template_id.as_deref())
            .flatten();
        let study_tapes = (!active_synced).then_some(self.study_tapes.as_slice());
        let clean_id = sanitize_notebook_id(&self.id);
        let wire = DocumentWireSer {
            id: &clean_id,
            title: &self.title,
            cover: self.cover,
            folder_path: sanitize_folder_path(&self.folder_path),
            favorite: self.favorite,
            tags: &self.tags,
            created_at_ms: self.created_at_ms.max(0),
            updated_at_ms: self.updated_at_ms.max(0),
            meta_updated_at_ms: self.meta_updated_at_ms.max(0),
            width: self.width,
            height: self.height,
            paper_template: self.paper_template,
            paper_color: self.paper_color,
            strokes,
            texts,
            shapes,
            images,
            pdf_backdrop,
            bookmarked,
            backdrop_rotation,
            custom_image_backdrop,
            custom_template_id,
            pdf_documents: &self.pdf_documents,
            image_templates: &self.image_templates,
            study_tapes,
            flashcards: &self.flashcards,
            next_stroke_id: self.next_stroke_id,
            pages: &self.pages,
            current_page: self.current_page,
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Document {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = DocumentWireDe::deserialize(deserializer)?;
        let active_from_pages = wire.strokes.is_none()
            && wire.texts.is_none()
            && wire.shapes.is_none()
            && wire.images.is_none()
            && wire.pdf_backdrop.is_none()
            && wire.bookmarked.is_none()
            && wire.backdrop_rotation.is_none()
            && wire.custom_image_backdrop.is_none()
            && wire.custom_template_id.is_none()
            && wire.study_tapes.is_none()
            && !wire.pages.is_empty();
        let mut doc = Self {
            id: sanitize_notebook_id(&wire.id),
            title: sanitize_notebook_title(&wire.title),
            cover: wire.cover,
            folder_path: sanitize_folder_path(&wire.folder_path),
            favorite: wire.favorite,
            tags: sanitize_notebook_tags(&wire.tags),
            created_at_ms: wire.created_at_ms.max(0),
            updated_at_ms: wire.updated_at_ms.max(0),
            meta_updated_at_ms: wire.meta_updated_at_ms.max(0),
            width: wire.width.clamp(64, 8192),
            height: wire.height.clamp(64, 8192),
            paper_template: wire.paper_template,
            paper_color: wire.paper_color,
            strokes: wire.strokes.unwrap_or_default(),
            texts: wire.texts.unwrap_or_default(),
            shapes: wire.shapes.unwrap_or_default(),
            images: wire.images.unwrap_or_default(),
            pdf_backdrop: wire.pdf_backdrop,
            pdf_documents: wire.pdf_documents,
            bookmarked: wire.bookmarked.unwrap_or(false),
            backdrop_rotation: wire.backdrop_rotation.unwrap_or(0) % 4,
            custom_image_backdrop: wire.custom_image_backdrop,
            custom_template_id: wire.custom_template_id,
            image_templates: wire.image_templates,
            study_tapes: wire.study_tapes.unwrap_or_default(),
            flashcards: wire.flashcards,
            next_stroke_id: wire.next_stroke_id,
            pages: wire.pages,
            current_page: wire.current_page,
        };
        if active_from_pages {
            doc.load_active_from_pages();
        }
        sanitize_document_text(&mut doc);
        sanitize_document_shapes(&mut doc);
        sanitize_document_study(&mut doc);
        sanitize_document_images(&mut doc);
        sanitize_document_pdfs(&mut doc);
        sanitize_document_image_templates(&mut doc);
        Ok(doc)
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new(1200, 1600)
    }
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        let w = width.clamp(64, 8192);
        let h = height.clamp(64, 8192);
        let template = PaperTemplate::RuledNarrow;
        let color = PaperColor::PureWhite;
        Self {
            id: String::new(),
            title: default_notebook_title(),
            cover: NotebookCover::default(),
            folder_path: String::new(),
            favorite: false,
            tags: Vec::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
            meta_updated_at_ms: 0,
            width: w,
            height: h,
            paper_template: template,
            paper_color: color,
            strokes: Vec::new(),
            texts: Vec::new(),
            shapes: Vec::new(),
            images: Vec::new(),
            pdf_backdrop: None,
            pdf_documents: Vec::new(),
            bookmarked: false,
            backdrop_rotation: 0,
            custom_image_backdrop: None,
            custom_template_id: None,
            image_templates: Vec::new(),
            study_tapes: Vec::new(),
            flashcards: Vec::new(),
            next_stroke_id: 1,
            pages: vec![NotebookPage::new(w, h, template, color)],
            current_page: 0,
        }
    }

    /// Re-applies the limits enforced during deserialization to a document that was assembled
    /// from separately loaded (untrusted) parts, such as a notebook package folder.
    pub fn sanitize_untrusted(&mut self) {
        self.width = self.width.clamp(64, 8192);
        self.height = self.height.clamp(64, 8192);
        for page in &mut self.pages {
            page.width = page.width.clamp(64, 8192);
            page.height = page.height.clamp(64, 8192);
        }
        sanitize_document_text(self);
        sanitize_document_shapes(self);
        sanitize_document_study(self);
        sanitize_document_images(self);
        sanitize_document_pdfs(self);
        sanitize_document_image_templates(self);
    }

    pub fn alloc_stroke_id(&mut self) -> StrokeId {
        let id = StrokeId(self.next_stroke_id);
        self.next_stroke_id = self.next_stroke_id.saturating_add(1);
        id
    }

    /// Captures the currently active top-level page state as a `NotebookPage`.
    pub fn active_page_snapshot(&self) -> NotebookPage {
        NotebookPage {
            width: self.width.clamp(64, 8192),
            height: self.height.clamp(64, 8192),
            paper_template: self.paper_template,
            paper_color: self.paper_color,
            strokes: self.strokes.clone(),
            texts: self.texts.clone(),
            shapes: self.shapes.clone(),
            images: self.images.clone(),
            pdf_backdrop: self.pdf_backdrop,
            bookmarked: self.bookmarked,
            backdrop_rotation: self.backdrop_rotation % 4,
            custom_image_backdrop: self.custom_image_backdrop,
            custom_template_id: sanitize_template_id_opt(self.custom_template_id.as_deref()),
            study_tapes: self.study_tapes.clone(),
        }
    }

    fn ensure_pages_initialized(&mut self) {
        if self.pages.is_empty() {
            self.pages.push(self.active_page_snapshot());
            self.current_page = 0;
        }
        let max_idx = self.pages.len().saturating_sub(1);
        if self.current_page > max_idx {
            self.current_page = max_idx;
        }
    }

    /// Writes the active top-level page fields back into `self.pages[self.current_page]`.
    pub fn sync_active_to_pages(&mut self) {
        self.ensure_pages_initialized();
        let snap = self.active_page_snapshot();
        if let Some(slot) = self.pages.get_mut(self.current_page) {
            *slot = snap;
        }
    }

    /// Loads `self.pages[self.current_page]` into the top-level active page fields.
    pub fn load_active_from_pages(&mut self) {
        self.ensure_pages_initialized();
        if let Some(page) = self.pages.get(self.current_page).cloned() {
            self.width = page.width.clamp(64, 8192);
            self.height = page.height.clamp(64, 8192);
            self.paper_template = page.paper_template;
            self.paper_color = page.paper_color;
            self.strokes = page.strokes;
            self.texts = page.texts;
            self.shapes = page.shapes;
            self.images = page.images;
            self.pdf_backdrop = page.pdf_backdrop;
            self.bookmarked = page.bookmarked;
            self.backdrop_rotation = page.backdrop_rotation % 4;
            self.custom_image_backdrop = page.custom_image_backdrop;
            self.custom_template_id = page.custom_template_id;
            self.study_tapes = page.study_tapes;
        }
    }

    /// Returns the embedded PDF and 0-based PDF page index referenced by `page`, if any.
    pub fn page_pdf_source(&self, page: &NotebookPage) -> Option<(&EmbeddedPdf, usize)> {
        let backdrop = page.pdf_backdrop?;
        let pdf = self.pdf_documents.get(backdrop.pdf_index)?;
        Some((pdf, backdrop.page_index))
    }

    /// Returns the embedded custom image template referenced by `page`, if any.
    pub fn page_custom_image_source(&self, page: &NotebookPage) -> Option<&EmbeddedImageTemplate> {
        let idx = page.custom_image_backdrop?;
        self.image_templates.get(idx)
    }

    /// Drops unreferenced entries in [`Self::pdf_documents`] and remaps page indices compactly.
    pub fn prune_unused_pdfs(&mut self) {
        self.sync_active_to_pages();
        if self.pdf_documents.is_empty() {
            self.pdf_backdrop = None;
            for page in &mut self.pages {
                page.pdf_backdrop = None;
            }
            return;
        }
        let mut used = vec![false; self.pdf_documents.len()];
        for page in &self.pages {
            if let Some(b) = page.pdf_backdrop
                && let Some(flag) = used.get_mut(b.pdf_index)
            {
                *flag = true;
            }
        }
        if used.iter().all(|&u| u) {
            return;
        }
        let mut remap = vec![None; self.pdf_documents.len()];
        let mut kept = Vec::new();
        for (idx, pdf) in std::mem::take(&mut self.pdf_documents)
            .into_iter()
            .enumerate()
        {
            if used.get(idx).copied().unwrap_or(false) {
                remap[idx] = Some(kept.len());
                kept.push(pdf);
            }
        }
        self.pdf_documents = kept;
        for page in &mut self.pages {
            page.pdf_backdrop = remap_pdf_backdrop(page.pdf_backdrop, &remap);
        }
        self.pdf_backdrop = remap_pdf_backdrop(self.pdf_backdrop, &remap);
    }

    /// Drops unreferenced entries in [`Self::image_templates`] and remaps page indices compactly.
    pub fn prune_unused_image_templates(&mut self) {
        self.sync_active_to_pages();
        if self.image_templates.is_empty() {
            self.custom_image_backdrop = None;
            for page in &mut self.pages {
                page.custom_image_backdrop = None;
            }
            return;
        }
        let mut used = vec![false; self.image_templates.len()];
        for page in &self.pages {
            if let Some(idx) = page.custom_image_backdrop
                && let Some(flag) = used.get_mut(idx)
            {
                *flag = true;
            }
        }
        if used.iter().all(|&u| u) {
            return;
        }
        let mut remap = vec![None; self.image_templates.len()];
        let mut kept = Vec::new();
        for (idx, tpl) in std::mem::take(&mut self.image_templates)
            .into_iter()
            .enumerate()
        {
            if used.get(idx).copied().unwrap_or(false) {
                remap[idx] = Some(kept.len());
                kept.push(tpl);
            }
        }
        self.image_templates = kept;
        for page in &mut self.pages {
            page.custom_image_backdrop = remap_image_backdrop(page.custom_image_backdrop, &remap);
        }
        self.custom_image_backdrop = remap_image_backdrop(self.custom_image_backdrop, &remap);
    }

    /// Prunes both unreferenced PDF documents and custom image templates.
    pub fn prune_unused_backdrops(&mut self) {
        self.prune_unused_pdfs();
        self.prune_unused_image_templates();
    }

    /// Inserts or deduplicates an embedded PDF on this document, returning its index.
    pub fn upsert_embedded_pdf(&mut self, pdf: EmbeddedPdf) -> Option<usize> {
        let clean = pdf.sanitized();
        if clean.bytes.is_empty() {
            return None;
        }
        if let Some(existing) = self
            .pdf_documents
            .iter()
            .position(|entry| entry.bytes.as_slice() == clean.bytes.as_slice())
        {
            return Some(existing);
        }
        let idx = self.pdf_documents.len();
        self.pdf_documents.push(clean);
        Some(idx)
    }

    /// Inserts or deduplicates an embedded custom image template on this document, returning its index.
    pub fn upsert_embedded_image_template(
        &mut self,
        template: EmbeddedImageTemplate,
    ) -> Option<usize> {
        let clean = template.sanitized();
        if clean.png_bytes.is_empty() {
            return None;
        }
        if let Some(existing) = self
            .image_templates
            .iter()
            .position(|entry| entry.png_bytes.as_slice() == clean.png_bytes.as_slice())
        {
            return Some(existing);
        }
        if self.image_templates.len() >= MAX_DOCUMENT_IMAGE_TEMPLATES {
            return None;
        }
        let used_bytes: usize = self
            .image_templates
            .iter()
            .map(|entry| entry.png_bytes.len())
            .sum();
        if used_bytes.saturating_add(clean.png_bytes.len()) > MAX_DOCUMENT_IMAGE_TEMPLATE_BYTES {
            return None;
        }
        let idx = self.image_templates.len();
        self.image_templates.push(clean);
        Some(idx)
    }

    /// Applies a saved [`CustomPaperTemplate`] as the paper backdrop of `page_index`.
    pub fn apply_custom_template_to_page(
        &mut self,
        page_index: usize,
        template: &CustomPaperTemplate,
    ) -> Result<(), String> {
        let clean_tpl = template
            .sanitized()
            .ok_or_else(|| "custom paper template payload is invalid or empty".to_string())?;
        self.sync_active_to_pages();
        if page_index >= self.pages.len() {
            return Err("target page index out of bounds".to_string());
        }
        match clean_tpl.kind {
            CustomTemplateKind::Image => {
                let img_idx = self
                    .upsert_embedded_image_template(EmbeddedImageTemplate {
                        name: clean_tpl.name,
                        png_bytes: clean_tpl.bytes,
                    })
                    .ok_or_else(|| "notebook custom image template limit reached".to_string())?;
                if let Some(page) = self.pages.get_mut(page_index) {
                    page.pdf_backdrop = None;
                    page.custom_image_backdrop = Some(img_idx);
                    page.custom_template_id = sanitize_template_id_opt(Some(&clean_tpl.id));
                    page.paper_template = PaperTemplate::Blank;
                    page.backdrop_rotation = 0;
                    page.width = clean_tpl.width.clamp(64, 8192);
                    page.height = clean_tpl.height.clamp(64, 8192);
                }
            }
            CustomTemplateKind::Pdf => {
                let pdf_idx = self
                    .upsert_embedded_pdf(EmbeddedPdf {
                        name: format!("{}.pdf", clean_tpl.name),
                        bytes: clean_tpl.bytes,
                    })
                    .ok_or_else(|| "notebook embedded PDF limit reached".to_string())?;
                if let Some(page) = self.pages.get_mut(page_index) {
                    page.custom_image_backdrop = None;
                    page.pdf_backdrop = Some(PdfPageBackdrop {
                        pdf_index: pdf_idx,
                        page_index: clean_tpl.pdf_page_index,
                    });
                    page.custom_template_id = sanitize_template_id_opt(Some(&clean_tpl.id));
                    page.paper_template = PaperTemplate::Blank;
                    page.backdrop_rotation = 0;
                    page.width = clean_tpl.width.clamp(64, 8192);
                    page.height = clean_tpl.height.clamp(64, 8192);
                }
            }
        }
        self.load_active_from_pages();
        self.prune_unused_backdrops();
        Ok(())
    }

    /// Clears any PDF or custom image template attached to `page_index`.
    pub fn clear_page_custom_template(&mut self, page_index: usize) -> bool {
        self.sync_active_to_pages();
        let Some(page) = self.pages.get_mut(page_index) else {
            return false;
        };
        page.pdf_backdrop = None;
        page.custom_image_backdrop = None;
        page.custom_template_id = None;
        page.backdrop_rotation = 0;
        self.load_active_from_pages();
        self.prune_unused_backdrops();
        true
    }

    /// Total page count in the notebook (always at least 1).
    pub fn page_count(&self) -> usize {
        self.pages.len().max(1)
    }

    /// 0-based index of the active page, guaranteed `< self.page_count()`.
    pub fn active_page_index(&self) -> usize {
        if self.pages.is_empty() {
            0
        } else {
            self.current_page.min(self.pages.len().saturating_sub(1))
        }
    }

    /// Switches to `index`, returning `false` if `index` is out of bounds.
    pub fn select_page(&mut self, index: usize) -> bool {
        self.sync_active_to_pages();
        if index >= self.pages.len() {
            return false;
        }
        self.current_page = index;
        self.load_active_from_pages();
        true
    }

    /// Reorders page `from` to index `to`, preserving the active page reference.
    pub fn move_page(&mut self, from: usize, to: usize) -> bool {
        self.sync_active_to_pages();
        if from >= self.pages.len() || to >= self.pages.len() || from == to {
            return false;
        }
        let page = self.pages.remove(from);
        self.pages.insert(to, page);
        self.current_page = if self.current_page == from {
            to
        } else if from < self.current_page && self.current_page <= to {
            self.current_page.saturating_sub(1)
        } else if to <= self.current_page && self.current_page < from {
            self.current_page.saturating_add(1)
        } else {
            self.current_page
        };
        self.load_active_from_pages();
        true
    }

    /// Rotates page `index` clockwise by `quarter_turns` 90-degree steps (negative values rotate CCW).
    pub fn rotate_page_quarter_turns(&mut self, index: usize, quarter_turns: i32) -> bool {
        let turns = quarter_turns.rem_euclid(4) as usize;
        if turns == 0 {
            return false;
        }
        self.sync_active_to_pages();
        let Some(page) = self.pages.get_mut(index) else {
            return false;
        };
        for _ in 0..turns {
            rotate_page_90_cw(page);
        }
        self.load_active_from_pages();
        true
    }

    /// Applies optional ruling `template` and/or `paper_color` across every page in the notebook.
    pub fn apply_template_to_all_pages(
        &mut self,
        template: Option<PaperTemplate>,
        paper_color: Option<PaperColor>,
        clear_custom_template: bool,
    ) {
        self.sync_active_to_pages();
        for page in &mut self.pages {
            if let Some(tpl) = template {
                page.paper_template = tpl;
            }
            if let Some(col) = paper_color {
                page.paper_color = col;
            }
            if clear_custom_template {
                page.pdf_backdrop = None;
                page.custom_image_backdrop = None;
                page.custom_template_id = None;
                page.backdrop_rotation = 0;
            }
        }
        self.load_active_from_pages();
        if clear_custom_template {
            self.prune_unused_backdrops();
        }
    }

    /// Applies a saved [`CustomPaperTemplate`] across every page in the notebook.
    pub fn apply_custom_template_to_all_pages(
        &mut self,
        template: &CustomPaperTemplate,
    ) -> Result<(), String> {
        let prev = self.active_page_index();
        self.sync_active_to_pages();
        for idx in 0..self.pages.len() {
            self.apply_custom_template_to_page(idx, template)?;
        }
        self.select_page(prev);
        Ok(())
    }

    /// Updates the sheet width and height of `page_index` (clamped to `64..=8192`).
    ///
    /// Returns `true` when `page_index` exists and its dimensions changed.
    pub fn set_page_dimensions(&mut self, page_index: usize, width: u32, height: u32) -> bool {
        let w = width.clamp(64, 8192);
        let h = height.clamp(64, 8192);
        self.sync_active_to_pages();
        let Some(page) = self.pages.get_mut(page_index) else {
            return false;
        };
        if page.width == w && page.height == h {
            return false;
        }
        page.width = w;
        page.height = h;
        self.load_active_from_pages();
        true
    }

    /// Updates sheet dimensions across every page in the notebook.
    ///
    /// Returns `true` when at least one page dimension changed.
    pub fn set_all_pages_dimensions(&mut self, width: u32, height: u32) -> bool {
        let w = width.clamp(64, 8192);
        let h = height.clamp(64, 8192);
        self.sync_active_to_pages();
        let mut changed = false;
        for page in &mut self.pages {
            if page.width != w || page.height != h {
                page.width = w;
                page.height = h;
                changed = true;
            }
        }
        if changed {
            self.load_active_from_pages();
        }
        changed
    }

    /// Applies `orientation` across every page while preserving each sheet's size ratio.
    pub fn set_all_pages_orientation(&mut self, orientation: PageOrientation) -> bool {
        self.sync_active_to_pages();
        let mut changed = false;
        for page in &mut self.pages {
            let (w, h) = orientation.apply(page.width, page.height);
            if page.width != w || page.height != h {
                page.width = w;
                page.height = h;
                changed = true;
            }
        }
        if changed {
            self.load_active_from_pages();
        }
        changed
    }

    /// Inserts a fresh blank page right after the active page and selects it.
    pub fn add_page(
        &mut self,
        template: Option<PaperTemplate>,
        paper_color: Option<PaperColor>,
    ) -> Option<usize> {
        self.add_page_with_size(None, None, template, paper_color)
    }

    /// Inserts a fresh blank page with explicit optional dimensions right after the active page.
    pub fn add_page_with_size(
        &mut self,
        width: Option<u32>,
        height: Option<u32>,
        template: Option<PaperTemplate>,
        paper_color: Option<PaperColor>,
    ) -> Option<usize> {
        self.sync_active_to_pages();
        let page = NotebookPage::new(
            width.unwrap_or(self.width).clamp(64, 8192),
            height.unwrap_or(self.height).clamp(64, 8192),
            template.unwrap_or(self.paper_template),
            paper_color.unwrap_or(self.paper_color),
        );
        let insert_at = self.current_page.saturating_add(1).min(self.pages.len());
        self.pages.insert(insert_at, page);
        self.current_page = insert_at;
        self.load_active_from_pages();
        Some(insert_at)
    }

    /// Duplicates `page_index` right after itself with freshly allocated stroke IDs.
    pub fn duplicate_page_at(&mut self, page_index: usize) -> Option<usize> {
        self.sync_active_to_pages();
        let source = self.pages.get(page_index)?.clone();
        let mut duplicated_strokes = Vec::with_capacity(source.strokes.len());
        for stroke in &source.strokes {
            let mut copy = (**stroke).clone();
            copy.id = self.alloc_stroke_id();
            duplicated_strokes.push(Arc::new(copy));
        }
        let page = NotebookPage {
            width: source.width,
            height: source.height,
            paper_template: source.paper_template,
            paper_color: source.paper_color,
            strokes: duplicated_strokes,
            texts: source.texts,
            shapes: source.shapes,
            images: source.images,
            pdf_backdrop: source.pdf_backdrop,
            bookmarked: source.bookmarked,
            backdrop_rotation: source.backdrop_rotation,
            custom_image_backdrop: source.custom_image_backdrop,
            custom_template_id: source.custom_template_id,
            study_tapes: source.study_tapes,
        };
        let insert_at = page_index.saturating_add(1).min(self.pages.len());
        self.pages.insert(insert_at, page);
        self.current_page = insert_at;
        self.load_active_from_pages();
        Some(insert_at)
    }

    /// Duplicates the active page right after itself with freshly allocated stroke IDs.
    pub fn duplicate_active_page(&mut self) -> Option<usize> {
        let active = self.active_page_index();
        self.duplicate_page_at(active)
    }

    /// Appends a cloned page from `source_doc` into `self`, remapping stroke IDs and backdrops.
    pub fn append_cloned_page(
        &mut self,
        source: &NotebookPage,
        source_doc: &Document,
    ) -> Result<usize, String> {
        self.sync_active_to_pages();
        let mut cloned_strokes = Vec::with_capacity(source.strokes.len());
        for stroke in &source.strokes {
            let mut copy = (**stroke).clone();
            copy.id = self.alloc_stroke_id();
            cloned_strokes.push(Arc::new(copy));
        }
        let pdf_backdrop = match source.pdf_backdrop {
            Some(b) => {
                let pdf = source_doc
                    .pdf_documents
                    .get(b.pdf_index)
                    .ok_or_else(|| "missing source PDF backdrop".to_string())?;
                let target_idx = self
                    .upsert_embedded_pdf(pdf.clone())
                    .ok_or_else(|| "destination notebook PDF limit reached".to_string())?;
                Some(PdfPageBackdrop {
                    pdf_index: target_idx,
                    page_index: b.page_index,
                })
            }
            None => None,
        };
        let custom_image_backdrop = match source.custom_image_backdrop {
            Some(img_idx) => {
                let tpl = source_doc
                    .image_templates
                    .get(img_idx)
                    .ok_or_else(|| "missing source custom image template".to_string())?;
                let target_idx = self
                    .upsert_embedded_image_template(tpl.clone())
                    .ok_or_else(|| {
                        "destination notebook image template limit reached".to_string()
                    })?;
                Some(target_idx)
            }
            None => None,
        };
        let page = NotebookPage {
            width: source.width.clamp(64, 8192),
            height: source.height.clamp(64, 8192),
            paper_template: source.paper_template,
            paper_color: source.paper_color,
            strokes: cloned_strokes,
            texts: source.texts.clone(),
            shapes: source.shapes.clone(),
            images: source.images.clone(),
            pdf_backdrop,
            bookmarked: source.bookmarked,
            backdrop_rotation: source.backdrop_rotation % 4,
            custom_image_backdrop,
            custom_template_id: sanitize_template_id_opt(source.custom_template_id.as_deref()),
            study_tapes: source.study_tapes.clone(),
        };
        let new_idx = self.pages.len();
        self.pages.push(page);
        Ok(new_idx)
    }

    /// Removes the page at `index` if at least two pages exist, keeping `current_page` valid.
    pub fn remove_page(&mut self, index: usize) -> bool {
        self.sync_active_to_pages();
        if self.pages.len() <= 1 || index >= self.pages.len() {
            return false;
        }
        self.pages.remove(index);
        if index < self.current_page {
            self.current_page = self.current_page.saturating_sub(1);
        } else if self.current_page >= self.pages.len() {
            self.current_page = self.pages.len().saturating_sub(1);
        }
        self.load_active_from_pages();
        self.prune_unused_backdrops();
        true
    }

    /// Non-mutating view of all pages with any live active-page changes applied.
    pub fn resolved_pages(&self) -> Vec<NotebookPage> {
        if self.pages.is_empty() {
            return vec![self.active_page_snapshot()];
        }
        let mut pages = self.pages.clone();
        let active_idx = self.active_page_index();
        if let Some(slot) = pages.get_mut(active_idx) {
            *slot = self.active_page_snapshot();
        }
        pages
    }

    /// Invokes `f` on resolved page `index` without cloning the full page list.
    pub fn with_resolved_page<R>(
        &self,
        index: usize,
        f: impl FnOnce(&NotebookPage) -> R,
    ) -> Option<R> {
        if self.pages.is_empty() {
            if index == 0 {
                let snap = self.active_page_snapshot();
                return Some(f(&snap));
            }
            return None;
        }
        if index >= self.pages.len() {
            return None;
        }
        if index == self.active_page_index() {
            let snap = self.active_page_snapshot();
            Some(f(&snap))
        } else {
            self.pages.get(index).map(f)
        }
    }

    /// Returns `(width, height)` for `index` without cloning page vectors.
    pub fn page_dimensions(&self, index: usize) -> Option<(u32, u32)> {
        if self.pages.is_empty() {
            return (index == 0).then(|| (self.width.clamp(64, 8192), self.height.clamp(64, 8192)));
        }
        if index == self.active_page_index() && index < self.pages.len() {
            Some((self.width.clamp(64, 8192), self.height.clamp(64, 8192)))
        } else {
            self.pages
                .get(index)
                .map(|p| (p.width.clamp(64, 8192), p.height.clamp(64, 8192)))
        }
    }

    /// Counts bookmarked pages without cloning the page list.
    pub fn bookmarked_page_count(&self) -> usize {
        if self.pages.is_empty() {
            return usize::from(self.bookmarked);
        }
        let active = self.active_page_index();
        self.pages
            .iter()
            .enumerate()
            .filter(|&(idx, p)| {
                if idx == active {
                    self.bookmarked
                } else {
                    p.bookmarked
                }
            })
            .count()
    }

    /// Returns the 0-based indices of bookmarked pages without cloning pages.
    pub fn bookmarked_page_indices(&self) -> Vec<usize> {
        if self.pages.is_empty() {
            return if self.bookmarked { vec![0] } else { Vec::new() };
        }
        let active = self.active_page_index();
        self.pages
            .iter()
            .enumerate()
            .filter_map(|(idx, p)| {
                let marked = if idx == active {
                    self.bookmarked
                } else {
                    p.bookmarked
                };
                marked.then_some(idx)
            })
            .collect()
    }

    /// Counts committed ink strokes across all pages without cloning the page list.
    pub fn total_stroke_count(&self) -> usize {
        if self.pages.is_empty() {
            return self.strokes.len();
        }
        let active = self.active_page_index();
        self.pages
            .iter()
            .enumerate()
            .map(|(idx, p)| {
                if idx == active {
                    self.strokes.len()
                } else {
                    p.strokes.len()
                }
            })
            .sum()
    }

    /// Builds a standalone single-page `Document` snapshot for rendering or exporting `index`.
    pub fn document_for_page(&self, index: usize) -> Option<Document> {
        let page = self.resolved_pages().into_iter().nth(index)?;
        let mut doc = Document::new(page.width, page.height);
        doc.title = self.title.clone();
        doc.cover = self.cover;
        doc.folder_path = self.folder_path.clone();
        doc.favorite = self.favorite;
        doc.tags = self.tags.clone();
        doc.created_at_ms = self.created_at_ms;
        doc.updated_at_ms = self.updated_at_ms;
        doc.paper_template = page.paper_template;
        doc.paper_color = page.paper_color;
        doc.strokes = page.strokes;
        doc.texts = page.texts;
        doc.shapes = page.shapes;
        doc.images = page.images;
        doc.bookmarked = page.bookmarked;
        doc.backdrop_rotation = page.backdrop_rotation % 4;
        doc.custom_template_id = page.custom_template_id;
        doc.study_tapes = page.study_tapes;
        if let Some(backdrop) = page.pdf_backdrop
            && let Some(pdf) = self.pdf_documents.get(backdrop.pdf_index)
        {
            doc.pdf_documents.push(pdf.clone());
            doc.pdf_backdrop = Some(PdfPageBackdrop {
                pdf_index: 0,
                page_index: backdrop.page_index,
            });
        }
        if let Some(img_idx) = page.custom_image_backdrop
            && let Some(tpl) = self.image_templates.get(img_idx)
        {
            doc.image_templates.push(tpl.clone());
            doc.custom_image_backdrop = Some(0);
        }
        doc.next_stroke_id = self.next_stroke_id;
        doc.sync_active_to_pages();
        Some(doc)
    }
}

fn rotate_sample_90_cw(sample: StrokeSample, old_height: f64) -> StrokeSample {
    let mut rotated = sample;
    rotated.x = old_height - sample.y;
    rotated.y = sample.x;
    rotated.tilt_x = -sample.tilt_y;
    rotated.tilt_y = sample.tilt_x;
    rotated.rotation = (sample.rotation + 90.0).rem_euclid(360.0);
    if rotated.rotation >= 360.0 {
        rotated.rotation = 0.0;
    }
    rotated
}

fn normalize_signed_angle_deg(angle: f64) -> f64 {
    if !angle.is_finite() {
        return 0.0;
    }
    let wrapped = (angle + 180.0).rem_euclid(360.0) - 180.0;
    if wrapped <= -180.0 { 180.0 } else { wrapped }
}

fn rotate_page_90_cw(page: &mut NotebookPage) {
    let old_w = page.width.clamp(64, 8192);
    let old_h = page.height.clamp(64, 8192);
    let old_h_f64 = f64::from(old_h);
    page.width = old_h;
    page.height = old_w;
    page.backdrop_rotation = (page.backdrop_rotation + 1) % 4;

    for stroke in &mut page.strokes {
        let raw_points: Vec<StrokeSample> = stroke
            .raw_points
            .iter()
            .copied()
            .map(|s| rotate_sample_90_cw(s, old_h_f64))
            .collect();
        let smoothed_points: Vec<StrokeSample> = stroke
            .smoothed_points
            .iter()
            .copied()
            .map(|s| rotate_sample_90_cw(s, old_h_f64))
            .collect();
        *stroke = Arc::new(InkStroke::new(
            stroke.id,
            stroke.pen.clone(),
            raw_points,
            smoothed_points,
        ));
    }

    for text in &mut page.texts {
        let mut updated = (**text).clone();
        if updated.max_width.is_none() {
            updated.max_width = Some(updated.effective_max_width(old_w));
        }
        let nx = old_h_f64 - updated.y;
        let ny = updated.x;
        updated.x = nx;
        updated.y = ny;
        updated.angle_degrees = normalize_signed_angle_deg(updated.angle_degrees + 90.0);
        *text = Arc::new(updated.sanitized());
    }

    for shape in &mut page.shapes {
        let mut updated = (**shape).clone();
        let cx = (updated.x1 + updated.x2) * 0.5;
        let cy = (updated.y1 + updated.y2) * 0.5;
        let ncx = old_h_f64 - cy;
        let ncy = cx;
        let dx1 = updated.x1 - cx;
        let dy1 = updated.y1 - cy;
        let dx2 = updated.x2 - cx;
        let dy2 = updated.y2 - cy;
        updated.x1 = ncx + dx1;
        updated.y1 = ncy + dy1;
        updated.x2 = ncx + dx2;
        updated.y2 = ncy + dy2;
        for pt in &mut updated.points {
            pt.x += ncx - cx;
            pt.y += ncy - cy;
        }
        updated.angle_degrees = normalize_signed_angle_deg(updated.angle_degrees + 90.0);
        *shape = Arc::new(updated.sanitized());
    }

    for img in &mut page.images {
        let mut updated = (**img).clone();
        let cx = updated.x + updated.width * 0.5;
        let cy = updated.y + updated.height * 0.5;
        let ncx = old_h_f64 - cy;
        let ncy = cx;
        updated.x = ncx - updated.width * 0.5;
        updated.y = ncy - updated.height * 0.5;
        updated.angle_degrees = normalize_signed_angle_deg(updated.angle_degrees + 90.0);
        *img = Arc::new(updated.sanitized());
    }
}

/// Maximum number of swatches permitted in a user ink or highlighter palette.
pub const MAX_PALETTE_COLORS: usize = 12;

/// Maximum number of saved color palette presets (including the 3 built-ins).
pub const MAX_PALETTE_PRESETS: usize = 32;

/// Maximum length for a color palette group name.
pub const MAX_PALETTE_NAME_LEN: usize = 32;

/// Built-in immutable palette preset IDs.
pub const BUILTIN_PALETTE_CLASSIC_ID: &str = "builtin_classic";
pub const BUILTIN_PALETTE_PASTEL_ID: &str = "builtin_pastel";
pub const BUILTIN_PALETTE_EARTH_ID: &str = "builtin_earth";

/// Maximum number of saved user custom pen presets.
pub const MAX_CUSTOM_PRESETS: usize = 64;

/// Maximum length for a custom preset name.
pub const MAX_PRESET_NAME_LEN: usize = 48;

/// Maximum length for a custom preset description.
pub const MAX_PRESET_DESC_LEN: usize = 160;

/// Maximum length for a custom preset ID.
pub const MAX_PRESET_ID_LEN: usize = 64;

/// Selects which quick color palette is being targeted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteTarget {
    #[default]
    Ink,
    Highlighter,
}

impl PaletteTarget {
    pub const fn for_pen(kind: PenKind) -> Self {
        match kind {
            PenKind::Highlighter => Self::Highlighter,
            PenKind::FountainPen | PenKind::BallPen | PenKind::BrushPen | PenKind::Pencil => {
                Self::Ink
            }
        }
    }
}

fn sanitize_trimmed_text(raw: &str, max_len: usize, fallback: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(max_len)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Sanitizes a user custom preset name.
pub fn sanitize_preset_name(raw: &str) -> String {
    sanitize_trimmed_text(raw, MAX_PRESET_NAME_LEN, "Custom Preset")
}

/// Sanitizes a color palette group name.
pub fn sanitize_palette_name(raw: &str) -> String {
    sanitize_trimmed_text(raw, MAX_PALETTE_NAME_LEN, "Custom Palette")
}

fn sanitize_swatch_list(raw: &[Color], fallback: fn() -> Vec<Color>) -> Vec<Color> {
    let mut out: Vec<Color> = raw
        .iter()
        .take(MAX_PALETTE_COLORS)
        .map(|c| c.sanitized().with_alpha(1.0))
        .collect();
    if out.is_empty() {
        out = fallback();
    }
    out
}

fn builtin_palette_slot(id: &str) -> Option<usize> {
    match id {
        BUILTIN_PALETTE_CLASSIC_ID => Some(0),
        BUILTIN_PALETTE_PASTEL_ID => Some(1),
        BUILTIN_PALETTE_EARTH_ID => Some(2),
        _ => None,
    }
}

/// Named color palette group pairing ink and highlighter quick swatches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorPalettePreset {
    pub id: String,
    pub name: String,
    pub built_in: bool,
    pub ink_colors: Vec<Color>,
    pub highlighter_colors: Vec<Color>,
}

impl Default for ColorPalettePreset {
    fn default() -> Self {
        Self::builtin_classic()
    }
}

fn is_legacy_unmodified_builtin_ink(id: &str, colors: &[Color]) -> bool {
    const LEGACY_CLASSIC: [[u8; 3]; 8] = [
        [0x1A, 0x1A, 0x1E],
        [0x1B, 0x3B, 0x6F],
        [0x1E, 0x63, 0xE9],
        [0xD9, 0x2B, 0x2B],
        [0x1D, 0x7A, 0x46],
        [0x6B, 0x2F, 0x8A],
        [0xD9, 0x6B, 0x1E],
        [0x0E, 0x7C, 0x7B],
    ];
    const LEGACY_PASTEL: [[u8; 3]; 8] = [
        [0x6E, 0x5A, 0x7A],
        [0xDE, 0x7C, 0x9C],
        [0xE8, 0x96, 0x7A],
        [0xD4, 0xA8, 0x57],
        [0x68, 0xB3, 0x90],
        [0x6F, 0xA3, 0xD8],
        [0x98, 0x86, 0xD2],
        [0x5E, 0xAE, 0xB2],
    ];
    const LEGACY_EARTH: [[u8; 3]; 8] = [
        [0x3B, 0x28, 0x20],
        [0xB8, 0x53, 0x3E],
        [0xC6, 0x8B, 0x2C],
        [0x5E, 0x71, 0x3A],
        [0x2F, 0x5D, 0x5E],
        [0x42, 0x56, 0x73],
        [0x7D, 0x44, 0x57],
        [0x8C, 0x72, 0x62],
    ];
    let legacy = match id {
        BUILTIN_PALETTE_CLASSIC_ID => &LEGACY_CLASSIC,
        BUILTIN_PALETTE_PASTEL_ID => &LEGACY_PASTEL,
        BUILTIN_PALETTE_EARTH_ID => &LEGACY_EARTH,
        _ => return false,
    };
    colors.len() == legacy.len()
        && colors.iter().zip(legacy.iter()).all(|(c, rgb)| {
            let [r, g, b, _] = c.to_rgba8();
            [r, g, b] == *rgb
        })
}

impl ColorPalettePreset {
    pub fn builtin_classic() -> Self {
        Self {
            id: BUILTIN_PALETTE_CLASSIC_ID.to_string(),
            name: "Classic".to_string(),
            built_in: true,
            ink_colors: INK_PALETTE.iter().map(|s| s.color).collect(),
            highlighter_colors: HIGHLIGHTER_PALETTE.iter().map(|s| s.color).collect(),
        }
    }

    pub fn builtin_pastel() -> Self {
        Self {
            id: BUILTIN_PALETTE_PASTEL_ID.to_string(),
            name: "Pastel".to_string(),
            built_in: true,
            ink_colors: PASTEL_INK_PALETTE.iter().map(|s| s.color).collect(),
            highlighter_colors: PASTEL_HIGHLIGHTER_PALETTE.iter().map(|s| s.color).collect(),
        }
    }

    pub fn builtin_earth() -> Self {
        Self {
            id: BUILTIN_PALETTE_EARTH_ID.to_string(),
            name: "Earth Tones".to_string(),
            built_in: true,
            ink_colors: EARTH_INK_PALETTE.iter().map(|s| s.color).collect(),
            highlighter_colors: EARTH_HIGHLIGHTER_PALETTE.iter().map(|s| s.color).collect(),
        }
    }

    pub fn built_ins() -> Vec<Self> {
        vec![
            Self::builtin_classic(),
            Self::builtin_pastel(),
            Self::builtin_earth(),
        ]
    }

    pub fn builtin_by_id(id: &str) -> Option<Self> {
        match id {
            BUILTIN_PALETTE_CLASSIC_ID => Some(Self::builtin_classic()),
            BUILTIN_PALETTE_PASTEL_ID => Some(Self::builtin_pastel()),
            BUILTIN_PALETTE_EARTH_ID => Some(Self::builtin_earth()),
            _ => None,
        }
    }

    pub fn colors(&self, target: PaletteTarget) -> &[Color] {
        match target {
            PaletteTarget::Ink => &self.ink_colors,
            PaletteTarget::Highlighter => &self.highlighter_colors,
        }
    }

    pub fn colors_mut(&mut self, target: PaletteTarget) -> &mut Vec<Color> {
        match target {
            PaletteTarget::Ink => &mut self.ink_colors,
            PaletteTarget::Highlighter => &mut self.highlighter_colors,
        }
    }

    pub fn sanitized(&self) -> Self {
        let id = sanitize_preset_id(&self.id);
        if let Some(base) = Self::builtin_by_id(&id) {
            return base;
        }
        let name = sanitize_palette_name(&self.name);
        let mut ink_colors: Vec<Color> = self
            .ink_colors
            .iter()
            .take(MAX_PALETTE_COLORS)
            .map(|c| c.sanitized().with_alpha(1.0))
            .collect();
        if ink_colors.is_empty() {
            ink_colors = UserStudioConfig::default_ink_palette();
        }
        let mut highlighter_colors: Vec<Color> = self
            .highlighter_colors
            .iter()
            .take(MAX_PALETTE_COLORS)
            .map(|c| c.sanitized().with_alpha(1.0))
            .collect();
        if highlighter_colors.is_empty() {
            highlighter_colors = UserStudioConfig::default_highlighter_palette();
        }
        Self {
            id,
            name,
            built_in: false,
            ink_colors,
            highlighter_colors,
        }
    }
}

/// Sanitizes a user custom preset description.
pub fn sanitize_preset_description(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_PRESET_DESC_LEN)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Sanitizes a custom preset ID to safe ASCII slug characters.
pub fn sanitize_preset_id(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .take(MAX_PRESET_ID_LEN)
        .collect()
}

/// User-authored pen preset pairing nib geometry, pressure dynamics, smoothing, and optional color.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomPenPreset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub settings: PenSettings,
    pub include_color: bool,
}

impl Default for CustomPenPreset {
    fn default() -> Self {
        Self {
            id: "custom_1".to_string(),
            name: "Custom Preset".to_string(),
            description: String::new(),
            settings: PenSettings::fountain_pen(),
            include_color: false,
        }
    }
}

impl CustomPenPreset {
    /// Returns a sanitized copy with bounded strings and valid pen settings.
    pub fn sanitized(&self) -> Self {
        Self {
            id: sanitize_preset_id(&self.id),
            name: sanitize_preset_name(&self.name),
            description: sanitize_preset_description(&self.description),
            settings: self.settings.sanitized(),
            include_color: self.include_color,
        }
    }

    /// Short concise summary badge for Settings cards.
    pub fn badge(&self) -> String {
        format!(
            "{} · {:.1} pt · {}",
            self.settings.kind.short_label(),
            self.settings.size,
            self.settings.smoothing.stabilization_summary()
        )
    }

    /// Plain-English description shown beneath the preset title.
    pub fn summary_description(&self) -> String {
        if !self.description.trim().is_empty() {
            return self.description.clone();
        }
        let color_note = if self.include_color {
            format!("Saved color {}", self.settings.color.to_hex())
        } else {
            "Keeps active ink color".to_string()
        };
        let pressure_note = if self.settings.pressure_enabled {
            "pressure-sensitive"
        } else {
            "uniform width"
        };
        format!(
            "{} ({pressure_note}, {:.0}% opacity) · {color_note}.",
            self.settings.kind.label(),
            self.settings.opacity * 100.0
        )
    }

    /// Builds `PenSettings` ready to apply, preserving `current_color` unless bound or overridden.
    pub fn to_applied_settings(
        &self,
        current_color: Color,
        preserve_color: Option<bool>,
    ) -> PenSettings {
        let mut s = self.settings.sanitized();
        let keep_current = preserve_color.unwrap_or(!self.include_color);
        if keep_current {
            s.color = current_color.sanitized();
        }
        s
    }

    /// Returns `true` when `pen` matches this preset's configured parameters.
    pub fn matches_pen(&self, pen: &PenSettings) -> bool {
        let clean = pen.sanitized();
        let ref_pen = self.settings.sanitized();
        let color_ok = !self.include_color
            || ((clean.color.r - ref_pen.color.r).abs() <= 0.015
                && (clean.color.g - ref_pen.color.g).abs() <= 0.015
                && (clean.color.b - ref_pen.color.b).abs() <= 0.015);
        color_ok
            && clean.kind == ref_pen.kind
            && (clean.size - ref_pen.size).abs() <= 0.15
            && (clean.opacity - ref_pen.opacity).abs() <= 0.02
            && (clean.hardness - ref_pen.hardness).abs() <= 0.02
            && (clean.nib_flatness - ref_pen.nib_flatness).abs() <= 0.02
            && (clean.nib_angle_deg - ref_pen.nib_angle_deg).abs() <= 1.0
            && (clean.start_taper_px - ref_pen.start_taper_px).abs() <= 0.5
            && (clean.end_taper_px - ref_pen.end_taper_px).abs() <= 0.5
            && clean.pressure_enabled == ref_pen.pressure_enabled
            && clean.velocity_pressure_simulation == ref_pen.velocity_pressure_simulation
            && (clean.pressure_size_min - ref_pen.pressure_size_min).abs() <= 0.02
            && (clean.pressure_opacity_min - ref_pen.pressure_opacity_min).abs() <= 0.02
            && (clean.tilt_sensitivity - ref_pen.tilt_sensitivity).abs() <= 0.02
            && clean.pressure_curve == ref_pen.pressure_curve
            && clean.smoothing == ref_pen.smoothing
    }
}

/// Application workspace UI color theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UiTheme {
    #[default]
    Light,
    Dark,
}

impl UiTheme {
    pub const ALL: &[Self] = &[Self::Light, Self::Dark];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Light => "Bright daylight workspace with subtle cool-gray framing.",
            Self::Dark => {
                "Low-glare charcoal studio workspace ideal for dim rooms and long sessions."
            }
        }
    }

    pub const fn toggled(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Light,
        }
    }
}

/// Concrete canvas paper background, ruling line, and margin colors for rendering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedPaperColors {
    pub background: Color,
    pub line: Color,
    pub margin: Color,
}

impl PaperColor {
    /// Resolves paper colors for workspace display, optionally adapting polarity and contrast
    /// to match `theme`. Canonical document colors remain untouched for export.
    #[must_use]
    pub fn resolved_colors(self, theme: UiTheme, auto_contrast: bool) -> ResolvedPaperColors {
        if !auto_contrast {
            return ResolvedPaperColors {
                background: self.background_color(),
                line: self.line_color(),
                margin: self.margin_color(),
            };
        }
        match (self, theme) {
            (Self::WarmCream, UiTheme::Dark) => ResolvedPaperColors {
                background: Color::from_rgba8(0x1E, 0x1D, 0x1B, 255),
                line: Color::from_rgba8(0x34, 0x31, 0x2C, 255),
                margin: Color::from_rgba8(0x94, 0x4D, 0x4D, 216),
            },
            (Self::PureWhite, UiTheme::Dark) => ResolvedPaperColors {
                background: Color::from_rgba8(0x17, 0x19, 0x20, 255),
                line: Color::from_rgba8(0x2B, 0x2F, 0x3A, 255),
                margin: Color::from_rgba8(0x8E, 0x4A, 0x4A, 216),
            },
            (Self::DarkSlate, UiTheme::Light) => ResolvedPaperColors {
                background: Color::from_rgba8(0xF4, 0xF6, 0xFA, 255),
                line: Color::from_rgba8(0xD5, 0xDC, 0xE6, 255),
                margin: Color::from_rgba8(0xE0, 0x8B, 0x8B, 216),
            },
            _ => ResolvedPaperColors {
                background: self.background_color(),
                line: self.line_color(),
                margin: self.margin_color(),
            },
        }
    }

    /// Resolves an ink or highlighter color for workspace display against this paper color.
    #[must_use]
    pub fn resolve_ink_color(
        self,
        theme: UiTheme,
        auto_contrast: bool,
        color: Color,
        is_highlighter: bool,
    ) -> Color {
        if !auto_contrast {
            return color.sanitized();
        }
        let source_bg = self.background_color();
        let target_bg = self.resolved_colors(theme, true).background;
        drawcraft_color::adapt_color_for_background(color, source_bg, target_bg, is_highlighter)
    }
}

#[cfg(test)]
mod theme_contrast_tests {
    use super::*;

    #[test]
    fn paper_and_ink_unchanged_when_auto_contrast_disabled() {
        let green = Color::from_rgba8(0x1D, 0x7A, 0x46, 255);
        for &paper in PaperColor::ALL {
            for &theme in UiTheme::ALL {
                let resolved = paper.resolved_colors(theme, false);
                assert_eq!(resolved.background, paper.background_color());
                assert_eq!(resolved.line, paper.line_color());
                assert_eq!(resolved.margin, paper.margin_color());
                assert_eq!(paper.resolve_ink_color(theme, false, green, false), green);
            }
        }
    }

    #[test]
    fn auto_contrast_adapts_paper_and_green_ink_to_active_theme() {
        let warm = PaperColor::WarmCream.resolved_colors(UiTheme::Dark, true);
        assert!(drawcraft_color::relative_luminance(warm.background) < 0.05);

        let green = Color::from_rgba8(0x1D, 0x7A, 0x46, 255);
        let adapted = PaperColor::WarmCream.resolve_ink_color(UiTheme::Dark, true, green, false);
        assert!(drawcraft_color::contrast_ratio(adapted, warm.background) >= 6.49);
        assert!(adapted.g > adapted.r && adapted.g > adapted.b);

        let light_from_dark = PaperColor::DarkSlate.resolved_colors(UiTheme::Light, true);
        assert!(drawcraft_color::relative_luminance(light_from_dark.background) > 0.85);
    }

    #[test]
    fn user_studio_config_defaults_auto_contrast_off() {
        let cfg = UserStudioConfig::default();
        assert!(!cfg.auto_contrast_with_theme);
        assert!(cfg.sanitized().auto_contrast_with_theme == cfg.auto_contrast_with_theme);
    }
}

pub const MAX_STYLE_FAVORITES: usize = 8;
pub const MAX_SAVED_ELEMENTS: usize = 48;
pub const MAX_ELEMENT_NAME_LEN: usize = 48;
pub const MAX_ELEMENT_STROKES: usize = 128;
pub const MAX_ELEMENT_SHAPES: usize = 64;
pub const MAX_ELEMENT_TEXTS: usize = 16;
pub const MAX_ELEMENT_IMAGES: usize = 16;

/// Saved toolbar quick-access writing style combining tool, color, and stroke thickness.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleFavorite {
    pub id: String,
    pub kind: PenKind,
    pub color: Color,
    pub size: f32,
}

impl Default for StyleFavorite {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: PenKind::FountainPen,
            color: INK_PALETTE[1].color,
            size: 2.2,
        }
    }
}

impl StyleFavorite {
    pub fn sanitized(&self) -> Self {
        let size = if self.size.is_finite() {
            self.size.clamp(0.5, 120.0)
        } else {
            2.2
        };
        Self {
            id: sanitize_preset_id(&self.id),
            kind: self.kind,
            color: self.color.sanitized().with_alpha(1.0),
            size,
        }
    }
}

/// Reusable sticker, diagram, or stamp normalized with local origin `(0.0, 0.0)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedElement {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub built_in: bool,
    pub width: f64,
    pub height: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strokes: Vec<InkStroke>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapes: Vec<GeometricShape>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<TextItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<PageImage>,
}

impl Default for SavedElement {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "Element".to_string(),
            built_in: false,
            width: 120.0,
            height: 120.0,
            strokes: Vec::new(),
            shapes: Vec::new(),
            texts: Vec::new(),
            images: Vec::new(),
        }
    }
}

fn png_ihdr_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const PNG_SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != PNG_SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h))
}

fn builtin_image_sticker(id: &str, name: &str, size: f64, png_bytes: &[u8]) -> SavedElement {
    let (pixel_width, pixel_height) = png_ihdr_dimensions(png_bytes).unwrap_or((320, 320));
    SavedElement {
        id: id.to_string(),
        name: name.to_string(),
        built_in: true,
        width: size,
        height: size,
        strokes: Vec::new(),
        shapes: Vec::new(),
        texts: Vec::new(),
        images: vec![PageImage {
            x: 0.0,
            y: 0.0,
            width: size,
            height: size,
            angle_degrees: 0.0,
            pixel_width,
            pixel_height,
            png_bytes: Arc::new(png_bytes.to_vec()),
        }],
    }
}

impl SavedElement {
    pub fn sanitized(&self) -> Option<Self> {
        let mut strokes = Vec::with_capacity(self.strokes.len().min(MAX_ELEMENT_STROKES));
        for st in self.strokes.iter().take(MAX_ELEMENT_STROKES) {
            if st.smoothed_points.len() >= 2 {
                strokes.push(st.sanitized());
            }
        }
        let shapes: Vec<GeometricShape> = self
            .shapes
            .iter()
            .take(MAX_ELEMENT_SHAPES)
            .map(GeometricShape::sanitized)
            .collect();
        let texts: Vec<TextItem> = self
            .texts
            .iter()
            .take(MAX_ELEMENT_TEXTS)
            .map(TextItem::sanitized)
            .collect();
        let images: Vec<PageImage> = self
            .images
            .iter()
            .take(MAX_ELEMENT_IMAGES)
            .map(PageImage::sanitized)
            .filter(|im| !im.png_bytes.is_empty())
            .collect();
        if strokes.is_empty() && shapes.is_empty() && texts.is_empty() && images.is_empty() {
            return None;
        }
        Some(Self {
            id: sanitize_preset_id(&self.id),
            name: sanitize_trimmed_text(&self.name, MAX_ELEMENT_NAME_LEN, "Element"),
            built_in: self.built_in,
            width: if self.width.is_finite() {
                self.width.clamp(8.0, 2000.0)
            } else {
                120.0
            },
            height: if self.height.is_finite() {
                self.height.clamp(8.0, 2000.0)
            } else {
                120.0
            },
            strokes,
            shapes,
            texts,
            images,
        })
    }

    /// Built-in starter stickers available out of the box.
    pub fn built_ins() -> Vec<Self> {
        static BUILTIN_STICKERS: std::sync::OnceLock<Vec<SavedElement>> =
            std::sync::OnceLock::new();
        BUILTIN_STICKERS
            .get_or_init(|| {
                let specs: &[(&str, &str, f64, &[u8])] = &[
                    (
                        "builtin_reminder",
                        "Reminder",
                        164.0,
                        include_bytes!("../assets/stickers/reminder_note.png"),
                    ),
                    (
                        "builtin_todo",
                        "To Do",
                        164.0,
                        include_bytes!("../assets/stickers/todo_checklist.png"),
                    ),
                    (
                        "builtin_important",
                        "Important",
                        172.0,
                        include_bytes!("../assets/stickers/important_alert.png"),
                    ),
                    (
                        "builtin_done",
                        "Done!",
                        164.0,
                        include_bytes!("../assets/stickers/done_stamp.png"),
                    ),
                    (
                        "builtin_deadline",
                        "Deadline",
                        168.0,
                        include_bytes!("../assets/stickers/deadline_clock.png"),
                    ),
                    (
                        "builtin_key_note",
                        "Key Note",
                        164.0,
                        include_bytes!("../assets/stickers/key_note.png"),
                    ),
                    (
                        "builtin_question",
                        "Question?",
                        168.0,
                        include_bytes!("../assets/stickers/question_note.png"),
                    ),
                    (
                        "builtin_idea_bulb",
                        "Idea Bulb",
                        156.0,
                        include_bytes!("../assets/stickers/idea_bulb.png"),
                    ),
                    (
                        "builtin_gold_star",
                        "Gold Star",
                        156.0,
                        include_bytes!("../assets/stickers/gold_star.png"),
                    ),
                    (
                        "builtin_streak_fire",
                        "On Fire",
                        156.0,
                        include_bytes!("../assets/stickers/streak_fire.png"),
                    ),
                    (
                        "builtin_rocket_boost",
                        "Rocket",
                        168.0,
                        include_bytes!("../assets/stickers/rocket_boost.png"),
                    ),
                    (
                        "builtin_study_cat",
                        "Study Cat",
                        168.0,
                        include_bytes!("../assets/stickers/study_cat.png"),
                    ),
                    (
                        "builtin_coffee_cup",
                        "Coffee",
                        156.0,
                        include_bytes!("../assets/stickers/coffee_cup.png"),
                    ),
                    (
                        "builtin_desk_plant",
                        "Desk Plant",
                        164.0,
                        include_bytes!("../assets/stickers/desk_plant.png"),
                    ),
                    (
                        "builtin_cosmic_planet",
                        "Planet",
                        168.0,
                        include_bytes!("../assets/stickers/cosmic_planet.png"),
                    ),
                ];
                specs
                    .iter()
                    .map(|&(id, name, size, bytes)| builtin_image_sticker(id, name, size, bytes))
                    .collect()
            })
            .clone()
    }
}

/// Multi-object clipboard payload supporting copy/cut/paste across pages and notebooks.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PageSelectionClipboard {
    pub origin_x: f64,
    pub origin_y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strokes: Vec<InkStroke>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapes: Vec<GeometricShape>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<TextItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<PageImage>,
}

impl PageSelectionClipboard {
    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty()
            && self.shapes.is_empty()
            && self.texts.is_empty()
            && self.images.is_empty()
    }
}

fn default_ui_scale() -> f32 {
    UserStudioConfig::DEFAULT_UI_SCALE
}

fn default_pen_sizes() -> [f32; 5] {
    UserStudioConfig::DEFAULT_PEN_SIZES
}

fn default_wildcard_ink() -> Color {
    UserStudioConfig::default_wildcard_ink()
}

fn default_wildcard_highlighter() -> Color {
    UserStudioConfig::default_wildcard_highlighter()
}

fn deserialize_pen_sizes<'de, D>(deserializer: D) -> Result<[f32; 5], D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Vec::<f32>::deserialize(deserializer)?;
    let mut out = UserStudioConfig::DEFAULT_PEN_SIZES;
    for (idx, val) in raw.into_iter().take(5).enumerate() {
        out[idx] = val;
    }
    Ok(out)
}

/// Action bound to a connected stylus shortcut button or inverted eraser end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StylusButtonAction {
    None,
    #[default]
    EraserHold,
    ToggleEraser,
    LassoSelectHold,
    ToggleSelect,
    PanCanvasHold,
    HighlighterHold,
    ToggleHighlighter,
    Undo,
    Redo,
}

impl StylusButtonAction {
    pub const ALL: &[Self] = &[
        Self::EraserHold,
        Self::ToggleEraser,
        Self::LassoSelectHold,
        Self::ToggleSelect,
        Self::PanCanvasHold,
        Self::HighlighterHold,
        Self::ToggleHighlighter,
        Self::Undo,
        Self::Redo,
        Self::None,
    ];

    pub const ERASER_END_OPTIONS: &[Self] = &[
        Self::EraserHold,
        Self::LassoSelectHold,
        Self::PanCanvasHold,
        Self::HighlighterHold,
        Self::None,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "Disabled",
            Self::EraserHold => "Eraser (Hold)",
            Self::ToggleEraser => "Toggle Pen / Eraser",
            Self::LassoSelectHold => "Lasso Select (Hold)",
            Self::ToggleSelect => "Toggle Lasso Select",
            Self::PanCanvasHold => "Pan Canvas (Hold)",
            Self::HighlighterHold => "Highlighter (Hold)",
            Self::ToggleHighlighter => "Toggle Pen / Highlighter",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::None => "Disables this input so pressing it triggers no shortcut action.",
            Self::EraserHold => {
                "Temporarily switches to the Eraser while held and returns to your active pen on release."
            }
            Self::ToggleEraser => {
                "Switches between your active pen and the Eraser each time you press the button."
            }
            Self::LassoSelectHold => {
                "Temporarily activates the Freeform Lasso tool while held so you can circle and move strokes."
            }
            Self::ToggleSelect => {
                "Switches between your active pen and the Freeform Lasso selection tool each time you press."
            }
            Self::PanCanvasHold => {
                "Pans and drags the page while held without drawing ink, returning to writing on release."
            }
            Self::HighlighterHold => {
                "Temporarily switches to the Highlighter while held and restores your active pen on release."
            }
            Self::ToggleHighlighter => {
                "Switches between your active pen and the Highlighter each time you press the button."
            }
            Self::Undo => {
                "Reverts the most recent stroke or page edit each time the button is pressed."
            }
            Self::Redo => {
                "Re-applies the last undone stroke or page edit each time the button is pressed."
            }
        }
    }

    pub const fn is_hold_action(self) -> bool {
        matches!(
            self,
            Self::EraserHold | Self::LassoSelectHold | Self::PanCanvasHold | Self::HighlighterHold
        )
    }

    pub const fn supports_eraser_end(self) -> bool {
        self.is_hold_action() || matches!(self, Self::None)
    }

    pub const fn sanitized_for_eraser_end(self) -> Self {
        if self.supports_eraser_end() {
            self
        } else {
            Self::EraserHold
        }
    }
}

fn default_barrel_button_action() -> StylusButtonAction {
    // One click on the pencil button (Apple Pencil Pro / side button) flips between pen and eraser.
    StylusButtonAction::ToggleEraser
}

fn default_secondary_barrel_action() -> StylusButtonAction {
    StylusButtonAction::PanCanvasHold
}

fn default_eraser_end_action() -> StylusButtonAction {
    StylusButtonAction::EraserHold
}

/// Persistent shortcut assignments for connected stylus side buttons and inverted eraser end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct StylusButtonConfig {
    #[serde(default = "default_barrel_button_action")]
    pub barrel_button: StylusButtonAction,
    #[serde(default = "default_secondary_barrel_action")]
    pub secondary_barrel_button: StylusButtonAction,
    #[serde(default = "default_eraser_end_action")]
    pub eraser_end: StylusButtonAction,
}

impl Default for StylusButtonConfig {
    fn default() -> Self {
        Self {
            barrel_button: default_barrel_button_action(),
            secondary_barrel_button: default_secondary_barrel_action(),
            eraser_end: default_eraser_end_action(),
        }
    }
}

impl StylusButtonConfig {
    #[must_use]
    pub const fn sanitized(&self) -> Self {
        Self {
            barrel_button: self.barrel_button,
            secondary_barrel_button: self.secondary_barrel_button,
            eraser_end: self.eraser_end.sanitized_for_eraser_end(),
        }
    }
}

/// Persistent studio configuration holding theme, UI scale, palettes, pen sizes, library layout,
/// user pen presets, saved custom paper templates, toolbar style favorites, stylus shortcuts, and reusable elements.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserStudioConfig {
    pub theme: UiTheme,
    pub auto_contrast_with_theme: bool,
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    /// Last selected stroke size for fountain, ball, brush, highlighter, and pencil tools.
    #[serde(
        default = "default_pen_sizes",
        deserialize_with = "deserialize_pen_sizes"
    )]
    pub pen_sizes: [f32; 5],
    pub ink_palette: Vec<Color>,
    pub highlighter_palette: Vec<Color>,
    #[serde(default = "default_wildcard_ink")]
    pub wildcard_ink: Color,
    #[serde(default = "default_wildcard_highlighter")]
    pub wildcard_highlighter: Color,
    #[serde(default)]
    pub palette_presets: Vec<ColorPalettePreset>,
    #[serde(default)]
    pub active_palette_id: String,
    pub custom_presets: Vec<CustomPenPreset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library_folders: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub library_folder_colors: BTreeMap<String, Color>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub library_folder_icons: BTreeMap<String, String>,
    /// Folders the user starred; they also appear under Favorites.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library_folder_favorites: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub library_tag_colors: BTreeMap<String, Color>,
    #[serde(default)]
    pub library_view_mode: LibraryViewMode,
    #[serde(default)]
    pub library_sort_by: LibrarySortBy,
    #[serde(default)]
    pub library_sort_ascending: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_templates: Vec<CustomPaperTemplate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub style_favorites: Vec<StyleFavorite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub saved_elements: Vec<SavedElement>,
    #[serde(default)]
    pub stylus_buttons: StylusButtonConfig,
    /// Mirrors the page and tool drawers so a left-handed palm never reaches the toolbar.
    #[serde(default, skip_serializing_if = "is_false")]
    pub left_handed: bool,
    /// `false` (default) is Pencil-only: the pencil draws and fingers pan/zoom. `true` lets a single finger draw too.
    #[serde(default, skip_serializing_if = "is_false")]
    pub finger_drawing: bool,
    /// Invert PDF page lightness for dark reading.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pdf_dark_mode: bool,
}

impl Default for UserStudioConfig {
    fn default() -> Self {
        Self {
            theme: UiTheme::default(),
            auto_contrast_with_theme: false,
            ui_scale: Self::DEFAULT_UI_SCALE,
            pen_sizes: Self::DEFAULT_PEN_SIZES,
            ink_palette: Self::default_ink_palette(),
            highlighter_palette: Self::default_highlighter_palette(),
            wildcard_ink: Self::default_wildcard_ink(),
            wildcard_highlighter: Self::default_wildcard_highlighter(),
            palette_presets: ColorPalettePreset::built_ins(),
            active_palette_id: BUILTIN_PALETTE_CLASSIC_ID.to_string(),
            custom_presets: Vec::new(),
            library_folders: Vec::new(),
            library_folder_colors: BTreeMap::new(),
            library_folder_icons: BTreeMap::new(),
            library_folder_favorites: Vec::new(),
            library_tag_colors: BTreeMap::new(),
            library_view_mode: LibraryViewMode::Grid,
            library_sort_by: LibrarySortBy::ModifiedDate,
            library_sort_ascending: false,
            custom_templates: Vec::new(),
            style_favorites: Vec::new(),
            saved_elements: Vec::new(),
            stylus_buttons: StylusButtonConfig::default(),
            left_handed: false,
            finger_drawing: false,
            pdf_dark_mode: false,
        }
    }
}

impl UserStudioConfig {
    pub const DEFAULT_PEN_SIZES: [f32; 5] = [2.2, 2.2, 2.2, 14.0, 2.8];
    pub const DEFAULT_UI_SCALE: f32 = 1.0;
    pub const MIN_UI_SCALE: f32 = 0.5;
    pub const MAX_UI_SCALE: f32 = 2.5;

    /// Clamps and rounds `scale` to a safe, deterministic percentage step (`0.50..=2.50`).
    #[must_use]
    pub fn sanitize_ui_scale(scale: f32) -> f32 {
        if scale.is_finite() {
            let clamped = scale.clamp(Self::MIN_UI_SCALE, Self::MAX_UI_SCALE);
            ((clamped * 100.0).round() / 100.0).clamp(Self::MIN_UI_SCALE, Self::MAX_UI_SCALE)
        } else {
            Self::DEFAULT_UI_SCALE
        }
    }

    pub fn default_ink_palette() -> Vec<Color> {
        INK_PALETTE.iter().map(|sw| sw.color).collect()
    }

    pub fn default_highlighter_palette() -> Vec<Color> {
        HIGHLIGHTER_PALETTE.iter().map(|sw| sw.color).collect()
    }

    pub fn default_wildcard_ink() -> Color {
        Color::from_rgba8(0xE1, 0x1D, 0x48, 0xFF)
    }

    pub fn default_wildcard_highlighter() -> Color {
        Color::from_rgba8(0xFF, 0xAB, 0x40, 0xFF)
    }

    #[must_use]
    pub fn wildcard_color(&self, target: PaletteTarget) -> Color {
        match target {
            PaletteTarget::Ink => self.wildcard_ink,
            PaletteTarget::Highlighter => self.wildcard_highlighter,
        }
    }

    pub fn set_wildcard_color(&mut self, target: PaletteTarget, color: Color) {
        let clean = color.sanitized().with_alpha(1.0);
        match target {
            PaletteTarget::Ink => self.wildcard_ink = clean,
            PaletteTarget::Highlighter => self.wildcard_highlighter = clean,
        }
    }

    pub fn palette(&self, target: PaletteTarget) -> &[Color] {
        match target {
            PaletteTarget::Ink => &self.ink_palette,
            PaletteTarget::Highlighter => &self.highlighter_palette,
        }
    }

    pub fn palette_mut(&mut self, target: PaletteTarget) -> &mut Vec<Color> {
        match target {
            PaletteTarget::Ink => &mut self.ink_palette,
            PaletteTarget::Highlighter => &mut self.highlighter_palette,
        }
    }

    pub fn palette_by_id(&self, id: &str) -> Option<&ColorPalettePreset> {
        self.palette_presets.iter().find(|p| p.id == id)
    }

    pub fn palette_by_id_mut(&mut self, id: &str) -> Option<&mut ColorPalettePreset> {
        self.palette_presets.iter_mut().find(|p| p.id == id)
    }

    pub fn active_palette(&self) -> Option<&ColorPalettePreset> {
        self.palette_by_id(&self.active_palette_id)
            .or_else(|| self.palette_presets.first())
    }

    pub fn active_palette_mut(&mut self) -> Option<&mut ColorPalettePreset> {
        let target_id = self.active_palette_id.clone();
        if let Some(pos) = self.palette_presets.iter().position(|p| p.id == target_id) {
            self.palette_presets.get_mut(pos)
        } else {
            self.palette_presets.first_mut()
        }
    }

    /// Synchronizes quick-access `ink_palette` / `highlighter_palette` from the active palette group.
    pub fn sync_active_palettes(&mut self) {
        if let Some(active) = self.active_palette() {
            let ink = active.ink_colors.clone();
            let hi = active.highlighter_colors.clone();
            self.ink_palette = ink;
            self.highlighter_palette = hi;
        }
    }

    fn normalize_folder_key(raw: &str) -> Option<String> {
        let clean = sanitize_folder_path(raw);
        (!clean.is_empty()).then_some(clean)
    }

    fn lookup_keyed_color<F>(
        map: &BTreeMap<String, Color>,
        raw_key: &str,
        normalize: F,
    ) -> Option<Color>
    where
        F: Fn(&str) -> Option<String>,
    {
        let clean = normalize(raw_key)?;
        let key = clean.to_ascii_lowercase();
        map.get(&key)
            .or_else(|| {
                map.iter()
                    .find(|(k, _)| normalize(k).is_some_and(|c| c.eq_ignore_ascii_case(&clean)))
                    .map(|(_, c)| c)
            })
            .map(|c| c.sanitized().with_alpha(1.0))
    }

    fn set_keyed_color<F>(
        map: &mut BTreeMap<String, Color>,
        raw_key: &str,
        color: Option<Color>,
        max_entries: usize,
        normalize: F,
    ) -> bool
    where
        F: Fn(&str) -> Option<String> + Copy,
    {
        let Some(clean) = normalize(raw_key) else {
            return false;
        };
        let prev = Self::lookup_keyed_color(map, &clean, normalize);
        let key = clean.to_ascii_lowercase();
        map.retain(|k, _| !normalize(k).is_some_and(|c| c.eq_ignore_ascii_case(&clean)));
        match color {
            Some(raw) => {
                if map.len() >= max_entries {
                    if let Some(old) = prev {
                        map.insert(key, old);
                    }
                    return false;
                }
                let next = raw.sanitized().with_alpha(1.0);
                map.insert(key, next);
                prev != Some(next)
            }
            None => prev.is_some(),
        }
    }

    /// Returns the custom color assigned to `path` (case-insensitively), if any.
    #[must_use]
    pub fn folder_color(&self, path: &str) -> Option<Color> {
        Self::lookup_keyed_color(
            &self.library_folder_colors,
            path,
            Self::normalize_folder_key,
        )
    }

    /// Assigns or clears a custom color on `path`. Returns `true` when the mapping changed.
    pub fn set_folder_color(&mut self, path: &str, color: Option<Color>) -> bool {
        Self::set_keyed_color(
            &mut self.library_folder_colors,
            path,
            color,
            MAX_LIBRARY_FOLDERS,
            Self::normalize_folder_key,
        )
    }

    /// Returns the custom icon key assigned to `path` (case-insensitively), if any.
    #[must_use]
    pub fn folder_icon(&self, path: &str) -> Option<&str> {
        let key = Self::normalize_folder_key(path)?.to_ascii_lowercase();
        self.library_folder_icons.get(&key).map(String::as_str)
    }

    /// Assigns or clears a custom icon key on `path`. Returns `true` when the mapping changed.
    pub fn set_folder_icon(&mut self, path: &str, icon: Option<&str>) -> bool {
        let Some(key) = Self::normalize_folder_key(path).map(|s| s.to_ascii_lowercase()) else {
            return false;
        };
        let prev = self.library_folder_icons.remove(&key);
        let next = icon
            .map(|raw| {
                raw.trim()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .take(32)
                    .collect::<String>()
                    .to_ascii_lowercase()
            })
            .filter(|s| !s.is_empty());
        match next {
            Some(clean) => {
                if self.library_folder_icons.len() >= MAX_LIBRARY_FOLDERS {
                    if let Some(old) = prev {
                        self.library_folder_icons.insert(key, old);
                    }
                    return false;
                }
                let changed = prev.as_deref() != Some(clean.as_str());
                self.library_folder_icons.insert(key, clean);
                changed
            }
            None => prev.is_some(),
        }
    }

    /// Moves folder icon mappings from `old_path` (and its subfolders) to `new_path`.
    pub fn remap_folder_icons(&mut self, old_path: &str, new_path: &str) {
        let old_clean = sanitize_folder_path(old_path);
        let new_clean = sanitize_folder_path(new_path);
        if old_clean.is_empty()
            || new_clean.is_empty()
            || old_clean.eq_ignore_ascii_case(&new_clean)
        {
            return;
        }
        let mut kept = BTreeMap::new();
        let mut moved = BTreeMap::new();
        for (raw_key, icon) in &self.library_folder_icons {
            let clean_key = sanitize_folder_path(raw_key);
            if clean_key.is_empty() {
                continue;
            }
            if let Some(suffix) = folder_path_suffix(&clean_key, &old_clean) {
                let next_key = if suffix.is_empty() {
                    new_clean.to_ascii_lowercase()
                } else {
                    format!("{new_clean}/{suffix}").to_ascii_lowercase()
                };
                moved.insert(next_key, icon.clone());
            } else {
                kept.insert(clean_key.to_ascii_lowercase(), icon.clone());
            }
        }
        kept.extend(moved);
        self.library_folder_icons = sanitize_library_folder_icons(&kept);
    }

    /// Whether `path` is starred (case-insensitive).
    #[must_use]
    pub fn is_folder_favorite(&self, path: &str) -> bool {
        let clean = sanitize_folder_path(path);
        !clean.is_empty()
            && self
                .library_folder_favorites
                .iter()
                .any(|f| f.eq_ignore_ascii_case(&clean))
    }

    /// Stars or unstars `path`. Returns `true` when the set changed.
    pub fn set_folder_favorite(&mut self, path: &str, favorite: bool) -> bool {
        let clean = sanitize_folder_path(path);
        if clean.is_empty() {
            return false;
        }
        let was = self.is_folder_favorite(&clean);
        if favorite == was {
            return false;
        }
        if favorite {
            if self.library_folder_favorites.len() >= MAX_LIBRARY_FOLDERS {
                return false;
            }
            self.library_folder_favorites.push(clean);
        } else {
            self.library_folder_favorites
                .retain(|f| !f.eq_ignore_ascii_case(&clean));
        }
        true
    }

    /// Re-points starred folders when `old_path` (and its subfolders) is renamed to `new_path`.
    pub fn remap_folder_favorites(&mut self, old_path: &str, new_path: &str) {
        let old_clean = sanitize_folder_path(old_path);
        let new_clean = sanitize_folder_path(new_path);
        if old_clean.is_empty()
            || new_clean.is_empty()
            || old_clean.eq_ignore_ascii_case(&new_clean)
        {
            return;
        }
        let remapped: Vec<String> = self
            .library_folder_favorites
            .iter()
            .map(|f| match folder_path_suffix(f, &old_clean) {
                Some(suffix) if suffix.is_empty() => new_clean.clone(),
                Some(suffix) => format!("{new_clean}/{suffix}"),
                None => f.clone(),
            })
            .collect();
        self.library_folder_favorites = sanitize_library_folders(&remapped);
    }

    /// Unstars `target_path` and any nested subfolders inside it.
    pub fn remove_folder_favorites(&mut self, target_path: &str) {
        let clean = sanitize_folder_path(target_path);
        if clean.is_empty() {
            return;
        }
        self.library_folder_favorites
            .retain(|f| folder_path_suffix(f, &clean).is_none());
    }

    /// Removes icon mappings for `target_path` and any nested subfolders inside it.
    pub fn remove_folder_icons(&mut self, target_path: &str) {
        let clean = sanitize_folder_path(target_path);
        if clean.is_empty() {
            return;
        }
        self.library_folder_icons
            .retain(|k, _| folder_path_suffix(&sanitize_folder_path(k), &clean).is_none());
    }

    /// Returns the custom color assigned to `tag` (case-insensitively), if any.
    #[must_use]
    pub fn tag_color(&self, tag: &str) -> Option<Color> {
        Self::lookup_keyed_color(&self.library_tag_colors, tag, sanitize_notebook_tag)
    }

    /// Assigns or clears a custom color on `tag`. Returns `true` when the mapping changed.
    pub fn set_tag_color(&mut self, tag: &str, color: Option<Color>) -> bool {
        Self::set_keyed_color(
            &mut self.library_tag_colors,
            tag,
            color,
            MAX_LIBRARY_TAG_COLORS,
            sanitize_notebook_tag,
        )
    }

    /// Moves folder color mappings from `old_path` (and its subfolders) to `new_path`.
    pub fn remap_folder_colors(&mut self, old_path: &str, new_path: &str) {
        let old_clean = sanitize_folder_path(old_path);
        let new_clean = sanitize_folder_path(new_path);
        if old_clean.is_empty()
            || new_clean.is_empty()
            || old_clean.eq_ignore_ascii_case(&new_clean)
        {
            return;
        }
        let mut kept = BTreeMap::new();
        let mut moved = BTreeMap::new();
        for (raw_key, color) in &self.library_folder_colors {
            let clean_key = sanitize_folder_path(raw_key);
            if clean_key.is_empty() {
                continue;
            }
            let clean_color = color.sanitized().with_alpha(1.0);
            if let Some(suffix) = folder_path_suffix(&clean_key, &old_clean) {
                let next_key = if suffix.is_empty() {
                    new_clean.to_ascii_lowercase()
                } else {
                    format!("{new_clean}/{suffix}").to_ascii_lowercase()
                };
                moved.insert(next_key, clean_color);
            } else {
                kept.insert(clean_key.to_ascii_lowercase(), clean_color);
            }
        }
        kept.extend(moved);
        self.library_folder_colors = sanitize_library_folder_colors(&kept);
    }

    /// Removes color mappings for `target_path` and any nested subfolders inside it.
    pub fn remove_folder_colors(&mut self, target_path: &str) {
        let clean = sanitize_folder_path(target_path);
        if clean.is_empty() {
            return;
        }
        self.library_folder_colors
            .retain(|k, _| folder_path_suffix(&sanitize_folder_path(k), &clean).is_none());
    }

    /// Returns a sanitized copy with bounded palettes, unique preset IDs, and clean settings.
    pub fn sanitized(&self) -> Self {
        let mut clean_ink = sanitize_swatch_list(&self.ink_palette, Self::default_ink_palette);
        if is_legacy_unmodified_builtin_ink(BUILTIN_PALETTE_CLASSIC_ID, &clean_ink) {
            clean_ink = Self::default_ink_palette();
        }
        let clean_hi =
            sanitize_swatch_list(&self.highlighter_palette, Self::default_highlighter_palette);
        let mut pen_sizes = Self::DEFAULT_PEN_SIZES;
        for (index, raw_size) in self.pen_sizes.iter().enumerate() {
            if let Some(clean_size) = pen_sizes.get_mut(index) {
                let fallback = Self::DEFAULT_PEN_SIZES.get(index).copied().unwrap_or(2.2);
                *clean_size = if raw_size.is_finite() {
                    raw_size.clamp(0.5, 120.0)
                } else {
                    fallback
                };
            }
        }

        let had_explicit_presets = !self.palette_presets.is_empty();
        let mut built_in_slots: [Option<ColorPalettePreset>; 3] = [None, None, None];
        let mut custom_list: Vec<ColorPalettePreset> = Vec::new();
        let mut pal_ids: std::collections::HashSet<String> = [
            BUILTIN_PALETTE_CLASSIC_ID.to_string(),
            BUILTIN_PALETTE_PASTEL_ID.to_string(),
            BUILTIN_PALETTE_EARTH_ID.to_string(),
        ]
        .into_iter()
        .collect();
        let mut next_pal_seq = 1usize;

        for raw_pal in self.palette_presets.iter().take(MAX_PALETTE_PRESETS) {
            let mut pal = raw_pal.sanitized();
            if let Some(slot_idx) = builtin_palette_slot(&pal.id) {
                if built_in_slots[slot_idx].is_none() {
                    built_in_slots[slot_idx] = Some(pal);
                }
                continue;
            }
            pal.built_in = false;
            if pal.id.is_empty() || !pal_ids.insert(pal.id.clone()) {
                loop {
                    let candidate = format!("palette_{next_pal_seq}");
                    next_pal_seq = next_pal_seq.saturating_add(1);
                    if pal_ids.insert(candidate.clone()) {
                        pal.id = candidate;
                        break;
                    }
                }
            }
            custom_list.push(pal);
        }

        let mut palette_presets = Vec::with_capacity(MAX_PALETTE_PRESETS);
        palette_presets.push(
            built_in_slots[0]
                .take()
                .unwrap_or_else(ColorPalettePreset::builtin_classic),
        );
        palette_presets.push(
            built_in_slots[1]
                .take()
                .unwrap_or_else(ColorPalettePreset::builtin_pastel),
        );
        palette_presets.push(
            built_in_slots[2]
                .take()
                .unwrap_or_else(ColorPalettePreset::builtin_earth),
        );
        for custom in custom_list {
            if palette_presets.len() >= MAX_PALETTE_PRESETS {
                break;
            }
            palette_presets.push(custom);
        }

        let mut active_palette_id = sanitize_preset_id(&self.active_palette_id);
        let mut migrated_legacy = false;
        if !had_explicit_presets
            && (clean_ink != Self::default_ink_palette()
                || clean_hi != Self::default_highlighter_palette())
            && palette_presets.len() < MAX_PALETTE_PRESETS
        {
            loop {
                let candidate = format!("palette_{next_pal_seq}");
                next_pal_seq = next_pal_seq.saturating_add(1);
                if pal_ids.insert(candidate.clone()) {
                    palette_presets.push(ColorPalettePreset {
                        id: candidate.clone(),
                        name: "Custom Palette".to_string(),
                        built_in: false,
                        ink_colors: clean_ink.clone(),
                        highlighter_colors: clean_hi.clone(),
                    });
                    active_palette_id = candidate;
                    migrated_legacy = true;
                    break;
                }
            }
        }

        if !palette_presets.iter().any(|p| p.id == active_palette_id) {
            active_palette_id = BUILTIN_PALETTE_CLASSIC_ID.to_string();
        }

        let (synced_ink, synced_hi) = if had_explicit_presets || migrated_legacy {
            if let Some(active) = palette_presets.iter().find(|p| p.id == active_palette_id) {
                (active.ink_colors.clone(), active.highlighter_colors.clone())
            } else {
                (clean_ink, clean_hi)
            }
        } else {
            (clean_ink, clean_hi)
        };

        let mut used_ids = std::collections::HashSet::new();
        let mut next_seq = 1usize;
        let mut presets = Vec::with_capacity(self.custom_presets.len().min(MAX_CUSTOM_PRESETS));
        for raw_preset in self.custom_presets.iter().take(MAX_CUSTOM_PRESETS) {
            let mut preset = raw_preset.sanitized();
            if preset.id.is_empty() || !used_ids.insert(preset.id.clone()) {
                loop {
                    let candidate = format!("custom_{next_seq}");
                    next_seq = next_seq.saturating_add(1);
                    if used_ids.insert(candidate.clone()) {
                        preset.id = candidate;
                        break;
                    }
                }
            }
            presets.push(preset);
        }

        let library_folders = sanitize_library_folders(&self.library_folders);
        let library_folder_colors = sanitize_library_folder_colors(&self.library_folder_colors);
        let library_folder_icons = sanitize_library_folder_icons(&self.library_folder_icons);
        let library_folder_favorites = sanitize_library_folders(&self.library_folder_favorites);
        let library_tag_colors = sanitize_library_tag_colors(&self.library_tag_colors);
        let mut tpl_ids = std::collections::HashSet::new();
        let mut next_tpl_seq = 1usize;
        let mut custom_templates =
            Vec::with_capacity(self.custom_templates.len().min(MAX_CUSTOM_TEMPLATES));
        let mut total_tpl_bytes = 0usize;
        const MAX_TOTAL_TEMPLATE_BYTES: usize = 12 * 1024 * 1024;
        for raw_tpl in self.custom_templates.iter().take(MAX_CUSTOM_TEMPLATES) {
            let Some(mut tpl) = raw_tpl.sanitized() else {
                continue;
            };
            let next_bytes = total_tpl_bytes.saturating_add(tpl.bytes.len());
            if next_bytes > MAX_TOTAL_TEMPLATE_BYTES {
                continue;
            }
            total_tpl_bytes = next_bytes;
            if tpl.id.is_empty() || !tpl_ids.insert(tpl.id.clone()) {
                loop {
                    let candidate = format!("tpl_{next_tpl_seq}");
                    next_tpl_seq = next_tpl_seq.saturating_add(1);
                    if tpl_ids.insert(candidate.clone()) {
                        tpl.id = candidate;
                        break;
                    }
                }
            }
            custom_templates.push(tpl);
        }

        let style_favorites = sanitize_style_favorites(&self.style_favorites);
        let saved_elements = sanitize_saved_elements(&self.saved_elements);

        Self {
            theme: self.theme,
            auto_contrast_with_theme: self.auto_contrast_with_theme,
            ui_scale: Self::sanitize_ui_scale(self.ui_scale),
            pen_sizes,
            ink_palette: synced_ink,
            highlighter_palette: synced_hi,
            wildcard_ink: self.wildcard_ink.sanitized().with_alpha(1.0),
            wildcard_highlighter: self.wildcard_highlighter.sanitized().with_alpha(1.0),
            palette_presets,
            active_palette_id,
            custom_presets: presets,
            library_folders,
            library_folder_colors,
            library_folder_icons,
            library_folder_favorites,
            library_tag_colors,
            library_view_mode: self.library_view_mode,
            library_sort_by: self.library_sort_by,
            library_sort_ascending: self.library_sort_ascending,
            custom_templates,
            style_favorites,
            saved_elements,
            stylus_buttons: self.stylus_buttons.sanitized(),
            left_handed: self.left_handed,
            finger_drawing: self.finger_drawing,
            pdf_dark_mode: self.pdf_dark_mode,
        }
    }
}

fn ensure_unique_item_id(
    id: &mut String,
    used: &mut std::collections::HashSet<String>,
    prefix: &str,
    seq: &mut usize,
) {
    if !id.is_empty() && used.insert(id.clone()) {
        return;
    }
    loop {
        let candidate = format!("{prefix}_{seq}");
        *seq = seq.saturating_add(1);
        if used.insert(candidate.clone()) {
            *id = candidate;
            return;
        }
    }
}

fn sanitize_style_favorites(raw: &[StyleFavorite]) -> Vec<StyleFavorite> {
    let mut used = std::collections::HashSet::new();
    let mut seq = 1usize;
    let mut out = Vec::with_capacity(raw.len().min(MAX_STYLE_FAVORITES));
    for item in raw.iter().take(MAX_STYLE_FAVORITES) {
        let mut clean = item.sanitized();
        ensure_unique_item_id(&mut clean.id, &mut used, "fav", &mut seq);
        out.push(clean);
    }
    out
}

fn sanitize_saved_elements(raw: &[SavedElement]) -> Vec<SavedElement> {
    let mut used = std::collections::HashSet::new();
    let mut seq = 1usize;
    let mut out = Vec::with_capacity(raw.len().min(MAX_SAVED_ELEMENTS));
    for item in raw.iter().take(MAX_SAVED_ELEMENTS) {
        let Some(mut clean) = item.sanitized() else {
            continue;
        };
        clean.built_in = false;
        ensure_unique_item_id(&mut clean.id, &mut used, "el", &mut seq);
        out.push(clean);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn pressure_curve_monotone_hermite_stays_within_unit_bounds() {
        for preset in PressureCurvePreset::ALL {
            let curve = PressureCurve::from_preset(*preset);
            let mut prev = -0.1_f32;
            for i in 0..=100 {
                let x = i as f32 / 100.0;
                let y = curve.eval(x);
                assert!((0.0..=1.0).contains(&y), "{preset:?} at {x}: {y}");
                assert!(
                    y + 1e-5 >= prev,
                    "{preset:?} non-monotone at {x}: {y} < {prev}"
                );
                prev = y;
            }
        }
    }

    #[test]
    fn pressure_curve_handles_hostile_points_without_panicking() {
        let c = PressureCurve::new(vec![
            [f32::NAN, 0.5],
            [0.3, 2.0],
            [0.3, -1.0],
            [f32::INFINITY, 0.1],
        ]);
        let y = c.eval(0.5);
        assert!(y.is_finite() && (0.0..=1.0).contains(&y));
        assert!(c.eval(f32::NAN).is_finite());
    }

    #[test]
    fn document_snapshot_clone_shares_stroke_arcs() {
        let mut doc = Document::default();
        let id = doc.alloc_stroke_id();
        let stroke = Arc::new(InkStroke::new(
            id,
            PenSettings::fountain_pen(),
            vec![StrokeSample::from_xy_pressure(10.0, 20.0, 0.5)],
            vec![StrokeSample::from_xy_pressure(10.0, 20.0, 0.5)],
        ));
        doc.strokes.push(stroke);
        let snap = doc.clone();
        assert_eq!(Arc::strong_count(&doc.strokes[0]), 2);
        assert_eq!(snap.strokes.len(), 1);
    }

    #[test]
    fn input_stabilization_config_defaults_and_sanitization() {
        assert_eq!(
            BufferAveraging::ALL,
            &[
                BufferAveraging::None,
                BufferAveraging::ArithmeticMean,
                BufferAveraging::VelocityGaussian
            ]
        );
        assert_eq!(BufferAveraging::None.label(), "None");
        assert_eq!(BufferAveraging::ArithmeticMean.label(), "Arithmetic mean");
        assert_eq!(
            BufferAveraging::VelocityGaussian.label(),
            "Velocity based Gaussian weights"
        );
        assert_eq!(
            BufferPreprocessor::ALL,
            &[
                BufferPreprocessor::None,
                BufferPreprocessor::Deadzone,
                BufferPreprocessor::Inertia
            ]
        );
        let def_sm = SmoothingConfig::default();
        assert_eq!(def_sm.stabilizer_mode, StabilizerMode::CircularBuffer);
        assert_eq!(def_sm.buffer_averaging, BufferAveraging::ArithmeticMean);
        assert_eq!(def_sm.buffer_preprocessor, BufferPreprocessor::None);
        assert_eq!(def_sm.buffer_size, 10);
        assert_eq!(def_sm.preset(), StabilizationPreset::ArithmeticBuffer10);
        assert!((def_sm.buffer_sigma - 0.5).abs() < 1e-6);
        assert!((def_sm.buffer_deadzone_radius - 1.3).abs() < 1e-6);
        assert!(def_sm.buffer_cusp_detection);
        assert!((def_sm.buffer_drag - 0.4).abs() < 1e-6);
        assert!((def_sm.buffer_mass - 5.0).abs() < 1e-6);
        assert!(def_sm.buffer_finalize_stroke);
        assert!(def_sm.is_buffer_active());

        let hostile_sm = SmoothingConfig {
            buffer_size: 1,
            buffer_sigma: f32::NAN,
            buffer_deadzone_radius: 999.0,
            buffer_drag: -1.0,
            buffer_mass: 0.1,
            ..def_sm
        }
        .sanitized();
        assert_eq!(hostile_sm.buffer_size, 2);
        assert!((hostile_sm.buffer_sigma - 0.5).abs() < 1e-6);
        assert!((hostile_sm.buffer_deadzone_radius - 50.0).abs() < 1e-6);
        assert!((hostile_sm.buffer_drag - 0.0).abs() < 1e-6);
        assert!((hostile_sm.buffer_mass - 1.0).abs() < 1e-6);

        let inactive_sm = SmoothingConfig {
            buffer_averaging: BufferAveraging::None,
            buffer_preprocessor: BufferPreprocessor::None,
            ..def_sm
        };
        assert!(!inactive_sm.is_buffer_active());
        assert_eq!(inactive_sm.stabilization_summary(), "None");
    }

    #[test]
    fn stabilization_presets_roundtrip() {
        for &preset in StabilizationPreset::ALL {
            assert!(!preset.label().is_empty());
            assert!(!preset.badge().is_empty());
            assert!(!preset.description().is_empty());
            let sm = preset.to_smoothing_config().sanitized();
            assert_eq!(StabilizationPreset::from_smoothing_config(&sm), preset);
            assert_eq!(sm.preset(), preset);
        }
    }

    #[test]
    fn pen_presets_roundtrip_matching() {
        for &pp in PenPreset::ALL {
            let mut settings = pp.settings();
            settings.color = Color::from_rgba8(0x11, 0x22, 0x33, 0xFF);
            assert_eq!(PenPreset::matching(&settings), Some(pp));
        }
        for pen in [
            PenSettings::fountain_pen(),
            PenSettings::ball_pen(),
            PenSettings::brush_pen(),
            PenSettings::highlighter(),
        ] {
            assert_eq!(pen.smoothing, SmoothingConfig::default());
        }
        assert_eq!(
            PenPreset::matching(&PenSettings::fountain_pen()),
            Some(PenPreset::FountainEveryday)
        );
        assert_eq!(
            PenPreset::matching(&PenSettings::ball_pen()),
            Some(PenPreset::BallpointGel)
        );
        assert_eq!(
            PenPreset::matching(&PenSettings::brush_pen()),
            Some(PenPreset::BrushFude)
        );
        assert_eq!(
            PenPreset::matching(&PenSettings::highlighter()),
            Some(PenPreset::HighlighterBroad)
        );
    }

    #[test]
    fn notebook_multipage_lifecycle_and_presets() {
        let mut doc = NotebookPreset::LectureCornell.create_document();
        assert_eq!(doc.title, "Lecture Notes");
        assert_eq!(doc.page_count(), 1);
        assert_eq!(doc.paper_template, PaperTemplate::Cornell);

        let id1 = doc.alloc_stroke_id();
        doc.strokes.push(Arc::new(InkStroke::new(
            id1,
            PenSettings::fountain_pen(),
            vec![StrokeSample::from_xy_pressure(24.0, 48.0, 0.7)],
            vec![StrokeSample::from_xy_pressure(24.0, 48.0, 0.7)],
        )));

        let second = doc.add_page(Some(PaperTemplate::Grid), Some(PaperColor::PureWhite));
        assert_eq!(second, Some(1));
        assert_eq!(doc.page_count(), 2);
        assert_eq!(doc.active_page_index(), 1);
        assert_eq!(doc.paper_template, PaperTemplate::Grid);
        assert!(doc.strokes.is_empty());

        assert!(doc.select_page(0));
        assert_eq!(doc.paper_template, PaperTemplate::Cornell);
        assert_eq!(doc.strokes.len(), 1);

        let dup = doc.duplicate_active_page();
        assert_eq!(dup, Some(1));
        assert_eq!(doc.page_count(), 3);
        assert_eq!(doc.strokes.len(), 1);
        assert_ne!(doc.strokes[0].id, id1);

        assert!(!doc.select_page(99));
        assert!(doc.remove_page(1));
        assert!(doc.remove_page(1));
        assert!(!doc.remove_page(0));
        assert_eq!(doc.page_count(), 1);

        assert_eq!(sanitize_notebook_title("  \n\t  "), "Quick Notes");
        assert_eq!(sanitize_notebook_title("  Calculus II  "), "Calculus II");
    }

    #[test]
    fn user_studio_config_sanitizes_hostile_palettes_and_custom_presets() {
        let hostile = UserStudioConfig {
            theme: UiTheme::Dark,
            auto_contrast_with_theme: true,
            pen_sizes: UserStudioConfig::DEFAULT_PEN_SIZES,
            ink_palette: Vec::new(),
            highlighter_palette: vec![Color::new(f32::NAN, 2.0, -1.0, 0.2); 20],
            palette_presets: Vec::new(),
            active_palette_id: String::new(),
            custom_presets: vec![
                CustomPenPreset {
                    id: "  !!! ".to_string(),
                    name: " \n\t ".to_string(),
                    description: "Hello\x07World".to_string(),
                    settings: PenSettings {
                        size: f32::NAN,
                        opacity: 10.0,
                        ..PenSettings::fountain_pen()
                    },
                    include_color: true,
                },
                CustomPenPreset {
                    id: "custom_1".to_string(),
                    name: "Blue Ink".to_string(),
                    description: String::new(),
                    settings: PenSettings::ball_pen(),
                    include_color: false,
                },
            ],
            ..UserStudioConfig::default()
        };
        let clean = hostile.sanitized();
        assert_eq!(clean.ink_palette.len(), INK_PALETTE.len());
        assert_eq!(clean.highlighter_palette.len(), MAX_PALETTE_COLORS);
        assert_eq!(clean.highlighter_palette[0].a, 1.0);
        assert_eq!(clean.custom_presets.len(), 2);
        assert_eq!(clean.custom_presets[0].name, "Custom Preset");
        assert_eq!(clean.custom_presets[0].description, "HelloWorld");
        assert_ne!(clean.custom_presets[0].id, clean.custom_presets[1].id);
        assert!(clean.custom_presets[0].settings.size.is_finite());

        let current_color = Color::from_rgba8(0xD9, 0x2B, 0x2B, 0xFF);
        let applied_without_color =
            clean.custom_presets[1].to_applied_settings(current_color, None);
        assert_eq!(applied_without_color.color, current_color);
        assert!(clean.custom_presets[1].matches_pen(&applied_without_color));
    }

    #[test]
    fn studio_config_always_preserves_three_builtin_color_palettes() {
        let def = UserStudioConfig::default();
        assert_eq!(def.palette_presets.len(), 3);
        assert_eq!(def.palette_presets[0].id, BUILTIN_PALETTE_CLASSIC_ID);
        assert_eq!(def.palette_presets[1].id, BUILTIN_PALETTE_PASTEL_ID);
        assert_eq!(def.palette_presets[2].id, BUILTIN_PALETTE_EARTH_ID);
        assert!(def.palette_presets.iter().all(|p| p.built_in));
        assert_eq!(def.active_palette_id, BUILTIN_PALETTE_CLASSIC_ID);

        // Even if built-ins are stripped or a user preset claims built_in=true, sanitized restores all 3.
        let tampered = UserStudioConfig {
            palette_presets: vec![ColorPalettePreset {
                id: "my_palette".to_string(),
                name: " \n ".to_string(),
                built_in: true,
                ink_colors: Vec::new(),
                highlighter_colors: Vec::new(),
            }],
            active_palette_id: "missing_id".to_string(),
            ..UserStudioConfig::default()
        };
        let clean = tampered.sanitized();
        assert_eq!(clean.palette_presets.len(), 4);
        assert!(clean.palette_presets[0..3].iter().all(|p| p.built_in));
        assert!(!clean.palette_presets[3].built_in);
        assert_eq!(clean.palette_presets[3].name, "Custom Palette");
        assert_eq!(clean.active_palette_id, BUILTIN_PALETTE_CLASSIC_ID);
    }

    #[test]
    fn text_item_sanitizes_spacing_and_wraps_within_page_width() {
        let dirty = TextItem {
            text: "Hello\x07 world".to_string(),
            x: f64::NAN,
            y: f64::INFINITY,
            size: f32::NAN,
            angle_degrees: 370.0,
            color: Color::BLACK,
            font: TextFont::Sans,
            line_spacing: f32::NAN,
            letter_spacing: 999.0,
            max_width: Some(f32::NAN),
            link_url: None,
            ..TextItem::default()
        };
        let clean = dirty.sanitized();
        assert_eq!(clean.text, "Hello world");
        assert_eq!(clean.x, 0.0);
        assert_eq!(clean.y, 0.0);
        assert_eq!(clean.size, 24.0);
        assert_eq!(clean.line_spacing, DEFAULT_TEXT_LINE_SPACING);
        assert_eq!(clean.letter_spacing, 32.0);
        assert_eq!(clean.max_width, None);
        assert_eq!(clean.effective_max_width(800), 792.0);

        let low = TextItem {
            line_spacing: 0.1,
            letter_spacing: -50.0,
            max_width: Some(2.0),
            ..TextItem::default()
        }
        .sanitized();
        assert_eq!(low.line_spacing, 0.8);
        assert_eq!(low.letter_spacing, -4.0);
        assert_eq!(low.max_width, Some(MIN_TEXT_BOX_WIDTH));
        assert_eq!(low.effective_max_width(800), MIN_TEXT_BOX_WIDTH);

        let wrapped = wrap_text_lines("alpha beta gamma", 65.0, |_| 10.0);
        assert_eq!(wrapped, vec!["alpha ", "beta ", "gamma"]);

        let long_word = wrap_text_lines("abcdefghij", 35.0, |_| 10.0);
        assert_eq!(long_word, vec!["abc", "def", "ghi", "j"]);
        assert_eq!(text_max_page_width(800, 750.0), 42.0);
    }

    #[test]
    fn page_image_sanitizes_computes_rotated_bounds_and_base64_roundtrips() {
        let dirty = PageImage {
            x: f64::NAN,
            y: f64::INFINITY,
            width: 2.0,
            height: 20_000.0,
            angle_degrees: 450.0,
            pixel_width: 0,
            pixel_height: 99999,
            png_bytes: Arc::new(vec![137, 80, 78, 71, 13, 10, 26, 10]),
        };
        let clean = dirty.sanitized();
        assert_eq!(clean.x, 0.0);
        assert_eq!(clean.y, 0.0);
        assert_eq!(clean.width, 8.0);
        assert_eq!(clean.height, 8192.0);
        assert!((clean.angle_degrees - 90.0).abs() < 1e-9);
        assert_eq!(clean.pixel_width, 1);
        assert_eq!(clean.pixel_height, MAX_IMAGE_PIXEL_DIM);

        let rotated = PageImage {
            x: 100.0,
            y: 200.0,
            width: 100.0,
            height: 40.0,
            angle_degrees: 90.0,
            pixel_width: 4,
            pixel_height: 2,
            png_bytes: Arc::new(vec![1, 2, 3, 4, 5]),
        };
        let b = rotated.bounds();
        assert!((b.x - 130.0).abs() < 1e-6);
        assert!((b.right() - 170.0).abs() < 1e-6);
        assert!((b.y - 170.0).abs() < 1e-6);
        assert!((b.bottom() - 270.0).abs() < 1e-6);

        let raw = [137u8, 80, 78, 71, 13, 10, 26, 10];
        let encoded = base64_encode_bytes(&raw);
        let decoded = base64_decode_bytes(&encoded).expect("decode base64");
        assert_eq!(decoded, raw);

        let mut doc = Document::new(800, 600);
        doc.images.push(Arc::new(rotated.clone()));
        let duplicated = doc.duplicate_active_page();
        assert_eq!(duplicated, Some(1));
        assert_eq!(doc.page_count(), 2);
        assert_eq!(doc.pages[1].images.len(), 1);
        assert_eq!(doc.pages[1].images[0].as_ref(), &rotated);
    }

    #[test]
    fn custom_notebook_cover_parses_derives_distinct_spine_and_roundtrips() {
        let parsed = NotebookCover::from_hex("#2A78B4").expect("valid custom hex");
        assert_eq!(parsed, NotebookCover::custom_rgb(0x2A, 0x78, 0xB4));
        assert!(parsed.is_custom());
        assert_eq!(parsed.label(), "Custom");
        assert_eq!(parsed.display_label(), "Custom (#2A78B4)");
        assert_eq!(parsed.custom_rgb_channels(), Some([0x2A, 0x78, 0xB4]));
        assert_ne!(parsed.primary_color(), parsed.spine_color());
        assert_eq!(NotebookCover::from_hex("bad"), None);
        assert_eq!(NotebookCover::from_hex("#12345678"), None);

        // Even pure black and pure white covers keep a visible spine contrast.
        let black = NotebookCover::custom_rgb(0, 0, 0);
        assert_ne!(black.primary_color(), black.spine_color());
        let white = NotebookCover::custom_rgb(255, 255, 255);
        assert_ne!(white.primary_color(), white.spine_color());

        // Legacy preset JSON and custom cover JSON both round-trip cleanly.
        let legacy: NotebookCover = serde_json::from_str("\"ClassicNavy\"").expect("legacy preset");
        assert_eq!(legacy, NotebookCover::ClassicNavy);
        let json = serde_json::to_string(&parsed).expect("serialize custom cover");
        let restored: NotebookCover = serde_json::from_str(&json).expect("deserialize custom");
        assert_eq!(restored, parsed);
    }

    #[test]
    fn link_url_sanitization_and_inline_recognition_handle_valid_and_hostile_inputs() {
        assert_eq!(
            sanitize_link_url("https://example.com/docs?q=1"),
            Some("https://example.com/docs?q=1".to_string())
        );
        assert_eq!(
            sanitize_link_url("www.rust-lang.org"),
            Some("https://www.rust-lang.org".to_string())
        );
        assert_eq!(
            sanitize_link_url("docs.rs/egui"),
            Some("https://docs.rs/egui".to_string())
        );
        assert_eq!(
            sanitize_link_url("mailto:hello@example.com"),
            Some("mailto:hello@example.com".to_string())
        );
        assert_eq!(sanitize_link_url("javascript:alert(1)"), None);
        assert_eq!(sanitize_link_url("data:text/html,hi"), None);
        assert_eq!(sanitize_link_url("not a link"), None);

        let sample = "See (https://en.wikipedia.org/wiki/Rust_(programming_language)), and www.example.com! Avoid e.g. or v1.2.";
        let spans = find_text_url_spans(sample);
        assert_eq!(spans.len(), 2);
        assert_eq!(
            spans[0].url,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        assert_eq!(spans[1].url, "https://www.example.com");

        let attached = TextItem {
            text: "Reference manual".to_string(),
            link_url: Some("docs.rs".to_string()),
            ..TextItem::default()
        }
        .sanitized();
        assert_eq!(attached.link_url.as_deref(), Some("https://docs.rs"));
        assert_eq!(
            attached.primary_link_url().as_deref(),
            Some("https://docs.rs")
        );
        assert_eq!(attached.resolved_links().len(), 1);
    }

    #[test]
    fn embedded_pdfs_sanitize_prune_duplicate_and_roundtrip() {
        let mut doc = Document::new(1190, 1684);
        doc.pdf_documents.push(EmbeddedPdf {
            name: "Lecture.pdf".to_string(),
            bytes: Arc::new(b"%PDF-1.4\n%%EOF\n".to_vec()),
        });
        doc.pdf_documents.push(EmbeddedPdf {
            name: "Invalid.pdf".to_string(),
            bytes: Arc::new(b"not a pdf".to_vec()),
        });
        doc.pdf_backdrop = Some(PdfPageBackdrop {
            pdf_index: 0,
            page_index: 0,
        });
        doc.sync_active_to_pages();

        // Duplicate keeps the PDF backdrop and shares the same Arc payload.
        assert_eq!(doc.duplicate_active_page(), Some(1));
        assert_eq!(doc.page_count(), 2);
        assert_eq!(
            doc.pages[1].pdf_backdrop,
            Some(PdfPageBackdrop {
                pdf_index: 0,
                page_index: 0,
            })
        );

        // JSON round-trip strips the invalid PDF entry and keeps valid backdrops intact.
        let json = serde_json::to_string(&doc).expect("serialize doc with pdf");
        let mut decoded: Document = serde_json::from_str(&json).expect("deserialize doc with pdf");
        assert_eq!(decoded.pdf_documents.len(), 1);
        assert_eq!(decoded.pdf_documents[0].name, "Lecture.pdf");
        assert_eq!(
            decoded.pdf_backdrop,
            Some(PdfPageBackdrop {
                pdf_index: 0,
                page_index: 0,
            })
        );

        // Single-page snapshot brings only the referenced PDF.
        let page0 = decoded.document_for_page(0).expect("page 0 snapshot");
        assert_eq!(page0.pdf_documents.len(), 1);
        assert_eq!(
            page0.pdf_backdrop,
            Some(PdfPageBackdrop {
                pdf_index: 0,
                page_index: 0,
            })
        );

        // Removing all PDF-backed pages prunes the unused PDF attachment.
        assert!(decoded.add_page(None, None).is_some());
        assert!(decoded.remove_page(0));
        assert!(decoded.remove_page(0));
        assert_eq!(decoded.page_count(), 1);
        assert!(decoded.pdf_documents.is_empty());
        assert_eq!(decoded.pdf_backdrop, None);
    }

    #[test]
    fn paper_template_geometry_covers_new_templates_within_page_bounds() {
        assert_eq!(PaperTemplate::ALL.len(), 11);
        for &tpl in PaperTemplate::ALL {
            assert!(!tpl.label().is_empty());
            assert!(!tpl.description().is_empty());
            let geom = paper_template_geometry(1200, 1600, tpl);
            for line in &geom.lines {
                assert!((0.0..=1200.0).contains(&line.x1));
                assert!((0.0..=1200.0).contains(&line.x2));
                assert!((0.0..=1600.0).contains(&line.y1));
                assert!((0.0..=1600.0).contains(&line.y2));
            }
            match tpl {
                PaperTemplate::Blank => {
                    assert!(geom.lines.is_empty());
                }
                PaperTemplate::Dotted => {
                    assert!(!geom.dots.is_empty());
                }
                PaperTemplate::Isometric | PaperTemplate::MusicStaff => {
                    assert!(!geom.lines.is_empty());
                }
                PaperTemplate::WeeklyPlanner | PaperTemplate::MonthlyPlanner => {
                    assert!(!geom.lines.is_empty());
                    assert!(!geom.labels.is_empty());
                }
                PaperTemplate::Checklist => {
                    assert!(!geom.lines.is_empty());
                    assert!(!geom.checkboxes.is_empty());
                    assert!(!geom.labels.is_empty());
                }
                _ => {
                    assert!(!geom.lines.is_empty());
                }
            }
        }
        let tiny = paper_template_geometry(8, 8, PaperTemplate::Checklist);
        assert!(tiny.lines.is_empty());
    }

    #[test]
    fn library_sanitizers_clean_nested_folders_tags_and_custom_templates() {
        assert_eq!(
            sanitize_folder_path(r"..\School\\Math\.\Calculus\"),
            "School/Math/Calculus"
        );
        assert_eq!(sanitize_folder_path("  /..// "), "");
        let folders = sanitize_library_folders(&[
            "Work/Projects/2026".to_string(),
            "work/projects/2026".to_string(),
            "../Bad".to_string(),
        ]);
        assert_eq!(
            folders,
            vec!["Work", "Work/Projects", "Work/Projects/2026", "Bad"]
        );

        let tags = sanitize_notebook_tags(&[
            "#math".to_string(),
            "MATH".to_string(),
            "  calculus\x07   notes ".to_string(),
            "   ".to_string(),
        ]);
        assert_eq!(tags, vec!["math", "calculus notes"]);

        let cfg = UserStudioConfig {
            library_folders: vec!["Lectures/CS101".to_string()],
            library_view_mode: LibraryViewMode::List,
            library_sort_by: LibrarySortBy::Name,
            library_sort_ascending: true,
            custom_templates: vec![
                CustomPaperTemplate {
                    id: "  ".to_string(),
                    name: "  Dot Sheet  ".to_string(),
                    kind: CustomTemplateKind::Image,
                    width: 1200,
                    height: 1600,
                    pdf_page_index: 0,
                    bytes: Arc::new(vec![1, 2, 3, 4]),
                },
                CustomPaperTemplate {
                    id: "bad_pdf".to_string(),
                    name: "Not a PDF".to_string(),
                    kind: CustomTemplateKind::Pdf,
                    width: 1200,
                    height: 1600,
                    pdf_page_index: 0,
                    bytes: Arc::new(b"not a pdf".to_vec()),
                },
            ],
            ..UserStudioConfig::default()
        };
        let clean = cfg.sanitized();
        assert_eq!(clean.library_folders, vec!["Lectures", "Lectures/CS101"]);
        assert_eq!(clean.library_view_mode, LibraryViewMode::List);
        assert_eq!(clean.library_sort_by, LibrarySortBy::Name);
        assert!(clean.library_sort_ascending);
        assert_eq!(clean.custom_templates.len(), 1);
        assert_eq!(clean.custom_templates[0].id, "tpl_1");
        assert_eq!(clean.custom_templates[0].name, "Dot Sheet");
    }

    #[test]
    fn document_rotation_preserves_geometry() {
        let mut doc = Document::new(800, 1200);
        let s_id = doc.alloc_stroke_id();
        doc.strokes.push(Arc::new(InkStroke::new(
            s_id,
            PenSettings::fountain_pen(),
            vec![StrokeSample::from_xy_pressure(100.0, 200.0, 0.7)],
            vec![StrokeSample::from_xy_pressure(100.0, 200.0, 0.7)],
        )));
        doc.texts.push(Arc::new(TextItem {
            text: "Hello".to_string(),
            x: 100.0,
            y: 200.0,
            angle_degrees: 0.0,
            ..TextItem::default()
        }));
        doc.shapes.push(Arc::new(GeometricShape {
            kind: ShapeKind::Triangle,
            x1: 100.0,
            y1: 200.0,
            x2: 200.0,
            y2: 260.0,
            angle_degrees: 0.0,
            ..GeometricShape::default()
        }));

        // Rotate 90 CW: (800, 1200) -> (1200, 800), (100, 200) -> (1000, 100).
        assert!(doc.rotate_page_quarter_turns(0, 1));
        assert_eq!((doc.width, doc.height), (1200, 800));
        let p0 = doc.strokes[0].smoothed_points[0];
        assert!((p0.x - 1000.0).abs() < 1e-6);
        assert!((p0.y - 100.0).abs() < 1e-6);
        assert!((doc.texts[0].x - 1000.0).abs() < 1e-6);
        assert!((doc.texts[0].angle_degrees - 90.0).abs() < 1e-6);
        assert!((doc.shapes[0].angle_degrees - 90.0).abs() < 1e-6);
        assert_eq!(doc.backdrop_rotation, 1);

        // Three more 90 CW turns restore original dimensions and positions.
        assert!(doc.rotate_page_quarter_turns(0, 3));
        assert_eq!((doc.width, doc.height), (800, 1200));
        let restored = doc.strokes[0].smoothed_points[0];
        assert!((restored.x - 100.0).abs() < 1e-6);
        assert!((restored.y - 200.0).abs() < 1e-6);
        assert_eq!(doc.backdrop_rotation, 0);
    }

    #[test]
    fn document_move_bookmark_transfer_and_custom_templates() {
        let mut doc = Document::new(800, 1200);
        doc.folder_path = "Work/Q1".to_string();
        doc.favorite = true;
        doc.tags = vec!["draft".to_string(), "geometry".to_string()];
        doc.created_at_ms = 1_700_000_000_000;
        doc.updated_at_ms = 1_700_000_005_000;
        doc.bookmarked = true;

        let tpl = CustomPaperTemplate {
            id: "tpl_grid".to_string(),
            name: "Custom Graph".to_string(),
            kind: CustomTemplateKind::Image,
            width: 800,
            height: 1200,
            pdf_page_index: 0,
            bytes: Arc::new(vec![0x89, b'P', b'N', b'G', 1, 2, 3]),
        };
        assert_eq!(doc.add_page(Some(PaperTemplate::Checklist), None), Some(1));
        assert!(doc.apply_custom_template_to_all_pages(&tpl).is_ok());
        assert_eq!(doc.image_templates.len(), 1);
        assert_eq!(doc.pages[0].custom_image_backdrop, Some(0));
        assert_eq!(doc.pages[1].custom_image_backdrop, Some(0));

        // Reorder page 1 -> 0 while tracking active page.
        doc.select_page(1);
        assert!(doc.move_page(1, 0));
        assert_eq!(doc.active_page_index(), 0);

        let json = serde_json::to_string(&doc).expect("serialize");
        let back: Document = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.folder_path, "Work/Q1");
        assert!(back.favorite);
        assert_eq!(back.tags, vec!["draft", "geometry"]);
        assert_eq!(back.image_templates.len(), 1);
        assert!(back.pages.iter().any(|p| p.bookmarked));
    }

    #[test]
    fn library_folder_colors_sanitize_remap_and_remove_cleanly() {
        let mut raw_colors = BTreeMap::new();
        raw_colors.insert(
            r"..\School\\Math\".to_string(),
            Color::new(f32::NAN, 0.4, 2.0, 0.2),
        );
        raw_colors.insert("  /..// ".to_string(), Color::BLACK);
        let cfg = UserStudioConfig {
            library_folder_colors: raw_colors,
            ..UserStudioConfig::default()
        };
        let mut clean = cfg.sanitized();
        assert_eq!(
            clean.folder_color("school/MATH"),
            Some(Color::new(0.0, 0.4, 1.0, 1.0))
        );

        let blue = Color::from_hex("#2563EB").unwrap();
        let green = Color::from_hex("#16A34A").unwrap();
        assert!(clean.set_folder_color("School", Some(blue)));
        assert!(!clean.set_folder_color("school", Some(blue)));
        assert!(clean.set_folder_color("School/Physics", Some(green)));
        assert_eq!(clean.folder_color("SCHOOL"), Some(blue));

        clean.remap_folder_colors("School", "Archive/College");
        assert_eq!(clean.folder_color("School"), None);
        assert_eq!(clean.folder_color("Archive/College"), Some(blue));
        assert_eq!(clean.folder_color("archive/college/physics"), Some(green));

        clean.remove_folder_colors("Archive/College");
        assert_eq!(clean.folder_color("Archive/College"), None);
        assert_eq!(clean.folder_color("Archive/College/Physics"), None);

        // Legacy studio config without `library_folder_colors` or `ui_scale` deserializes cleanly.
        let legacy: UserStudioConfig = serde_json::from_str("{}").expect("legacy json");
        assert!(legacy.library_folder_colors.is_empty());
        assert!((legacy.ui_scale - 1.0).abs() < 1e-6);

        assert!((UserStudioConfig::sanitize_ui_scale(f32::NAN) - 1.0).abs() < 1e-6);
        assert!((UserStudioConfig::sanitize_ui_scale(f32::INFINITY) - 1.0).abs() < 1e-6);
        assert!(
            (UserStudioConfig::sanitize_ui_scale(0.1) - UserStudioConfig::MIN_UI_SCALE).abs()
                < 1e-6
        );
        assert!(
            (UserStudioConfig::sanitize_ui_scale(9.0) - UserStudioConfig::MAX_UI_SCALE).abs()
                < 1e-6
        );
        assert!((UserStudioConfig::sanitize_ui_scale(1.254) - 1.25).abs() < 1e-6);
    }

    #[test]
    fn library_tag_colors_sanitize_and_update_cleanly() {
        let mut raw_colors = BTreeMap::new();
        raw_colors.insert(
            "  #Exam  Notes\x07 ".to_string(),
            Color::new(f32::NAN, 0.4, 2.0, 0.2),
        );
        raw_colors.insert("  ###  ".to_string(), Color::BLACK);
        let cfg = UserStudioConfig {
            library_tag_colors: raw_colors,
            ..UserStudioConfig::default()
        };
        let mut clean = cfg.sanitized();
        assert_eq!(
            clean.tag_color("#exam notes"),
            Some(Color::new(0.0, 0.4, 1.0, 1.0))
        );
        assert_eq!(clean.library_tag_colors.len(), 1);

        let blue = Color::from_hex("#2563EB").unwrap();
        let rose = Color::from_hex("#DB2777").unwrap();
        assert!(clean.set_tag_color("Study", Some(blue)));
        assert!(!clean.set_tag_color("#study", Some(blue)));
        assert_eq!(clean.tag_color("STUDY"), Some(blue));
        assert!(clean.set_tag_color("study", Some(rose)));
        assert_eq!(clean.tag_color("Study"), Some(rose));
        assert!(clean.set_tag_color("#Study", None));
        assert_eq!(clean.tag_color("study"), None);
        assert!(!clean.set_tag_color("study", None));

        let legacy: UserStudioConfig = serde_json::from_str("{}").expect("legacy json");
        assert!(legacy.library_tag_colors.is_empty());
    }

    #[test]
    fn page_resolution_presets_stay_within_bounds_and_roundtrip_dimensions() {
        for &preset in PageResolutionPreset::ALL {
            let (w, h) = preset.dimensions();
            assert!((64..=8192).contains(&w));
            assert!((64..=8192).contains(&h));
            if preset != PageResolutionPreset::Custom {
                assert_eq!(PageResolutionPreset::from_dimensions(w, h), preset);
                let (pw, ph) = preset.dimensions_with_orientation(PageOrientation::Portrait);
                let (lw, lh) = preset.dimensions_with_orientation(PageOrientation::Landscape);
                assert!(pw <= ph);
                assert!(lw >= lh);
                assert_eq!(PageResolutionPreset::detect(pw, ph).0, preset);
                assert_eq!(PageResolutionPreset::detect(lw, lh).0, preset);
                if preset != PageResolutionPreset::Square {
                    assert_eq!(
                        PageResolutionPreset::detect(pw, ph).1,
                        PageOrientation::Portrait
                    );
                    assert_eq!(
                        PageResolutionPreset::detect(lw, lh).1,
                        PageOrientation::Landscape
                    );
                }
            }
        }
        assert_eq!(
            PageResolutionPreset::from_dimensions(2560, 1440),
            PageResolutionPreset::Custom
        );
        assert_eq!(
            PageResolutionPreset::detect(2560, 1440),
            (PageResolutionPreset::Custom, PageOrientation::Landscape)
        );
    }

    #[test]
    fn document_page_dimensions_and_orientation_helpers_respect_per_page_sizes() {
        let mut doc = Document::new(1200, 1600);
        let idx = doc
            .add_page_with_size(
                Some(1684),
                Some(1190),
                Some(PaperTemplate::Grid),
                Some(PaperColor::PureWhite),
            )
            .expect("second page added");
        assert_eq!(idx, 1);
        assert_eq!(doc.width, 1684);
        assert_eq!(doc.height, 1190);
        assert_eq!(doc.pages[0].width, 1200);
        assert_eq!(doc.pages[0].height, 1600);

        // Resizing page 0 leaves page 1 independent and clamps bounds.
        assert!(doc.set_page_dimensions(0, 16, 20000));
        assert_eq!(doc.pages[0].width, 64);
        assert_eq!(doc.pages[0].height, 8192);
        assert_eq!(doc.pages[1].width, 1684);
        assert_eq!(doc.pages[1].height, 1190);

        // Out-of-bounds page returns false cleanly.
        assert!(!doc.set_page_dimensions(99, 800, 600));

        // Orientation-only apply flips both pages independently.
        assert!(doc.set_all_pages_orientation(PageOrientation::Portrait));
        assert_eq!((doc.pages[0].width, doc.pages[0].height), (64, 8192));
        assert_eq!((doc.pages[1].width, doc.pages[1].height), (1190, 1684));
        assert!(!doc.set_all_pages_orientation(PageOrientation::Portrait));

        // Setting uniform dimensions across all pages updates all sheets.
        assert!(doc.set_all_pages_dimensions(1920, 1080));
        assert_eq!((doc.pages[0].width, doc.pages[0].height), (1920, 1080));
        assert_eq!((doc.pages[1].width, doc.pages[1].height), (1920, 1080));
    }

    #[test]
    fn pencil_pen_and_legacy_4_element_pen_sizes_deserialize_cleanly() {
        let legacy: UserStudioConfig =
            serde_json::from_str(r#"{ "pen_sizes": [1.8, 2.4, 9.0, 18.0] }"#)
                .expect("legacy 4-element pen_sizes should deserialize");
        assert_eq!(
            legacy.pen_sizes,
            [1.8, 2.4, 9.0, 18.0, UserStudioConfig::DEFAULT_PEN_SIZES[4]]
        );

        let pencil = PenSettings::pencil().sanitized();
        assert_eq!(pencil.kind, PenKind::Pencil);
        assert!((pencil.pressure_opacity_min - 0.35).abs() < 1e-5);
        assert_eq!(PaletteTarget::for_pen(PenKind::Pencil), PaletteTarget::Ink);
        assert_eq!(PenPreset::for_kind(PenKind::Pencil).len(), 2);
    }

    #[test]
    fn polygon_shape_fill_and_line_style_sanitize_and_rotate() {
        let poly = GeometricShape {
            kind: ShapeKind::Polygon,
            fill_color: Some(Color::from_rgba8(0x28, 0x66, 0xE8, 64)),
            line_style: ShapeLineStyle::Dashed,
            points: vec![
                Point::new(100.0, 100.0),
                Point::new(220.0, 100.0),
                Point::new(260.0, 180.0),
                Point::new(140.0, 180.0),
            ],
            ..GeometricShape::default()
        }
        .sanitized();
        assert!(poly.is_closed());
        assert_eq!(poly.x1, 100.0);
        assert_eq!(poly.x2, 260.0);
        assert_eq!(poly.y1, 100.0);
        assert_eq!(poly.y2, 180.0);
        assert!(poly.line_style.dash_pattern(4.0).is_some());
        let segs = poly.path_segments();
        assert_eq!(segs.len(), 1);
        assert!(segs[0].1);
        assert_eq!(segs[0].0.len(), 4);

        let mut doc = Document::default();
        doc.shapes.push(Arc::new(poly.clone()));
        assert!(doc.rotate_page_quarter_turns(0, 1));
        let rot = &doc.shapes[0];
        assert_eq!(rot.angle_degrees, 90.0);
        assert_eq!(rot.points.len(), 4);
    }

    #[test]
    fn rich_text_spans_lists_checkboxes_and_wrapping_work_cleanly() {
        let mut item = TextItem {
            text: "Title line\n[ ] First task".to_string(),
            alignment: TextAlignment::Center,
            ..TextItem::default()
        };
        assert!(item.toggle_span_style(0, 5, TextStyleAttr::Bold));
        assert!(item.toggle_span_style(0, 5, TextStyleAttr::Underline));
        assert!(item.set_span_highlight(0, 5, Some(Color::from_rgba8(0xFF, 0xEE, 0x58, 180))));
        let (b, i, u, hl) = item.style_active_across(0, 5);
        assert!(b && !i && u && hl.is_some());

        let old = item.text.clone();
        item.text = format!("My {old}");
        item.adjust_spans_for_edit(&old, &item.text.clone());
        assert_eq!(item.spans[0].start, 3);
        assert_eq!(item.spans[0].end, 8);

        assert!(item.toggle_checkbox_at_paragraph(1));
        assert!(item.text.ends_with("[x] First task"));

        let wrapped = wrap_styled_text_lines(&item, 400.0, |_| 10.0);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0].list_kind, None);
        assert_eq!(
            wrapped[1].list_kind,
            Some(ListItemKind::Checkbox { checked: true })
        );

        let red = Color::from_rgba8(220, 40, 40, 255);
        assert!(item.set_span_font(0, 5, Some(TextFont::Monospace)));
        assert!(item.set_span_size(0, 5, Some(36.0)));
        assert!(item.set_span_color(0, 5, Some(red)));
        assert_eq!(item.effective_font_across(0, 5), Some(TextFont::Monospace));
        assert_eq!(item.effective_font_across(0, 8), None);
        assert_eq!(item.effective_size_across(0, 5), Some(36.0));
        assert_eq!(item.effective_size_across(0, 8), None);
        assert_eq!(item.effective_color_across(0, 5), Some(red));
        assert_eq!(item.effective_color_across(0, 8), None);

        let custom_wrapped = wrap_styled_text_lines_with(&item, 400.0, |_ch, font, sz| {
            if font == TextFont::Monospace {
                sz * 0.62
            } else {
                sz * 0.5
            }
        });
        assert_eq!(custom_wrapped[0].runs[0].font, Some(TextFont::Monospace));
        assert_eq!(custom_wrapped[0].runs[0].size, Some(36.0));
        assert_eq!(custom_wrapped[0].runs[0].color, Some(red));
    }

    #[test]
    fn style_favorites_and_saved_elements_sanitize_bounds_and_builtins() {
        let builtins = SavedElement::built_ins();
        assert_eq!(builtins.len(), 15);
        for b in &builtins {
            assert!(b.built_in);
            assert!(b.sanitized().is_some());
            assert_eq!(b.images.len(), 1);
            assert_eq!(b.images[0].pixel_width, 320);
            assert_eq!(b.images[0].pixel_height, 320);
            assert!(!b.images[0].png_bytes.is_empty());
        }

        let empty_el = SavedElement {
            name: "Empty".to_string(),
            ..SavedElement::default()
        };
        assert!(empty_el.sanitized().is_none());

        let fav = StyleFavorite {
            id: String::new(),
            kind: PenKind::Pencil,
            color: Color::from_rgba8(0x22, 0x33, 0x44, 100),
            size: 999.0,
        };
        let cfg = UserStudioConfig {
            style_favorites: vec![fav],
            saved_elements: builtins,
            ..UserStudioConfig::default()
        }
        .sanitized();
        assert_eq!(cfg.style_favorites.len(), 1);
        assert_eq!(cfg.style_favorites[0].id, "fav_1");
        assert_eq!(cfg.style_favorites[0].size, 120.0);
        assert!(!cfg.saved_elements[0].built_in);
    }

    #[test]
    fn wildcard_colors_and_readonly_builtin_palettes_sanitize_cleanly() {
        let def = UserStudioConfig::default();
        assert_eq!(
            def.wildcard_color(PaletteTarget::Ink),
            UserStudioConfig::default_wildcard_ink()
        );
        assert_eq!(
            def.wildcard_color(PaletteTarget::Highlighter),
            UserStudioConfig::default_wildcard_highlighter()
        );

        let legacy: UserStudioConfig = serde_json::from_str("{}").expect("legacy json");
        assert_eq!(
            legacy.wildcard_ink,
            UserStudioConfig::default_wildcard_ink()
        );
        assert_eq!(
            legacy.wildcard_highlighter,
            UserStudioConfig::default_wildcard_highlighter()
        );

        let mut tampered_classic = ColorPalettePreset::builtin_classic();
        tampered_classic.name = "Hijacked".to_string();
        tampered_classic.ink_colors = vec![Color::BLACK];
        tampered_classic.highlighter_colors = vec![Color::WHITE];

        let mut tampered_pastel = ColorPalettePreset::builtin_pastel();
        tampered_pastel.ink_colors = vec![Color::BLACK];

        let mut tampered_earth = ColorPalettePreset::builtin_earth();
        tampered_earth.highlighter_colors = vec![Color::WHITE];

        let dirty = UserStudioConfig {
            wildcard_ink: Color::new(f32::NAN, 2.0, -1.0, 0.25),
            wildcard_highlighter: Color::new(0.5, f32::INFINITY, 0.25, 0.1),
            palette_presets: vec![tampered_classic, tampered_pastel, tampered_earth],
            ..UserStudioConfig::default()
        };
        let mut clean = dirty.sanitized();
        assert_eq!(clean.wildcard_ink, Color::new(0.0, 1.0, 0.0, 1.0));
        assert_eq!(clean.wildcard_highlighter, Color::new(0.5, 0.0, 0.25, 1.0));
        assert_eq!(clean.palette_presets, ColorPalettePreset::built_ins());

        let blue = Color::from_hex("#2563EB").unwrap();
        let lime = Color::from_hex("#84CC16").unwrap();
        clean.set_wildcard_color(PaletteTarget::Ink, blue.with_alpha(0.3));
        clean.set_wildcard_color(PaletteTarget::Highlighter, lime.with_alpha(0.5));
        assert_eq!(
            clean.wildcard_color(PaletteTarget::Ink),
            blue.with_alpha(1.0)
        );
        assert_eq!(
            clean.wildcard_color(PaletteTarget::Highlighter),
            lime.with_alpha(1.0)
        );
    }

    #[test]
    fn stylus_button_config_defaults_roundtrip_and_sanitizes_eraser_end() {
        let def = StylusButtonConfig::default();
        assert_eq!(def.barrel_button, StylusButtonAction::ToggleEraser);
        assert_eq!(
            def.secondary_barrel_button,
            StylusButtonAction::PanCanvasHold
        );
        assert_eq!(def.eraser_end, StylusButtonAction::EraserHold);

        let legacy: UserStudioConfig = serde_json::from_str("{}").expect("legacy studio config");
        assert_eq!(legacy.stylus_buttons, def);

        let custom = UserStudioConfig {
            stylus_buttons: StylusButtonConfig {
                barrel_button: StylusButtonAction::Undo,
                secondary_barrel_button: StylusButtonAction::LassoSelectHold,
                eraser_end: StylusButtonAction::HighlighterHold,
            },
            ..UserStudioConfig::default()
        };
        let encoded = serde_json::to_string(&custom).expect("serialize stylus config");
        let decoded: UserStudioConfig =
            serde_json::from_str(&encoded).expect("deserialize stylus config");
        assert_eq!(decoded.stylus_buttons, custom.stylus_buttons);

        // Non-hold actions on the inverted eraser end sanitize back to EraserHold.
        let dirty = UserStudioConfig {
            stylus_buttons: StylusButtonConfig {
                barrel_button: StylusButtonAction::Redo,
                secondary_barrel_button: StylusButtonAction::ToggleEraser,
                eraser_end: StylusButtonAction::Undo,
            },
            ..UserStudioConfig::default()
        };
        let clean = dirty.sanitized();
        assert_eq!(clean.stylus_buttons.barrel_button, StylusButtonAction::Redo);
        assert_eq!(
            clean.stylus_buttons.secondary_barrel_button,
            StylusButtonAction::ToggleEraser
        );
        assert_eq!(
            clean.stylus_buttons.eraser_end,
            StylusButtonAction::EraserHold
        );
    }
}
