use std::io::{Read, Write};
use std::sync::Arc;

use drawcraft_color::Color;
use drawcraft_doc::{
    BufferAveraging, BufferPreprocessor, Document, EmbeddedImageTemplate, EmbeddedPdf, Flashcard,
    GeometricShape, InkStroke, MAX_CUSTOM_TEMPLATE_IMAGE_BYTES, MAX_DOCUMENT_IMAGE_TEMPLATE_BYTES,
    MAX_DOCUMENT_IMAGE_TEMPLATES, MAX_FLASHCARDS, MAX_IMAGE_PNG_BYTES, MAX_PAGE_IMAGES,
    MAX_PAGE_SHAPES, MAX_PAGE_TEXT_ITEMS, MAX_POLYGON_VERTICES, MAX_STUDY_TAPES_PER_PAGE,
    MAX_TAGS_PER_NOTEBOOK, MAX_TEXT_SPANS, NotebookCover, NotebookPage, PageImage, PaperColor,
    PaperTemplate, PdfPageBackdrop, PenKind, PenSettings, Point, PressureCurve, QuantizedSample,
    ReviewSchedule, ShapeKind, ShapeLineStyle, SmoothingConfig, StabilizerMode, StrokeId,
    StrokeSample, StudyTapeItem, TextAlignment, TextFont, TextItem, TextSpan, sanitize_folder_path,
    sanitize_notebook_id, sanitize_notebook_tags, sanitize_template_id,
};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};

use crate::FormatError;

/// Magic header written by DrawCraft (`.dcraft` v2 binary).
pub const BINARY_MAGIC: &[u8; 4] = b"DCFT";
/// Legacy magic of PaperCraft `.pcraft` files; still readable.
pub const LEGACY_BINARY_MAGIC: &[u8; 4] = b"PCFT";
/// Legacy magic of NoteCraft `.ncraft` files; still readable.
pub const NOTECRAFT_BINARY_MAGIC: &[u8; 4] = b"NCFT";
/// Magic of pre-release `.scraft` files (the project's earlier name); still readable.
pub const EARLY_BINARY_MAGIC: &[u8; 4] = b"SCFT";
pub const BINARY_VERSION: u16 = 2;

/// Returns true when `bytes` starts with the current or a supported legacy binary magic.
#[must_use]
pub fn has_binary_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(BINARY_MAGIC)
        || bytes.starts_with(LEGACY_BINARY_MAGIC)
        || bytes.starts_with(NOTECRAFT_BINARY_MAGIC)
        || bytes.starts_with(EARLY_BINARY_MAGIC)
}
const HEADER_LEN: usize = 8;

const MIN_DECOMPRESSED_BYTES_CAP: usize = 128 * 1024 * 1024;
const MAX_DECOMPRESSION_RATIO: usize = 64;
const MAX_PEN_PALETTE: usize = 65_535;
const MAX_STROKES_PER_PAGE: usize = 500_000;
const MAX_POINTS_PER_STROKE: usize = 200_000;
const MAX_CURVE_POINTS: usize = PressureCurve::MAX_POINTS;
const MAX_STRING_BYTES: usize = 65_535;

const SAMPLE_FLAG_FULL_PRESSURE: u8 = 1 << 0;
const SAMPLE_FLAG_CONST_PRESSURE: u8 = 1 << 1;
const SAMPLE_FLAG_HAS_TILT: u8 = 1 << 2;
const SAMPLE_FLAG_HAS_ROT: u8 = 1 << 3;
const SAMPLE_FLAG_HAS_TIME: u8 = 1 << 4;

fn write_cover(out: &mut Vec<u8>, v: NotebookCover) {
    match v {
        NotebookCover::ClassicNavy => out.push(0),
        NotebookCover::SageGreen => out.push(1),
        NotebookCover::Terracotta => out.push(2),
        NotebookCover::SlateGraphite => out.push(3),
        NotebookCover::WarmSand => out.push(4),
        NotebookCover::PlumStudio => out.push(5),
        NotebookCover::Custom { r, g, b } => {
            out.push(6);
            out.push(r);
            out.push(g);
            out.push(b);
        }
    }
}

fn read_cover(r: &mut Reader<'_>) -> Result<NotebookCover, FormatError> {
    match r.read_u8()? {
        0 => Ok(NotebookCover::ClassicNavy),
        1 => Ok(NotebookCover::SageGreen),
        2 => Ok(NotebookCover::Terracotta),
        3 => Ok(NotebookCover::SlateGraphite),
        4 => Ok(NotebookCover::WarmSand),
        5 => Ok(NotebookCover::PlumStudio),
        6 => Ok(NotebookCover::Custom {
            r: r.read_u8()?,
            g: r.read_u8()?,
            b: r.read_u8()?,
        }),
        other => Err(FormatError::BinaryCorrupt(format!(
            "unknown cover tag {other}"
        ))),
    }
}

fn template_to_u8(v: PaperTemplate) -> u8 {
    match v {
        PaperTemplate::Blank => 0,
        PaperTemplate::RuledNarrow => 1,
        PaperTemplate::RuledWide => 2,
        PaperTemplate::Grid => 3,
        PaperTemplate::Dotted => 4,
        PaperTemplate::Cornell => 5,
        PaperTemplate::Isometric => 6,
        PaperTemplate::MusicStaff => 7,
        PaperTemplate::WeeklyPlanner => 8,
        PaperTemplate::MonthlyPlanner => 9,
        PaperTemplate::Checklist => 10,
    }
}

fn u8_to_template(b: u8) -> Result<PaperTemplate, FormatError> {
    match b {
        0 => Ok(PaperTemplate::Blank),
        1 => Ok(PaperTemplate::RuledNarrow),
        2 => Ok(PaperTemplate::RuledWide),
        3 => Ok(PaperTemplate::Grid),
        4 => Ok(PaperTemplate::Dotted),
        5 => Ok(PaperTemplate::Cornell),
        6 => Ok(PaperTemplate::Isometric),
        7 => Ok(PaperTemplate::MusicStaff),
        8 => Ok(PaperTemplate::WeeklyPlanner),
        9 => Ok(PaperTemplate::MonthlyPlanner),
        10 => Ok(PaperTemplate::Checklist),
        other => Err(FormatError::BinaryCorrupt(format!(
            "unknown paper template tag {other}"
        ))),
    }
}

fn color_to_u8(v: PaperColor) -> u8 {
    match v {
        PaperColor::WarmCream => 0,
        PaperColor::PureWhite => 1,
        PaperColor::DarkSlate => 2,
    }
}

fn u8_to_color(b: u8) -> Result<PaperColor, FormatError> {
    match b {
        0 => Ok(PaperColor::WarmCream),
        1 => Ok(PaperColor::PureWhite),
        2 => Ok(PaperColor::DarkSlate),
        other => Err(FormatError::BinaryCorrupt(format!(
            "unknown paper color tag {other}"
        ))),
    }
}

fn pen_kind_to_u8(v: PenKind) -> u8 {
    match v {
        PenKind::FountainPen => 0,
        PenKind::BallPen => 1,
        PenKind::BrushPen => 2,
        PenKind::Highlighter => 3,
        PenKind::Pencil => 4,
    }
}

fn u8_to_pen_kind(b: u8) -> Result<PenKind, FormatError> {
    match b {
        0 => Ok(PenKind::FountainPen),
        1 => Ok(PenKind::BallPen),
        2 => Ok(PenKind::BrushPen),
        3 => Ok(PenKind::Highlighter),
        4 => Ok(PenKind::Pencil),
        other => Err(FormatError::BinaryCorrupt(format!(
            "unknown pen kind tag {other}"
        ))),
    }
}

fn stab_mode_to_u8(v: StabilizerMode) -> u8 {
    match v {
        StabilizerMode::None => 0,
        StabilizerMode::OneEuro => 1,
        StabilizerMode::PulledString => 2,
        StabilizerMode::ExponentialMovingAverage => 3,
        StabilizerMode::GaussianWindow => 4,
        StabilizerMode::CircularBuffer => 5,
        StabilizerMode::ModeledSpring => 6,
        StabilizerMode::CurvedBezier => 7,
        StabilizerMode::SimpleDirect => 8,
    }
}

fn u8_to_stab_mode(b: u8) -> Result<StabilizerMode, FormatError> {
    match b {
        0 => Ok(StabilizerMode::None),
        1 => Ok(StabilizerMode::OneEuro),
        2 => Ok(StabilizerMode::PulledString),
        3 => Ok(StabilizerMode::ExponentialMovingAverage),
        4 => Ok(StabilizerMode::GaussianWindow),
        5 => Ok(StabilizerMode::CircularBuffer),
        6 => Ok(StabilizerMode::ModeledSpring),
        7 => Ok(StabilizerMode::CurvedBezier),
        8 => Ok(StabilizerMode::SimpleDirect),
        other => Err(FormatError::BinaryCorrupt(format!(
            "unknown stabilizer mode tag {other}"
        ))),
    }
}

fn buf_avg_to_u8(v: BufferAveraging) -> u8 {
    match v {
        BufferAveraging::ArithmeticMean => 0,
        BufferAveraging::VelocityGaussian => 1,
        BufferAveraging::None => 2,
    }
}

fn u8_to_buf_avg(b: u8) -> Result<BufferAveraging, FormatError> {
    match b {
        0 => Ok(BufferAveraging::ArithmeticMean),
        1 => Ok(BufferAveraging::VelocityGaussian),
        2 => Ok(BufferAveraging::None),
        other => Err(FormatError::BinaryCorrupt(format!(
            "unknown buffer averaging tag {other}"
        ))),
    }
}

fn buf_pre_to_u8(v: BufferPreprocessor) -> u8 {
    match v {
        BufferPreprocessor::None => 0,
        BufferPreprocessor::Deadzone => 1,
        BufferPreprocessor::Inertia => 2,
    }
}

fn u8_to_buf_pre(b: u8) -> Result<BufferPreprocessor, FormatError> {
    match b {
        0 => Ok(BufferPreprocessor::None),
        1 => Ok(BufferPreprocessor::Deadzone),
        2 => Ok(BufferPreprocessor::Inertia),
        other => Err(FormatError::BinaryCorrupt(format!(
            "unknown buffer preprocessor tag {other}"
        ))),
    }
}

fn write_varint(out: &mut Vec<u8>, mut val: u64) {
    loop {
        let mut byte = (val & 0x7F) as u8;
        val >>= 7;
        if val != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if val == 0 {
            break;
        }
    }
}

fn write_zigzag(out: &mut Vec<u8>, val: i64) {
    let encoded = ((val << 1) ^ (val >> 63)) as u64;
    write_varint(out, encoded);
}

fn write_str(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let capped = &bytes[..bytes.len().min(MAX_STRING_BYTES)];
    write_varint(out, capped.len() as u64);
    out.extend_from_slice(capped);
}

const EXT_TAG_PAGE_OBJECTS: u8 = 1;
const EXT_TAG_TEXT_BOX_WIDTHS: u8 = 2;
const EXT_TAG_TEXT_LINKS: u8 = 3;
const EXT_TAG_PDF_BACKDROPS: u8 = 4;
const EXT_TAG_LIBRARY_META: u8 = 5;
const EXT_TAG_PAGE_EXTRAS_AND_TEMPLATES: u8 = 6;
const EXT_TAG_SHAPES_AND_RICH_TEXT: u8 = 7;
const EXT_TAG_DOCUMENT_IDENTITY: u8 = 8;
/// DrawCraft extension: per-page study tapes plus the notebook flashcard deck.
const EXT_TAG_STUDY_ITEMS: u8 = 9;

fn write_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn write_f64(out: &mut Vec<u8>, v: f64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn text_font_to_u8(font: TextFont) -> u8 {
    match font {
        TextFont::Sans => 0,
        TextFont::Monospace => 1,
    }
}

fn u8_to_text_font(b: u8) -> Result<TextFont, FormatError> {
    match b {
        0 => Ok(TextFont::Sans),
        1 => Ok(TextFont::Monospace),
        other => Err(FormatError::BinaryCorrupt(format!(
            "unknown text font tag {other}"
        ))),
    }
}

fn shape_kind_to_u8(kind: ShapeKind) -> u8 {
    match kind {
        ShapeKind::Line => 0,
        ShapeKind::Rectangle => 1,
        ShapeKind::Ellipse => 2,
        ShapeKind::Triangle => 3,
        ShapeKind::Arrow => 4,
        ShapeKind::Polygon => 5,
    }
}

fn u8_to_shape_kind(b: u8) -> Result<ShapeKind, FormatError> {
    match b {
        0 => Ok(ShapeKind::Line),
        1 => Ok(ShapeKind::Rectangle),
        2 => Ok(ShapeKind::Ellipse),
        3 => Ok(ShapeKind::Triangle),
        4 => Ok(ShapeKind::Arrow),
        5 => Ok(ShapeKind::Polygon),
        other => Err(FormatError::BinaryCorrupt(format!(
            "unknown shape kind tag {other}"
        ))),
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, FormatError> {
        let b =
            self.data.get(self.pos).copied().ok_or_else(|| {
                FormatError::BinaryCorrupt("unexpected end of stream".to_string())
            })?;
        self.pos += 1;
        Ok(b)
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], FormatError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| FormatError::BinaryCorrupt("length overflow".to_string()))?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| FormatError::BinaryCorrupt("unexpected end of stream".to_string()))?;
        self.pos = end;
        Ok(slice)
    }

    fn read_u16(&mut self) -> Result<u16, FormatError> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_f32(&mut self) -> Result<f32, FormatError> {
        let b = self.read_bytes(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_f64(&mut self) -> Result<f64, FormatError> {
        let b = self.read_bytes(8)?;
        Ok(f64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn has_remaining(&self) -> bool {
        self.pos < self.data.len()
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn read_varint(&mut self) -> Result<u64, FormatError> {
        let mut res = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.read_u8()?;
            let part = u64::from(b & 0x7F);
            if shift == 63 && part > 1 {
                return Err(FormatError::BinaryCorrupt("varint overflow".to_string()));
            }
            res |= part << shift;
            if (b & 0x80) == 0 {
                return Ok(res);
            }
        }
        Err(FormatError::BinaryCorrupt("malformed varint".to_string()))
    }

    fn read_varint_usize(&mut self, max: usize, label: &str) -> Result<usize, FormatError> {
        let raw = self.read_varint()?;
        let val = usize::try_from(raw)
            .map_err(|_| FormatError::BinaryCorrupt(format!("{label} count overflow")))?;
        if val > max {
            return Err(FormatError::BinaryCorrupt(format!(
                "{label} count {val} exceeds limit {max}"
            )));
        }
        Ok(val)
    }

    fn read_zigzag(&mut self) -> Result<i64, FormatError> {
        let raw = self.read_varint()?;
        Ok(((raw >> 1) as i64) ^ (-((raw & 1) as i64)))
    }

    fn read_str(&mut self) -> Result<String, FormatError> {
        let len = self.read_varint_usize(MAX_STRING_BYTES, "string")?;
        let bytes = self.read_bytes(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|e| FormatError::BinaryCorrupt(format!("invalid UTF-8 string: {e}")))
    }
}

fn write_color_rgba(out: &mut Vec<u8>, c: Color) {
    write_f32(out, c.r);
    write_f32(out, c.g);
    write_f32(out, c.b);
    write_f32(out, c.a);
}

fn read_color_rgba(r: &mut Reader<'_>) -> Result<Color, FormatError> {
    Ok(Color {
        r: r.read_f32()?,
        g: r.read_f32()?,
        b: r.read_f32()?,
        a: r.read_f32()?,
    })
}

fn write_text_item(out: &mut Vec<u8>, item: &TextItem) {
    let clean = item.sanitized();
    write_str(out, &clean.text);
    write_f64(out, clean.x);
    write_f64(out, clean.y);
    write_f32(out, clean.size);
    write_f64(out, clean.angle_degrees);
    write_color_rgba(out, clean.color);
    out.push(text_font_to_u8(clean.font));
    write_f32(out, clean.line_spacing);
    write_f32(out, clean.letter_spacing);
}

fn read_text_item(r: &mut Reader<'_>) -> Result<TextItem, FormatError> {
    let text = r.read_str()?;
    let x = r.read_f64()?;
    let y = r.read_f64()?;
    let size = r.read_f32()?;
    let angle_degrees = r.read_f64()?;
    let color = read_color_rgba(r)?;
    let font = u8_to_text_font(r.read_u8()?)?;
    let line_spacing = r.read_f32()?;
    let letter_spacing = r.read_f32()?;
    Ok(TextItem {
        text,
        x,
        y,
        size,
        angle_degrees,
        color,
        font,
        line_spacing,
        letter_spacing,
        max_width: None,
        link_url: None,
        ..TextItem::default()
    }
    .sanitized())
}

fn write_shape_item(out: &mut Vec<u8>, shape: &GeometricShape) {
    let clean = shape.sanitized();
    out.push(shape_kind_to_u8(clean.kind));
    write_f64(out, clean.x1);
    write_f64(out, clean.y1);
    write_f64(out, clean.x2);
    write_f64(out, clean.y2);
    write_f64(out, clean.angle_degrees);
    write_f64(out, clean.curve);
    write_color_rgba(out, clean.color);
    write_f32(out, clean.width);
}

fn read_shape_item(r: &mut Reader<'_>) -> Result<GeometricShape, FormatError> {
    let kind = u8_to_shape_kind(r.read_u8()?)?;
    let x1 = r.read_f64()?;
    let y1 = r.read_f64()?;
    let x2 = r.read_f64()?;
    let y2 = r.read_f64()?;
    let angle_degrees = r.read_f64()?;
    let curve = r.read_f64()?;
    let color = read_color_rgba(r)?;
    let width = r.read_f32()?;
    Ok(GeometricShape {
        kind,
        x1,
        y1,
        x2,
        y2,
        angle_degrees,
        curve,
        color,
        width,
        ..GeometricShape::default()
    }
    .sanitized())
}

fn write_page_image(out: &mut Vec<u8>, image: &PageImage) {
    let clean = image.sanitized();
    write_f64(out, clean.x);
    write_f64(out, clean.y);
    write_f64(out, clean.width);
    write_f64(out, clean.height);
    write_f64(out, clean.angle_degrees);
    write_varint(out, u64::from(clean.pixel_width));
    write_varint(out, u64::from(clean.pixel_height));
    let byte_len = clean.png_bytes.len().min(MAX_IMAGE_PNG_BYTES);
    write_varint(out, byte_len as u64);
    out.extend_from_slice(&clean.png_bytes[..byte_len]);
}

fn read_page_image(r: &mut Reader<'_>) -> Result<Arc<PageImage>, FormatError> {
    let x = r.read_f64()?;
    let y = r.read_f64()?;
    let width = r.read_f64()?;
    let height = r.read_f64()?;
    let angle_degrees = r.read_f64()?;
    let pixel_width = u32::try_from(r.read_varint()?).unwrap_or(u32::MAX);
    let pixel_height = u32::try_from(r.read_varint()?).unwrap_or(u32::MAX);
    let png_len = r.read_varint_usize(MAX_IMAGE_PNG_BYTES, "image png")?;
    let png_bytes = r.read_bytes(png_len)?.to_vec();
    Ok(Arc::new(
        PageImage {
            x,
            y,
            width,
            height,
            angle_degrees,
            pixel_width,
            pixel_height,
            png_bytes: Arc::new(png_bytes),
        }
        .sanitized(),
    ))
}

fn write_page_extensions(out: &mut Vec<u8>, doc: &Document, pages: &[NotebookPage]) {
    let has_objects = pages
        .iter()
        .any(|p| !p.texts.is_empty() || !p.shapes.is_empty() || !p.images.is_empty());
    if has_objects {
        out.push(EXT_TAG_PAGE_OBJECTS);
        let mut has_custom_text_widths = false;
        for page in pages {
            let text_count = page.texts.len().min(MAX_PAGE_TEXT_ITEMS);
            write_varint(out, text_count as u64);
            for item in page.texts.iter().take(text_count) {
                write_text_item(out, item);
                if item.sanitized().max_width.is_some() {
                    has_custom_text_widths = true;
                }
            }

            let shape_count = page.shapes.len().min(MAX_PAGE_SHAPES);
            write_varint(out, shape_count as u64);
            for shape in page.shapes.iter().take(shape_count) {
                write_shape_item(out, shape);
            }

            let image_count = page.images.len().min(MAX_PAGE_IMAGES);
            write_varint(out, image_count as u64);
            for image in page.images.iter().take(image_count) {
                write_page_image(out, image);
            }
        }
        if has_custom_text_widths {
            out.push(EXT_TAG_TEXT_BOX_WIDTHS);
            for page in pages {
                let text_count = page.texts.len().min(MAX_PAGE_TEXT_ITEMS);
                let entries: Vec<(usize, f32)> = page
                    .texts
                    .iter()
                    .take(text_count)
                    .enumerate()
                    .filter_map(|(idx, item)| item.sanitized().max_width.map(|w| (idx, w)))
                    .collect();
                write_varint(out, entries.len() as u64);
                for (idx, width) in entries {
                    write_varint(out, idx as u64);
                    write_f32(out, width);
                }
            }
        }
        write_text_links_extension(out, pages);
        write_shapes_and_rich_text_extension(out, pages);
    }
    write_pdf_backdrops_extension(out, &doc.pdf_documents, pages);
    write_library_meta_extension(out, doc);
    write_page_extras_extension(out, &doc.image_templates, pages);
    write_document_identity_extension(out, doc);
    write_study_items_extension(out, &doc.flashcards, pages);
}

fn write_study_items_extension(
    out: &mut Vec<u8>,
    flashcards: &[Flashcard],
    pages: &[NotebookPage],
) {
    if flashcards.is_empty() && pages.iter().all(|p| p.study_tapes.is_empty()) {
        return;
    }
    out.push(EXT_TAG_STUDY_ITEMS);
    for page in pages {
        let count = page.study_tapes.len().min(MAX_STUDY_TAPES_PER_PAGE);
        write_varint(out, count as u64);
        for tape in page.study_tapes.iter().take(count) {
            let t = tape.sanitized();
            write_varint(out, t.id);
            write_f32(out, t.x);
            write_f32(out, t.y);
            write_f32(out, t.width);
            write_f32(out, t.height);
            write_varint(out, u64::from(t.color_rgba));
            out.push(u8::from(t.revealed));
        }
    }
    let deck = flashcards.len().min(MAX_FLASHCARDS);
    write_varint(out, deck as u64);
    for card in flashcards.iter().take(deck) {
        let c = card.clone().sanitized();
        write_varint(out, c.id);
        write_str(out, &c.front);
        write_str(out, &c.back);
        write_varint(out, c.source_page.map_or(0, |p| u64::from(p) + 1));
        write_varint(out, c.schedule.due_ms.max(0) as u64);
        write_varint(out, u64::from(c.schedule.interval_days));
        write_varint(out, u64::from(c.schedule.ease_permille));
        write_varint(out, u64::from(c.schedule.repetitions));
        write_varint(out, u64::from(c.schedule.lapses));
    }
}

fn read_study_items_extension(
    r: &mut Reader<'_>,
    pages: &mut [NotebookPage],
    flashcards: &mut Vec<Flashcard>,
) -> Result<(), FormatError> {
    let corrupt = |what: &str| FormatError::BinaryCorrupt(format!("{what} out of range"));
    for page in pages.iter_mut() {
        let count = r.read_varint_usize(MAX_STUDY_TAPES_PER_PAGE, "study tape")?;
        let mut tapes = Vec::with_capacity(count.min(64));
        for _ in 0..count {
            let id = r.read_varint()?;
            let x = r.read_f32()?;
            let y = r.read_f32()?;
            let width = r.read_f32()?;
            let height = r.read_f32()?;
            let color_rgba =
                u32::try_from(r.read_varint()?).map_err(|_| corrupt("study tape color"))?;
            let revealed = r.read_u8()? != 0;
            tapes.push(
                StudyTapeItem {
                    id,
                    x,
                    y,
                    width,
                    height,
                    color_rgba,
                    revealed,
                }
                .sanitized(),
            );
        }
        page.study_tapes = tapes;
    }
    let deck = r.read_varint_usize(MAX_FLASHCARDS, "flashcard")?;
    let mut cards = Vec::with_capacity(deck.min(256));
    for _ in 0..deck {
        let id = r.read_varint()?;
        let front = r.read_str()?;
        let back = r.read_str()?;
        let source = r.read_varint()?;
        let source_page = match source {
            0 => None,
            n => Some(u32::try_from(n - 1).map_err(|_| corrupt("flashcard source page"))?),
        };
        let due_ms = i64::try_from(r.read_varint()?).unwrap_or(i64::MAX);
        let interval_days =
            u32::try_from(r.read_varint()?).map_err(|_| corrupt("flashcard interval"))?;
        let ease_permille =
            u16::try_from(r.read_varint()?).map_err(|_| corrupt("flashcard ease"))?;
        let repetitions =
            u32::try_from(r.read_varint()?).map_err(|_| corrupt("flashcard repetitions"))?;
        let lapses = u32::try_from(r.read_varint()?).map_err(|_| corrupt("flashcard lapses"))?;
        cards.push(
            Flashcard {
                id,
                front,
                back,
                source_page,
                schedule: ReviewSchedule {
                    due_ms,
                    interval_days,
                    ease_permille,
                    repetitions,
                    lapses,
                },
            }
            .sanitized(),
        );
    }
    *flashcards = cards;
    Ok(())
}

#[derive(Default)]
struct DecodedExtensions {
    flashcards: Vec<Flashcard>,
    id: String,
    meta_updated_at_ms: i64,
    folder_path: String,
    favorite: bool,
    tags: Vec<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
    pdf_documents: Vec<EmbeddedPdf>,
    image_templates: Vec<EmbeddedImageTemplate>,
}

fn write_document_identity_extension(out: &mut Vec<u8>, doc: &Document) {
    let clean_id = sanitize_notebook_id(&doc.id);
    let meta_ts = doc.meta_updated_at_ms.max(0);
    if clean_id.is_empty() && meta_ts == 0 {
        return;
    }
    out.push(EXT_TAG_DOCUMENT_IDENTITY);
    write_str(out, &clean_id);
    write_varint(out, meta_ts as u64);
}

fn read_document_identity_extension(
    r: &mut Reader<'_>,
    ext: &mut DecodedExtensions,
) -> Result<(), FormatError> {
    ext.id = sanitize_notebook_id(&r.read_str()?);
    let meta_ts = r.read_varint()?;
    ext.meta_updated_at_ms = i64::try_from(meta_ts).unwrap_or(i64::MAX);
    Ok(())
}

fn write_library_meta_extension(out: &mut Vec<u8>, doc: &Document) {
    let folder = sanitize_folder_path(&doc.folder_path);
    let tags = sanitize_notebook_tags(&doc.tags);
    let created = doc.created_at_ms.max(0);
    let updated = doc.updated_at_ms.max(0);
    if folder.is_empty() && !doc.favorite && tags.is_empty() && created == 0 && updated == 0 {
        return;
    }
    out.push(EXT_TAG_LIBRARY_META);
    write_str(out, &folder);
    out.push(u8::from(doc.favorite));
    write_varint(out, tags.len() as u64);
    for tag in &tags {
        write_str(out, tag);
    }
    write_varint(out, created as u64);
    write_varint(out, updated as u64);
}

fn read_library_meta_extension(
    r: &mut Reader<'_>,
    meta: &mut DecodedExtensions,
) -> Result<(), FormatError> {
    meta.folder_path = sanitize_folder_path(&r.read_str()?);
    meta.favorite = r.read_u8()? != 0;
    let tag_count = r.read_varint_usize(MAX_TAGS_PER_NOTEBOOK, "notebook tag")?;
    let mut raw_tags = Vec::with_capacity(tag_count);
    for _ in 0..tag_count {
        raw_tags.push(r.read_str()?);
    }
    meta.tags = sanitize_notebook_tags(&raw_tags);
    let created = r.read_varint()?;
    let updated = r.read_varint()?;
    meta.created_at_ms = i64::try_from(created).unwrap_or(i64::MAX);
    meta.updated_at_ms = i64::try_from(updated).unwrap_or(i64::MAX);
    Ok(())
}

fn write_page_extras_extension(
    out: &mut Vec<u8>,
    image_templates: &[EmbeddedImageTemplate],
    pages: &[NotebookPage],
) {
    let has_extras = !image_templates.is_empty()
        || pages.iter().any(|p| {
            p.bookmarked
                || !p.backdrop_rotation.is_multiple_of(4)
                || p.custom_image_backdrop.is_some()
                || p.custom_template_id.is_some()
        });
    if !has_extras {
        return;
    }
    out.push(EXT_TAG_PAGE_EXTRAS_AND_TEMPLATES);
    let tpl_count = image_templates.len().min(MAX_DOCUMENT_IMAGE_TEMPLATES);
    write_varint(out, tpl_count as u64);
    for tpl in image_templates.iter().take(tpl_count) {
        let clean = tpl.sanitized();
        write_str(out, &clean.name);
        let len = clean.png_bytes.len().min(MAX_CUSTOM_TEMPLATE_IMAGE_BYTES);
        write_varint(out, len as u64);
        out.extend_from_slice(&clean.png_bytes[..len]);
    }
    for page in pages {
        let rot = page.backdrop_rotation % 4;
        let tpl_id = page
            .custom_template_id
            .as_deref()
            .map(sanitize_template_id)
            .filter(|id| !id.is_empty());
        let img_idx = page.custom_image_backdrop.filter(|&idx| idx < tpl_count);
        let flags = u8::from(page.bookmarked)
            | (rot << 1)
            | (u8::from(img_idx.is_some()) << 3)
            | (u8::from(tpl_id.is_some()) << 4);
        out.push(flags);
        if let Some(idx) = img_idx {
            write_varint(out, idx as u64);
        }
        if let Some(id) = tpl_id {
            write_str(out, &id);
        }
    }
}

fn read_page_extras_extension(
    r: &mut Reader<'_>,
    image_templates: &mut Vec<EmbeddedImageTemplate>,
    pages: &mut [NotebookPage],
) -> Result<(), FormatError> {
    let tpl_count = r.read_varint_usize(MAX_DOCUMENT_IMAGE_TEMPLATES, "image template")?;
    let mut loaded = Vec::with_capacity(tpl_count.min(8));
    let mut total_bytes = 0usize;
    for _ in 0..tpl_count {
        let name = r.read_str()?;
        let byte_len =
            r.read_varint_usize(MAX_CUSTOM_TEMPLATE_IMAGE_BYTES, "image template bytes")?;
        total_bytes = total_bytes
            .checked_add(byte_len)
            .filter(|&tot| tot <= MAX_DOCUMENT_IMAGE_TEMPLATE_BYTES)
            .ok_or_else(|| {
                FormatError::BinaryCorrupt(
                    "embedded image template total size exceeds limit".to_string(),
                )
            })?;
        let raw = r.read_bytes(byte_len)?.to_vec();
        loaded.push(
            EmbeddedImageTemplate {
                name,
                png_bytes: Arc::new(raw),
            }
            .sanitized(),
        );
    }
    *image_templates = loaded;

    for page in pages.iter_mut() {
        let flags = r.read_u8()?;
        page.bookmarked = (flags & 0x01) != 0;
        page.backdrop_rotation = (flags >> 1) & 0x03;
        let has_img = (flags & (1 << 3)) != 0;
        let has_tpl_id = (flags & (1 << 4)) != 0;
        page.custom_image_backdrop = if has_img {
            let idx = r.read_varint_usize(MAX_DOCUMENT_IMAGE_TEMPLATES, "custom image index")?;
            (idx < image_templates.len()).then_some(idx)
        } else {
            None
        };
        page.custom_template_id = if has_tpl_id {
            let id = sanitize_template_id(&r.read_str()?);
            (!id.is_empty()).then_some(id)
        } else {
            None
        };
    }
    Ok(())
}

fn write_pdf_backdrops_extension(out: &mut Vec<u8>, pdfs: &[EmbeddedPdf], pages: &[NotebookPage]) {
    if pdfs.is_empty() && pages.iter().all(|p| p.pdf_backdrop.is_none()) {
        return;
    }
    out.push(EXT_TAG_PDF_BACKDROPS);
    write_varint(out, pdfs.len() as u64);
    for pdf in pdfs {
        let clean = pdf.sanitized();
        write_str(out, &clean.name);
        write_varint(out, clean.bytes.len() as u64);
        out.extend_from_slice(&clean.bytes);
    }
    for page in pages {
        match page.pdf_backdrop {
            Some(backdrop) => {
                out.push(1);
                write_varint(out, backdrop.pdf_index as u64);
                write_varint(out, backdrop.page_index as u64);
            }
            None => out.push(0),
        }
    }
}

fn read_pdf_backdrops_extension(
    r: &mut Reader<'_>,
    pdfs: &mut Vec<EmbeddedPdf>,
    pages: &mut [NotebookPage],
) -> Result<(), FormatError> {
    let raw_pdf_count = r.read_varint()?;
    let pdf_count = usize::try_from(raw_pdf_count)
        .map_err(|_| FormatError::BinaryCorrupt("embedded pdf count overflow".to_string()))?;
    // Each entry consumes at least 1 byte for name length and 1 byte for payload length.
    if pdf_count > r.remaining() / 2 {
        return Err(FormatError::BinaryCorrupt(format!(
            "embedded pdf count {pdf_count} exceeds stream bounds"
        )));
    }
    let mut loaded = Vec::with_capacity(pdf_count.min(8));
    for _ in 0..pdf_count {
        let name = r.read_str()?;
        let raw_byte_len = r.read_varint()?;
        let byte_len = usize::try_from(raw_byte_len)
            .map_err(|_| FormatError::BinaryCorrupt("pdf payload count overflow".to_string()))?;
        if byte_len > r.remaining() {
            return Err(FormatError::BinaryCorrupt(format!(
                "pdf payload size {byte_len} exceeds stream bounds"
            )));
        }
        let raw = r.read_bytes(byte_len)?.to_vec();
        loaded.push(
            EmbeddedPdf {
                name,
                bytes: Arc::new(raw),
            }
            .sanitized(),
        );
    }
    *pdfs = loaded;

    for page in pages.iter_mut() {
        match r.read_u8()? {
            0 => page.pdf_backdrop = None,
            1 => {
                let pdf_index = r.read_varint_usize(usize::MAX, "pdf index")?;
                let page_index = r.read_varint_usize(usize::MAX, "pdf page index")?;
                if pdfs.get(pdf_index).is_some_and(|pdf| !pdf.bytes.is_empty()) {
                    page.pdf_backdrop = Some(PdfPageBackdrop {
                        pdf_index,
                        page_index,
                    });
                } else {
                    page.pdf_backdrop = None;
                }
            }
            other => {
                return Err(FormatError::BinaryCorrupt(format!(
                    "unknown page pdf backdrop flag {other}"
                )));
            }
        }
    }
    Ok(())
}

fn write_text_links_extension(out: &mut Vec<u8>, pages: &[NotebookPage]) {
    let has_links = pages.iter().any(|page| {
        page.texts
            .iter()
            .take(MAX_PAGE_TEXT_ITEMS)
            .any(|item| item.sanitized().link_url.is_some())
    });
    if !has_links {
        return;
    }
    out.push(EXT_TAG_TEXT_LINKS);
    for page in pages {
        let entries: Vec<(usize, String)> = page
            .texts
            .iter()
            .take(MAX_PAGE_TEXT_ITEMS)
            .enumerate()
            .filter_map(|(idx, item)| item.sanitized().link_url.map(|url| (idx, url)))
            .collect();
        write_varint(out, entries.len() as u64);
        for (idx, url) in entries {
            write_varint(out, idx as u64);
            write_str(out, &url);
        }
    }
}

fn read_text_links_extension(
    r: &mut Reader<'_>,
    pages: &mut [NotebookPage],
) -> Result<(), FormatError> {
    for page in pages.iter_mut() {
        let count = r.read_varint_usize(page.texts.len(), "text link")?;
        for _ in 0..count {
            let idx = r.read_varint_usize(page.texts.len().saturating_sub(1), "text link index")?;
            let url = r.read_str()?;
            let Some(slot) = page.texts.get_mut(idx) else {
                return Err(FormatError::BinaryCorrupt(format!(
                    "text link index {idx} out of bounds"
                )));
            };
            let mut item = slot.as_ref().clone();
            item.link_url = drawcraft_doc::sanitize_link_url(&url);
            *slot = Arc::new(item.sanitized());
        }
    }
    Ok(())
}

fn read_page_objects_extension(
    r: &mut Reader<'_>,
    pages: &mut [NotebookPage],
) -> Result<(), FormatError> {
    for page in pages.iter_mut() {
        let text_count = r.read_varint_usize(MAX_PAGE_TEXT_ITEMS, "text item")?;
        page.texts.reserve(text_count.min(256));
        for _ in 0..text_count {
            page.texts.push(Arc::new(read_text_item(r)?));
        }

        let shape_count = r.read_varint_usize(MAX_PAGE_SHAPES, "shape")?;
        page.shapes.reserve(shape_count.min(256));
        for _ in 0..shape_count {
            page.shapes.push(Arc::new(read_shape_item(r)?));
        }

        let image_count = r.read_varint_usize(MAX_PAGE_IMAGES, "image")?;
        page.images.reserve(image_count.min(64));
        for _ in 0..image_count {
            page.images.push(read_page_image(r)?);
        }
    }
    Ok(())
}

fn read_text_widths_extension(
    r: &mut Reader<'_>,
    pages: &mut [NotebookPage],
) -> Result<(), FormatError> {
    for page in pages.iter_mut() {
        let count = r.read_varint_usize(page.texts.len(), "text box width")?;
        for _ in 0..count {
            let idx =
                r.read_varint_usize(page.texts.len().saturating_sub(1), "text box width index")?;
            let width = r.read_f32()?;
            let Some(slot) = page.texts.get_mut(idx) else {
                return Err(FormatError::BinaryCorrupt(format!(
                    "text box width index {idx} out of bounds"
                )));
            };
            let mut item = slot.as_ref().clone();
            item.max_width = Some(width);
            *slot = Arc::new(item.sanitized());
        }
    }
    Ok(())
}

fn read_page_extensions(
    r: &mut Reader<'_>,
    pages: &mut [NotebookPage],
) -> Result<DecodedExtensions, FormatError> {
    let mut ext = DecodedExtensions::default();
    let mut seen_objects = false;
    let mut seen_text_widths = false;
    let mut seen_text_links = false;
    let mut seen_pdf_backdrops = false;
    let mut seen_library_meta = false;
    let mut seen_page_extras = false;
    let mut seen_shapes_rich_text = false;
    let mut seen_identity = false;
    let mut seen_study = false;
    while r.has_remaining() {
        let tag = r.read_u8()?;
        match tag {
            EXT_TAG_PAGE_OBJECTS if !seen_objects => {
                seen_objects = true;
                read_page_objects_extension(r, pages)?;
            }
            EXT_TAG_TEXT_BOX_WIDTHS if seen_objects && !seen_text_widths => {
                seen_text_widths = true;
                read_text_widths_extension(r, pages)?;
            }
            EXT_TAG_TEXT_LINKS if seen_objects && !seen_text_links => {
                seen_text_links = true;
                read_text_links_extension(r, pages)?;
            }
            EXT_TAG_PDF_BACKDROPS if !seen_pdf_backdrops => {
                seen_pdf_backdrops = true;
                read_pdf_backdrops_extension(r, &mut ext.pdf_documents, pages)?;
            }
            EXT_TAG_LIBRARY_META if !seen_library_meta => {
                seen_library_meta = true;
                read_library_meta_extension(r, &mut ext)?;
            }
            EXT_TAG_PAGE_EXTRAS_AND_TEMPLATES if !seen_page_extras => {
                seen_page_extras = true;
                read_page_extras_extension(r, &mut ext.image_templates, pages)?;
            }
            EXT_TAG_SHAPES_AND_RICH_TEXT if seen_objects && !seen_shapes_rich_text => {
                seen_shapes_rich_text = true;
                read_shapes_and_rich_text_extension(r, pages)?;
            }
            EXT_TAG_DOCUMENT_IDENTITY if !seen_identity => {
                seen_identity = true;
                read_document_identity_extension(r, &mut ext)?;
            }
            EXT_TAG_STUDY_ITEMS if !seen_study => {
                seen_study = true;
                read_study_items_extension(r, pages, &mut ext.flashcards)?;
            }
            _ => {
                return Err(FormatError::BinaryCorrupt(format!(
                    "unknown binary extension tag {tag}"
                )));
            }
        }
    }
    Ok(ext)
}

fn write_shapes_and_rich_text_extension(out: &mut Vec<u8>, pages: &[NotebookPage]) {
    let any_shape_extras = pages.iter().any(|p| {
        p.shapes.iter().any(|s| {
            let s = s.sanitized();
            s.fill_color.is_some() || s.line_style != ShapeLineStyle::Solid || !s.points.is_empty()
        })
    });
    let any_rich_text = pages.iter().any(|p| {
        p.texts.iter().any(|t| {
            let t = t.sanitized();
            t.alignment != TextAlignment::Left || !t.spans.is_empty()
        })
    });
    if !any_shape_extras && !any_rich_text {
        return;
    }

    out.push(EXT_TAG_SHAPES_AND_RICH_TEXT);
    for page in pages {
        let shape_count = page.shapes.len().min(MAX_PAGE_SHAPES);
        write_varint(out, shape_count as u64);
        for shape in page.shapes.iter().take(shape_count) {
            let clean = shape.sanitized();
            let style_bits = match clean.line_style {
                ShapeLineStyle::Solid => 0_u8,
                ShapeLineStyle::Dashed => 1_u8,
                ShapeLineStyle::Dotted => 2_u8,
            };
            let has_fill = clean.fill_color.is_some();
            let has_pts = !clean.points.is_empty();
            let flags = u8::from(has_fill) | (style_bits << 1) | (u8::from(has_pts) << 3);
            out.push(flags);
            if let Some(fill) = clean.fill_color {
                write_f32(out, fill.r);
                write_f32(out, fill.g);
                write_f32(out, fill.b);
                write_f32(out, fill.a);
            }
            if has_pts {
                let pt_count = clean.points.len().min(MAX_POLYGON_VERTICES);
                write_varint(out, pt_count as u64);
                for pt in clean.points.iter().take(pt_count) {
                    write_f64(out, pt.x);
                    write_f64(out, pt.y);
                }
            }
        }

        let text_count = page.texts.len().min(MAX_PAGE_TEXT_ITEMS);
        write_varint(out, text_count as u64);
        for item in page.texts.iter().take(text_count) {
            let clean = item.sanitized();
            let align_byte = match clean.alignment {
                TextAlignment::Left => 0_u8,
                TextAlignment::Center => 1_u8,
                TextAlignment::Right => 2_u8,
            };
            out.push(align_byte);
            let span_count = clean.spans.len().min(MAX_TEXT_SPANS);
            write_varint(out, span_count as u64);
            for span in clean.spans.iter().take(span_count) {
                write_varint(out, span.start as u64);
                write_varint(out, span.end as u64);
                let hl_bit = u8::from(span.highlight.is_some()) << 3;
                let font_bit = u8::from(span.font.is_some()) << 4;
                let size_bit = u8::from(span.size.is_some()) << 5;
                let color_bit = u8::from(span.color.is_some()) << 6;
                let style_bits = u8::from(span.bold)
                    | (u8::from(span.italic) << 1)
                    | (u8::from(span.underline) << 2)
                    | hl_bit
                    | font_bit
                    | size_bit
                    | color_bit;
                out.push(style_bits);
                if let Some(hl) = span.highlight {
                    write_f32(out, hl.r);
                    write_f32(out, hl.g);
                    write_f32(out, hl.b);
                    write_f32(out, hl.a);
                }
                if let Some(f) = span.font {
                    out.push(match f {
                        TextFont::Sans => 0_u8,
                        TextFont::Monospace => 1_u8,
                    });
                }
                if let Some(sz) = span.size {
                    write_f32(out, sz);
                }
                if let Some(col) = span.color {
                    write_f32(out, col.r);
                    write_f32(out, col.g);
                    write_f32(out, col.b);
                    write_f32(out, col.a);
                }
            }
        }
    }
}

fn read_shapes_and_rich_text_extension(
    r: &mut Reader<'_>,
    pages: &mut [NotebookPage],
) -> Result<(), FormatError> {
    for page in pages.iter_mut() {
        let shape_count = usize::try_from(r.read_varint()?)
            .map_err(|_| FormatError::BinaryCorrupt("shape count overflow".to_string()))?;
        if shape_count > MAX_PAGE_SHAPES {
            return Err(FormatError::BinaryCorrupt(
                "shape count exceeds limit".to_string(),
            ));
        }
        for shape_idx in 0..shape_count {
            let flags = r.read_u8()?;
            let has_fill = (flags & 1) != 0;
            let line_style = match (flags >> 1) & 0b11 {
                1 => ShapeLineStyle::Dashed,
                2 => ShapeLineStyle::Dotted,
                _ => ShapeLineStyle::Solid,
            };
            let has_pts = (flags & 0b1000) != 0;
            let fill_color = if has_fill {
                let red = r.read_f32()?;
                let green = r.read_f32()?;
                let blue = r.read_f32()?;
                let alpha = r.read_f32()?;
                Some(Color::new(red, green, blue, alpha).sanitized())
            } else {
                None
            };
            let points = if has_pts {
                let pt_count = usize::try_from(r.read_varint()?).map_err(|_| {
                    FormatError::BinaryCorrupt("polygon vertex overflow".to_string())
                })?;
                if pt_count > MAX_POLYGON_VERTICES {
                    return Err(FormatError::BinaryCorrupt(
                        "polygon vertex count exceeds limit".to_string(),
                    ));
                }
                let mut pts = Vec::with_capacity(pt_count);
                for _ in 0..pt_count {
                    pts.push(Point::new(r.read_f64()?, r.read_f64()?));
                }
                pts
            } else {
                Vec::new()
            };
            if let Some(slot) = page.shapes.get_mut(shape_idx) {
                let mut updated = slot.as_ref().clone();
                updated.fill_color = fill_color;
                updated.line_style = line_style;
                if has_pts {
                    updated.points = points;
                }
                *slot = Arc::new(updated.sanitized());
            }
        }

        let text_count = usize::try_from(r.read_varint()?)
            .map_err(|_| FormatError::BinaryCorrupt("rich text count overflow".to_string()))?;
        if text_count > MAX_PAGE_TEXT_ITEMS {
            return Err(FormatError::BinaryCorrupt(
                "rich text count exceeds limit".to_string(),
            ));
        }
        for text_idx in 0..text_count {
            let alignment = match r.read_u8()? {
                1 => TextAlignment::Center,
                2 => TextAlignment::Right,
                _ => TextAlignment::Left,
            };
            let span_count = usize::try_from(r.read_varint()?)
                .map_err(|_| FormatError::BinaryCorrupt("text span overflow".to_string()))?;
            if span_count > MAX_TEXT_SPANS {
                return Err(FormatError::BinaryCorrupt(
                    "text span count exceeds limit".to_string(),
                ));
            }
            let mut spans = Vec::with_capacity(span_count);
            for _ in 0..span_count {
                let start = usize::try_from(r.read_varint()?).unwrap_or(usize::MAX);
                let end = usize::try_from(r.read_varint()?).unwrap_or(usize::MAX);
                let bits = r.read_u8()?;
                let highlight = if (bits & 0b0000_1000) != 0 {
                    Some(
                        Color::new(r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?)
                            .sanitized(),
                    )
                } else {
                    None
                };
                let font = if (bits & 0b0001_0000) != 0 {
                    Some(match r.read_u8()? {
                        0 => TextFont::Sans,
                        1 => TextFont::Monospace,
                        other => {
                            return Err(FormatError::BinaryCorrupt(format!(
                                "unknown text span font tag {other}"
                            )));
                        }
                    })
                } else {
                    None
                };
                let size = if (bits & 0b0010_0000) != 0 {
                    let raw = r.read_f32()?;
                    raw.is_finite().then(|| raw.clamp(6.0, 120.0))
                } else {
                    None
                };
                let color = if (bits & 0b0100_0000) != 0 {
                    Some(
                        Color::new(r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?)
                            .sanitized(),
                    )
                } else {
                    None
                };
                spans.push(TextSpan {
                    start,
                    end,
                    bold: (bits & 1) != 0,
                    italic: (bits & 2) != 0,
                    underline: (bits & 4) != 0,
                    highlight,
                    font,
                    size,
                    color,
                });
            }
            if let Some(slot) = page.texts.get_mut(text_idx) {
                let mut updated = slot.as_ref().clone();
                updated.alignment = alignment;
                updated.spans = spans;
                *slot = Arc::new(updated.sanitized());
            }
        }
    }
    Ok(())
}

fn write_smoothing_config(out: &mut Vec<u8>, s: &SmoothingConfig) {
    let flags = u8::from(s.catmull_rom_subdivision)
        | (u8::from(s.adjust_for_zoom) << 1)
        | (u8::from(s.catch_up) << 2)
        | (u8::from(s.catch_up_on_end) << 3)
        | (u8::from(s.buffer_cusp_detection) << 4)
        | (u8::from(s.buffer_finalize_stroke) << 5);
    out.push(stab_mode_to_u8(s.stabilizer_mode));
    out.push(buf_avg_to_u8(s.buffer_averaging));
    out.push(buf_pre_to_u8(s.buffer_preprocessor));
    out.push(flags);
    write_f32(out, s.stabilizer_strength);
    write_f32(out, s.spline_tension);
    write_f32(out, s.streamline_amount);
    write_f32(out, s.prediction_ms);
    write_varint(out, u64::from(s.buffer_size));
    write_f32(out, s.buffer_sigma);
    write_f32(out, s.buffer_deadzone_radius);
    write_f32(out, s.buffer_drag);
    write_f32(out, s.buffer_mass);
}

fn read_smoothing_config(r: &mut Reader<'_>) -> Result<SmoothingConfig, FormatError> {
    let stabilizer_mode = u8_to_stab_mode(r.read_u8()?)?;
    let buffer_averaging = u8_to_buf_avg(r.read_u8()?)?;
    let buffer_preprocessor = u8_to_buf_pre(r.read_u8()?)?;
    let flags = r.read_u8()?;
    let stabilizer_strength = r.read_f32()?;
    let spline_tension = r.read_f32()?;
    let streamline_amount = r.read_f32()?;
    let prediction_ms = r.read_f32()?;
    let buffer_size = r.read_varint_usize(120, "buffer_size")? as u16;
    let buffer_sigma = r.read_f32()?;
    let buffer_deadzone_radius = r.read_f32()?;
    let buffer_drag = r.read_f32()?;
    let buffer_mass = r.read_f32()?;
    Ok(SmoothingConfig {
        stabilizer_mode,
        stabilizer_strength,
        catmull_rom_subdivision: (flags & (1 << 0)) != 0,
        spline_tension,
        adjust_for_zoom: (flags & (1 << 1)) != 0,
        catch_up: (flags & (1 << 2)) != 0,
        catch_up_on_end: (flags & (1 << 3)) != 0,
        streamline_amount,
        prediction_ms,
        buffer_averaging,
        buffer_preprocessor,
        buffer_size,
        buffer_sigma,
        buffer_deadzone_radius,
        buffer_cusp_detection: (flags & (1 << 4)) != 0,
        buffer_drag,
        buffer_mass,
        buffer_finalize_stroke: (flags & (1 << 5)) != 0,
    }
    .sanitized())
}

fn decode_legacy_smoothing_preset(idx: u8) -> Result<SmoothingConfig, FormatError> {
    let def = SmoothingConfig::default();
    match idx {
        0 => Ok(def),
        1 => Ok(SmoothingConfig {
            stabilizer_mode: StabilizerMode::OneEuro,
            stabilizer_strength: 0.28,
            catmull_rom_subdivision: true,
            spline_tension: 0.5,
            adjust_for_zoom: true,
            catch_up: true,
            catch_up_on_end: true,
            streamline_amount: 0.12,
            prediction_ms: 8.0,
            ..def
        }),
        2 => Ok(SmoothingConfig {
            stabilizer_mode: StabilizerMode::PulledString,
            stabilizer_strength: 0.30,
            catmull_rom_subdivision: true,
            spline_tension: 0.45,
            adjust_for_zoom: true,
            catch_up: false,
            catch_up_on_end: true,
            streamline_amount: 0.20,
            prediction_ms: 4.0,
            ..def
        }),
        3 => Ok(SmoothingConfig {
            stabilizer_mode: StabilizerMode::None,
            stabilizer_strength: 0.0,
            catmull_rom_subdivision: false,
            spline_tension: 0.0,
            adjust_for_zoom: true,
            catch_up: false,
            catch_up_on_end: false,
            streamline_amount: 0.0,
            prediction_ms: 0.0,
            ..def
        }),
        _ => Err(FormatError::BinaryCorrupt(format!(
            "unknown stabilization preset index {idx}"
        ))),
    }
}

fn write_pen_entry(out: &mut Vec<u8>, pen: &PenSettings) {
    let pen = pen.sanitized();
    let def = PenSettings::for_kind(pen.kind);
    out.push(pen_kind_to_u8(pen.kind));

    let col = pen.color.sanitized();
    let rgba8 = col.to_rgba8();
    let exact_rgba8 = Color::from_rgba8(rgba8[0], rgba8[1], rgba8[2], rgba8[3]) == col;
    if exact_rgba8 {
        out.push(0);
        out.extend_from_slice(&rgba8);
    } else {
        out.push(1);
        write_f32(out, col.r);
        write_f32(out, col.g);
        write_f32(out, col.b);
        write_f32(out, col.a);
    }

    let mut mask = 0u16;
    if pen.size != def.size {
        mask |= 1 << 0;
    }
    if pen.opacity != def.opacity {
        mask |= 1 << 1;
    }
    if pen.hardness != def.hardness {
        mask |= 1 << 2;
    }
    if pen.pressure_enabled != def.pressure_enabled
        || pen.velocity_pressure_simulation != def.velocity_pressure_simulation
        || pen.pressure_size_min != def.pressure_size_min
        || pen.pressure_opacity_min != def.pressure_opacity_min
    {
        mask |= 1 << 3;
    }
    if pen.tilt_sensitivity != def.tilt_sensitivity
        || pen.nib_flatness != def.nib_flatness
        || pen.nib_angle_deg != def.nib_angle_deg
        || pen.start_taper_px != def.start_taper_px
        || pen.end_taper_px != def.end_taper_px
    {
        mask |= 1 << 4;
    }
    if pen.smoothing != def.smoothing {
        mask |= 1 << 6;
    }
    if pen.pressure_curve != def.pressure_curve {
        mask |= 1 << 7;
    }

    out.extend_from_slice(&mask.to_le_bytes());
    if (mask & (1 << 0)) != 0 {
        write_f32(out, pen.size);
    }
    if (mask & (1 << 1)) != 0 {
        write_f32(out, pen.opacity);
    }
    if (mask & (1 << 2)) != 0 {
        write_f32(out, pen.hardness);
    }
    if (mask & (1 << 3)) != 0 {
        let pflags =
            u8::from(pen.pressure_enabled) | (u8::from(pen.velocity_pressure_simulation) << 1);
        out.push(pflags);
        write_f32(out, pen.pressure_size_min);
        write_f32(out, pen.pressure_opacity_min);
    }
    if (mask & (1 << 4)) != 0 {
        write_f32(out, pen.tilt_sensitivity);
        write_f32(out, pen.nib_flatness);
        write_f32(out, pen.nib_angle_deg);
        write_f32(out, pen.start_taper_px);
        write_f32(out, pen.end_taper_px);
    }
    if (mask & (1 << 6)) != 0 {
        write_smoothing_config(out, &pen.smoothing);
    }
    if (mask & (1 << 7)) != 0 {
        let pts = &pen.pressure_curve.points;
        let count = pts.len().min(MAX_CURVE_POINTS);
        write_varint(out, count as u64);
        for pt in pts.iter().take(count) {
            write_f32(out, pt[0]);
            write_f32(out, pt[1]);
        }
    }
}

fn read_pen_entry(r: &mut Reader<'_>) -> Result<PenSettings, FormatError> {
    let kind = u8_to_pen_kind(r.read_u8()?)?;
    let mut pen = PenSettings::for_kind(kind);

    let color_mode = r.read_u8()?;
    pen.color = match color_mode {
        0 => {
            let b = r.read_bytes(4)?;
            Color::from_rgba8(b[0], b[1], b[2], b[3])
        }
        1 => Color::new(r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?),
        other => {
            return Err(FormatError::BinaryCorrupt(format!(
                "unknown pen color encoding {other}"
            )));
        }
    };

    let mask = r.read_u16()?;
    if (mask & (1 << 0)) != 0 {
        pen.size = r.read_f32()?;
    }
    if (mask & (1 << 1)) != 0 {
        pen.opacity = r.read_f32()?;
    }
    if (mask & (1 << 2)) != 0 {
        pen.hardness = r.read_f32()?;
    }
    if (mask & (1 << 3)) != 0 {
        let pflags = r.read_u8()?;
        pen.pressure_enabled = (pflags & (1 << 0)) != 0;
        pen.velocity_pressure_simulation = (pflags & (1 << 1)) != 0;
        pen.pressure_size_min = r.read_f32()?;
        pen.pressure_opacity_min = r.read_f32()?;
    }
    if (mask & (1 << 4)) != 0 {
        pen.tilt_sensitivity = r.read_f32()?;
        pen.nib_flatness = r.read_f32()?;
        pen.nib_angle_deg = r.read_f32()?;
        pen.start_taper_px = r.read_f32()?;
        pen.end_taper_px = r.read_f32()?;
    }
    if (mask & (1 << 5)) != 0 {
        let idx = r.read_u8()?;
        pen.smoothing = decode_legacy_smoothing_preset(idx)?;
    } else if (mask & (1 << 6)) != 0 {
        pen.smoothing = read_smoothing_config(r)?;
    }
    if (mask & (1 << 7)) != 0 {
        let count = r.read_varint_usize(MAX_CURVE_POINTS, "pressure_curve")?;
        let mut pts = Vec::with_capacity(count);
        for _ in 0..count {
            pts.push([r.read_f32()?, r.read_f32()?]);
        }
        pen.pressure_curve = PressureCurve::new(pts);
    }

    Ok(pen.sanitized())
}

fn write_stroke_samples(out: &mut Vec<u8>, samples: &[StrokeSample]) {
    let n = samples.len().min(MAX_POINTS_PER_STROKE);
    write_varint(out, n as u64);
    let q_samples: Vec<QuantizedSample> = samples.iter().take(n).map(|s| s.quantize()).collect();

    let full_p = q_samples.iter().all(|q| q.pressure_q16 == 65_535);
    let const_p = !full_p
        && q_samples.first().is_some_and(|first| {
            q_samples
                .iter()
                .all(|q| q.pressure_q16 == first.pressure_q16)
        });
    let has_tilt = q_samples
        .iter()
        .any(|q| q.tilt_x_cdeg != 0 || q.tilt_y_cdeg != 0);
    let has_rot = q_samples.iter().any(|q| q.rot_cdeg != 0);
    let has_time = q_samples.iter().any(|q| q.time_ms != 0);

    let mut flags = 0u8;
    if full_p {
        flags |= SAMPLE_FLAG_FULL_PRESSURE;
    }
    if const_p {
        flags |= SAMPLE_FLAG_CONST_PRESSURE;
    }
    if has_tilt {
        flags |= SAMPLE_FLAG_HAS_TILT;
    }
    if has_rot {
        flags |= SAMPLE_FLAG_HAS_ROT;
    }
    if has_time {
        flags |= SAMPLE_FLAG_HAS_TIME;
    }
    out.push(flags);

    if const_p && let Some(first) = q_samples.first() {
        out.extend_from_slice(&first.pressure_q16.to_le_bytes());
    }

    let mut prev_x = 0i64;
    let mut prev_y = 0i64;
    let mut prev_p = 65_535i32;
    let mut prev_tx = 0i32;
    let mut prev_ty = 0i32;
    let mut prev_rot = 0i32;
    let mut prev_t = 0i64;

    for q in q_samples {
        write_zigzag(out, q.x_centipx - prev_x);
        prev_x = q.x_centipx;
        write_zigzag(out, q.y_centipx - prev_y);
        prev_y = q.y_centipx;
        if !full_p && !const_p {
            let p = i32::from(q.pressure_q16);
            write_zigzag(out, i64::from(p - prev_p));
            prev_p = p;
        }
        if has_tilt {
            let tx = i32::from(q.tilt_x_cdeg);
            let ty = i32::from(q.tilt_y_cdeg);
            write_zigzag(out, i64::from(tx - prev_tx));
            write_zigzag(out, i64::from(ty - prev_ty));
            prev_tx = tx;
            prev_ty = ty;
        }
        if has_rot {
            let rot = i32::from(q.rot_cdeg);
            write_zigzag(out, i64::from(rot - prev_rot));
            prev_rot = rot;
        }
        if has_time {
            let t = q.time_ms as i64;
            write_zigzag(out, t.saturating_sub(prev_t));
            prev_t = t;
        }
    }
}

fn read_stroke_samples(r: &mut Reader<'_>) -> Result<Vec<StrokeSample>, FormatError> {
    let n = r.read_varint_usize(MAX_POINTS_PER_STROKE, "stroke sample")?;
    let flags = r.read_u8()?;
    let full_p = (flags & SAMPLE_FLAG_FULL_PRESSURE) != 0;
    let const_p = (flags & SAMPLE_FLAG_CONST_PRESSURE) != 0;
    let has_tilt = (flags & SAMPLE_FLAG_HAS_TILT) != 0;
    let has_rot = (flags & SAMPLE_FLAG_HAS_ROT) != 0;
    let has_time = (flags & SAMPLE_FLAG_HAS_TIME) != 0;

    let const_p_val = if const_p { r.read_u16()? } else { 65_535 };

    let mut out = Vec::with_capacity(n.min(4096));
    let mut prev_x = 0i64;
    let mut prev_y = 0i64;
    let mut prev_p = 65_535i32;
    let mut prev_tx = 0i32;
    let mut prev_ty = 0i32;
    let mut prev_rot = 0i32;
    let mut prev_t = 0i64;

    for _ in 0..n {
        prev_x = prev_x
            .checked_add(r.read_zigzag()?)
            .ok_or_else(|| FormatError::BinaryCorrupt("sample x overflow".to_string()))?;
        prev_y = prev_y
            .checked_add(r.read_zigzag()?)
            .ok_or_else(|| FormatError::BinaryCorrupt("sample y overflow".to_string()))?;

        let pressure_q16 = if full_p {
            65_535
        } else if const_p {
            const_p_val
        } else {
            let dp = i32::try_from(r.read_zigzag()?)
                .map_err(|_| FormatError::BinaryCorrupt("pressure delta overflow".to_string()))?;
            prev_p = prev_p.saturating_add(dp).clamp(0, 65_535);
            prev_p as u16
        };

        let (tilt_x_cdeg, tilt_y_cdeg) = if has_tilt {
            let dtx = i32::try_from(r.read_zigzag()?)
                .map_err(|_| FormatError::BinaryCorrupt("tilt x delta overflow".to_string()))?;
            let dty = i32::try_from(r.read_zigzag()?)
                .map_err(|_| FormatError::BinaryCorrupt("tilt y delta overflow".to_string()))?;
            prev_tx = prev_tx.saturating_add(dtx).clamp(-9000, 9000);
            prev_ty = prev_ty.saturating_add(dty).clamp(-9000, 9000);
            (prev_tx as i16, prev_ty as i16)
        } else {
            (0, 0)
        };

        let rot_cdeg = if has_rot {
            let drot = i32::try_from(r.read_zigzag()?)
                .map_err(|_| FormatError::BinaryCorrupt("rotation delta overflow".to_string()))?;
            prev_rot = prev_rot.saturating_add(drot).rem_euclid(36_000);
            prev_rot as u16
        } else {
            0
        };

        let time_ms = if has_time {
            let dt = r.read_zigzag()?;
            prev_t = prev_t.saturating_add(dt).clamp(0, 1_000_000_000_000);
            prev_t as u64
        } else {
            0
        };

        out.push(StrokeSample::from_quantized(QuantizedSample {
            x_centipx: prev_x,
            y_centipx: prev_y,
            pressure_q16,
            tilt_x_cdeg,
            tilt_y_cdeg,
            rot_cdeg,
            time_ms,
        }));
    }

    Ok(out)
}

fn collect_pen_palette(
    pages: &[NotebookPage],
) -> Result<(Vec<PenSettings>, Vec<Vec<usize>>), FormatError> {
    let mut palette: Vec<PenSettings> = Vec::new();
    let mut page_pen_indices: Vec<Vec<usize>> = Vec::with_capacity(pages.len());

    for page in pages {
        let mut indices = Vec::with_capacity(page.strokes.len());
        for stroke in &page.strokes {
            let clean = stroke.pen.sanitized();
            let idx = if let Some(pos) = palette.iter().position(|p| *p == clean) {
                pos
            } else {
                if palette.len() >= MAX_PEN_PALETTE {
                    return Err(FormatError::BinaryCorrupt(
                        "document exceeded maximum pen palette entries".to_string(),
                    ));
                }
                let pos = palette.len();
                palette.push(clean);
                pos
            };
            indices.push(idx);
        }
        page_pen_indices.push(indices);
    }

    Ok((palette, page_pen_indices))
}

/// Encodes a [`Document`] into a compact zlib-compressed `.dcraft` v2 binary byte vector.
pub fn encode_binary_v2(doc: &Document) -> Result<Vec<u8>, FormatError> {
    let pages = doc.resolved_pages();
    let (palette, page_pen_indices) = collect_pen_palette(&pages)?;

    let mut raw = Vec::with_capacity(4096);
    write_str(&mut raw, &doc.title);
    write_cover(&mut raw, doc.cover);
    write_varint(&mut raw, doc.active_page_index() as u64);
    write_varint(&mut raw, doc.next_stroke_id);

    write_varint(&mut raw, palette.len() as u64);
    for pen in &palette {
        write_pen_entry(&mut raw, pen);
    }

    write_varint(&mut raw, pages.len() as u64);
    let mut prev_id = 0i64;
    for (page, pen_indices) in pages.iter().zip(&page_pen_indices) {
        write_varint(&mut raw, u64::from(page.width.clamp(64, 8192)));
        write_varint(&mut raw, u64::from(page.height.clamp(64, 8192)));
        raw.push(template_to_u8(page.paper_template));
        raw.push(color_to_u8(page.paper_color));

        write_varint(&mut raw, page.strokes.len() as u64);
        for (stroke, &pen_idx) in page.strokes.iter().zip(pen_indices) {
            let id_signed = stroke.id.0 as i64;
            write_zigzag(&mut raw, id_signed.wrapping_sub(prev_id));
            prev_id = id_signed;
            write_varint(&mut raw, pen_idx as u64);
            write_stroke_samples(&mut raw, &stroke.smoothed_points);
        }
    }
    write_page_extensions(&mut raw, doc, &pages);

    let est_cap = if doc.pdf_documents.is_empty() {
        HEADER_LEN + raw.len() / 2
    } else {
        HEADER_LEN + raw.len()
    };
    let mut out = Vec::with_capacity(est_cap);
    out.extend_from_slice(BINARY_MAGIC);
    out.extend_from_slice(&BINARY_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());

    // Embedded PDFs already hold compressed streams; fast deflate avoids multi-second CPU stalls
    // while standard stroke-only notebooks keep default compression.
    let compression = if !doc.pdf_documents.is_empty() || raw.len() > 2 * 1024 * 1024 {
        Compression::fast()
    } else {
        Compression::default()
    };
    let mut encoder = ZlibEncoder::new(out, compression);
    encoder
        .write_all(&raw)
        .map_err(|e| FormatError::Compression(e.to_string()))?;
    encoder
        .finish()
        .map_err(|e| FormatError::Compression(e.to_string()))
}

/// Decodes a `.dcraft` (or legacy `.ncraft`) v2 binary container into a [`Document`].
pub fn decode_binary_v2(bytes: &[u8]) -> Result<Document, FormatError> {
    if bytes.len() < HEADER_LEN || !has_binary_magic(bytes) {
        return Err(FormatError::InvalidFormat(
            "not a DrawCraft binary file".to_string(),
        ));
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != BINARY_VERSION {
        return Err(FormatError::UnsupportedVersion(u32::from(version)));
    }
    let flags = u16::from_le_bytes([bytes[6], bytes[7]]);
    if flags != 0 {
        return Err(FormatError::UnsupportedVersion(u32::from(version)));
    }

    // Bound decompression relative to the input stream (with a 128 MiB baseline) to block
    // zip bombs without restricting large embedded PDF notebooks.
    let max_decompressed = bytes
        .len()
        .saturating_mul(MAX_DECOMPRESSION_RATIO)
        .max(MIN_DECOMPRESSED_BYTES_CAP);
    let mut decoder =
        ZlibDecoder::new(&bytes[HEADER_LEN..]).take((max_decompressed as u64).saturating_add(1));
    let mut raw = Vec::new();
    decoder
        .read_to_end(&mut raw)
        .map_err(|e| FormatError::BinaryCorrupt(format!("zlib decompression failed: {e}")))?;
    if raw.len() > max_decompressed {
        return Err(FormatError::PayloadTooLarge {
            limit_bytes: max_decompressed,
        });
    }

    let mut r = Reader::new(&raw);
    let title = r.read_str()?;
    let cover = read_cover(&mut r)?;
    let current_page = r.read_varint_usize(usize::MAX, "current_page")?;
    let next_stroke_id = r.read_varint()?;

    let pen_count = r.read_varint_usize(MAX_PEN_PALETTE, "pen palette")?;
    let mut palette = Vec::with_capacity(pen_count.min(256));
    for _ in 0..pen_count {
        palette.push(read_pen_entry(&mut r)?);
    }

    let raw_page_count = r.read_varint()?;
    let page_count = usize::try_from(raw_page_count)
        .map_err(|_| FormatError::BinaryCorrupt("page count overflow".to_string()))?;
    // Each serialized page consumes at least 5 bytes (width, height, template, color, stroke_count).
    if page_count > r.remaining() / 5 {
        return Err(FormatError::BinaryCorrupt(format!(
            "page count {page_count} exceeds stream bounds"
        )));
    }
    if page_count == 0 {
        return Err(FormatError::BinaryCorrupt(
            "document must contain at least one page".to_string(),
        ));
    }

    let mut pages = Vec::with_capacity(page_count.min(64));
    let mut prev_id = 0i64;
    for _ in 0..page_count {
        let w = r.read_varint_usize(8192, "page width")? as u32;
        let h = r.read_varint_usize(8192, "page height")? as u32;
        let paper_template = u8_to_template(r.read_u8()?)?;
        let paper_color = u8_to_color(r.read_u8()?)?;

        let stroke_count = r.read_varint_usize(MAX_STROKES_PER_PAGE, "stroke")?;
        let mut strokes = Vec::with_capacity(stroke_count.min(1024));
        for _ in 0..stroke_count {
            let delta = r.read_zigzag()?;
            let id_signed = prev_id.wrapping_add(delta);
            prev_id = id_signed;
            let id = StrokeId(id_signed as u64);

            let pen_idx = r.read_varint_usize(pen_count.saturating_sub(1), "pen index")?;
            let pen = palette.get(pen_idx).cloned().ok_or_else(|| {
                FormatError::BinaryCorrupt(format!("pen index {pen_idx} out of bounds"))
            })?;
            let smoothed = read_stroke_samples(&mut r)?;
            strokes.push(Arc::new(InkStroke::new(id, pen, Vec::new(), smoothed)));
        }
        let mut page = NotebookPage::new(w, h, paper_template, paper_color);
        page.strokes = strokes;
        pages.push(page);
    }
    let ext = read_page_extensions(&mut r, &mut pages)?;

    let first = &pages[0];
    let mut doc = Document::new(first.width, first.height);
    doc.id = ext.id;
    doc.title = title;
    doc.cover = cover;
    doc.folder_path = ext.folder_path;
    doc.favorite = ext.favorite;
    doc.tags = ext.tags;
    doc.created_at_ms = ext.created_at_ms;
    doc.updated_at_ms = ext.updated_at_ms;
    doc.meta_updated_at_ms = ext.meta_updated_at_ms;
    doc.next_stroke_id = next_stroke_id.max(1);
    doc.pdf_documents = ext.pdf_documents;
    doc.image_templates = ext.image_templates;
    doc.flashcards = ext.flashcards;
    doc.pages = pages;
    doc.current_page = current_page;
    doc.load_active_from_pages();
    Ok(doc)
}
