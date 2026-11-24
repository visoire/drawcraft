//! L3 `.dcraft` compressed binary (v2) and JSON (v1) native document serialization.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod binary;
pub mod import;
pub mod package;
pub mod package_dir;

use drawcraft_doc::Document;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use binary::{
    BINARY_MAGIC, BINARY_VERSION, EARLY_BINARY_MAGIC, LEGACY_BINARY_MAGIC, NOTECRAFT_BINARY_MAGIC,
    has_binary_magic,
};
pub use package::{
    EARLY_PACKAGE_MAGIC, LEGACY_PACKAGE_MAGIC, MAX_PACKAGE_NOTEBOOKS, MAX_PACKAGE_TRASHED,
    NOTECRAFT_PACKAGE_MAGIC, PACKAGE_MAGIC, PACKAGE_VERSION, PackagedTrashEntry, WorkspacePackage,
    load_package_bytes, looks_like_package_bytes, save_package_bytes,
};

pub const FORMAT_MAGIC: &str = "drawcraft";
/// JSON envelope tags of earlier formats that remain readable (PaperCraft, NoteCraft).
const LEGACY_FORMAT_MAGICS: [&str; 2] = ["papercraft", "notecraft"];
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum FormatError {
    #[error("JSON serialization/deserialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unrecognized document format `{0}`")]
    InvalidFormat(String),
    #[error("unsupported .dcraft schema version {0}")]
    UnsupportedVersion(u32),
    #[error("compression failed: {0}")]
    Compression(String),
    #[error("corrupted binary .dcraft stream: {0}")]
    BinaryCorrupt(String),
    #[error(".dcraft payload exceeds maximum supported limit ({limit_bytes} bytes)")]
    PayloadTooLarge { limit_bytes: usize },
}

#[derive(Serialize, Deserialize)]
struct NcraftEnvelope {
    format: String,
    version: u32,
    document: Document,
}

fn build_envelope(doc: &Document) -> NcraftEnvelope {
    NcraftEnvelope {
        format: FORMAT_MAGIC.to_string(),
        version: SCHEMA_VERSION,
        document: doc.clone(),
    }
}

pub fn save_document(doc: &Document) -> Result<String, FormatError> {
    Ok(serde_json::to_string_pretty(&build_envelope(doc))?)
}

/// Serializes a document using compact `.ncraft` JSON (whitespace-free) for sync/storage.
pub fn save_document_compact(doc: &Document) -> Result<String, FormatError> {
    Ok(serde_json::to_string(&build_envelope(doc))?)
}

pub fn load_document(json_str: &str) -> Result<Document, FormatError> {
    let env: NcraftEnvelope = serde_json::from_str(json_str)?;
    if env.format != FORMAT_MAGIC && !LEGACY_FORMAT_MAGICS.contains(&env.format.as_str()) {
        return Err(FormatError::InvalidFormat(env.format));
    }
    if env.version != SCHEMA_VERSION {
        return Err(FormatError::UnsupportedVersion(env.version));
    }
    Ok(env.document)
}

/// Serializes a document into compact zlib-compressed `.dcraft` v2 binary format.
pub fn save_document_bytes(doc: &Document) -> Result<Vec<u8>, FormatError> {
    binary::encode_binary_v2(doc)
}

/// Loads a `.dcraft` (or legacy `.ncraft`) document from compressed binary v2 bytes
/// or legacy UTF-8 v1 JSON bytes.
pub fn load_document_bytes(bytes: &[u8]) -> Result<Document, FormatError> {
    if has_binary_magic(bytes) {
        return binary::decode_binary_v2(bytes);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| {
        FormatError::InvalidFormat("unrecognized binary .dcraft header".to_string())
    })?;
    let trimmed = text.trim_start_matches('\u{FEFF}').trim_start();
    if !trimmed.starts_with('{') {
        return Err(FormatError::InvalidFormat(
            "expected DCFT binary header or JSON object".to_string(),
        ));
    }
    load_document(trimmed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use drawcraft_doc::{
        Flashcard, InkStroke, PaperColor, PaperTemplate, PenSettings, ReviewGrade, StrokeSample,
        StudyTapeItem,
    };
    use std::sync::Arc;

    fn study_doc() -> Document {
        let mut doc = Document::new(800, 1000);
        doc.study_tapes
            .push(StudyTapeItem::new(7, 10.0, 20.0, 120.0, 30.0));
        doc.sync_active_to_pages();
        doc.pages.push(drawcraft_doc::NotebookPage::new(
            800,
            1000,
            PaperTemplate::Grid,
            PaperColor::PureWhite,
        ));
        let mut card = Flashcard::new(1, "Mitochondria?", "Powerhouse of the cell", Some(1), 5_000);
        card.schedule = card.schedule.review(ReviewGrade::Good, 5_000);
        doc.flashcards.push(card);
        doc
    }

    #[test]
    fn study_items_round_trip_through_binary_and_json() {
        let doc = study_doc();
        let from_bin = load_document_bytes(&save_document_bytes(&doc).unwrap()).unwrap();
        assert_eq!(from_bin.flashcards, doc.flashcards);
        assert_eq!(from_bin.pages[0].study_tapes, doc.pages[0].study_tapes);
        assert!(from_bin.pages[1].study_tapes.is_empty());
        assert_eq!(from_bin.study_tapes, doc.study_tapes);

        let from_json = load_document(&save_document(&doc).unwrap()).unwrap();
        assert_eq!(from_json.flashcards, doc.flashcards);
        assert_eq!(from_json.pages[0].study_tapes, doc.pages[0].study_tapes);
    }

    #[test]
    fn documents_without_study_items_carry_no_study_extension() {
        let plain = save_document_bytes(&Document::new(800, 1000)).unwrap();
        let with = save_document_bytes(&study_doc()).unwrap();
        assert!(with.len() > plain.len());
        assert!(load_document_bytes(&plain).unwrap().flashcards.is_empty());
    }

    #[test]
    fn legacy_magics_and_envelopes_remain_readable() {
        let doc = Document::new(640, 480);
        let mut bytes = save_document_bytes(&doc).unwrap();
        assert!(bytes.starts_with(BINARY_MAGIC));
        for legacy in [
            LEGACY_BINARY_MAGIC,
            NOTECRAFT_BINARY_MAGIC,
            EARLY_BINARY_MAGIC,
        ] {
            bytes[..4].copy_from_slice(legacy);
            assert_eq!(load_document_bytes(&bytes).unwrap().width, 640);
        }
        let json = save_document_compact(&doc)
            .unwrap()
            .replace("\"drawcraft\"", "\"papercraft\"");
        assert_eq!(load_document(&json).unwrap().height, 480);
    }

    #[test]
    fn truncated_study_extension_is_rejected_not_panicking() {
        let bytes = save_document_bytes(&study_doc()).unwrap();
        // Corrupt the compressed stream tail: must surface an error, never panic.
        for cut in [bytes.len() - 1, bytes.len() / 2, 9] {
            let _ = load_document_bytes(&bytes[..cut]);
        }
        assert!(load_document_bytes(&bytes[..9]).is_err());
    }

    #[test]
    fn round_trip_preserves_document_and_strokes() {
        let mut doc = Document::new(800, 1000);
        doc.paper_template = PaperTemplate::Dotted;
        doc.paper_color = PaperColor::DarkSlate;
        let id = doc.alloc_stroke_id();
        let pts = vec![
            StrokeSample::from_xy_pressure(10.0, 20.0, 0.4),
            StrokeSample::from_xy_pressure(50.0, 80.0, 0.9),
        ];
        doc.strokes.push(Arc::new(InkStroke::new(
            id,
            PenSettings::brush_pen(),
            pts.clone(),
            pts,
        )));

        let saved = save_document(&doc).unwrap();
        let loaded = load_document(&saved).unwrap();
        assert_eq!(loaded.title, doc.title);
        assert_eq!(loaded.paper_template, doc.paper_template);
        assert_eq!(loaded.paper_color, doc.paper_color);
        assert_eq!(loaded.strokes.len(), 1);
        assert_eq!(loaded.strokes[0].id, id);
        assert_eq!(loaded.strokes[0].pen, doc.strokes[0].pen);
        assert_eq!(
            loaded.strokes[0].smoothed_points,
            doc.strokes[0].smoothed_points
        );
        assert_eq!(loaded.strokes[0].bounds, doc.strokes[0].bounds);
        // Raw input points are intentionally omitted during persistence to keep files lightweight.
        assert!(loaded.strokes[0].raw_points.is_empty());

        let compact = save_document_compact(&doc).unwrap();
        assert!(compact.len() < saved.len());
        let loaded_compact = load_document(&compact).unwrap();
        assert_eq!(
            loaded_compact.strokes[0].smoothed_points,
            doc.strokes[0].smoothed_points
        );
    }

    #[test]
    fn binary_v2_round_trip_preserves_multipage_document_and_slashes_size() {
        let mut doc = Document::new(1200, 1600);
        doc.title = "Linear Algebra Lecture 04".to_string();
        doc.cover = drawcraft_doc::NotebookCover::SageGreen;
        doc.paper_template = PaperTemplate::Grid;
        doc.paper_color = PaperColor::WarmCream;

        let mut custom_pen = PenSettings::fountain_pen();
        custom_pen.color = drawcraft_color::Color::from_rgba8(0x1B, 0x4D, 0x3E, 0xFF);
        custom_pen.size = 3.75;

        for stroke_idx in 0..40 {
            let id = doc.alloc_stroke_id();
            let mut pts = Vec::with_capacity(60);
            for k in 0..60 {
                let x = 80.0 + f64::from(k) * 6.5;
                let y = 140.0 + f64::from(stroke_idx) * 25.0 + (f64::from(k) * 0.25).sin() * 14.0;
                let p = if stroke_idx % 2 == 0 {
                    1.0
                } else {
                    0.3 + 0.55 * (k as f32 / 59.0)
                };
                pts.push(StrokeSample::from_xy_pressure(x, y, p));
            }
            doc.strokes.push(Arc::new(InkStroke::new(
                id,
                if stroke_idx % 5 == 0 {
                    PenSettings::highlighter()
                } else {
                    custom_pen.clone()
                },
                pts.clone(),
                pts,
            )));
        }
        doc.sync_active_to_pages();

        let v1_json = save_document(&doc).unwrap();
        let v2_bytes = save_document_bytes(&doc).unwrap();
        assert!(v2_bytes.starts_with(BINARY_MAGIC));
        assert!(
            v2_bytes.len() * 8 < v1_json.len(),
            "expected v2 binary ({} B) to be >8x smaller than JSON ({} B)",
            v2_bytes.len(),
            v1_json.len()
        );

        let loaded = load_document_bytes(&v2_bytes).unwrap();
        assert_eq!(loaded.title, doc.title);
        assert_eq!(loaded.cover, doc.cover);
        assert_eq!(loaded.page_count(), 1);
        assert_eq!(loaded.strokes.len(), 40);
        for (orig, dec) in doc.strokes.iter().zip(loaded.strokes.iter()) {
            assert_eq!(orig.id, dec.id);
            assert_eq!(orig.pen, dec.pen);
            assert_eq!(orig.smoothed_points.len(), dec.smoothed_points.len());
            for (a, b) in orig.smoothed_points.iter().zip(dec.smoothed_points.iter()) {
                assert!((a.x - b.x).abs() <= 0.01);
                assert!((a.y - b.y).abs() <= 0.01);
                assert!((a.pressure - b.pressure).abs() <= 0.0001);
            }
        }

        // Backward-compatibility: load_document_bytes also loads legacy v1 JSON cleanly.
        let loaded_from_json = load_document_bytes(v1_json.as_bytes()).unwrap();
        assert_eq!(loaded_from_json.strokes.len(), 40);
    }

    #[test]
    fn binary_v2_rejects_corrupt_and_hostile_streams() {
        assert!(matches!(
            load_document_bytes(b"PCFT\x02\x00\x00\x00not_zlib_garbage"),
            Err(FormatError::BinaryCorrupt(_))
        ));
        assert!(matches!(
            load_document_bytes(b"NCFT\x02\x00\x00\x00not_zlib_garbage"),
            Err(FormatError::BinaryCorrupt(_))
        ));
        assert!(matches!(
            load_document_bytes(b"PCFT\x63\x00\x00\x00"),
            Err(FormatError::UnsupportedVersion(99))
        ));
        assert!(matches!(
            load_document_bytes(b"\x00\x01\x02\x03\x04"),
            Err(FormatError::InvalidFormat(_))
        ));
    }

    #[test]
    fn rejects_unsupported_version_and_magic_and_accepts_legacy_json() {
        let legacy_ok = r#"{"format":"notecraft","version":1,"document":{"width":800,"height":1000,"paper_template":"Blank","paper_color":"PureWhite","strokes":[],"next_stroke_id":1}}"#;
        assert!(load_document(legacy_ok).is_ok());

        let bad_ver = r#"{"format":"drawcraft","version":99,"document":{"width":800,"height":1000,"paper_template":"Blank","paper_color":"PureWhite","strokes":[],"next_stroke_id":1}}"#;
        assert!(matches!(
            load_document(bad_ver),
            Err(FormatError::UnsupportedVersion(99))
        ));

        let bad_fmt = r#"{"format":"other","version":1,"document":{"width":800,"height":1000,"paper_template":"Blank","paper_color":"PureWhite","strokes":[],"next_stroke_id":1}}"#;
        assert!(matches!(
            load_document(bad_fmt),
            Err(FormatError::InvalidFormat(_))
        ));
    }

    #[test]
    fn v1_json_and_v2_binary_roundtrip_texts_shapes_and_images() {
        use drawcraft_color::Color;
        use drawcraft_doc::{GeometricShape, PageImage, ShapeKind, TextFont, TextItem};

        let mut doc = Document::new(900, 1200);
        doc.texts.push(Arc::new(TextItem {
            text: "Diagram note".to_string(),
            x: 42.5,
            y: 64.0,
            size: 20.0,
            angle_degrees: -12.0,
            color: Color::from_rgba8(18, 52, 86, 255),
            font: TextFont::Monospace,
            line_spacing: 1.4,
            letter_spacing: 1.5,
            max_width: Some(180.5),
            link_url: Some("https://docs.rs".to_string()),
            ..TextItem::default()
        }));
        doc.texts.push(Arc::new(TextItem {
            text: "Auto width note".to_string(),
            x: 60.0,
            y: 120.0,
            ..TextItem::default()
        }));
        doc.shapes.push(Arc::new(GeometricShape {
            kind: ShapeKind::Arrow,
            x1: 100.0,
            y1: 200.0,
            x2: 280.0,
            y2: 320.0,
            angle_degrees: 15.0,
            curve: 8.0,
            color: Color::from_rgba8(200, 40, 20, 255),
            width: 3.5,
            ..GeometricShape::default()
        }));
        let fake_png = vec![137u8, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3, 4];
        let image = PageImage {
            x: 150.0,
            y: 400.0,
            width: 320.0,
            height: 240.0,
            angle_degrees: 22.5,
            pixel_width: 640,
            pixel_height: 480,
            png_bytes: Arc::new(fake_png.clone()),
        };
        doc.images.push(Arc::new(image.clone()));
        doc.sync_active_to_pages();

        let json_str = save_document(&doc).unwrap();
        let from_json = load_document(&json_str).unwrap();
        assert_eq!(from_json.texts, doc.texts);
        assert_eq!(from_json.shapes, doc.shapes);
        assert_eq!(from_json.images.len(), 1);
        assert_eq!(from_json.images[0].as_ref(), &image);

        let bin_bytes = save_document_bytes(&doc).unwrap();
        let from_bin = load_document_bytes(&bin_bytes).unwrap();
        assert_eq!(from_bin.texts, doc.texts);
        assert_eq!(from_bin.shapes, doc.shapes);
        assert_eq!(from_bin.images.len(), 1);
        assert_eq!(from_bin.images[0].as_ref(), &image);
        assert_eq!(from_bin.images[0].png_bytes.as_slice(), fake_png.as_slice());

        // Legacy JSON without max_width deserializes cleanly with max_width == None.
        let mut legacy_val: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        if let Some(pages) = legacy_val["document"]["pages"].as_array_mut() {
            for page in pages {
                if let Some(texts) = page["texts"].as_array_mut() {
                    for text in texts {
                        if let Some(obj) = text.as_object_mut() {
                            obj.remove("max_width");
                        }
                    }
                }
            }
        }
        let legacy_json = serde_json::to_string(&legacy_val).unwrap();
        let from_legacy = load_document(&legacy_json).unwrap();
        assert_eq!(from_legacy.texts[0].max_width, None);
        assert_eq!(from_legacy.texts[1].max_width, None);
    }

    #[test]
    fn v1_json_and_v2_binary_roundtrip_custom_cover_color() {
        let mut doc = Document::new(800, 1000);
        doc.title = "Custom Cover Book".to_string();
        doc.cover = drawcraft_doc::NotebookCover::custom_rgb(0x2A, 0x78, 0xB4);
        doc.sync_active_to_pages();

        let json_bytes = save_document_compact(&doc).unwrap();
        let from_json = load_document_bytes(json_bytes.as_bytes()).unwrap();
        assert_eq!(from_json.cover, doc.cover);

        let bin_bytes = save_document_bytes(&doc).unwrap();
        let from_bin = load_document_bytes(&bin_bytes).unwrap();
        assert_eq!(from_bin.cover, doc.cover);
    }

    #[test]
    fn v1_json_and_v2_binary_roundtrip_embedded_pdfs_and_page_backdrops() {
        use drawcraft_doc::{EmbeddedPdf, PdfPageBackdrop};

        let mut doc = Document::new(1190, 1684);
        doc.title = "Annotated Slides".to_string();
        doc.pdf_documents.push(EmbeddedPdf {
            name: "Slides.pdf".to_string(),
            bytes: Arc::new(b"%PDF-1.4\n1 0 obj\n<<>>\nendobj\n%%EOF\n".to_vec()),
        });
        doc.pdf_backdrop = Some(PdfPageBackdrop {
            pdf_index: 0,
            page_index: 0,
        });
        doc.add_page(None, None);
        doc.pdf_backdrop = Some(PdfPageBackdrop {
            pdf_index: 0,
            page_index: 1,
        });
        doc.sync_active_to_pages();

        let json_bytes = save_document_compact(&doc).unwrap();
        let from_json = load_document_bytes(json_bytes.as_bytes()).unwrap();
        assert_eq!(from_json.pdf_documents, doc.pdf_documents);
        assert_eq!(from_json.pages[0].pdf_backdrop, doc.pages[0].pdf_backdrop);
        assert_eq!(from_json.pages[1].pdf_backdrop, doc.pages[1].pdf_backdrop);

        let bin_bytes = save_document_bytes(&doc).unwrap();
        let from_bin = load_document_bytes(&bin_bytes).unwrap();
        assert_eq!(from_bin.pdf_documents, doc.pdf_documents);
        assert_eq!(from_bin.pages[0].pdf_backdrop, doc.pages[0].pdf_backdrop);
        assert_eq!(from_bin.pages[1].pdf_backdrop, doc.pages[1].pdf_backdrop);
        assert_eq!(from_bin.pdf_backdrop, doc.pdf_backdrop);
    }

    #[test]
    fn v1_json_and_v2_binary_roundtrip_library_metadata_bookmarks_rotation_and_new_templates() {
        use drawcraft_doc::{EmbeddedImageTemplate, PaperTemplate};

        let mut doc = Document::new(1200, 1600);
        doc.title = "Studio Planner".to_string();
        doc.folder_path = "Work/Q4/Planning".to_string();
        doc.favorite = true;
        doc.tags = vec!["schedule".to_string(), "music".to_string()];
        doc.created_at_ms = 1_700_000_000_000;
        doc.updated_at_ms = 1_700_010_000_000;
        doc.image_templates.push(EmbeddedImageTemplate {
            name: "Letterhead".to_string(),
            png_bytes: Arc::new(vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 42]),
        });
        doc.paper_template = PaperTemplate::Isometric;
        doc.bookmarked = true;
        doc.backdrop_rotation = 3;
        doc.custom_image_backdrop = Some(0);
        doc.custom_template_id = Some("tpl_letterhead".to_string());

        for tpl in [
            PaperTemplate::MusicStaff,
            PaperTemplate::WeeklyPlanner,
            PaperTemplate::MonthlyPlanner,
            PaperTemplate::Checklist,
        ] {
            doc.add_page(Some(tpl), None).unwrap();
        }
        doc.bookmarked = true;
        doc.sync_active_to_pages();

        let json_bytes = save_document_compact(&doc).unwrap();
        let from_json = load_document_bytes(json_bytes.as_bytes()).unwrap();
        assert_eq!(from_json, doc);

        let bin_bytes = save_document_bytes(&doc).unwrap();
        let from_bin = load_document_bytes(&bin_bytes).unwrap();
        assert_eq!(from_bin, doc);
    }

    #[test]
    fn v1_json_and_v2_binary_roundtrip_unbounded_pdf_and_reject_hostile_counts() {
        use drawcraft_doc::{EmbeddedPdf, PdfPageBackdrop};
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write as _;

        // Payload > 32 MiB must serialize and deserialize without truncation.
        let mut large_pdf = b"%PDF-1.4\n%".to_vec();
        large_pdf.resize(34 * 1024 * 1024, b'x');
        large_pdf.extend_from_slice(b"\n%%EOF\n");

        let mut doc = Document::new(800, 1000);
        doc.pdf_documents.push(EmbeddedPdf {
            name: "Large.pdf".to_string(),
            bytes: Arc::new(large_pdf.clone()),
        });
        doc.pdf_backdrop = Some(PdfPageBackdrop {
            pdf_index: 0,
            page_index: 0,
        });
        for idx in 1..520 {
            doc.add_page(None, None).unwrap();
            doc.pdf_backdrop = Some(PdfPageBackdrop {
                pdf_index: 0,
                page_index: idx,
            });
        }
        doc.sync_active_to_pages();

        let bin_bytes = save_document_bytes(&doc).unwrap();
        let from_bin = load_document_bytes(&bin_bytes).unwrap();
        assert_eq!(from_bin.page_count(), 520);
        assert_eq!(from_bin.pdf_documents.len(), 1);
        assert_eq!(from_bin.pdf_documents[0].bytes.len(), large_pdf.len());
        assert_eq!(
            from_bin.pages[519].pdf_backdrop,
            Some(PdfPageBackdrop {
                pdf_index: 0,
                page_index: 519,
            })
        );

        let json_str = save_document_compact(&doc).unwrap();
        let from_json = load_document_bytes(json_str.as_bytes()).unwrap();
        assert_eq!(from_json.page_count(), 520);
        assert_eq!(from_json.pdf_documents[0].bytes.len(), large_pdf.len());

        // Hostile v2 binary claiming 100 million pages in a tiny payload must fail cleanly.
        let raw_hostile = [
            0u8, // empty title
            0u8, // ClassicNavy cover
            0u8, // current_page = 0
            1u8, // next_stroke_id = 1
            0u8, // pen_count = 0
            0x80, 0x94, 0xEB, 0xDC, 0x03, // page_count = 1_000_000_000
        ];
        let mut out = Vec::new();
        out.extend_from_slice(BINARY_MAGIC);
        out.extend_from_slice(&BINARY_VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        let mut encoder = ZlibEncoder::new(out, Compression::default());
        encoder.write_all(&raw_hostile).unwrap();
        let hostile_bin = encoder.finish().unwrap();
        assert!(matches!(
            load_document_bytes(&hostile_bin),
            Err(FormatError::BinaryCorrupt(_))
        ));
    }

    #[test]
    fn v1_json_and_v2_binary_roundtrip_pencil_polygon_fills_and_rich_text() {
        use drawcraft_color::Color;
        use drawcraft_doc::{
            GeometricShape, InkStroke, PenKind, PenSettings, Point, ShapeKind, ShapeLineStyle,
            StrokeSample, TextAlignment, TextFont, TextItem, TextSpan,
        };

        let mut doc = Document::new(900, 1200);
        let id = doc.alloc_stroke_id();
        let pts = vec![
            StrokeSample::from_xy_pressure(50.0, 50.0, 0.35),
            StrokeSample::from_xy_pressure(120.0, 80.0, 0.85),
        ];
        doc.strokes.push(Arc::new(InkStroke::new(
            id,
            PenSettings::pencil(),
            pts.clone(),
            pts,
        )));

        doc.shapes.push(Arc::new(GeometricShape {
            kind: ShapeKind::Polygon,
            fill_color: Some(Color::from_rgba8(0x28, 0x66, 0xE8, 96)),
            line_style: ShapeLineStyle::Dashed,
            points: vec![
                Point::new(100.0, 100.0),
                Point::new(220.0, 120.0),
                Point::new(190.0, 240.0),
            ],
            color: Color::from_rgba8(0x1E, 0x29, 0x3B, 255),
            width: 3.0,
            ..GeometricShape::default()
        }));

        doc.texts.push(Arc::new(TextItem {
            text: "[x] Highlight bold italic".to_string(),
            x: 80.0,
            y: 300.0,
            alignment: TextAlignment::Right,
            spans: vec![
                TextSpan {
                    start: 4,
                    end: 13,
                    bold: true,
                    italic: false,
                    underline: true,
                    highlight: Some(Color::from_rgba8(0xFF, 0xEE, 0x58, 180)),
                    font: Some(TextFont::Monospace),
                    size: Some(32.0),
                    color: Some(Color::from_rgba8(0xDC, 0x26, 0x26, 255)),
                },
                TextSpan {
                    start: 14,
                    end: 25,
                    bold: false,
                    italic: true,
                    underline: false,
                    highlight: None,
                    ..TextSpan::default()
                },
            ],
            ..TextItem::default()
        }));
        doc.sync_active_to_pages();

        let bin = save_document_bytes(&doc).unwrap();
        let from_bin = load_document_bytes(&bin).unwrap();
        assert_eq!(from_bin.strokes[0].pen.kind, PenKind::Pencil);
        assert_eq!(from_bin.shapes[0].kind, ShapeKind::Polygon);
        assert_eq!(from_bin.shapes[0].line_style, ShapeLineStyle::Dashed);
        assert_eq!(from_bin.shapes[0].points.len(), 3);
        assert!(from_bin.shapes[0].fill_color.is_some());
        assert_eq!(from_bin.texts[0].alignment, TextAlignment::Right);
        assert_eq!(from_bin.texts[0].spans.len(), 2);
        assert!(from_bin.texts[0].spans[0].bold);
        assert!(from_bin.texts[0].spans[0].underline);
        assert_eq!(from_bin.texts[0].spans[0].font, Some(TextFont::Monospace));
        assert_eq!(from_bin.texts[0].spans[0].size, Some(32.0));
        assert_eq!(
            from_bin.texts[0].spans[0].color,
            Some(Color::from_rgba8(0xDC, 0x26, 0x26, 255))
        );
        assert!(from_bin.texts[0].spans[1].italic);
        assert_eq!(from_bin.texts[0].spans[1].font, None);

        let json_str = save_document_compact(&doc).unwrap();
        let from_json = load_document_bytes(json_str.as_bytes()).unwrap();
        assert_eq!(from_json.shapes[0], from_bin.shapes[0]);
        assert_eq!(from_json.texts[0], from_bin.texts[0]);
    }

    #[test]
    fn binary_v2_and_json_roundtrip_all_stabilization_presets() {
        use drawcraft_doc::StabilizationPreset;

        let mut doc = Document::new(800, 1000);
        let pts = vec![
            StrokeSample::from_xy_pressure(20.0, 20.0, 0.4),
            StrokeSample::from_xy_pressure(80.0, 60.0, 0.8),
        ];
        for &preset in StabilizationPreset::ALL {
            let id = doc.alloc_stroke_id();
            let mut pen = PenSettings::fountain_pen();
            pen.smoothing = preset.to_smoothing_config();
            doc.strokes
                .push(Arc::new(InkStroke::new(id, pen, pts.clone(), pts.clone())));
        }
        doc.sync_active_to_pages();

        let bin = save_document_bytes(&doc).unwrap();
        let from_bin = load_document_bytes(&bin).unwrap();
        assert_eq!(from_bin.strokes.len(), StabilizationPreset::ALL.len());
        for (idx, &preset) in StabilizationPreset::ALL.iter().enumerate() {
            assert_eq!(from_bin.strokes[idx].pen.smoothing.preset(), preset);
        }

        let json_str = save_document_compact(&doc).unwrap();
        let from_json = load_document_bytes(json_str.as_bytes()).unwrap();
        assert_eq!(from_json.strokes.len(), StabilizationPreset::ALL.len());
        for (idx, &preset) in StabilizationPreset::ALL.iter().enumerate() {
            assert_eq!(from_json.strokes[idx].pen.smoothing.preset(), preset);
        }
    }

    #[test]
    fn binary_v2_and_json_preserve_document_identity_and_meta_timestamp() {
        let mut doc = Document::new(1200, 1600);
        doc.id = "550e8400-e29b-41d4-a716-446655440000".to_string();
        doc.meta_updated_at_ms = 1_750_000_001_234;
        doc.title = "Geometry".to_string();
        doc.sync_active_to_pages();

        let bin = save_document_bytes(&doc).unwrap();
        let from_bin = load_document_bytes(&bin).unwrap();
        assert_eq!(from_bin.id, doc.id);
        assert_eq!(from_bin.meta_updated_at_ms, doc.meta_updated_at_ms);

        let json_str = save_document_compact(&doc).unwrap();
        let from_json = load_document_bytes(json_str.as_bytes()).unwrap();
        assert_eq!(from_json.id, doc.id);
        assert_eq!(from_json.meta_updated_at_ms, doc.meta_updated_at_ms);
    }
}
