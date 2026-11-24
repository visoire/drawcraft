//! File-size regression report: builds a realistic multi-page handwritten notebook and
//! measures `.dcraft`, `.svg`, `.pdf`, and `.png` output sizes.
//!
//! Run with `cargo test -p drawcraft-engine --test size_report -- --nocapture` to print the table.

use std::sync::Arc;

use drawcraft_compose::{export_notebook_pdf, render_document, render_document_svg};
use drawcraft_doc::{InkStroke, NotebookPreset, PaperTemplate, PenSettings, StrokeSample};
use drawcraft_format::{load_document_bytes, save_document, save_document_bytes};
use drawcraft_paint::{simplify_committed_spine, smooth_stroke_samples};

const PAGES: usize = 10;
const LINES_PER_PAGE: usize = 22;
const WORDS_PER_LINE: usize = 7;

fn synth_word(x0: f64, y0: f64, seed: usize) -> Vec<StrokeSample> {
    let glyphs = 3 + seed % 4;
    let width = 18.0 * glyphs as f64;
    let n = 40 + (seed % 5) * 8;
    (0..n)
        .map(|k| {
            let t = k as f64 / (n - 1) as f64;
            let phase = t * glyphs as f64 * std::f64::consts::TAU;
            let x = x0 + t * width + phase.sin() * 3.0;
            let y = y0 - (phase.cos() * 0.5 + 0.5) * 11.0 + ((seed as f64) * 0.7).sin() * 1.5;
            let pressure = (0.45 + 0.35 * (t * std::f64::consts::PI).sin()) as f32;
            let mut s = StrokeSample::from_xy_pressure(x, y, pressure);
            s.time_ms = k as f64 * 8.0;
            s
        })
        .collect()
}

fn build_notebook(simplify: bool) -> drawcraft_doc::Document {
    let mut doc = NotebookPreset::EverydayLined.create_document();
    let pen = PenSettings::fountain_pen();
    for page_idx in 0..PAGES {
        if page_idx > 0 {
            doc.add_page(Some(PaperTemplate::Dotted), None);
        }
        for line in 0..LINES_PER_PAGE {
            let y = 120.0 + line as f64 * 60.0;
            let mut x = 140.0;
            for word in 0..WORDS_PER_LINE {
                let seed = page_idx * 1000 + line * 31 + word * 7;
                let raw = synth_word(x, y, seed);
                let dense = smooth_stroke_samples(&raw, &pen.smoothing, 1.0);
                let smoothed = if simplify {
                    simplify_committed_spine(&dense, &pen)
                } else {
                    dense
                };
                let id = doc.alloc_stroke_id();
                doc.strokes
                    .push(Arc::new(InkStroke::new(id, pen.clone(), raw, smoothed)));
                x += 135.0;
            }
        }
    }
    doc.sync_active_to_pages();
    doc
}

#[test]
fn notebook_output_sizes_stay_compact() {
    let dense_doc = build_notebook(false);
    let doc = build_notebook(true);

    let json_dense = save_document(&dense_doc).unwrap_or_default();
    let json = save_document(&doc).unwrap_or_default();
    let binary = save_document_bytes(&doc).unwrap_or_default();
    let reloaded = load_document_bytes(&binary);
    assert!(reloaded.is_ok(), "binary .dcraft must reload cleanly");
    if let Ok(re) = reloaded {
        assert_eq!(re.page_count(), PAGES);
    }

    let pdf = export_notebook_pdf(&doc);
    let page0 = doc.document_for_page(0).unwrap_or_default();
    let svg = render_document_svg(&page0);
    let png = render_document(&page0, None)
        .encode_png()
        .unwrap_or_default();
    let raw_rgba = (page0.width as usize) * (page0.height as usize) * 4;

    let strokes = PAGES * LINES_PER_PAGE * WORDS_PER_LINE;
    println!("\n=== DrawCraft output size report ({PAGES} pages, {strokes} strokes) ===");
    println!(
        ".dcraft JSON (dense points)        : {:>10} bytes",
        json_dense.len()
    );
    println!(
        ".dcraft JSON (simplified points)   : {:>10} bytes",
        json.len()
    );
    println!(
        ".dcraft v2 binary + zlib           : {:>10} bytes",
        binary.len()
    );
    println!(
        ".pdf  ({PAGES} pages, vector)          : {:>10} bytes",
        pdf.len()
    );
    println!(
        ".svg  (page 1, vector)             : {:>10} bytes",
        svg.len()
    );
    println!(
        ".png  (page 1, compressed)         : {:>10} bytes",
        png.len()
    );
    println!(
        ".png  (page 1, raw RGBA baseline)  : {:>10} bytes",
        raw_rgba
    );

    assert!(!binary.is_empty() && binary.len() * 10 < json_dense.len());
    assert!(json.len() < json_dense.len());
    assert!(!png.is_empty() && png.len() * 4 < raw_rgba);
}
