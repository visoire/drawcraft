//! Import of foreign containers picked in the Library's *Import* dialog or dropped on the app:
//! notebook-app backups (`.zip` of `.goodnotes`), single `.goodnotes` notebooks and Word documents.
//!
//! Notebooks from a backup are queued and imported one per frame so the UI stays responsive and
//! shows progress; each keeps its folder from the backup. Handwriting strokes of the source app strokes are a
//! proprietary format and are not converted: pages come from the embedded background PDF, or the
//! preview picture when a notebook has no PDF.

use drawcraft_format::import::{self, ArchiveKind, DocxBlock, ImportedNotebook};
use serde_json::json;

use crate::{AppScreen, DrawCraftApp, PickedOpenFile};

const PAGE_MARGIN: f64 = 96.0;
const PAGE_TOP: f64 = 120.0;
const PAGE_BOTTOM_PAD: f64 = 110.0;
const MAX_INLINE_IMAGE_H: f64 = 640.0;

/// Notebooks still waiting to be imported plus the counters shown in the progress banner.
#[derive(Clone, Debug, Default)]
pub struct ImportQueue {
    pub pending: std::collections::VecDeque<ImportedNotebook>,
    pub total: usize,
    pub done: usize,
    pub preview_only: usize,
    pub failed: usize,
}

impl ImportQueue {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.total > 0
    }
}

fn picked_name_and_bytes(picked: &PickedOpenFile) -> Option<(String, Vec<u8>)> {
    match picked {
        PickedOpenFile::Bytes { name, bytes } => Some((name.clone(), bytes.clone())),
        PickedOpenFile::Path(path) => {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !["docx", "goodnotes", "zip"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
            {
                return None;
            }
            let bytes = std::fs::read(path).ok()?;
            let name = path.file_name()?.to_str()?.to_string();
            Some((name, bytes))
        }
    }
}

/// Minimal standard base64 (the engine accepts pictures as base64).
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk.first().copied().unwrap_or(0),
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let sextet = |shift: u32| {
            ALPHABET
                .get(((n >> shift) & 63) as usize)
                .copied()
                .map_or('=', char::from)
        };
        out.push(sextet(18));
        out.push(sextet(12));
        out.push(if chunk.len() > 1 { sextet(6) } else { '=' });
        out.push(if chunk.len() > 2 { sextet(0) } else { '=' });
    }
    out
}

fn stem(name: &str) -> String {
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let s = leaf.rsplit_once('.').map_or(leaf, |(s, _)| s).trim();
    if s.is_empty() {
        "Imported".to_string()
    } else {
        s.chars().take(80).collect()
    }
}

/// Rough height in page points of `text` set at `size` inside a column `width` wide.
fn estimate_text_height(text: &str, size: f64, width: f64) -> f64 {
    let chars_per_line = (width / (size * 0.52)).max(8.0);
    let lines: f64 = text
        .split('\n')
        .map(|line| {
            (line.chars().count() as f64 / chars_per_line)
                .ceil()
                .max(1.0)
        })
        .sum();
    lines * size * 1.45
}

impl DrawCraftApp {
    /// Opens the Import dialog (PDF, Word, notebook-app backup or notebook, DrawCraft files).
    pub(crate) fn trigger_import_dialog(&mut self) -> bool {
        self.view.pdf_import_error = None;
        self.view.pdf_import_target = crate::PdfImportTarget::NewNotebook;
        if let Some(pick) = self.hooks.pick_open_file.as_mut() {
            return match pick(crate::FileDialogKind::Import) {
                Some(picked) => {
                    self.handle_opened_file_event(picked);
                    true
                }
                None => false,
            };
        }
        let msg = "No file picker hook available to browse for a file to import.".to_string();
        self.session.last_export_status = Some(msg.clone());
        self.view.pdf_import_error = Some(msg);
        false
    }

    /// Handles notebook-app archives and Word files. Returns `true` when `picked` was one of those
    /// (successfully or not) and must not be treated as anything else.
    pub(crate) fn try_import_foreign(&mut self, picked: &PickedOpenFile) -> bool {
        if crate::is_picked_package(picked) {
            return false;
        }
        let Some((name, bytes)) = picked_name_and_bytes(picked) else {
            return false;
        };
        match import::sniff_archive(&bytes) {
            ArchiveKind::Docx => {
                match import::read_docx(&bytes) {
                    Ok(blocks) => {
                        let folder = self.current_import_folder();
                        self.import_docx(&stem(&name), &blocks, &folder);
                    }
                    Err(err) => self.import_failed(&err.to_string()),
                }
                true
            }
            ArchiveKind::NotebookBackup | ArchiveKind::SingleNotebook => {
                match import::read_notebook_archive(&bytes, &name) {
                    Ok(mut notebooks) => {
                        let base = self.current_import_folder();
                        if !base.is_empty() {
                            for nb in &mut notebooks {
                                nb.folder_path = if nb.folder_path.is_empty() {
                                    base.clone()
                                } else {
                                    format!("{base}/{}", nb.folder_path)
                                };
                            }
                        }
                        self.view.import_queue.total += notebooks.len();
                        self.view.import_queue.pending.extend(notebooks);
                        self.view.pdf_import_error = None;
                    }
                    Err(err) => self.import_failed(&err.to_string()),
                }
                true
            }
            ArchiveKind::Other => false,
        }
    }

    fn import_failed(&mut self, message: &str) {
        let msg = format!("Import failed: {message}.");
        self.session.last_export_status = Some(msg.clone());
        self.view.pdf_import_error = Some(msg);
    }

    fn current_import_folder(&self) -> String {
        match crate::library_folders::current_scope(self) {
            crate::library_folders::LibraryScope::Folder(path) => path,
            _ => String::new(),
        }
    }

    /// Imports one queued notebook per frame and finishes with a summary.
    pub(crate) fn pump_import_queue(&mut self, ctx: &egui::Context) {
        let Some(nb) = self.view.import_queue.pending.pop_front() else {
            if self.view.import_queue.is_active() {
                self.finish_import_queue();
            }
            return;
        };
        let preview_only = nb.pdf.is_none();
        if self.import_backup_notebook(nb) {
            self.view.import_queue.done += 1;
            if preview_only {
                self.view.import_queue.preview_only += 1;
            }
        } else {
            self.view.import_queue.failed += 1;
        }
        ctx.request_repaint();
    }

    fn finish_import_queue(&mut self) {
        let q = std::mem::take(&mut self.view.import_queue);
        let mut msg = format!("Imported {} of {} notebooks.", q.done, q.total);
        if q.preview_only > 0 {
            msg.push_str(&format!(
                " {} had no PDF pages, so only their preview picture was kept.",
                q.preview_only
            ));
        }
        if q.failed > 0 {
            msg.push_str(&format!(" {} could not be read.", q.failed));
        }
        msg.push_str(" Handwriting strokes of the source app are not converted.");
        self.session.last_export_status = Some(msg);
        self.view.screen = AppScreen::Home;
        self.view.last_autosave_s = -10.0;
    }

    fn import_backup_notebook(&mut self, nb: ImportedNotebook) -> bool {
        if !nb.folder_path.is_empty() {
            let _ = self
                .session
                .execute("library.folder.create", &json!({ "path": nb.folder_path }));
        }
        if let Some(pdf) = nb.pdf {
            let prepared = drawcraft_engine::prepare_pdf_import_from_bytes(
                pdf,
                Some(format!("{}.pdf", nb.name)),
                drawcraft_engine::PdfImportMode::NewNotebook,
            );
            let Ok(prepared) = prepared else {
                return false;
            };
            if self
                .session
                .apply_prepared_pdf_import(prepared, false, false)
                .is_err()
            {
                return false;
            }
            let _ = self
                .session
                .execute("notebook.rename", &json!({ "title": nb.name }));
            if !nb.folder_path.is_empty() {
                let _ = self.session.execute(
                    "notebook.setMetadata",
                    &json!({ "folder_path": nb.folder_path }),
                );
            }
            return true;
        }
        let payload = json!({
            "title": nb.name,
            "preset": "SketchBlank",
            "folder_path": nb.folder_path,
        });
        if self.session.execute("notebook.create", &payload).is_err() {
            return false;
        }
        if let Some(preview) = nb.thumbnail.or_else(|| nb.images.into_iter().next()) {
            let _ = self.insert_image_from_bytes(&preview);
        }
        true
    }

    fn import_docx(&mut self, title: &str, blocks: &[DocxBlock], folder: &str) {
        let payload = json!({
            "title": title,
            "template": "Blank",
            "folder_path": folder,
        });
        if self.session.execute("notebook.create", &payload).is_err() {
            self.import_failed("the notebook limit is reached");
            return;
        }
        let width = f64::from(self.session.doc.width);
        let height = f64::from(self.session.doc.height);
        let column = (width - PAGE_MARGIN * 2.0).max(200.0);
        let bottom = height - PAGE_BOTTOM_PAD;
        let mut y = PAGE_TOP;
        for block in blocks {
            let (needed, gap) = match block {
                DocxBlock::Paragraph { text, heading } => {
                    let size = heading_size(*heading);
                    (estimate_text_height(text, size, column), size * 0.6)
                }
                DocxBlock::Image(bytes) => {
                    let (w, h) = image_size(bytes, column, MAX_INLINE_IMAGE_H);
                    let _ = w;
                    (h, 24.0)
                }
            };
            if y + needed > bottom && y > PAGE_TOP {
                let _ = self.session.execute("page.add", &json!({}));
                let last = self.session.doc.page_count().saturating_sub(1);
                let _ = self
                    .session
                    .execute("page.select", &json!({ "index": last }));
                y = PAGE_TOP;
            }
            match block {
                DocxBlock::Paragraph { text, heading } => {
                    let _ = self.session.execute(
                        "text.insert",
                        &json!({
                            "text": text,
                            "x": PAGE_MARGIN,
                            "y": y,
                            "size": heading_size(*heading),
                            "max_width": column,
                        }),
                    );
                }
                DocxBlock::Image(bytes) => {
                    let (w, h) = image_size(bytes, column, MAX_INLINE_IMAGE_H);
                    let _ = self.session.execute(
                        "image.insert",
                        &json!({
                            "base64_bytes": base64(bytes),
                            "x": PAGE_MARGIN,
                            "y": y,
                            "width": w,
                            "height": h,
                        }),
                    );
                }
            }
            y += needed + gap;
        }
        let _ = self.session.execute("page.select", &json!({ "index": 0 }));
        crate::sync_view_to_active_notebook(self);
        self.view.inspector_open = false;
        self.view.screen = AppScreen::Editor;
    }
}

fn heading_size(heading: u8) -> f64 {
    match heading {
        1 => 44.0,
        2 => 36.0,
        3 => 32.0,
        _ => 28.0,
    }
}

/// Fits a picture into `max_w` x `max_h` keeping its aspect ratio (falls back to a square).
fn image_size(bytes: &[u8], max_w: f64, max_h: f64) -> (f64, f64) {
    let (w, h) = image::load_from_memory(bytes)
        .map(|img| (f64::from(img.width()), f64::from(img.height())))
        .unwrap_or((max_w, max_w));
    if w <= 0.0 || h <= 0.0 {
        return (max_w.min(max_h), max_w.min(max_h));
    }
    let scale = (max_w / w).min(max_h / h).min(1.0);
    ((w * scale).max(8.0), (h * scale).max(8.0))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_reference_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn text_height_grows_with_length_and_lines() {
        let short = estimate_text_height("hi", 28.0, 900.0);
        let long = estimate_text_height(&"word ".repeat(400), 28.0, 900.0);
        let two = estimate_text_height("a\nb", 28.0, 900.0);
        assert!(long > short * 5.0);
        assert!(two > short);
    }

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut out);
            for (name, data) in entries {
                w.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn backup_import_creates_notebooks_in_their_folders_one_per_frame() {
        let mut app = DrawCraftApp::default();
        let pdf = app.session.export_pdf_bytes().unwrap();
        let notebook = zip_of(&[
            ("index.notes.pb", b"x"),
            ("attachments/AAAA", &pdf),
            ("thumbnail.jpg", &[0xFF, 0xD8, 0xFF, 0xE0]),
        ]);
        let backup = zip_of(&[
            ("School/Physics/Kinematics.goodnotes", &notebook),
            ("Whiteboard.goodnotes", &notebook),
        ]);
        let before = app.session.notebooks.len();

        app.handle_opened_file_event(PickedOpenFile::Bytes {
            name: "Backup.zip".to_string(),
            bytes: backup,
        });
        assert!(app.view.import_queue.is_active());
        assert_eq!(app.view.import_queue.total, 2);
        assert_eq!(app.session.notebooks.len(), before, "nothing imported yet");

        let ctx = egui::Context::default();
        app.pump_import_queue(&ctx);
        assert_eq!(
            app.session.notebooks.len(),
            before + 1,
            "one notebook per frame"
        );
        app.pump_import_queue(&ctx);
        app.pump_import_queue(&ctx); // empty queue: finishes with a summary
        assert!(!app.view.import_queue.is_active());
        assert_eq!(app.session.notebooks.len(), before + 2);
        let titles: Vec<(&str, &str)> = app
            .session
            .notebooks
            .iter()
            .map(|n| (n.title.as_str(), n.folder_path.as_str()))
            .collect();
        assert!(
            titles.contains(&("Kinematics", "School/Physics")),
            "{titles:?}"
        );
        assert!(titles.contains(&("Whiteboard", "")), "{titles:?}");
        assert_eq!(app.view.screen, AppScreen::Home);
        assert!(
            app.session
                .last_export_status
                .as_deref()
                .is_some_and(|m| m.starts_with("Imported 2 of 2"))
        );
    }

    #[test]
    fn docx_import_flows_paragraphs_onto_pages() {
        let paragraph = |t: &str| format!("<w:p><w:r><w:t>{t}</w:t></w:r></w:p>");
        let body: String = (0..60)
            .map(|i| paragraph(&format!("Paragraph {i} {}", "lorem ipsum ".repeat(30))))
            .collect();
        let document = format!("<w:document><w:body>{body}</w:body></w:document>");
        let docx = zip_of(&[("word/document.xml", document.as_bytes())]);
        let mut app = DrawCraftApp::default();
        app.handle_opened_file_event(PickedOpenFile::Bytes {
            name: "Essay.docx".to_string(),
            bytes: docx,
        });
        assert_eq!(app.session.doc.title, "Essay");
        assert!(
            app.session.doc.page_count() > 1,
            "long text spills to new pages"
        );
        assert_eq!(app.view.screen, AppScreen::Editor);
    }

    #[test]
    fn broken_archives_report_an_error_instead_of_crashing() {
        let mut app = DrawCraftApp::default();
        let before = app.session.notebooks.len();
        let bad = zip_of(&[("readme.txt", b"nothing here")]);
        app.handle_opened_file_event(PickedOpenFile::Bytes {
            name: "x.zip".to_string(),
            bytes: bad,
        });
        assert_eq!(app.session.notebooks.len(), before);
    }
}
